//! Lists of library files – favourites, print lists and collections – plus the print log and
//! the colours chosen for tags. Like tags and notes they hang on file content, so they survive
//! renames and moves and are shared by identical copies.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::{Error, Library, entry_from_row, nanos};
use crate::listing::Entry;
use crate::query;
use crate::userdata::{ListData, ListItemData, PrintRecord};

/// User data next to the tags: kept across index rebuilds.
pub(crate) const LISTS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS lists (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    name TEXT NOT NULL COLLATE NOCASE,
    -- order in the sidebar, per kind
    position INTEGER NOT NULL,
    archived INTEGER NOT NULL DEFAULT 0,
    -- automatic collections: a saved search whose results are the entries
    query TEXT,
    created_ns INTEGER NOT NULL,
    UNIQUE (kind, name)
);
CREATE TABLE IF NOT EXISTS list_items (
    list_id INTEGER NOT NULL REFERENCES lists(id) ON DELETE CASCADE,
    content_hash TEXT NOT NULL,
    -- print lists: how many to print, and how many of them are done
    quantity INTEGER NOT NULL DEFAULT 1,
    done INTEGER NOT NULL DEFAULT 0,
    added_ns INTEGER NOT NULL,
    PRIMARY KEY (list_id, content_hash)
);
CREATE INDEX IF NOT EXISTS list_items_content ON list_items(content_hash);
CREATE TABLE IF NOT EXISTS prints (
    id INTEGER PRIMARY KEY,
    content_hash TEXT NOT NULL,
    result TEXT NOT NULL,
    -- whole seconds, so a re-imported backup does not log the same print twice
    at_ns INTEGER NOT NULL,
    list_id INTEGER REFERENCES lists(id) ON DELETE SET NULL,
    note TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS prints_unique ON prints(content_hash, result, at_ns);
-- Chosen palette slots; a tag without a row gets its automatic colour. Separate from `tags`,
-- whose unused rows are deleted, so a colour outlives the tag's last file.
CREATE TABLE IF NOT EXISTS tag_colors (
    name TEXT PRIMARY KEY COLLATE NOCASE,
    color INTEGER NOT NULL
);
";

/// Most results an automatic collection shows (and counts).
const SMART_LIMIT: usize = 10_000;

/// Brings lists tables of early 0.4 builds up to date (they had no `query`); run at every open.
pub(crate) fn upgrade(conn: &Connection) -> Result<(), Error> {
    let has_query = conn.prepare("SELECT 1 FROM pragma_table_info('lists') WHERE name = 'query'")?.exists([])?;
    let has_lists = conn.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'lists'")?.exists([])?;
    if has_lists && !has_query {
        conn.execute_batch("ALTER TABLE lists ADD COLUMN query TEXT")?;
    }
    Ok(())
}

/// What [`Library::export_lists`] hands to the user-data export.
pub(crate) struct ExportedLists {
    pub lists: Vec<ListData>,
    /// Logged prints by content hash.
    pub prints: BTreeMap<String, Vec<PrintRecord>>,
    pub tag_colors: BTreeMap<String, u8>,
}

/// The one favourites list's internal name (the UI shows its own label).
const FAVORITES: &str = "favorites";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListKind {
    Favorites,
    Print,
    Collection,
}

impl ListKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ListKind::Favorites => "favorites",
            ListKind::Print => "print",
            ListKind::Collection => "collection",
        }
    }

    pub fn parse(s: &str) -> Option<ListKind> {
        [ListKind::Favorites, ListKind::Print, ListKind::Collection].into_iter().find(|k| k.as_str() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListInfo {
    pub id: i64,
    pub kind: ListKind,
    pub name: String,
    /// Print lists that were finished; they stay for the record.
    pub archived: bool,
    /// Automatic collections: the saved search that makes up their entries.
    pub query: Option<String>,
    /// Entries whose content is in the index now.
    pub count: usize,
    /// Print lists: entries with copies still to print.
    pub open: usize,
}

/// One entry of a list: a file with the content, and for print lists the copies wanted and done.
#[derive(Debug, Clone, PartialEq)]
pub struct ListEntry {
    pub entry: Entry,
    pub quantity: u32,
    pub done: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrintResult {
    Printed,
    Failed,
}

impl PrintResult {
    fn as_str(self) -> &'static str {
        match self {
            PrintResult::Printed => "printed",
            PrintResult::Failed => "failed",
        }
    }

    fn parse(s: &str) -> PrintResult {
        if s == "failed" { PrintResult::Failed } else { PrintResult::Printed }
    }
}

/// The print log of some files, summed up.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrintStatus {
    pub printed: u32,
    pub failed: u32,
    pub last_printed: Option<SystemTime>,
    /// The newest entry of the log.
    pub last: Option<(PrintResult, SystemTime)>,
}

/// What a gallery tile shows about a file besides its own facts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Marks {
    pub favorite: bool,
    /// How often it was printed.
    pub printed: u32,
    /// The newest print of it failed.
    pub last_failed: bool,
    pub tags: Vec<String>,
}

