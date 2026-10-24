//! The library index: every folder and file below the library roots, plus the model facts
//! learned while making thumbnails. SQLite in WAL mode, so the UI can read while the
//! background scanner writes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

use crate::cache::{ThumbCache, ThumbMeta};
use crate::checks::{CHECKS_VERSION, MeshChecks};
use crate::formats::Kind;
use crate::listing::Entry;
use crate::printer::{self, Fit, Printer};
use crate::query::{Mark, Query};
use crate::userdata::{UserData, UserEntry};

pub use rusqlite::Error;

const SCHEMA_VERSION: i32 = 6;
/// `kind` values of model files ([`Kind::is_model`]) for SQL `IN (…)` lists.
const MODEL_KINDS: &str = "'stl', '3mf', 'step', 'obj', 'ply', 'gltf'";
/// The index proper: a cache of the disk, rebuilt when its layout changes.
const INDEX_SCHEMA: &str = "
CREATE TABLE dirs (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    parent TEXT,
    mtime_ns INTEGER NOT NULL,
    -- when the scan first found the folder; NULL for folders of a first (baseline) scan
    added_ns INTEGER
);
CREATE INDEX dirs_parent ON dirs(parent);
CREATE TABLE files (
    id INTEGER PRIMARY KEY,
    dir_id INTEGER NOT NULL REFERENCES dirs(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    size INTEGER NOT NULL,
    mtime_ns INTEGER NOT NULL,
    -- lower-case name + folder path relative to the library root, for substring search
    search TEXT NOT NULL,
    -- thumbnail cache key the facts below belong to; NULL until the file was processed
    meta_key TEXT,
    triangles INTEGER,
    size_x REAL,
    size_y REAL,
    size_z REAL,
    content_hash TEXT,
    geo_sig TEXT,
    -- when the scan first found the file (or its new contents); NULL in a baseline scan
    added_ns INTEGER,
    -- G-code facts, for filters: print time estimate, printer sliced for, filament used
    print_s INTEGER,
    gcode_printer TEXT,
    filament_g REAL,
    -- the title a 3MF project names
    title TEXT,
    UNIQUE (dir_id, name)
);
CREATE INDEX files_content ON files(content_hash);
CREATE INDEX files_geo ON files(geo_sig);
";
/// User data: kept across index rebuilds. Tags hang on file content, so they survive renames
/// and moves and are shared by identical copies.
const TAGS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS tags (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE
);
CREATE TABLE IF NOT EXISTS file_tags (
    content_hash TEXT NOT NULL,
    tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (content_hash, tag_id)
);
CREATE INDEX IF NOT EXISTS file_tags_tag ON file_tags(tag_id);
CREATE TABLE IF NOT EXISTS notes (
    content_hash TEXT PRIMARY KEY,
    text TEXT NOT NULL,
    -- lower-case copy for search
    search TEXT NOT NULL,
    updated_ns INTEGER NOT NULL
);
";
/// More user data (v6): small settings of the index, the inbox of new files, mesh checks and
/// the journal of applied suggestions. Keyed by content like the tags.
const USER_SCHEMA_V6: &str = "
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS inbox (
    content_hash TEXT PRIMARY KEY,
    first_seen_ns INTEGER NOT NULL,
    -- NULL while the file waits in the inbox
    done_ns INTEGER,
    -- 'baseline', 'user' or 'proposal:<batch uid>'
    done_by TEXT
);
CREATE INDEX IF NOT EXISTS inbox_open ON inbox(first_seen_ns) WHERE done_ns IS NULL;
CREATE TABLE IF NOT EXISTS mesh_checks (
    content_hash TEXT PRIMARY KEY,
    version INTEGER NOT NULL,
    -- problem bits, so searches need not parse the facts
    flags INTEGER NOT NULL,
    facts TEXT NOT NULL,
    checked_ns INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS changes (
    id INTEGER PRIMARY KEY,
    -- 'proposal:<batch uid>'
    origin TEXT NOT NULL,
    title TEXT NOT NULL,
    applied_ns INTEGER NOT NULL,
    undone_ns INTEGER,
    -- JSON list of steps that revert the change, applied in reverse order
    undo TEXT NOT NULL
);
";
/// Columns v6 added to the index tables of v1–v5 databases, which are upgraded in place.
const INDEX_COLUMNS_V6: [(&str, &str, &str); 6] = [
    ("dirs", "added_ns", "INTEGER"),
    ("files", "added_ns", "INTEGER"),
    ("files", "print_s", "INTEGER"),
    ("files", "gcode_printer", "TEXT"),
    ("files", "filament_g", "REAL"),
    ("files", "title", "TEXT"),
];

/// Files that share content (exact copies) or geometry (re-exports, moved/rotated copies).
#[derive(Debug, Clone, PartialEq)]
pub struct DuplicateGroup {
    pub exact: bool,
    pub files: Vec<Entry>,
}

impl DuplicateGroup {
    /// Disk space the extra copies take.
    pub fn wasted(&self) -> u64 {
        let total: u64 = self.files.iter().map(|f| f.size).sum();
        total - self.files.iter().map(|f| f.size).max().unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub files: usize,
    /// Model files (STL/3MF/STEP).
    pub models: usize,
    /// Files whose thumbnail facts are stored.
    pub processed: usize,
    /// Files that can have facts (models, images, PDFs).
    pub processable: usize,
}

pub struct Library {
    pub(crate) conn: Connection,
    /// The user's printers, for `passt:` and `is:zugross` ([`Library::set_printers`]).
    printers: Arc<Vec<Printer>>,
    /// Libraries in their first (baseline) scan: what it finds is not new ([`crate::inbox`]).
    baseline_roots: HashSet<PathBuf>,
}

impl Library {
    /// `~/.local/share/meshgrove/library.db`
    pub fn default_location() -> Option<PathBuf> {
        Some(dirs::data_dir()?.join("meshgrove").join("library.db"))
    }

    pub fn open(path: &Path) -> Result<Library, Error> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        Self::init(Connection::open(path)?)
    }

    /// Read-only connection for UI queries. `None` until the writer has created the index
    /// (opening early would race the writer's schema setup).
    pub fn open_reader(path: &Path) -> Option<Library> {
        use rusqlite::OpenFlags;
        let conn =
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
                .ok()?;
        conn.busy_timeout(Duration::from_secs(2)).ok()?;
        let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).ok()?;
        if version != SCHEMA_VERSION {
            return None;
        }
        let printers = Arc::new(Vec::new());
        printer_functions(&conn, &printers).ok()?;
        Some(Library { conn, printers, baseline_roots: HashSet::new() })
    }

    pub fn open_in_memory() -> Result<Library, Error> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Library, Error> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version != SCHEMA_VERSION {
            if version == 0 || version > SCHEMA_VERSION {
                // The index is only a cache of the disk: rebuild it instead of migrating.
                conn.execute_batch("DROP TABLE IF EXISTS files; DROP TABLE IF EXISTS dirs;")?;
                conn.execute_batch(INDEX_SCHEMA)?;
            }
            // v2 added tags, v3 notes, v5 lists, prints and tag colours, v6 the inbox, mesh checks
            // and the change journal plus a few index columns.
            conn.execute_batch(TAGS_SCHEMA)?;
            conn.execute_batch(crate::lists::LISTS_SCHEMA)?;
            conn.execute_batch(USER_SCHEMA_V6)?;
            upgrade_index(&conn)?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        crate::lists::upgrade(&conn)?;
        reclassify_unknown(&conn)?;
        crate::inbox::start(&conn)?;
        let printers = Arc::new(Vec::new());
        printer_functions(&conn, &printers)?;
        Ok(Library { conn, printers, baseline_roots: HashSet::new() })
    }

    /// While on, folders and files indexed below `root` count as there before (not new).
    pub fn set_baseline_root(&mut self, root: &Path, on: bool) {
        if on {
            self.baseline_roots.insert(root.to_path_buf());
        } else {
            self.baseline_roots.remove(root);
        }
    }

    /// The printers `passt:`/`fits:` and `is:zugross` check against.
    pub fn set_printers(&mut self, printers: &[Printer]) -> Result<(), Error> {
        self.printers = Arc::new(printers.to_vec());
        printer_functions(&self.conn, &self.printers)
    }

    /// A value of the `meta` table (small facts about the index, e.g. `last_scan_ns`).
    pub fn meta_get(&self, key: &str) -> Option<String> {
        self.conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional().ok()?
    }

    pub fn meta_set(&mut self, key: &str, value: &str) -> Result<(), Error> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }

    pub fn dir_mtime(&self, dir: &Path) -> Option<i64> {
        self.conn
            .query_row("SELECT mtime_ns FROM dirs WHERE path = ?1", [path_str(dir)?], |r| r.get(0))
            .optional()
            .ok()?
    }

    pub fn child_dirs(&self, dir: &Path) -> Vec<PathBuf> {
        let Some(dir) = path_str(dir) else { return Vec::new() };
        let Ok(mut stmt) = self.conn.prepare_cached("SELECT path FROM dirs WHERE parent = ?1") else {
            return Vec::new();
        };
        stmt.query_map([dir], |r| r.get::<_, String>(0))
            .map(|rows| rows.filter_map(Result::ok).map(PathBuf::from).collect())
            .unwrap_or_default()
    }

    /// Records the current contents of `dir` (below library root `root`): files are diffed
    /// against the index (changed files lose their stale facts), vanished subfolders are removed.
    /// Returns whether anything in the folder was new, changed or gone.
    pub fn update_dir(
        &mut self,
        root: &Path,
        dir: &Path,
        mtime_ns: i64,
        files: &[Entry],
        subdirs: &[PathBuf],
    ) -> Result<bool, Error> {
        let Some(dir_s) = path_str(dir) else { return Ok(false) };
        let parent = if dir == root { None } else { dir.parent().and_then(path_str) };
        let rel = dir.strip_prefix(root).unwrap_or(dir).to_string_lossy().to_lowercase();
        // When it was found: for the inbox; nothing in a baseline scan.
        let added = (!self.baseline_roots.contains(root)).then(|| nanos(Some(SystemTime::now())));
        let tx = self.conn.transaction()?;
        let mut changed = tx.query_row("SELECT 1 FROM dirs WHERE path = ?1", [dir_s], |_| Ok(())).optional()?.is_none();
        tx.execute(
            "INSERT INTO dirs (path, parent, mtime_ns, added_ns) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET parent = excluded.parent, mtime_ns = excluded.mtime_ns",
            params![dir_s, parent, mtime_ns, added],
        )?;
        let dir_id: i64 = tx.query_row("SELECT id FROM dirs WHERE path = ?1", [dir_s], |r| r.get(0))?;

        let mut known: HashMap<String, (i64, i64, i64)> = HashMap::new();
        {
            let mut stmt = tx.prepare_cached("SELECT name, id, size, mtime_ns FROM files WHERE dir_id = ?1")?;
            for row in stmt.query_map([dir_id], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))? {
                let (name, v) = row?;
                known.insert(name, v);
            }
        }
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO files (dir_id, name, kind, size, mtime_ns, search, added_ns) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            // New contents: new for the inbox as well (unless the content is known).
            let mut update = tx.prepare_cached(
                "UPDATE files SET kind = ?2, size = ?3, mtime_ns = ?4, meta_key = NULL, triangles = NULL,
                 size_x = NULL, size_y = NULL, size_z = NULL, content_hash = NULL, geo_sig = NULL,
                 print_s = NULL, gcode_printer = NULL, filament_g = NULL, title = NULL, added_ns = ?5 WHERE id = ?1",
            )?;
            for f in files.iter().filter(|f| !f.is_dir()) {
                let mtime = nanos(f.modified);
                match known.remove(&f.name) {
                    Some((_, size, old_mtime)) if size == f.size as i64 && old_mtime == mtime => {}
                    Some((id, _, _)) => {
                        update.execute(params![id, f.kind.as_str(), f.size as i64, mtime, added])?;
                        changed = true;
                    }
                    None => {
                        let search = format!("{}\n{rel}", f.name.to_lowercase());
                        insert.execute(params![dir_id, f.name, f.kind.as_str(), f.size as i64, mtime, search, added])?;
                        changed = true;
                    }
                }
            }
            let mut delete = tx.prepare_cached("DELETE FROM files WHERE id = ?1")?;
            for (id, _, _) in known.into_values() {
                delete.execute([id])?;
                changed = true;
            }
        }
        let children: Vec<String> = {
            let mut stmt = tx.prepare_cached("SELECT path FROM dirs WHERE parent = ?1")?;
            stmt.query_map([dir_s], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        // New subfolders are indexed when the scan gets to them; the folder shows them already.
        changed |= subdirs.iter().any(|s| !children.iter().any(|c| path_str(s) == Some(c.as_str())));
        for child in children {
            if !subdirs.iter().any(|s| path_str(s) == Some(child.as_str())) {
                remove_tree(&tx, Path::new(&child))?;
                changed = true;
            }
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Forgets a folder and everything below it.
    pub fn remove_tree(&mut self, dir: &Path) -> Result<(), Error> {
        remove_tree(&self.conn, dir)
    }

    /// Forgets everything that is not below one of `roots`.
    pub fn retain_roots(&mut self, roots: &[PathBuf]) -> Result<(), Error> {
        let all: Vec<String> = {
            let mut stmt = self.conn.prepare("SELECT path FROM dirs WHERE parent IS NULL")?;
            stmt.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        for top in all {
            if !roots.iter().any(|r| path_str(r) == Some(top.as_str())) {
                self.remove_tree(Path::new(&top))?;
            }
        }
        Ok(())
    }

    /// Stores the facts learned while making `path`'s thumbnail (cache key `key`). A missing
    /// content hash keeps the known one (a failure from the cache does not know it; changed
    /// contents already cleared it in [`Library::update_dir`]).
    pub fn store_meta(&mut self, path: &Path, key: &str, meta: &ThumbMeta) -> Result<(), Error> {
        let (Some(dir), Some(name)) = (path.parent().and_then(path_str), path.file_name().and_then(|n| n.to_str()))
        else {
            return Ok(());
        };
        let size = meta.size_mm.unwrap_or([f32::NAN; 3]).map(|v| if v.is_finite() { Some(v as f64) } else { None });
        let gcode = meta.gcode.as_deref();
        self.conn.execute(
            "UPDATE files SET meta_key = ?3, triangles = ?4, size_x = ?5, size_y = ?6, size_z = ?7,
                 content_hash = COALESCE(?8, content_hash), geo_sig = ?9, print_s = ?10, gcode_printer = ?11,
                 filament_g = ?12
             WHERE name = ?2 AND dir_id = (SELECT id FROM dirs WHERE path = ?1)",
            params![
                dir,
                name,
                key,
                meta.triangles.map(|t| t as i64),
                size[0],
                size[1],
                size[2],
                meta.content_hash,
                meta.geo_sig,
                gcode.and_then(|g| g.print_time.as_deref()).and_then(crate::loaders::gcode::print_seconds),
                gcode.and_then(|g| g.printer.as_deref()),
                gcode.and_then(|g| g.filament_g).map(f64::from),
            ],
        )?;
        if let (Some(checks), Some(hash)) = (&meta.checks, &meta.content_hash) {
            self.store_checks(hash, checks)?;
        }
        if let Some(hash) = &meta.content_hash {
            crate::inbox::note_seen(&self.conn, hash)?;
        }
        Ok(())
    }

    /// Saves the printability check of the content `hash`.
    pub fn store_checks(&mut self, hash: &str, checks: &MeshChecks) -> Result<(), Error> {
        let facts = serde_json::to_string(checks).map_err(|e| Error::ToSqlConversionFailure(Box::new(e)))?;
        self.conn.execute(
            "INSERT INTO mesh_checks (content_hash, version, flags, facts, checked_ns) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(content_hash) DO UPDATE SET version = excluded.version, flags = excluded.flags,
                 facts = excluded.facts, checked_ns = excluded.checked_ns",
            params![hash, checks.version, checks.flags(), facts, nanos(Some(SystemTime::now()))],
        )?;
        Ok(())
    }

    /// The stored printability check of `path`'s content (current check version only).
    pub fn checks_of(&self, path: &Path) -> Result<Option<MeshChecks>, Error> {
        let Some(hash) = self.content_hash_of(path)? else { return Ok(None) };
        let facts: Option<String> = self
            .conn
            .query_row(
                "SELECT facts FROM mesh_checks WHERE content_hash = ?1 AND version = ?2",
                params![hash, CHECKS_VERSION],
                |r| r.get(0),
            )
            .optional()?;
        Ok(facts.and_then(|f| serde_json::from_str(&f).ok()))
    }

    /// Model files whose content has no current printability check, one per content, smallest
    /// first: (file, content hash). Their facts must be known (thumbnail processed).
    pub fn needing_checks(&self, limit: usize) -> Result<Vec<(Entry, String)>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT d.path, f.name, f.kind, f.size, f.mtime_ns, f.content_hash FROM files f JOIN dirs d ON d.id = f.dir_id
             WHERE f.kind IN ('stl', '3mf', 'obj', 'ply', 'gltf') AND f.content_hash IS NOT NULL
               AND f.content_hash NOT IN (SELECT content_hash FROM mesh_checks WHERE version = ?1)
             GROUP BY f.content_hash ORDER BY f.size LIMIT ?2",
        )?;
        stmt.query_map(params![CHECKS_VERSION, limit as i64], |r| Ok((entry_from_row(r)?, r.get(5)?)))?.collect()
    }

    /// Models, images, PDFs, G-code and texts whose thumbnail facts are missing or out of date, for background processing.
    pub fn needing_meta(&self) -> Result<Vec<(Entry, String)>, Error> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT d.path, f.name, f.kind, f.size, f.mtime_ns, f.meta_key FROM files f JOIN dirs d ON d.id = f.dir_id
             WHERE f.kind IN ({MODEL_KINDS}, 'image', 'pdf', 'gcode', 'text') ORDER BY d.path, f.name"
        ))?;
        let rows = stmt.query_map([], |r| Ok((entry_from_row(r)?, r.get::<_, Option<String>>(5)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (entry, stored) = row?;
            let key = ThumbCache::key(&entry.path, entry.size, entry.modified, None);
            if stored.as_deref() != Some(key.as_str()) {
                out.push((entry, key));
            }
        }
        Ok(out)
    }

    /// Files matching `query`, folders excluded, sorted by name.
    pub fn search(&self, query: &Query, limit: usize) -> Result<Vec<Entry>, Error> {
        let mut args: Vec<String> = Vec::new();
        let mut sql = String::from(
            "SELECT d.path, f.name, f.kind, f.size, f.mtime_ns FROM files f JOIN dirs d ON d.id = f.dir_id WHERE 1",
        );
        sql += &self.query_sql(query, &mut args);
        sql += &format!(" ORDER BY f.name COLLATE NOCASE LIMIT {limit}");
        let mut stmt = self.conn.prepare(&sql)?;
        stmt.query_map(params_from_iter(args), entry_from_row)?.collect()
    }

    /// The conditions of `query` on `files f` and `dirs d`, each starting with " AND "; their
    /// values are appended to `args` (placeholders numbered after those already there).
    pub(crate) fn query_sql(&self, query: &Query, args: &mut Vec<String>) -> String {
        let mut sql = String::new();
        for term in &query.terms {
            args.push(format!("%{}%", escape_like(term)));
            let n = args.len();
            // A word may also appear in the file's note.
            sql += &format!(
                " AND (f.search LIKE ?{n} ESCAPE '\\'
                   OR f.content_hash IN (SELECT content_hash FROM notes WHERE search LIKE ?{n} ESCAPE '\\'))"
            );
        }
        for tag in &query.tags {
            args.push(tag.clone());
            sql += &format!(
                " AND f.content_hash IN (SELECT ft.content_hash FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
                   WHERE t.name = ?{} COLLATE NOCASE)",
                args.len()
            );
        }
        for list in &query.lists {
            args.push(list.clone());
            sql += &format!(
                " AND f.content_hash IN (SELECT li.content_hash FROM list_items li JOIN lists l ON l.id = li.list_id
                   WHERE l.name = ?{} COLLATE NOCASE)",
                args.len()
            );
        }
        for name in &query.fits {
            match self.printers.iter().position(|p| Some(p) == printer::find(name, &self.printers)) {
                // G-code fits the printer it was sliced for; models and G-code of unknown origin go by size.
                Some(i) => {
                    sql += &format!(
                        " AND CASE WHEN f.kind = 'gcode' AND f.gcode_printer IS NOT NULL
                           THEN mg_sliced_for(f.gcode_printer, {i}) ELSE mg_fits(f.size_x, f.size_y, f.size_z, {i}) END"
                    )
                }
                // An unknown printer matches nothing, like an unknown file type.
                None => sql += " AND 0",
            }
        }
        for mark in &query.marks {
            let unprinted = format!(
                " AND f.kind IN ({MODEL_KINDS}, 'gcode') AND (f.content_hash IS NULL
                   OR f.content_hash NOT IN (SELECT content_hash FROM prints WHERE result = 'printed'))"
            );
            let untagged = format!(
                " AND f.kind IN ({MODEL_KINDS}, 'gcode')
                   AND (f.content_hash IS NULL OR f.content_hash NOT IN (SELECT content_hash FROM file_tags))"
            );
            let too_big = format!(
                " AND f.kind IN ({MODEL_KINDS}) AND f.size_x IS NOT NULL AND NOT mg_fits_any(f.size_x, f.size_y, f.size_z)"
            );
            // What checks::problems reports: the mesh check, suspicious sizes, too big for every printer.
            let tiny = if crate::config::model_units() == crate::config::ModelUnits::Millimetres {
                "OR (f.kind IN ('stl', 'obj', 'ply') AND MAX(f.size_x, f.size_y, f.size_z) < 5.0)"
            } else {
                ""
            };
            let fits_none = if self.printers.is_empty() { "" } else { "OR NOT mg_fits_any(f.size_x, f.size_y, f.size_z)" };
            let problem = format!(
                " AND f.kind IN ({MODEL_KINDS}) AND (
                     f.content_hash IN (SELECT content_hash FROM mesh_checks WHERE flags != 0 AND version = {CHECKS_VERSION})
                     OR (f.size_x IS NOT NULL AND (MAX(f.size_x, f.size_y, f.size_z) > 2000.0 {fits_none}))
                     {tiny})"
            );
            sql += match mark {
                Mark::Favorite => {
                    " AND f.content_hash IN (SELECT li.content_hash FROM list_items li JOIN lists l ON l.id = li.list_id
                       WHERE l.kind = 'favorites')"
                }
                Mark::Printed => " AND f.content_hash IN (SELECT content_hash FROM prints WHERE result = 'printed')",
                // Only models and G-code are printed; a readme is not "unprinted".
                Mark::Unprinted => &unprinted,
                Mark::Failed => {
                    " AND (SELECT p.result FROM prints p WHERE p.content_hash = f.content_hash
                       ORDER BY p.at_ns DESC, p.id DESC LIMIT 1) = 'failed'"
                }
                Mark::TooBig if self.printers.is_empty() => " AND 0",
                Mark::TooBig => &too_big,
                Mark::Problem => &problem,
                Mark::New => " AND f.content_hash IN (SELECT content_hash FROM inbox WHERE done_ns IS NULL)",
                Mark::Untagged => &untagged,
            };
        }
        if !query.kinds.is_empty() {
            let list: Vec<String> = query.kinds.iter().map(|k| format!("'{}'", k.as_str())).collect();
            sql += &format!(" AND f.kind IN ({})", list.join(","));
        }
        sql
    }

    /// Up to `n` thumbnail-capable files in or below `dir` for its mosaic: models before images
    /// before G-code, files directly inside before those in subfolders.
    pub fn folder_preview(&self, dir: &Path, n: usize) -> Result<Vec<Entry>, Error> {
        let Some(d) = path_str(dir) else { return Ok(Vec::new()) };
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT d.path, f.name, f.kind, f.size, f.mtime_ns FROM files f JOIN dirs d ON d.id = f.dir_id
             WHERE (d.path = ?1 OR (d.path > ?2 AND d.path < ?3)) AND f.kind IN ({MODEL_KINDS}, 'image', 'gcode')
             ORDER BY CASE f.kind WHEN 'image' THEN 1 WHEN 'gcode' THEN 2 ELSE 0 END, (d.path = ?1) DESC, d.path, f.name COLLATE NOCASE LIMIT ?4"
        ))?;
        stmt.query_map(params![d, format!("{d}/"), format!("{d}0"), n as i64], entry_from_row)?.collect()
    }

    pub(crate) fn content_hash_of(&self, path: &Path) -> Result<Option<String>, Error> {
        let (Some(dir), Some(name)) = (path.parent().and_then(path_str), path.file_name().and_then(|n| n.to_str()))
        else {
            return Ok(None);
        };
        // Archive members keep their inner path in `name`; try the plain split first.
        let direct = self
            .conn
            .query_row(
                "SELECT f.content_hash FROM files f JOIN dirs d ON d.id = f.dir_id WHERE d.path = ?1 AND f.name = ?2",
                params![dir, name],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?;
        if let Some(found) = direct {
            return Ok(found);
        }
        let Some((archive, member)) = crate::archive::split(path) else { return Ok(None) };
        self.conn
            .query_row(
                "SELECT f.content_hash FROM files f JOIN dirs d ON d.id = f.dir_id WHERE d.path = ?1 AND f.name = ?2",
                params![path_str(&archive), member],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .map(Option::flatten)
    }

    /// The file's content hash, computed and stored now if the file was not processed yet.
    /// `None` for files outside the library.
    pub(crate) fn ensure_content_hash(&mut self, path: &Path) -> Result<Option<String>, Error> {
        if let Some(hash) = self.content_hash_of(path)? {
            return Ok(Some(hash));
        }
        let (archive, member) = match crate::archive::split(path) {
            Some((a, m)) => (a, m),
            None => (path.parent().map(Path::to_path_buf).unwrap_or_default(), listing_name(path)),
        };
        let Ok(bytes) = crate::archive::read(path) else { return Ok(None) };
        let hash = crate::dedupe::content_hash(&bytes);
        let updated = self.conn.execute(
            "UPDATE files SET content_hash = ?3 WHERE name = ?2 AND dir_id = (SELECT id FROM dirs WHERE path = ?1)",
            params![path_str(&archive), member, hash],
        )?;
        Ok((updated > 0).then_some(hash))
    }

    /// Adds `tag` to the library files at `paths`.
    pub fn tag(&mut self, paths: &[PathBuf], tag: &str) -> Result<(), Error> {
        let tag = tag.trim();
        if tag.is_empty() {
            return Ok(());
        }
        self.conn.execute("INSERT OR IGNORE INTO tags (name) VALUES (?1)", [tag])?;
        let id: i64 = self.conn.query_row("SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE", [tag], |r| r.get(0))?;
        for path in paths {
            if let Some(hash) = self.ensure_content_hash(path)? {
                self.conn.execute(
                    "INSERT OR IGNORE INTO file_tags (content_hash, tag_id) VALUES (?1, ?2)",
                    params![hash, id],
                )?;
            }
        }
        Ok(())
    }

    /// Removes `tag` from the files at `paths`; tags nobody uses any more disappear.
    pub fn untag(&mut self, paths: &[PathBuf], tag: &str) -> Result<(), Error> {
        for path in paths {
            if let Some(hash) = self.content_hash_of(path)? {
                self.conn.execute(
                    "DELETE FROM file_tags WHERE content_hash = ?1
                     AND tag_id = (SELECT id FROM tags WHERE name = ?2 COLLATE NOCASE)",
                    params![hash, tag],
                )?;
            }
        }
        self.conn.execute("DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM file_tags)", [])?;
        Ok(())
    }

    /// Tags on any of `paths`, with how many of those files carry each.
    pub fn tags_of(&self, paths: &[PathBuf]) -> Result<Vec<(String, usize)>, Error> {
        let mut counts: Vec<(String, usize)> = Vec::new();
        let mut stmt = self.conn.prepare_cached(
            "SELECT t.name FROM file_tags ft JOIN tags t ON t.id = ft.tag_id WHERE ft.content_hash = ?1",
        )?;
        for path in paths {
            let Some(hash) = self.content_hash_of(path)? else { continue };
            for name in stmt.query_map([hash], |r| r.get::<_, String>(0))? {
                let name = name?;
                match counts.iter_mut().find(|(n, _)| *n == name) {
                    Some((_, c)) => *c += 1,
                    None => counts.push((name, 1)),
                }
            }
        }
        counts.sort_by_key(|(name, _)| name.to_lowercase());
        Ok(counts)
    }

    /// Every tag with the number of indexed files carrying it.
    pub fn all_tags(&self) -> Result<Vec<(String, usize)>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT t.name, COUNT(f.id) FROM tags t
             JOIN file_tags ft ON ft.tag_id = t.id
             LEFT JOIN files f ON f.content_hash = ft.content_hash
             GROUP BY t.id ORDER BY t.name COLLATE NOCASE",
        )?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as usize)))?.collect()
    }

    /// Sets (or with an empty text removes) the note of the files at `paths`.
    pub fn set_note(&mut self, paths: &[PathBuf], text: &str) -> Result<(), Error> {
        let text = text.trim();
        for path in paths {
            let Some(hash) = self.ensure_content_hash(path)? else { continue };
            if text.is_empty() {
                self.conn.execute("DELETE FROM notes WHERE content_hash = ?1", [hash])?;
            } else {
                self.conn.execute(
                    "INSERT INTO notes (content_hash, text, search, updated_ns) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(content_hash) DO UPDATE SET text = excluded.text, search = excluded.search,
                         updated_ns = excluded.updated_ns",
                    params![hash, text, text.to_lowercase(), nanos(Some(SystemTime::now()))],
                )?;
            }
        }
        Ok(())
    }

    /// The common note of `paths`; `mixed` when the files carry different notes.
    pub fn notes_of(&self, paths: &[PathBuf]) -> Result<(Option<String>, bool), Error> {
        let mut notes = Vec::new();
        for path in paths {
            let note = match self.content_hash_of(path)? {
                Some(hash) => self
                    .conn
                    .query_row("SELECT text FROM notes WHERE content_hash = ?1", [hash], |r| r.get::<_, String>(0))
                    .optional()?,
                None => None,
            };
            notes.push(note);
        }
        let first = notes.first().cloned().flatten();
        let mixed = notes.iter().any(|n| *n != notes[0]);
        Ok((if mixed { None } else { first }, mixed))
    }

    /// Tags, notes, prints, lists and tag colours, keyed by content, with the current paths of
    /// those files for reference.
    pub fn export_user_data(&self) -> Result<UserData, Error> {
        let mut entries: std::collections::BTreeMap<String, UserEntry> = std::collections::BTreeMap::new();
        fn entry(entries: &mut std::collections::BTreeMap<String, UserEntry>, hash: String) -> &mut UserEntry {
            entries.entry(hash.clone()).or_insert_with(|| UserEntry { content_hash: hash, ..Default::default() })
        }
        let mut stmt = self.conn.prepare(
            "SELECT ft.content_hash, t.name FROM file_tags ft JOIN tags t ON t.id = ft.tag_id ORDER BY t.name",
        )?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (hash, tag) = row?;
            entry(&mut entries, hash).tags.push(tag);
        }
        let mut stmt = self.conn.prepare("SELECT content_hash, text FROM notes")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (hash, text) = row?;
            entry(&mut entries, hash).note = Some(text);
        }
        let crate::lists::ExportedLists { lists, prints, tag_colors } = self.export_lists()?;
        for (hash, records) in prints {
            entry(&mut entries, hash).prints = records;
        }
        // Files on lists get an entry too, so the export says which files the hashes were.
        for item in lists.iter().flat_map(|l| &l.items) {
            entry(&mut entries, item.content_hash.clone());
        }
        let mut stmt = self
            .conn
            .prepare("SELECT d.path, f.name FROM files f JOIN dirs d ON d.id = f.dir_id WHERE f.content_hash = ?1")?;
        for e in entries.values_mut() {
            for row in stmt.query_map([&e.content_hash], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
                let (dir, name) = row?;
                e.files.push(format!("{dir}/{name}"));
            }
        }
        Ok(UserData { lists, tag_colors, ..UserData::new(entries.into_values().collect()) })
    }

    /// Merges exported user data: tags are added, a note only fills a file without one, lists,
    /// prints and tag colours merge as in `import_lists`. Returns how many entries were applied.
    pub fn import_user_data(&mut self, data: &UserData) -> Result<usize, Error> {
        let tx = self.conn.transaction()?;
        for e in &data.entries {
            for tag in e.tags.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
                tx.execute("INSERT OR IGNORE INTO tags (name) VALUES (?1)", [tag])?;
                tx.execute(
                    "INSERT OR IGNORE INTO file_tags (content_hash, tag_id)
                     SELECT ?1, id FROM tags WHERE name = ?2 COLLATE NOCASE",
                    params![e.content_hash, tag],
                )?;
            }
            if let Some(note) = e.note.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                tx.execute(
                    "INSERT OR IGNORE INTO notes (content_hash, text, search, updated_ns) VALUES (?1, ?2, ?3, ?4)",
                    params![e.content_hash, note, note.to_lowercase(), nanos(Some(SystemTime::now()))],
                )?;
            }
        }
        tx.commit()?;
        let prints: Vec<(String, Vec<_>)> = data
            .entries
            .iter()
            .filter(|e| !e.prints.is_empty())
            .map(|e| (e.content_hash.clone(), e.prints.clone()))
            .collect();
        self.import_lists(&data.lists, &prints, &data.tag_colors)?;
        Ok(data.entries.len())
    }

    /// Groups of duplicate models, the ones wasting most space first.
    pub fn duplicate_groups(&self) -> Result<Vec<DuplicateGroup>, Error> {
        let select = "SELECT d.path, f.name, f.kind, f.size, f.mtime_ns FROM files f JOIN dirs d ON d.id = f.dir_id";
        let mut groups = Vec::new();
        for (exact, keys_sql, files_sql) in [
            (
                true,
                format!(
                    "SELECT content_hash FROM files WHERE content_hash IS NOT NULL AND kind IN ({MODEL_KINDS})
                     GROUP BY content_hash HAVING COUNT(*) > 1"
                ),
                format!("{select} WHERE f.content_hash = ?1 ORDER BY d.path, f.name"),
            ),
            (
                false,
                // Same geometry but not all byte-identical (re-exports, other formats, moved parts).
                "SELECT geo_sig FROM files WHERE geo_sig IS NOT NULL
                 GROUP BY geo_sig HAVING COUNT(DISTINCT content_hash) > 1"
                    .to_owned(),
                format!("{select} WHERE f.geo_sig = ?1 ORDER BY d.path, f.name"),
            ),
        ] {
            let keys: Vec<String> =
                self.conn.prepare(&keys_sql)?.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
            let mut stmt = self.conn.prepare(&files_sql)?;
            for key in keys {
                let files = stmt.query_map([key], entry_from_row)?.collect::<Result<Vec<_>, _>>()?;
                groups.push(DuplicateGroup { exact, files });
            }
        }
        groups.sort_by_key(|g| std::cmp::Reverse(g.wasted()));
        Ok(groups)
    }

    pub fn counts(&self) -> Result<Counts, Error> {
        self.conn.query_row(
            &format!(
                "SELECT COUNT(*),
                        COALESCE(SUM(kind IN ({MODEL_KINDS})), 0),
                        COALESCE(SUM(meta_key IS NOT NULL), 0),
                        COALESCE(SUM(kind IN ({MODEL_KINDS}, 'image', 'pdf', 'gcode', 'text')), 0)
                 FROM files"
            ),
            [],
            |r| {
                Ok(Counts {
                    files: r.get::<_, i64>(0)? as usize,
                    models: r.get::<_, i64>(1)? as usize,
                    processed: r.get::<_, i64>(2)? as usize,
                    processable: r.get::<_, i64>(3)? as usize,
                })
            },
        )
    }
}

