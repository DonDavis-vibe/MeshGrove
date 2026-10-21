//! The library index: every folder and file below the library roots, plus the model facts
//! learned while making thumbnails. SQLite in WAL mode, so the UI can read while the
//! background scanner writes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

use crate::cache::{ThumbCache, ThumbMeta};
use crate::formats::Kind;
use crate::listing::Entry;
use crate::query::Query;
use crate::userdata::{UserData, UserEntry};

pub use rusqlite::Error;

const SCHEMA_VERSION: i32 = 4;
/// The index proper: a cache of the disk, rebuilt when its layout changes.
const INDEX_SCHEMA: &str = "
CREATE TABLE dirs (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    parent TEXT,
    mtime_ns INTEGER NOT NULL
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
    conn: Connection,
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
        (version == SCHEMA_VERSION).then_some(Library { conn })
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
            } else if version < 4 {
                // v4 knows OBJ, PLY and glTF; unchanged folders are not rescanned, so reclassify.
                for (kind, ext) in [("obj", "obj"), ("ply", "ply"), ("gltf", "glb"), ("gltf", "gltf")] {
                    conn.execute(
                        "UPDATE files SET kind = ?1, meta_key = NULL WHERE kind = 'other' AND name LIKE '%.' || ?2",
                        params![kind, ext],
                    )?;
                }
            }
            conn.execute_batch(TAGS_SCHEMA)?; // v2 added tags, v3 notes; the index tables are unchanged
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        Ok(Library { conn })
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
    pub fn update_dir(
        &mut self,
        root: &Path,
        dir: &Path,
        mtime_ns: i64,
        files: &[Entry],
        subdirs: &[PathBuf],
    ) -> Result<(), Error> {
        let Some(dir_s) = path_str(dir) else { return Ok(()) };
        let parent = if dir == root { None } else { dir.parent().and_then(path_str) };
        let rel = dir.strip_prefix(root).unwrap_or(dir).to_string_lossy().to_lowercase();
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO dirs (path, parent, mtime_ns) VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET parent = excluded.parent, mtime_ns = excluded.mtime_ns",
            params![dir_s, parent, mtime_ns],
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
                "INSERT INTO files (dir_id, name, kind, size, mtime_ns, search) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            let mut update = tx.prepare_cached(
                "UPDATE files SET kind = ?2, size = ?3, mtime_ns = ?4, meta_key = NULL, triangles = NULL,
                 size_x = NULL, size_y = NULL, size_z = NULL, content_hash = NULL, geo_sig = NULL WHERE id = ?1",
            )?;
            for f in files.iter().filter(|f| !f.is_dir()) {
                let mtime = nanos(f.modified);
                match known.remove(&f.name) {
                    Some((_, size, old_mtime)) if size == f.size as i64 && old_mtime == mtime => {}
                    Some((id, _, _)) => {
                        update.execute(params![id, f.kind.as_str(), f.size as i64, mtime])?;
                    }
                    None => {
                        let search = format!("{}\n{rel}", f.name.to_lowercase());
                        insert.execute(params![dir_id, f.name, f.kind.as_str(), f.size as i64, mtime, search])?;
                    }
                }
            }
            let mut delete = tx.prepare_cached("DELETE FROM files WHERE id = ?1")?;
            for (id, _, _) in known.into_values() {
                delete.execute([id])?;
            }
        }
        let children: Vec<String> = {
            let mut stmt = tx.prepare_cached("SELECT path FROM dirs WHERE parent = ?1")?;
            stmt.query_map([dir_s], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        for child in children {
            if !subdirs.iter().any(|s| path_str(s) == Some(child.as_str())) {
                remove_tree(&tx, Path::new(&child))?;
            }
        }
        tx.commit()
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

    /// Stores the facts learned while making `path`'s thumbnail (cache key `key`).
    pub fn store_meta(&mut self, path: &Path, key: &str, meta: &ThumbMeta) -> Result<(), Error> {
        let (Some(dir), Some(name)) = (path.parent().and_then(path_str), path.file_name().and_then(|n| n.to_str()))
        else {
            return Ok(());
        };
        let size = meta.size_mm.unwrap_or([f32::NAN; 3]).map(|v| if v.is_finite() { Some(v as f64) } else { None });
        self.conn.execute(
            "UPDATE files SET meta_key = ?3, triangles = ?4, size_x = ?5, size_y = ?6, size_z = ?7,
                 content_hash = ?8, geo_sig = ?9
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
                meta.geo_sig
            ],
        )?;
        Ok(())
    }

    /// Models, images and PDFs whose thumbnail facts are missing or out of date, for background processing.
    pub fn needing_meta(&self) -> Result<Vec<(Entry, String)>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT d.path, f.name, f.kind, f.size, f.mtime_ns, f.meta_key FROM files f JOIN dirs d ON d.id = f.dir_id
             WHERE f.kind IN ('stl', '3mf', 'step', 'image', 'pdf') ORDER BY d.path, f.name",
        )?;
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
        let mut sql = String::from(
            "SELECT d.path, f.name, f.kind, f.size, f.mtime_ns FROM files f JOIN dirs d ON d.id = f.dir_id WHERE 1",
        );
        let mut args: Vec<String> = Vec::new();
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
        if !query.kinds.is_empty() {
            let list: Vec<String> = query.kinds.iter().map(|k| format!("'{}'", k.as_str())).collect();
            sql += &format!(" AND f.kind IN ({})", list.join(","));
        }
        sql += &format!(" ORDER BY f.name COLLATE NOCASE LIMIT {limit}");
        let mut stmt = self.conn.prepare(&sql)?;
        stmt.query_map(params_from_iter(args), entry_from_row)?.collect()
    }

    /// Up to `n` thumbnail-capable files in or below `dir` for its mosaic: models before images,
    /// files directly inside before those in subfolders.
    pub fn folder_preview(&self, dir: &Path, n: usize) -> Result<Vec<Entry>, Error> {
        let Some(d) = path_str(dir) else { return Ok(Vec::new()) };
        let mut stmt = self.conn.prepare_cached(
            "SELECT d.path, f.name, f.kind, f.size, f.mtime_ns FROM files f JOIN dirs d ON d.id = f.dir_id
             WHERE (d.path = ?1 OR (d.path > ?2 AND d.path < ?3)) AND f.kind IN ('stl', '3mf', 'step', 'image')
             ORDER BY (f.kind = 'image'), (d.path = ?1) DESC, d.path, f.name COLLATE NOCASE LIMIT ?4",
        )?;
        stmt.query_map(params![d, format!("{d}/"), format!("{d}0"), n as i64], entry_from_row)?.collect()
    }

    fn content_hash_of(&self, path: &Path) -> Result<Option<String>, Error> {
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
    fn ensure_content_hash(&mut self, path: &Path) -> Result<Option<String>, Error> {
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

    /// Tags and notes, keyed by content, with the current paths of those files for reference.
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
        let mut stmt = self
            .conn
            .prepare("SELECT d.path, f.name FROM files f JOIN dirs d ON d.id = f.dir_id WHERE f.content_hash = ?1")?;
        for e in entries.values_mut() {
            for row in stmt.query_map([&e.content_hash], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
                let (dir, name) = row?;
                e.files.push(format!("{dir}/{name}"));
            }
        }
        Ok(UserData::new(entries.into_values().collect()))
    }

    /// Merges exported tags and notes: tags are added, a note only fills a file without one.
    /// Returns how many entries were applied.
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
        Ok(data.entries.len())
    }

    /// Groups of duplicate models, the ones wasting most space first.
    pub fn duplicate_groups(&self) -> Result<Vec<DuplicateGroup>, Error> {
        let select = "SELECT d.path, f.name, f.kind, f.size, f.mtime_ns FROM files f JOIN dirs d ON d.id = f.dir_id";
        let mut groups = Vec::new();
        for (exact, keys_sql, files_sql) in [
            (
                true,
                "SELECT content_hash FROM files WHERE content_hash IS NOT NULL AND kind IN ('stl', '3mf', 'step')
                 GROUP BY content_hash HAVING COUNT(*) > 1",
                format!("{select} WHERE f.content_hash = ?1 ORDER BY d.path, f.name"),
            ),
            (
                false,
                // Same geometry but not all byte-identical (re-exports, other formats, moved parts).
                "SELECT geo_sig FROM files WHERE geo_sig IS NOT NULL
                 GROUP BY geo_sig HAVING COUNT(DISTINCT content_hash) > 1",
                format!("{select} WHERE f.geo_sig = ?1 ORDER BY d.path, f.name"),
            ),
        ] {
            let keys: Vec<String> =
                self.conn.prepare(keys_sql)?.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
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
            "SELECT COUNT(*),
                    COALESCE(SUM(kind IN ('stl', '3mf', 'step')), 0),
                    COALESCE(SUM(meta_key IS NOT NULL), 0),
                    COALESCE(SUM(kind IN ('stl', '3mf', 'step', 'image', 'pdf')), 0)
             FROM files",
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

fn entry_from_row(r: &rusqlite::Row) -> Result<Entry, Error> {
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
fn path_str(p: &Path) -> Option<&str> {
    p.to_str()
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::*;
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
        lib.update_dir(root, root, 1, &[file("/lib/3D_druck", "readme.txt", 5, 1)], &[minis.into()]).unwrap();
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
    fn old_index_versions_are_upgraded_keeping_their_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(INDEX_SCHEMA).unwrap();
            conn.execute("INSERT INTO dirs (path, parent, mtime_ns) VALUES ('/r', NULL, 1)", []).unwrap();
            for name in ["Scan.PLY", "fig.glb", "notes.txt"] {
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
        assert_eq!(kinds, ["ply", "gltf", "other"], "new model formats reclassified");
        lib.update_dir(Path::new("/r"), Path::new("/r"), 2, &[file("/r", "a.stl", 1, 1)], &[]).unwrap();
        lib.store_meta(Path::new("/r/a.stl"), "k", &ThumbMeta { content_hash: Some("h".into()), ..Default::default() })
            .unwrap();
        lib.tag(&[PathBuf::from("/r/a.stl")], "x").unwrap();
        assert_eq!(lib.all_tags().unwrap(), [("x".into(), 1)]);
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