impl Library {
    /// Every list, favourites first, then print lists and collections in sidebar order.
    pub fn lists(&self) -> Result<Vec<ListInfo>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT l.id, l.kind, l.name, l.archived, l.query,
                    COUNT(DISTINCT CASE WHEN f.id IS NOT NULL THEN li.content_hash END),
                    COUNT(DISTINCT CASE WHEN f.id IS NOT NULL AND li.done < li.quantity THEN li.content_hash END)
             FROM lists l
             LEFT JOIN list_items li ON li.list_id = l.id
             LEFT JOIN files f ON f.content_hash = li.content_hash
             GROUP BY l.id
             ORDER BY CASE l.kind WHEN 'favorites' THEN 0 WHEN 'print' THEN 1 ELSE 2 END, l.position, l.id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })?;
        let mut lists = Vec::new();
        for row in rows {
            let (id, kind, name, archived, query, count, open) = row?;
            let Some(kind) = ListKind::parse(&kind) else { continue };
            let open = if kind == ListKind::Print { open as usize } else { 0 };
            let count = match &query {
                Some(q) => self.search(&query::parse(q), SMART_LIMIT)?.len(),
                None => count as usize,
            };
            lists.push(ListInfo { id, kind, name, archived, query, count, open });
        }
        Ok(lists)
    }

    /// The favourites list, created on first use.
    pub fn favorites_id(&mut self) -> Result<i64, Error> {
        self.create_list(ListKind::Favorites, FAVORITES)
    }

    /// A new list at the end of its kind, or the existing one of that kind and name.
    pub fn create_list(&mut self, kind: ListKind, name: &str) -> Result<i64, Error> {
        let name = if kind == ListKind::Favorites { FAVORITES } else { name.trim() };
        if name.is_empty() {
            return Err(Error::InvalidParameterName("empty list name".into()));
        }
        self.conn.execute(
            "INSERT OR IGNORE INTO lists (kind, name, position, created_ns)
             VALUES (?1, ?2, (SELECT COALESCE(MAX(position), -1) + 1 FROM lists WHERE kind = ?1), ?3)",
            params![kind.as_str(), name, now_ns()],
        )?;
        self.conn
            .query_row("SELECT id FROM lists WHERE kind = ?1 AND name = ?2", params![kind.as_str(), name], |r| r.get(0))
    }

    /// An automatic collection: its entries are the results of the search `query`.
    pub fn create_smart_list(&mut self, name: &str, query: &str) -> Result<i64, Error> {
        let id = self.create_list(ListKind::Collection, name)?;
        self.conn.execute("UPDATE lists SET query = ?2 WHERE id = ?1", params![id, query.trim()])?;
        Ok(id)
    }

    /// False if another list of the same kind already has the name.
    pub fn rename_list(&mut self, id: i64, name: &str) -> Result<bool, Error> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(false);
        }
        match self.conn.execute("UPDATE lists SET name = ?2 WHERE id = ?1 AND kind != 'favorites'", params![id, name]) {
            Ok(_) => Ok(true),
            Err(Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Deletes the list and its entries; the print log keeps its prints.
    pub fn delete_list(&mut self, id: i64) -> Result<(), Error> {
        self.conn.execute("DELETE FROM lists WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn set_list_archived(&mut self, id: i64, archived: bool) -> Result<(), Error> {
        self.conn.execute("UPDATE lists SET archived = ?2 WHERE id = ?1", params![id, archived])?;
        Ok(())
    }

    /// Adds the library files at `paths`; returns how many were new to the list.
    pub fn add_to_list(&mut self, id: i64, paths: &[PathBuf]) -> Result<usize, Error> {
        let mut added = 0;
        for path in paths {
            if let Some(hash) = self.ensure_content_hash(path)? {
                added += self.conn.execute(
                    "INSERT OR IGNORE INTO list_items (list_id, content_hash, added_ns) VALUES (?1, ?2, ?3)",
                    params![id, hash, now_ns()],
                )?;
            }
        }
        Ok(added)
    }

    pub fn remove_from_list(&mut self, id: i64, paths: &[PathBuf]) -> Result<(), Error> {
        for hash in self.hashes(paths)? {
            self.conn.execute("DELETE FROM list_items WHERE list_id = ?1 AND content_hash = ?2", params![id, hash])?;
        }
        Ok(())
    }

    /// Puts the files at `paths` on the favourites list or takes them off.
    pub fn set_favorite(&mut self, paths: &[PathBuf], favorite: bool) -> Result<(), Error> {
        let id = self.favorites_id()?;
        if favorite {
            self.add_to_list(id, paths)?;
        } else {
            self.remove_from_list(id, paths)?;
        }
        Ok(())
    }

    /// Copies wanted on a print list (at least one); copies done never exceed them.
    pub fn set_quantity(&mut self, id: i64, paths: &[PathBuf], quantity: u32) -> Result<(), Error> {
        let quantity = quantity.max(1);
        for hash in self.hashes(paths)? {
            self.conn.execute(
                "UPDATE list_items SET quantity = ?3, done = MIN(done, ?3) WHERE list_id = ?1 AND content_hash = ?2",
                params![id, hash, quantity],
            )?;
        }
        Ok(())
    }

    /// The list's files, one per content (the first copy by path). Print lists put entries
    /// with copies left to print first.
    pub fn list_entries(&self, id: i64) -> Result<Vec<ListEntry>, Error> {
        let saved: Option<String> =
            self.conn.query_row("SELECT query FROM lists WHERE id = ?1", [id], |r| r.get(0)).optional()?.flatten();
        if let Some(q) = saved {
            let found = self.search(&query::parse(&q), SMART_LIMIT)?;
            return Ok(found.into_iter().map(|entry| ListEntry { entry, quantity: 1, done: 0 }).collect());
        }
        let mut stmt = self.conn.prepare(
            "SELECT d.path, f.name, f.kind, f.size, f.mtime_ns, li.quantity, li.done
             FROM list_items li
             JOIN lists l ON l.id = li.list_id
             JOIN files f ON f.id = (
                 SELECT f2.id FROM files f2 JOIN dirs d2 ON d2.id = f2.dir_id
                 WHERE f2.content_hash = li.content_hash ORDER BY d2.path, f2.name LIMIT 1)
             JOIN dirs d ON d.id = f.dir_id
             WHERE li.list_id = ?1
             ORDER BY CASE WHEN l.kind = 'print' THEN li.done >= li.quantity ELSE 0 END, f.name COLLATE NOCASE",
        )?;
        stmt.query_map([id], |r| Ok(ListEntry { entry: entry_from_row(r)?, quantity: r.get(5)?, done: r.get(6)? }))?
            .collect()
    }

    /// The lists holding any of `paths`, with how many of those files each holds.
    pub fn lists_of(&self, paths: &[PathBuf]) -> Result<Vec<(ListInfo, usize)>, Error> {
        let mut counts: HashMap<i64, usize> = HashMap::new();
        let mut stmt = self.conn.prepare_cached("SELECT list_id FROM list_items WHERE content_hash = ?1")?;
        for hash in self.hashes(paths)? {
            for id in stmt.query_map([hash], |r| r.get::<_, i64>(0))? {
                *counts.entry(id?).or_default() += 1;
            }
        }
        Ok(self.lists()?.into_iter().filter_map(|l| counts.get(&l.id).map(|&n| (l.clone(), n))).collect())
    }

    /// Logs a print of each file at `paths`. Printed on a print list also counts a copy as done.
    pub fn record_print(&mut self, paths: &[PathBuf], result: PrintResult, list: Option<i64>) -> Result<(), Error> {
        let at = now_ns();
        for path in paths {
            let Some(hash) = self.ensure_content_hash(path)? else { continue };
            self.conn.execute(
                "INSERT OR IGNORE INTO prints (content_hash, result, at_ns, list_id) VALUES (?1, ?2, ?3, ?4)",
                params![hash, result.as_str(), at, list],
            )?;
            if result == PrintResult::Printed
                && let Some(list) = list
            {
                self.conn.execute(
                    "UPDATE list_items SET done = MIN(done + 1, quantity) WHERE list_id = ?1 AND content_hash = ?2",
                    params![list, hash],
                )?;
            }
        }
        Ok(())
    }

    /// Takes back the newest log entry of each file at `paths`, and the copy it counted on a list.
    pub fn undo_print(&mut self, paths: &[PathBuf]) -> Result<(), Error> {
        for hash in self.hashes(paths)? {
            let last = self
                .conn
                .query_row(
                    "SELECT id, result, list_id FROM prints WHERE content_hash = ?1 ORDER BY at_ns DESC, id DESC LIMIT 1",
                    [&hash],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<i64>>(2)?)),
                )
                .optional()?;
            let Some((id, result, list)) = last else { continue };
            self.conn.execute("DELETE FROM prints WHERE id = ?1", [id])?;
            if PrintResult::parse(&result) == PrintResult::Printed
                && let Some(list) = list
            {
                self.conn.execute(
                    "UPDATE list_items SET done = MAX(done - 1, 0) WHERE list_id = ?1 AND content_hash = ?2",
                    params![list, hash],
                )?;
            }
        }
        Ok(())
    }

    /// The print log of the files at `paths`, summed up.
    pub fn print_status(&self, paths: &[PathBuf]) -> Result<PrintStatus, Error> {
        let mut status = PrintStatus::default();
        let mut stmt = self.conn.prepare_cached("SELECT result, at_ns FROM prints WHERE content_hash = ?1")?;
        for hash in self.hashes(paths)? {
            for row in stmt.query_map([hash], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
                let (result, at) = row?;
                let (result, at) = (PrintResult::parse(&result), time(at));
                match result {
                    PrintResult::Printed => {
                        status.printed += 1;
                        status.last_printed = status.last_printed.max(Some(at));
                    }
                    PrintResult::Failed => status.failed += 1,
                }
                if status.last.is_none_or(|(_, t)| at >= t) {
                    status.last = Some((result, at));
                }
            }
        }
        Ok(status)
    }

    /// Favourite, print and tag marks of every indexed file that has any.
    pub fn marks(&self) -> Result<HashMap<PathBuf, Marks>, Error> {
        let mut by_hash: HashMap<String, Marks> = HashMap::new();
        let mut stmt = self.conn.prepare(
            "SELECT li.content_hash FROM list_items li JOIN lists l ON l.id = li.list_id WHERE l.kind = 'favorites'",
        )?;
        for hash in stmt.query_map([], |r| r.get::<_, String>(0))? {
            by_hash.entry(hash?).or_default().favorite = true;
        }
        let mut stmt = self.conn.prepare(
            "SELECT content_hash,
                    SUM(result = 'printed'),
                    (SELECT p2.result FROM prints p2 WHERE p2.content_hash = p.content_hash
                     ORDER BY p2.at_ns DESC, p2.id DESC LIMIT 1)
             FROM prints p GROUP BY content_hash",
        )?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))? {
            let (hash, printed, last) = row?;
            let marks = by_hash.entry(hash).or_default();
            marks.printed = printed as u32;
            marks.last_failed = PrintResult::parse(&last) == PrintResult::Failed;
        }
        let mut stmt = self.conn.prepare(
            "SELECT ft.content_hash, t.name FROM file_tags ft JOIN tags t ON t.id = ft.tag_id
             ORDER BY t.name COLLATE NOCASE",
        )?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (hash, tag) = row?;
            by_hash.entry(hash).or_default().tags.push(tag);
        }
        let mut stmt = self
            .conn
            .prepare("SELECT d.path, f.name, f.content_hash FROM files f JOIN dirs d ON d.id = f.dir_id WHERE f.content_hash IS NOT NULL")?;
        let mut out = HashMap::new();
        for row in
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?
        {
            let (dir, name, hash) = row?;
            if let Some(marks) = by_hash.get(&hash) {
                out.insert(Path::new(&dir).join(name), marks.clone());
            }
        }
        Ok(out)
    }

    /// Chosen palette slots by lower-case tag name.
    pub fn tag_colors(&self) -> Result<HashMap<String, u8>, Error> {
        let mut stmt = self.conn.prepare("SELECT name, color FROM tag_colors")?;
        stmt.query_map([], |r| Ok((r.get::<_, String>(0)?.to_lowercase(), r.get::<_, u8>(1)?)))?.collect()
    }

    /// Chooses a palette slot for a tag; `None` goes back to the automatic colour.
    pub fn set_tag_color(&mut self, tag: &str, color: Option<u8>) -> Result<(), Error> {
        let tag = tag.trim();
        match color {
            Some(color) => self.conn.execute(
                "INSERT INTO tag_colors (name, color) VALUES (?1, ?2)
                 ON CONFLICT(name) DO UPDATE SET color = excluded.color",
                params![tag, color],
            )?,
            None => self.conn.execute("DELETE FROM tag_colors WHERE name = ?1", [tag])?,
        };
        Ok(())
    }

    /// Lists, print log and tag colours for the user-data export.
    pub(crate) fn export_lists(&self) -> Result<ExportedLists, Error> {
        let mut lists = Vec::new();
        let mut stmt = self
            .conn
            .prepare("SELECT id, kind, name, archived, created_ns, query FROM lists ORDER BY kind, position, id")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })?;
        let mut items_stmt = self.conn.prepare(
            "SELECT content_hash, quantity, done, added_ns FROM list_items WHERE list_id = ?1
             ORDER BY added_ns, content_hash",
        )?;
        for row in rows {
            let (id, kind, name, archived, created, query) = row?;
            let Some(kind) = ListKind::parse(&kind) else { continue };
            let items = items_stmt
                .query_map([id], |r| {
                    Ok(ListItemData {
                        content_hash: r.get(0)?,
                        quantity: r.get(1)?,
                        done: r.get(2)?,
                        added: secs(r.get(3)?),
                    })
                })?
                .collect::<Result<_, _>>()?;
            lists.push(ListData { kind, name, archived, query, created: secs(created), items });
        }
        let mut prints: BTreeMap<String, Vec<PrintRecord>> = BTreeMap::new();
        let mut stmt = self.conn.prepare(
            "SELECT p.content_hash, p.result, p.at_ns, l.name, p.note FROM prints p
             LEFT JOIN lists l ON l.id = p.list_id ORDER BY p.at_ns, p.id",
        )?;
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })? {
            let (hash, result, at, list, note) = row?;
            prints.entry(hash).or_default().push(PrintRecord {
                result: PrintResult::parse(&result),
                at: secs(at),
                list,
                note,
            });
        }
        let colors = self
            .conn
            .prepare("SELECT name, color FROM tag_colors ORDER BY name")?
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u8>(1)?)))?
            .collect::<Result<_, _>>()?;
        Ok(ExportedLists { lists, prints, tag_colors: colors })
    }

    /// Merges exported lists, prints and colours: missing lists are created, entries added
    /// (existing ones keep their counts), prints logged once, chosen colours kept.
    pub(crate) fn import_lists(
        &mut self,
        lists: &[ListData],
        prints: &[(String, Vec<PrintRecord>)],
        colors: &BTreeMap<String, u8>,
    ) -> Result<(), Error> {
        let tx = self.conn.transaction()?;
        let mut print_lists: HashMap<String, i64> = HashMap::new();
        for list in lists {
            let name = if list.kind == ListKind::Favorites { FAVORITES } else { list.name.trim() };
            if name.is_empty() {
                continue;
            }
            tx.execute(
                "INSERT OR IGNORE INTO lists (kind, name, position, archived, query, created_ns)
                 VALUES (?1, ?2, (SELECT COALESCE(MAX(position), -1) + 1 FROM lists WHERE kind = ?1), ?3, ?4, ?5)",
                params![list.kind.as_str(), name, list.archived, list.query, list.created * 1_000_000_000],
            )?;
            let id: i64 = tx.query_row(
                "SELECT id FROM lists WHERE kind = ?1 AND name = ?2",
                params![list.kind.as_str(), name],
                |r| r.get(0),
            )?;
            if list.kind == ListKind::Print {
                print_lists.insert(name.to_lowercase(), id);
            }
            for item in &list.items {
                tx.execute(
                    "INSERT OR IGNORE INTO list_items (list_id, content_hash, quantity, done, added_ns)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        id,
                        item.content_hash,
                        item.quantity.max(1),
                        item.done.min(item.quantity.max(1)),
                        item.added * 1_000_000_000
                    ],
                )?;
            }
        }
        for (hash, records) in prints {
            for p in records {
                let list = p.list.as_ref().and_then(|name| print_lists.get(&name.to_lowercase()).copied());
                tx.execute(
                    "INSERT OR IGNORE INTO prints (content_hash, result, at_ns, list_id, note) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![hash, p.result.as_str(), p.at * 1_000_000_000, list, p.note],
                )?;
            }
        }
        for (name, color) in colors {
            tx.execute("INSERT OR IGNORE INTO tag_colors (name, color) VALUES (?1, ?2)", params![name, color])?;
        }
        tx.commit()
    }

    /// Content hashes of the library files at `paths`, each once.
    fn hashes(&self, paths: &[PathBuf]) -> Result<Vec<String>, Error> {
        let mut out: Vec<String> = Vec::new();
        for path in paths {
            if let Some(hash) = self.content_hash_of(path)?
                && !out.contains(&hash)
            {
                out.push(hash);
            }
        }
        Ok(out)
    }
}