fn listing_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn remove_tree(conn: &Connection, dir: &Path) -> Result<(), Error> {
    let Some(d) = path_str(dir) else { return Ok(()) };
    conn.execute(
        "DELETE FROM dirs WHERE path = ?1 OR (path > ?2 AND path < ?3)",
        params![d, format!("{d}/"), format!("{d}0")],
    )?;
    Ok(())
}

pub(crate) fn entry_from_row(r: &rusqlite::Row) -> Result<Entry, Error> {
    let dir: String = r.get(0)?;
    let name: String = r.get(1)?;
    let kind: String = r.get(2)?;
    let mtime: i64 = r.get(4)?;
    Ok(Entry {
        path: Path::new(&dir).join(&name),
        name,
        kind: Kind::parse(&kind),
        size: r.get::<_, i64>(3)? as u64,
        modified: (mtime != 0).then(|| SystemTime::UNIX_EPOCH + Duration::from_nanos(mtime as u64)),
    })
}

/// Nanoseconds since the epoch, 0 when unknown.
pub fn nanos(t: Option<SystemTime>) -> i64 {
    t.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as i64)
}

/// Non-UTF-8 paths cannot be stored (and are vanishingly rare); they are skipped.
/// Files indexed as unknown before Meshgrove learnt their format (OBJ, PLY and glTF in v4,
/// G-code and texts in 0.4): unchanged folders are not rescanned, so classify them again by name.
/// SQL functions for the printer filters: `mg_fits(x, y, z, printer)` and `mg_fits_any(x, y, z)`
/// (by size, turning the model if needed), `mg_sliced_for(name, printer)` (G-code's printer).
/// `printer` is an index into `printers`; registering again replaces the functions.
fn printer_functions(conn: &Connection, printers: &Arc<Vec<Printer>>) -> Result<(), Error> {
    use rusqlite::functions::{Context, FunctionFlags};
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    fn size(ctx: &Context) -> Result<Option<[f32; 3]>, Error> {
        let v = [ctx.get::<Option<f64>>(0)?, ctx.get::<Option<f64>>(1)?, ctx.get::<Option<f64>>(2)?];
        Ok(match v {
            [Some(x), Some(y), Some(z)] => Some([x as f32, y as f32, z as f32]),
            _ => None,
        })
    }
    let fits = |size: [f32; 3], p: &Printer| !matches!(printer::fit(size, &p.bed()), Fit::TooBig(_));
    let list = printers.clone();
    conn.create_scalar_function("mg_fits", 4, flags, move |ctx| {
        let p = list.get(ctx.get::<i64>(3)? as usize);
        Ok(size(ctx)?.zip(p).is_some_and(|(s, p)| fits(s, p)))
    })?;
    let list = printers.clone();
    conn.create_scalar_function("mg_fits_any", 3, flags, move |ctx| {
        Ok(size(ctx)?.is_some_and(|s| list.iter().any(|p| fits(s, p))))
    })?;
    let list = printers.clone();
    conn.create_scalar_function("mg_sliced_for", 2, flags, move |ctx| {
        let name = ctx.get::<Option<String>>(0)?;
        let p = list.get(ctx.get::<i64>(1)? as usize);
        Ok(name.zip(p).is_some_and(|(n, p)| p.matches_gcode(&n)))
    })?;
    Ok(())
}