/// Now, in whole seconds as nanoseconds (see the `prints` table).
fn now_ns() -> i64 {
    nanos(Some(SystemTime::now())) / 1_000_000_000 * 1_000_000_000
}

fn secs(ns: i64) -> i64 {
    ns / 1_000_000_000
}

fn time(ns: i64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_nanos(ns.max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::ThumbMeta;
    use crate::formats::Kind;
    use crate::query;

    const ROOT: &str = "/lib/3D_druck";

    fn file(dir: &str, name: &str) -> Entry {
        Entry {
            name: name.into(),
            path: Path::new(dir).join(name),
            kind: Kind::from_file_name(name),
            size: 10,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        }
    }

    /// Knight and its copy share content; the readme has no hash.
    fn library() -> Library {
        let mut lib = Library::open_in_memory().unwrap();
        let root = Path::new(ROOT);
        let names = ["Knight.stl", "Knight_copy.stl", "Dragon.3mf", "Lid.stl", "readme.txt"];
        let files: Vec<Entry> = names.iter().map(|n| file(ROOT, n)).collect();
        lib.update_dir(root, root, 1, &files, &[]).unwrap();
        for (name, hash) in
            [("Knight.stl", "h-k"), ("Knight_copy.stl", "h-k"), ("Dragon.3mf", "h-d"), ("Lid.stl", "h-l")]
        {
            let meta = ThumbMeta { content_hash: Some(hash.into()), ..Default::default() };
            lib.store_meta(&path(name), "k", &meta).unwrap();
        }
        lib
    }

    fn path(name: &str) -> PathBuf {
        Path::new(ROOT).join(name)
    }

    fn names(entries: &[ListEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.entry.name.as_str()).collect()
    }

    fn search(lib: &Library, q: &str) -> Vec<String> {
        lib.search(&query::parse(q), 50).unwrap().into_iter().map(|e| e.name).collect()
    }

    #[test]
    fn lists_are_created_renamed_and_deleted() {
        let mut lib = library();
        let markt = lib.create_list(ListKind::Print, "  Weihnachtsmarkt ").unwrap();
        let gifts = lib.create_list(ListKind::Print, "Geschenke").unwrap();
        let airsoft = lib.create_list(ListKind::Collection, "Airsoft").unwrap();
        assert_eq!(lib.create_list(ListKind::Print, "weihnachtsmarkt").unwrap(), markt, "same name, any case");
        assert!(lib.create_list(ListKind::Collection, " ").is_err());
        let fav = lib.favorites_id().unwrap();
        assert_eq!(lib.favorites_id().unwrap(), fav);

        let order: Vec<(ListKind, String)> = lib.lists().unwrap().into_iter().map(|l| (l.kind, l.name)).collect();
        assert_eq!(
            order,
            [
                (ListKind::Favorites, "favorites".into()),
                (ListKind::Print, "Weihnachtsmarkt".into()),
                (ListKind::Print, "Geschenke".into()),
                (ListKind::Collection, "Airsoft".into()),
            ]
        );
        assert!(!lib.rename_list(gifts, "WEIHNACHTSMARKT").unwrap(), "name taken");
        assert!(lib.rename_list(gifts, "Geburtstage").unwrap());
        assert!(lib.rename_list(airsoft, "Weihnachtsmarkt").unwrap(), "other kinds may share a name");

        lib.add_to_list(airsoft, &[path("Lid.stl")]).unwrap();
        lib.delete_list(airsoft).unwrap();
        assert_eq!(lib.lists().unwrap().len(), 3);
        let left: i64 = lib.conn.query_row("SELECT COUNT(*) FROM list_items", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 0, "entries went with the list");
    }

    #[test]
    fn entries_follow_content_and_count_once() {
        let mut lib = library();
        let id = lib.create_list(ListKind::Collection, "Burg").unwrap();
        let added = lib
            .add_to_list(
                id,
                &[path("Knight.stl"), path("Knight_copy.stl"), path("Dragon.3mf"), PathBuf::from("/elsewhere/x.stl")],
            )
            .unwrap();
        assert_eq!(added, 2, "the copy is the same content; files outside the library are skipped");
        assert_eq!(names(&lib.list_entries(id).unwrap()), ["Dragon.3mf", "Knight.stl"]);
        assert_eq!(lib.lists().unwrap()[0].count, 2);

        // Knight renamed on disk: its row is gone, the copy still has the content.
        let root = Path::new(ROOT);
        let files: Vec<Entry> = ["Knight_copy.stl", "Dragon.3mf", "Lid.stl"].iter().map(|n| file(ROOT, n)).collect();
        lib.update_dir(root, root, 2, &files, &[]).unwrap();
        assert_eq!(names(&lib.list_entries(id).unwrap()), ["Dragon.3mf", "Knight_copy.stl"]);

        let (info, n) = lib.lists_of(&[path("Knight_copy.stl"), path("Lid.stl")]).unwrap().remove(0);
        assert_eq!((info.name.as_str(), n), ("Burg", 1));
        lib.remove_from_list(id, &[path("Knight_copy.stl")]).unwrap();
        assert_eq!(names(&lib.list_entries(id).unwrap()), ["Dragon.3mf"]);
    }

    #[test]
    fn print_lists_count_copies_and_log_prints() {
        let mut lib = library();
        let markt = lib.create_list(ListKind::Print, "Markt").unwrap();
        lib.add_to_list(markt, &[path("Knight.stl"), path("Lid.stl")]).unwrap();
        lib.set_quantity(markt, &[path("Knight.stl")], 2).unwrap();
        lib.set_quantity(markt, &[path("Lid.stl")], 0).unwrap();

        lib.record_print(&[path("Knight.stl")], PrintResult::Printed, Some(markt)).unwrap();
        lib.record_print(&[path("Lid.stl")], PrintResult::Failed, Some(markt)).unwrap();
        let entries = lib.list_entries(markt).unwrap();
        let counts: Vec<(&str, u32, u32)> =
            entries.iter().map(|e| (e.entry.name.as_str(), e.quantity, e.done)).collect();
        assert_eq!(counts, [("Knight.stl", 2, 1), ("Lid.stl", 1, 0)], "at least one copy; failed prints count nothing");
        assert_eq!(lib.lists().unwrap()[0].open, 2);

        // A second print finishes Knight (it sinks below Lid); a third never goes past the quantity.
        // Each needs its own second in the log, so the times are shifted by hand.
        lib.conn.execute("UPDATE prints SET at_ns = at_ns - 10000000000", []).unwrap();
        lib.record_print(&[path("Knight_copy.stl")], PrintResult::Printed, Some(markt)).unwrap();
        lib.conn.execute("UPDATE prints SET at_ns = at_ns - 10000000000", []).unwrap();
        lib.record_print(&[path("Knight.stl")], PrintResult::Printed, Some(markt)).unwrap();
        assert_eq!(names(&lib.list_entries(markt).unwrap()), ["Lid.stl", "Knight.stl"]);
        assert_eq!(lib.list_entries(markt).unwrap()[1].done, 2);
        assert_eq!(lib.lists().unwrap()[0].open, 1);

        let status = lib.print_status(&[path("Knight.stl")]).unwrap();
        assert_eq!((status.printed, status.failed), (3, 0));
        assert_eq!(status.last.map(|(r, _)| r), Some(PrintResult::Printed));
        assert!(status.last_printed.is_some());

        lib.undo_print(&[path("Knight.stl")]).unwrap();
        assert_eq!(lib.print_status(&[path("Knight.stl")]).unwrap().printed, 2);
        assert_eq!(lib.list_entries(markt).unwrap().iter().find(|e| e.entry.name == "Knight.stl").unwrap().done, 1);
        let lid = lib.print_status(&[path("Lid.stl")]).unwrap();
        assert_eq!((lid.printed, lid.failed, lid.last.map(|(r, _)| r)), (0, 1, Some(PrintResult::Failed)));

        lib.delete_list(markt).unwrap();
        assert_eq!(lib.print_status(&[path("Knight.stl")]).unwrap().printed, 2, "the log outlives the list");
    }

    #[test]
    fn marks_and_search_filters() {
        let mut lib = library();
        lib.set_favorite(&[path("Dragon.3mf")], true).unwrap();
        lib.tag(&[path("Dragon.3mf")], "Garten").unwrap();
        lib.tag(&[path("Dragon.3mf")], "Deko").unwrap();
        lib.record_print(&[path("Knight.stl")], PrintResult::Printed, None).unwrap();
        lib.conn.execute("UPDATE prints SET at_ns = at_ns - 10000000000", []).unwrap();
        lib.record_print(&[path("Knight.stl")], PrintResult::Failed, None).unwrap();
        lib.record_print(&[path("Lid.stl")], PrintResult::Failed, None).unwrap();
        let markt = lib.create_list(ListKind::Print, "Weihnachts Markt").unwrap();
        lib.add_to_list(markt, &[path("Lid.stl")]).unwrap();

        let marks = lib.marks().unwrap();
        let dragon = &marks[&path("Dragon.3mf")];
        assert!(dragon.favorite);
        assert_eq!(dragon.tags, ["Deko", "Garten"]);
        let knight = &marks[&path("Knight_copy.stl")];
        assert_eq!((knight.printed, knight.last_failed), (1, true), "copies share the marks");
        assert!(!marks.contains_key(&path("readme.txt")));

        assert_eq!(search(&lib, "is:fav"), ["Dragon.3mf"]);
        assert_eq!(search(&lib, "is:gedruckt"), ["Knight.stl", "Knight_copy.stl"]);
        assert_eq!(search(&lib, "is:unprinted"), ["Dragon.3mf", "Lid.stl"], "models only, failed-only counts");
        assert_eq!(search(&lib, "is:failed"), ["Knight.stl", "Knight_copy.stl", "Lid.stl"]);
        assert_eq!(search(&lib, r#"liste:"weihnachts markt""#), ["Lid.stl"]);
        assert_eq!(search(&lib, "is:fav tag:garten"), ["Dragon.3mf"]);
        assert!(search(&lib, "is:fav lid").is_empty());

        lib.set_favorite(&[path("Dragon.3mf")], false).unwrap();
        assert!(search(&lib, "is:fav").is_empty());
    }

    #[test]
    fn tag_colors_outlive_their_tag() {
        let mut lib = library();
        lib.tag(&[path("Lid.stl")], "Küche").unwrap();
        lib.set_tag_color("küche", Some(3)).unwrap();
        lib.set_tag_color("Garten", Some(4)).unwrap();
        lib.set_tag_color("Garten", Some(5)).unwrap();
        lib.untag(&[path("Lid.stl")], "Küche").unwrap();
        let colors = lib.tag_colors().unwrap();
        assert_eq!((colors["küche"], colors["garten"]), (3, 5));
        lib.set_tag_color("GARTEN", None).unwrap();
        assert!(!lib.tag_colors().unwrap().contains_key("garten"));
    }

    #[test]
    fn export_and_import_carry_lists_prints_and_colors() {
        let mut lib = library();
        let markt = lib.create_list(ListKind::Print, "Markt").unwrap();
        lib.add_to_list(markt, &[path("Knight.stl")]).unwrap();
        lib.set_quantity(markt, &[path("Knight.stl")], 3).unwrap();
        lib.record_print(&[path("Knight.stl")], PrintResult::Printed, Some(markt)).unwrap();
        lib.set_favorite(&[path("Lid.stl")], true).unwrap();
        lib.set_list_archived(markt, true).unwrap();
        lib.set_tag_color("Garten", Some(4)).unwrap();

        let data = lib.export_user_data().unwrap();
        assert_eq!(data.lists.len(), 2);
        let knight = data.entries.iter().find(|e| e.content_hash == "h-k").unwrap();
        assert_eq!(knight.files, ["/lib/3D_druck/Knight.stl", "/lib/3D_druck/Knight_copy.stl"]);
        assert_eq!(knight.prints.len(), 1);
        assert_eq!(knight.prints[0].list.as_deref(), Some("Markt"));
        assert!(data.entries.iter().any(|e| e.content_hash == "h-l"), "favourite without tags still listed");
        let json = serde_json::to_string(&data).unwrap();

        let mut fresh = library();
        fresh.import_user_data(&serde_json::from_str(&json).unwrap()).unwrap();
        fresh.import_user_data(&serde_json::from_str(&json).unwrap()).unwrap();
        let lists = fresh.lists().unwrap();
        let markt = lists.iter().find(|l| l.kind == ListKind::Print).unwrap();
        assert!(markt.archived);
        let entry = &fresh.list_entries(markt.id).unwrap()[0];
        assert_eq!((entry.quantity, entry.done), (3, 1));
        assert_eq!(fresh.print_status(&[path("Knight.stl")]).unwrap().printed, 1, "importing twice logs once");
        assert!(fresh.marks().unwrap()[&path("Lid.stl")].favorite);
        assert_eq!(fresh.tag_colors().unwrap()["garten"], 4);
        assert_eq!(fresh.export_user_data().unwrap(), data, "a round trip keeps everything");
    }

    #[test]
    fn version_one_exports_still_import() {
        let json = r#"{"format":"meshgrove-user-data","version":1,"entries":[
            {"content_hash":"h-d","files":["/old/Dragon.3mf"],"tags":["bemalt"],"note":"rot"}]}"#;
        let mut lib = library();
        assert_eq!(lib.import_user_data(&serde_json::from_str(json).unwrap()).unwrap(), 1);
        assert_eq!(lib.tags_of(&[path("Dragon.3mf")]).unwrap(), [("bemalt".into(), 1)]);
        assert!(lib.lists().unwrap().is_empty());
    }

    #[test]
    fn a_version_4_library_gains_lists_and_keeps_its_tags() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("library.db");
        {
            let mut lib = Library::open(&db).unwrap();
            lib.update_dir(Path::new("/r"), Path::new("/r"), 1, &[file("/r", "a.stl")], &[]).unwrap();
            let meta = ThumbMeta { content_hash: Some("h".into()), ..Default::default() };
            lib.store_meta(Path::new("/r/a.stl"), "k", &meta).unwrap();
            lib.tag(&[PathBuf::from("/r/a.stl")], "alt").unwrap();
            // Back to how 0.3 left the file: no list tables, version 4.
            lib.conn
                .execute_batch(
                    "DROP TABLE lists; DROP TABLE list_items; DROP TABLE prints; DROP TABLE tag_colors;
                     PRAGMA user_version = 4;",
                )
                .unwrap();
        }
        let mut lib = Library::open(&db).unwrap();
        assert_eq!(lib.all_tags().unwrap(), [("alt".into(), 1)]);
        assert_eq!(lib.counts().unwrap().files, 1, "the index is kept, not rebuilt");
        lib.set_favorite(&[PathBuf::from("/r/a.stl")], true).unwrap();
        assert_eq!(lib.lists().unwrap()[0].count, 1);
        assert!(Library::open_reader(&db).is_some(), "readers see the new version");
    }

    #[test]
    fn automatic_collections_are_saved_searches() {
        let mut lib = library();
        lib.tag(&[path("Dragon.3mf"), path("Lid.stl")], "Garten").unwrap();
        lib.tag(&[path("Lid.stl")], "Geschenk").unwrap();
        let id = lib.create_smart_list("Garten-Geschenke", "tag:garten tag:geschenk").unwrap();
        assert_eq!(names(&lib.list_entries(id).unwrap()), ["Lid.stl"]);
        lib.tag(&[path("Dragon.3mf")], "Geschenk").unwrap();
        assert_eq!(names(&lib.list_entries(id).unwrap()), ["Dragon.3mf", "Lid.stl"], "follows the tags");
        let info = lib.lists().unwrap().into_iter().find(|l| l.id == id).unwrap();
        assert_eq!((info.count, info.query.as_deref()), (2, Some("tag:garten tag:geschenk")));

        let data = lib.export_user_data().unwrap();
        let mut fresh = library();
        fresh.import_user_data(&data).unwrap();
        let copy = fresh.lists().unwrap().into_iter().find(|l| l.name == "Garten-Geschenke").unwrap();
        assert_eq!(copy.query.as_deref(), Some("tag:garten tag:geschenk"));
    }

    #[test]
    fn early_list_tables_get_the_query_column() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("library.db");
        {
            let lib = Library::open(&db).unwrap();
            lib.conn.execute_batch("ALTER TABLE lists DROP COLUMN query").unwrap();
        }
        let mut lib = Library::open(&db).unwrap();
        let id = lib.create_smart_list("Alles", "ext:stl").unwrap();
        assert!(lib.lists().unwrap().iter().any(|l| l.id == id && l.query.is_some()));
    }
}