/// Adds the v6 index columns to an index of an older version. G-code files are processed again:
/// their facts come back from the thumbnail cache (no new rendering) and fill the new columns.
fn upgrade_index(conn: &Connection) -> Result<(), Error> {
    let mut added = false;
    for (table, column, kind) in INDEX_COLUMNS_V6 {
        let exists =
            conn.prepare(&format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1"))?.exists([column])?;
        if !exists {
            conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {kind}"))?;
            added = true;
        }
    }
    if added {
        conn.execute("UPDATE files SET meta_key = NULL WHERE kind = 'gcode'", [])?;
    }
    Ok(())
}

fn reclassify_unknown(conn: &Connection) -> Result<(), Error> {
    let known: Vec<(i64, Kind)> = conn
        .prepare("SELECT id, name FROM files WHERE kind = 'other'")?
        .query_map([], |r| Ok((r.get(0)?, r.get::<_, String>(1)?)))?
        .filter_map(Result::ok)
        .map(|(id, name)| (id, Kind::from_file_name(&name)))
        .filter(|(_, kind)| *kind != Kind::Other)
        .collect();
    if known.is_empty() {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    for (id, kind) in known {
        tx.execute("UPDATE files SET kind = ?2, meta_key = NULL WHERE id = ?1", params![id, kind.as_str()])?;
    }
    tx.commit()?;
    Ok(())
}

fn path_str(p: &Path) -> Option<&str> {
    p.to_str()
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loaders::gcode::GcodeFacts;
    use crate::query;

    fn file(dir: &str, name: &str, size: u64, t: u64) -> Entry {
        Entry {
            name: name.into(),
            path: Path::new(dir).join(name),
            kind: Kind::from_file_name(name),
            size,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(t)),
        }
    }

    fn library() -> Library {
        let mut lib = Library::open_in_memory().unwrap();
        let root = Path::new("/lib/3D_druck");
        let minis = Path::new("/lib/3D_druck/minis");
        lib.update_dir(root, root, 1, &[file("/lib/3D_druck", "slicer.ini", 5, 1)], &[minis.into()]).unwrap();
        lib.update_dir(
            root,
            minis,
            2,
            &[
                file("/lib/3D_druck/minis", "Knight.stl", 100, 1),
                file("/lib/3D_druck/minis", "box_lid.3mf", 50, 1),
                file("/lib/3D_druck/minis", "photo.jpg", 10, 1),
            ],
            &[],
        )
        .unwrap();
        lib
    }

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn search_by_words_folder_and_kind() {
        let lib = library();
        assert_eq!(names(&lib.search(&query::parse("knig"), 50).unwrap()), ["Knight.stl"]);
        assert_eq!(names(&lib.search(&query::parse("minis"), 50).unwrap()), ["box_lid.3mf", "Knight.stl", "photo.jpg"]);
        assert_eq!(names(&lib.search(&query::parse("minis ext:3mf"), 50).unwrap()), ["box_lid.3mf"]);
        // "_" is a literal underscore, not a LIKE wildcard; the root's own name is not searchable.
        assert_eq!(names(&lib.search(&query::parse("x_lid"), 50).unwrap()), ["box_lid.3mf"]);
        assert!(lib.search(&query::parse("3d_druck"), 50).unwrap().is_empty());
        assert_eq!(lib.search(&query::parse("knig"), 50).unwrap()[0].path, Path::new("/lib/3D_druck/minis/Knight.stl"));
    }

    #[test]
    fn rescans_diff_files_and_drop_vanished_folders() {
        let mut lib = library();
        let minis = Path::new("/lib/3D_druck/minis");
        let root = Path::new("/lib/3D_druck");
        lib.store_meta(&minis.join("Knight.stl"), "k1", &ThumbMeta { triangles: Some(7), ..Default::default() })
            .unwrap();
        let lid = file("/lib/3D_druck/minis", "box_lid.3mf", 50, 1);
        let lid_key = ThumbCache::key(&lid.path, lid.size, lid.modified, None);
        lib.store_meta(&lid.path, &lid_key, &ThumbMeta::default()).unwrap();
        assert_eq!(lib.counts().unwrap(), Counts { files: 4, models: 2, processed: 2, processable: 3 });

        // Knight changed on disk, photo deleted, a new file appeared.
        lib.update_dir(
            root,
            minis,
            3,
            &[
                file("/lib/3D_druck/minis", "Knight.stl", 120, 2),
                file("/lib/3D_druck/minis", "box_lid.3mf", 50, 1),
                file("/lib/3D_druck/minis", "new.stl", 1, 1),
            ],
            &[],
        )
        .unwrap();
        assert_eq!(lib.counts().unwrap(), Counts { files: 4, models: 3, processed: 1, processable: 3 });
        let pending: Vec<String> = lib.needing_meta().unwrap().into_iter().map(|(e, _)| e.name).collect();
        assert_eq!(pending, ["Knight.stl", "new.stl"], "box_lid is unchanged and already processed");

        assert_eq!(lib.dir_mtime(minis), Some(3));
        lib.update_dir(root, root, 4, &[], &[]).unwrap();
        assert_eq!(lib.dir_mtime(minis), None, "subfolder vanished with its files");
        assert_eq!(lib.counts().unwrap().files, 0);
    }

    #[test]
    fn mosaic_prefers_models_and_direct_children() {
        let mut lib = library();
        let root = Path::new("/lib/3D_druck");
        let sub = Path::new("/lib/3D_druck/minis/bases");
        lib.update_dir(root, sub, 5, &[file("/lib/3D_druck/minis/bases", "a_base.stl", 1, 1)], &[]).unwrap();
        let preview = lib.folder_preview(Path::new("/lib/3D_druck/minis"), 3).unwrap();
        assert_eq!(names(&preview), ["box_lid.3mf", "Knight.stl", "a_base.stl"]);
        // A sibling folder whose name starts with the same text must not leak in.
        lib.update_dir(
            root,
            Path::new("/lib/3D_druck/minis2"),
            6,
            &[file("/lib/3D_druck/minis2", "zz.stl", 1, 1)],
            &[],
        )
        .unwrap();
        assert_eq!(lib.folder_preview(Path::new("/lib/3D_druck/minis"), 9).unwrap().len(), 4);
    }

    #[test]
    fn tags_follow_content_and_feed_search() {
        let mut lib = library();
        let minis = Path::new("/lib/3D_druck/minis");
        let meta = |hash: &str| ThumbMeta { content_hash: Some(hash.into()), ..Default::default() };
        lib.store_meta(&minis.join("Knight.stl"), "k", &meta("h-knight")).unwrap();
        lib.store_meta(&minis.join("box_lid.3mf"), "k", &meta("h-lid")).unwrap();
        let both = [minis.join("Knight.stl"), minis.join("box_lid.3mf")];
        lib.tag(&both, "Castle Tower").unwrap();
        lib.tag(&both[..1], "bemalt").unwrap();
        lib.tag(&both[..1], "  ").unwrap();
        assert_eq!(lib.tags_of(&both).unwrap(), [("bemalt".into(), 1), ("Castle Tower".into(), 2)]);
        assert_eq!(
            names(&lib.search(&query::parse(r#"tag:"castle tower""#), 9).unwrap()),
            ["box_lid.3mf", "Knight.stl"]
        );
        assert_eq!(names(&lib.search(&query::parse("tag:bemalt box"), 9).unwrap()), Vec::<&str>::new());

        // Renamed on disk: the new row gets the same content hash once processed, and the tags are back.
        let root = Path::new("/lib/3D_druck");
        lib.update_dir(
            root,
            minis,
            9,
            &[file("/lib/3D_druck/minis", "Knight_v2.stl", 100, 1), file("/lib/3D_druck/minis", "box_lid.3mf", 50, 1)],
            &[],
        )
        .unwrap();
        lib.store_meta(&minis.join("Knight_v2.stl"), "k", &meta("h-knight")).unwrap();
        assert_eq!(
            lib.tags_of(&[minis.join("Knight_v2.stl")]).unwrap(),
            [("bemalt".into(), 1), ("Castle Tower".into(), 1)]
        );

        lib.untag(&both[1..], "Castle Tower").unwrap();
        lib.untag(&[minis.join("Knight_v2.stl")], "bemalt").unwrap();
        assert_eq!(lib.all_tags().unwrap(), [("Castle Tower".into(), 1)], "unused tag 'bemalt' is gone");
    }

    #[test]
    fn notes_are_searchable_and_exported_with_tags() {
        let mut lib = library();
        let minis = Path::new("/lib/3D_druck/minis");
        let meta = |hash: &str| ThumbMeta { content_hash: Some(hash.into()), ..Default::default() };
        lib.store_meta(&minis.join("Knight.stl"), "k", &meta("h-knight")).unwrap();
        lib.store_meta(&minis.join("box_lid.3mf"), "k", &meta("h-lid")).unwrap();
        let knight = [minis.join("Knight.stl")];
        lib.set_note(&knight, "  0,12 mm Schichthöhe, Tree-Support  ").unwrap();
        lib.tag(&knight, "bemalt").unwrap();
        assert_eq!(lib.notes_of(&knight).unwrap(), (Some("0,12 mm Schichthöhe, Tree-Support".into()), false));
        let both = [minis.join("Knight.stl"), minis.join("box_lid.3mf")];
        assert_eq!(lib.notes_of(&both).unwrap(), (None, true));
        assert_eq!(names(&lib.search(&query::parse("schichthöhe"), 9).unwrap()), ["Knight.stl"], "found via the note");

        let data = lib.export_user_data().unwrap();
        assert_eq!(data.entries.len(), 1);
        assert_eq!(data.entries[0].files, ["/lib/3D_druck/minis/Knight.stl"]);
        let json = serde_json::to_string(&data).unwrap();

        // Fresh library with the same file content: import brings tags and note back.
        let mut fresh = library();
        fresh.store_meta(&minis.join("Knight.stl"), "k", &meta("h-knight")).unwrap();
        assert_eq!(fresh.import_user_data(&serde_json::from_str(&json).unwrap()).unwrap(), 1);
        assert_eq!(fresh.tags_of(&knight).unwrap(), [("bemalt".into(), 1)]);
        assert_eq!(fresh.notes_of(&knight).unwrap().0.as_deref(), Some("0,12 mm Schichthöhe, Tree-Support"));

        lib.set_note(&knight, "").unwrap();
        assert_eq!(lib.notes_of(&knight).unwrap(), (None, false));
    }

    #[test]
    fn duplicate_groups_exact_and_geometry() {
        let mut lib = library();
        let root = Path::new("/lib/3D_druck");
        let other = Path::new("/lib/3D_druck/copies");
        lib.update_dir(
            root,
            other,
            1,
            &[file("/lib/3D_druck/copies", "Knight.stl", 100, 1), file("/lib/3D_druck/copies", "knight.3mf", 30, 1)],
            &[],
        )
        .unwrap();
        let meta = |hash: &str, sig: &str| ThumbMeta {
            content_hash: Some(hash.into()),
            geo_sig: Some(sig.into()),
            ..Default::default()
        };
        lib.store_meta(Path::new("/lib/3D_druck/minis/Knight.stl"), "k", &meta("h1", "g1")).unwrap();
        lib.store_meta(Path::new("/lib/3D_druck/copies/Knight.stl"), "k", &meta("h1", "g1")).unwrap();
        lib.store_meta(Path::new("/lib/3D_druck/copies/knight.3mf"), "k", &meta("h2", "g1")).unwrap();
        lib.store_meta(Path::new("/lib/3D_druck/minis/box_lid.3mf"), "k", &meta("h3", "g3")).unwrap();
        let groups = lib.duplicate_groups().unwrap();
        assert_eq!(groups.len(), 2);
        let exact = groups.iter().find(|g| g.exact).unwrap();
        assert_eq!(exact.files.len(), 2);
        assert_eq!(exact.wasted(), 100);
        let geo = groups.iter().find(|g| !g.exact).unwrap();
        assert_eq!(geo.files.len(), 3, "the 3MF re-export joins the two STL copies");
        assert_eq!(geo.wasted(), 130);
        assert!(groups[0].wasted() >= groups[1].wasted());
    }

    #[test]
    fn model_kinds_list_matches_the_formats() {
        let listed: Vec<Kind> = MODEL_KINDS.split(", ").map(|k| Kind::parse(k.trim_matches('\''))).collect();
        let all = [
            Kind::Folder,
            Kind::Stl,
            Kind::ThreeMf,
            Kind::Step,
            Kind::Obj,
            Kind::Ply,
            Kind::Gltf,
            Kind::Archive,
            Kind::Image,
            Kind::Pdf,
            Kind::Gcode,
            Kind::Text,
            Kind::Other,
        ];
        assert_eq!(listed, all.into_iter().filter(|k| k.is_model()).collect::<Vec<_>>());
    }

    /// The index tables as v1 to v5 created them.
    const INDEX_SCHEMA_V5: &str = "
        CREATE TABLE dirs (id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, parent TEXT, mtime_ns INTEGER NOT NULL);
        CREATE INDEX dirs_parent ON dirs(parent);
        CREATE TABLE files (
            id INTEGER PRIMARY KEY, dir_id INTEGER NOT NULL REFERENCES dirs(id) ON DELETE CASCADE,
            name TEXT NOT NULL, kind TEXT NOT NULL, size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL,
            search TEXT NOT NULL, meta_key TEXT, triangles INTEGER, size_x REAL, size_y REAL, size_z REAL,
            content_hash TEXT, geo_sig TEXT, UNIQUE (dir_id, name));
        CREATE INDEX files_content ON files(content_hash);
        CREATE INDEX files_geo ON files(geo_sig);
    ";

    #[test]
    fn v5_libraries_gain_the_v6_columns_and_keep_their_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(INDEX_SCHEMA_V5).unwrap();
            conn.execute_batch(TAGS_SCHEMA).unwrap();
            conn.execute_batch(crate::lists::LISTS_SCHEMA).unwrap();
            conn.execute("INSERT INTO dirs (path, parent, mtime_ns) VALUES ('/r', NULL, 1)", []).unwrap();
            for (name, kind) in [("a.stl", "stl"), ("a.gcode", "gcode")] {
                conn.execute(
                    "INSERT INTO files (dir_id, name, kind, size, mtime_ns, search, meta_key, content_hash)
                     VALUES (1, ?1, ?2, 1, 1, ?1, 'key', ?1)",
                    [name, kind],
                )
                .unwrap();
            }
            conn.execute("INSERT INTO tags (name) VALUES ('Garten')", []).unwrap();
            conn.execute("INSERT INTO file_tags (content_hash, tag_id) VALUES ('a.stl', 1)", []).unwrap();
            conn.pragma_update(None, "user_version", 5).unwrap();
        }
        let mut lib = Library::open(&path).unwrap();
        assert_eq!(lib.all_tags().unwrap(), [("Garten".into(), 1)], "tags kept");
        let keys: Vec<Option<String>> = lib
            .conn
            .prepare("SELECT meta_key FROM files ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(keys, [None, Some("key".into())], "only G-code is processed again");

        let facts = GcodeFacts {
            printer: Some("Anycubic Kobra S1".into()),
            print_time: Some("1h 6m 46s".into()),
            filament_g: Some(12.5),
            ..Default::default()
        };
        let meta = ThumbMeta { gcode: Some(Box::new(facts)), ..Default::default() };
        lib.store_meta(Path::new("/r/a.gcode"), "new", &meta).unwrap();
        let row: (Option<i64>, Option<String>, Option<f64>) = lib
            .conn
            .query_row("SELECT print_s, gcode_printer, filament_g FROM files WHERE name = 'a.gcode'", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(row, (Some(4006), Some("Anycubic Kobra S1".into()), Some(12.5)));

        lib.meta_set("last_scan_ns", "42").unwrap();
        lib.meta_set("last_scan_ns", "43").unwrap();
        drop(lib);
        let reader = Library::open_reader(&path).expect("upgraded index is ready");
        assert_eq!(reader.meta_get("last_scan_ns").as_deref(), Some("43"));
        assert_eq!(reader.meta_get("missing"), None);
    }

    #[test]
    fn old_index_versions_are_upgraded_keeping_their_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(INDEX_SCHEMA_V5).unwrap();
            conn.execute("INSERT INTO dirs (path, parent, mtime_ns) VALUES ('/r', NULL, 1)", []).unwrap();
            for name in ["Scan.PLY", "fig.glb", "notes.odt", "Deckel.gcode", "README"] {
                conn.execute(
                    "INSERT INTO files (dir_id, name, kind, size, mtime_ns, search) VALUES (1, ?1, 'other', 1, 1, ?1)",
                    [name],
                )
                .unwrap();
            }
            conn.pragma_update(None, "user_version", 1).unwrap();
        }
        let mut lib = Library::open(&path).unwrap();
        assert_eq!(lib.dir_mtime(Path::new("/r")), Some(1), "index kept");
        let kinds: Vec<String> = lib
            .conn
            .prepare("SELECT kind FROM files ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(kinds, ["gcode", "text", "ply", "gltf", "other"], "new formats reclassified");
        lib.update_dir(Path::new("/r"), Path::new("/r"), 2, &[file("/r", "a.stl", 1, 1)], &[]).unwrap();
        lib.store_meta(Path::new("/r/a.stl"), "k", &ThumbMeta { content_hash: Some("h".into()), ..Default::default() })
            .unwrap();
        lib.tag(&[PathBuf::from("/r/a.stl")], "x").unwrap();
        assert_eq!(lib.all_tags().unwrap(), [("x".into(), 1)]);
    }

    #[test]
    fn printers_filter_what_fits_on_them() {
        let mut lib = Library::open_in_memory().unwrap();
        let r = Path::new("/r");
        let files = ["mittel.stl", "riesig.stl", "stab.stl", "s1.gcode", "mini.gcode", "neu.stl"].map(|n| file("/r", n, 1, 1));
        lib.update_dir(r, r, 1, &files, &[]).unwrap();
        for (name, size, sliced_for) in [
            ("mittel.stl", [170.0, 170.0, 10.0], None),
            ("riesig.stl", [300.0, 300.0, 10.0], None),
            ("stab.stl", [320.0, 10.0, 10.0], None),
            ("s1.gcode", [10.0, 10.0, 1.0], Some("Anycubic Kobra S1 0.4 nozzle")),
            ("mini.gcode", [10.0, 10.0, 1.0], Some("Bambu Lab A1 mini 0.4 nozzle")),
        ] {
            let gcode = sliced_for.map(|p| Box::new(GcodeFacts { printer: Some(p.into()), ..Default::default() }));
            let meta = ThumbMeta { size_mm: Some(size), gcode, ..Default::default() };
            lib.store_meta(&r.join(name), "k", &meta).unwrap();
        }
        let search = |lib: &Library, q: &str| -> Vec<String> {
            lib.search(&query::parse(q), 99).unwrap().into_iter().map(|e| e.name).collect()
        };
        assert!(search(&lib, "is:zugross").is_empty(), "no printers: nothing is too big");
        assert!(search(&lib, "passt:a1").is_empty(), "unknown printer matches nothing");

        let printer = |name: &str, side: f32| Printer::from(printer::Bed { name: name.into(), x: side, y: side, z: side });
        lib.set_printers(&[printer("Anycubic Kobra S1", 250.0), printer("Bambu Lab A1 mini", 180.0)]).unwrap();
        assert_eq!(search(&lib, r#"passt:"Kobra S1""#), ["mittel.stl", "s1.gcode", "stab.stl"], "diagonal rod fits");
        assert_eq!(search(&lib, "passt:mini"), ["mini.gcode", "mittel.stl"]);
        assert_eq!(search(&lib, "is:zugross"), ["riesig.stl"], "unprocessed neu.stl is not judged");
        assert_eq!(search(&lib, "passt:mini passt:s1"), ["mittel.stl"]);
    }

    #[test]
    fn printability_checks_are_kept_and_searched() {
        let mut lib = Library::open_in_memory().unwrap();
        let r = Path::new("/r");
        let files = ["dicht.stl", "loch.stl", "winzig.stl", "neu.stl", "kopie.stl"].map(|n| file("/r", n, 1, 1));
        lib.update_dir(r, r, 1, &files, &[]).unwrap();
        let sound = crate::checks::check(&crate::Mesh::cube(20.0));
        let holey = MeshChecks { open_edges: 3, ..sound.clone() };
        for (name, hash, size, checks) in [
            ("dicht.stl", "h1", 20.0, Some(sound.clone())),
            ("loch.stl", "h2", 20.0, Some(holey)),
            ("winzig.stl", "h3", 3.0, Some(sound.clone())),
            ("kopie.stl", "h2", 20.0, None),
        ] {
            let meta = ThumbMeta {
                size_mm: Some([size; 3]),
                content_hash: Some(hash.into()),
                checks: checks.map(Box::new),
                ..Default::default()
            };
            lib.store_meta(&r.join(name), "k", &meta).unwrap();
        }
        lib.store_meta(&r.join("neu.stl"), "k", &ThumbMeta { content_hash: Some("h4".into()), ..Default::default() })
            .unwrap();
        assert_eq!(lib.checks_of(&r.join("kopie.stl")).unwrap().map(|c| c.open_edges), Some(3), "shared by content");
        lib.store_meta(&r.join("loch.stl"), "k2", &ThumbMeta::default()).unwrap();
        assert!(lib.checks_of(&r.join("loch.stl")).unwrap().is_some(), "a failure from the cache keeps the hash");
        let waiting: Vec<String> = lib.needing_checks(9).unwrap().into_iter().map(|(_, h)| h).collect();
        assert_eq!(waiting, ["h4"]);
        let search = |q: &str| -> Vec<String> {
            lib.search(&query::parse(q), 99).unwrap().into_iter().map(|e| e.name).collect()
        };
        assert_eq!(search("is:problem"), ["kopie.stl", "loch.stl", "winzig.stl"]);
        let marks = lib.marks().unwrap();
        assert!(marks[&r.join("loch.stl")].problem && !marks.contains_key(&r.join("dicht.stl")));
    }

    #[test]
    fn reader_opens_only_a_ready_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        assert!(Library::open_reader(&path).is_none(), "no index yet");
        let mut writer = Library::open(&path).unwrap();
        writer.update_dir(Path::new("/r"), Path::new("/r"), 1, &[file("/r", "a.stl", 1, 1)], &[]).unwrap();
        let reader = Library::open_reader(&path).expect("index exists now");
        assert_eq!(names(&reader.search(&query::parse("a"), 9).unwrap()), ["a.stl"]);
    }

    #[test]
    fn retain_roots_forgets_removed_libraries() {
        let mut lib = library();
        lib.retain_roots(&[PathBuf::from("/lib/3D_druck")]).unwrap();
        assert_eq!(lib.counts().unwrap().files, 4);
        lib.retain_roots(&[PathBuf::from("/other")]).unwrap();
        assert_eq!(lib.counts().unwrap().files, 0);
    }
}
