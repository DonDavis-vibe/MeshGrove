//! The inbox ("Eingang"): files whose content the library has not seen before, grouped by the
//! drop they came with (a new folder, a customer's ZIP), until the user marks them as done.
//!
//! Content counts as seen once its hash is in the `inbox` table; a first (baseline) scan of a
//! library records everything as done, so only what arrives later waits in the inbox.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::{Error, Library, entry_from_row, nanos};
use crate::listing::Entry;

/// Kinds that come into the inbox: what gets printed (and tagged).
pub(crate) const INBOX_KINDS: &str = "'stl', '3mf', 'step', 'obj', 'ply', 'gltf', 'gcode'";

#[derive(Debug, Clone, PartialEq)]
pub struct InboxItem {
    pub entry: Entry,
    pub content_hash: String,
    pub first_seen_ns: i64,
}

/// New files that came together: everything below one new folder or archive.
#[derive(Debug, Clone, PartialEq)]
pub struct InboxGroup {
    pub folder: PathBuf,
    /// The library the folder is in.
    pub root: PathBuf,
    /// When the newest of its files was seen.
    pub newest_ns: i64,
    pub items: Vec<InboxItem>,
    /// Files of the same drop whose content the library knew already (copies).
    pub known: usize,
}

/// Records every content known now as seen, once: the library before the inbox existed.
pub(crate) fn start(conn: &Connection) -> Result<(), Error> {
    let started: Option<String> =
        conn.query_row("SELECT value FROM meta WHERE key = 'inbox_since_ns'", [], |r| r.get(0)).optional()?;
    if started.is_some() {
        return Ok(());
    }
    let now = nanos(Some(SystemTime::now()));
    conn.execute(
        &format!(
            "INSERT OR IGNORE INTO inbox (content_hash, first_seen_ns, done_ns, done_by)
             SELECT DISTINCT content_hash, ?1, ?1, 'baseline' FROM files
             WHERE content_hash IS NOT NULL AND kind IN ({INBOX_KINDS})"
        ),
        [now],
    )?;
    conn.execute("INSERT INTO meta (key, value) VALUES ('inbox_since_ns', ?1)", [now.to_string()])?;
    Ok(())
}

/// Notes content `hash` as seen: new if one of its files arrived after the baseline scan
/// and the content was not seen before.
pub(crate) fn note_seen(conn: &Connection, hash: &str) -> Result<(), Error> {
    let now = nanos(Some(SystemTime::now()));
    conn.execute(
        &format!(
            "INSERT OR IGNORE INTO inbox (content_hash, first_seen_ns, done_ns, done_by)
             SELECT ?1, ?2, CASE WHEN COUNT(added_ns) = 0 THEN ?2 END, CASE WHEN COUNT(added_ns) = 0 THEN 'baseline' END
             FROM files WHERE content_hash = ?1 AND kind IN ({INBOX_KINDS}) HAVING COUNT(*) > 0"
        ),
        params![hash, now],
    )?;
    Ok(())
}

fn baseline_key(root: &Path) -> String {
    format!("baseline_done:{}", root.display())
}

impl Library {
    /// Whether `root` still needs its first full scan; until then what it finds is not new.
    pub fn baseline_pending(&self, root: &Path) -> bool {
        self.meta_get(&baseline_key(root)).is_none()
    }

    /// The first full scan of `root` is done: from now on its new files come into the inbox.
    pub fn finish_baseline(&mut self, root: &Path) -> Result<(), Error> {
        self.meta_set(&baseline_key(root), &nanos(Some(SystemTime::now())).to_string())
    }

    /// Files waiting in the inbox (one per content), in groups, newest drop first. `roots` are
    /// the libraries; a group never reaches above the library it is in.
    pub fn inbox(&self, roots: &[PathBuf], limit: usize) -> Result<Vec<InboxGroup>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT d.path, f.name, f.kind, f.size, f.mtime_ns, i.content_hash, i.first_seen_ns
             FROM inbox i JOIN files f ON f.content_hash = i.content_hash JOIN dirs d ON d.id = f.dir_id
             WHERE i.done_ns IS NULL ORDER BY i.first_seen_ns DESC, d.path, f.name",
        )?;
        let rows = stmt.query_map([], |r| Ok((entry_from_row(r)?, r.get::<_, String>(5)?, r.get::<_, i64>(6)?)))?;
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut added: HashMap<PathBuf, bool> = HashMap::new();
        let mut groups: Vec<InboxGroup> = Vec::new();
        let mut count = 0;
        for row in rows {
            let (entry, hash, first_seen) = row?;
            if !seen.insert(hash.clone()) {
                continue; // a copy of a file already listed
            }
            if count == limit {
                break;
            }
            count += 1;
            let dir = entry.path.parent().map(Path::to_path_buf).unwrap_or_default();
            let root = roots.iter().filter(|r| dir.starts_with(r)).max_by_key(|r| r.as_os_str().len());
            let folder = match root {
                Some(root) => self.drop_folder(&dir, root, &mut added)?,
                None => dir.clone(),
            };
            let item = InboxItem { entry, content_hash: hash, first_seen_ns: first_seen };
            match groups.iter_mut().find(|g| g.folder == folder) {
                Some(g) => {
                    g.newest_ns = g.newest_ns.max(first_seen);
                    g.items.push(item);
                }
                None => groups.push(InboxGroup {
                    folder,
                    root: root.cloned().unwrap_or(dir),
                    newest_ns: first_seen,
                    items: vec![item],
                    known: 0,
                }),
            }
        }
        for g in &mut groups {
            g.known = self.known_in(&g.folder)?;
        }
        groups.sort_by_key(|g| std::cmp::Reverse(g.newest_ns));
        Ok(groups)
    }

    /// The topmost folder (or archive) between `dir` and `root` that arrived after the baseline
    /// scan, i.e. what was dropped into the library; `dir` itself when it was there before.
    fn drop_folder(&self, dir: &Path, root: &Path, added: &mut HashMap<PathBuf, bool>) -> Result<PathBuf, Error> {
        let mut folder = dir.to_path_buf();
        let mut current = dir;
        while current != root && current.starts_with(root) {
            let is_new = match added.get(current) {
                Some(&known) => known,
                None => {
                    let value: Option<Option<i64>> = self
                        .conn
                        .query_row("SELECT added_ns FROM dirs WHERE path = ?1", [current.to_string_lossy()], |r| {
                            r.get(0)
                        })
                        .optional()?;
                    let is_new = value.flatten().is_some();
                    added.insert(current.to_path_buf(), is_new);
                    is_new
                }
            };
            if is_new {
                folder = current.to_path_buf();
            }
            let Some(parent) = current.parent() else { break };
            current = parent;
        }
        Ok(folder)
    }

    /// Files below `folder` that arrived after the baseline but whose content was known.
    fn known_in(&self, folder: &Path) -> Result<usize, Error> {
        let f = folder.to_string_lossy();
        let count: i64 = self.conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM files f JOIN dirs d ON d.id = f.dir_id
                 WHERE (d.path = ?1 OR (d.path > ?2 AND d.path < ?3)) AND f.added_ns IS NOT NULL
                   AND f.kind IN ({INBOX_KINDS})
                   AND f.content_hash IN (SELECT content_hash FROM inbox WHERE done_ns IS NOT NULL)"
            ),
            params![f, format!("{f}/"), format!("{f}0")],
            |r| r.get(0),
        )?;
        Ok(count as usize)
    }

    /// How many contents wait in the inbox (that still have a file in the library).
    pub fn inbox_count(&self) -> Result<usize, Error> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM inbox i WHERE i.done_ns IS NULL
               AND EXISTS (SELECT 1 FROM files f WHERE f.content_hash = i.content_hash)",
            [],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    /// Takes the contents of `paths` out of the inbox; `by` says who ('user', 'proposal:<uid>').
    /// Returns the hashes that were waiting (to reopen them on undo).
    pub fn inbox_done(&mut self, paths: &[PathBuf], by: &str) -> Result<Vec<String>, Error> {
        let mut hashes = Vec::new();
        for path in paths {
            if let Some(hash) = self.content_hash_of(path)? {
                hashes.push(hash);
            }
        }
        let now = nanos(Some(SystemTime::now()));
        let tx = self.conn.unchecked_transaction()?;
        let mut done = Vec::new();
        for hash in hashes {
            let changed = tx.execute(
                "UPDATE inbox SET done_ns = ?2, done_by = ?3 WHERE content_hash = ?1 AND done_ns IS NULL",
                params![hash, now, by],
            )?;
            if changed > 0 {
                done.push(hash);
            }
        }
        tx.commit()?;
        Ok(done)
    }

    /// Empties the inbox.
    pub fn inbox_done_all(&mut self, by: &str) -> Result<usize, Error> {
        let now = nanos(Some(SystemTime::now()));
        self.conn.execute("UPDATE inbox SET done_ns = ?1, done_by = ?2 WHERE done_ns IS NULL", params![now, by])
    }

    /// Puts contents back into the inbox (undo of [`Library::inbox_done`]).
    pub fn inbox_reopen(&mut self, hashes: &[String]) -> Result<(), Error> {
        let tx = self.conn.unchecked_transaction()?;
        for hash in hashes {
            tx.execute("UPDATE inbox SET done_ns = NULL, done_by = NULL WHERE content_hash = ?1", [hash])?;
        }
        tx.commit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::ThumbMeta;
    use crate::formats::Kind;
    use crate::query;
    use std::time::Duration;

    fn file(dir: &Path, name: &str) -> Entry {
        Entry {
            name: name.into(),
            path: dir.join(name),
            kind: Kind::from_file_name(name),
            size: 1,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        }
    }

    /// Indexes `dir` (below `root`) with these files and processes them with these hashes.
    fn put(lib: &mut Library, root: &Path, dir: &Path, files: &[(&str, &str)], subdirs: &[&Path]) {
        let entries: Vec<Entry> = files.iter().map(|(n, _)| file(dir, n)).collect();
        let subdirs: Vec<PathBuf> = subdirs.iter().map(|p| p.to_path_buf()).collect();
        lib.update_dir(root, dir, 1, &entries, &subdirs).unwrap();
        for (name, hash) in files {
            let meta = ThumbMeta { content_hash: Some((*hash).into()), ..Default::default() };
            lib.store_meta(&dir.join(name), "k", &meta).unwrap();
        }
    }

    fn names(groups: &[InboxGroup]) -> Vec<(String, Vec<String>)> {
        groups
            .iter()
            .map(|g| (g.folder.display().to_string(), g.items.iter().map(|i| i.entry.name.clone()).collect()))
            .collect()
    }

    #[test]
    fn the_first_scan_is_known_and_later_files_are_new() {
        let mut lib = Library::open_in_memory().unwrap();
        let root = Path::new("/lib");
        let old = Path::new("/lib/alt");
        let kunde = Path::new("/lib/kunde");
        let teile = Path::new("/lib/kunde/teile");
        assert!(lib.baseline_pending(root));
        lib.set_baseline_root(root, true);
        put(&mut lib, root, root, &[], &[old]);
        put(&mut lib, root, old, &[("a.stl", "ha"), ("b.3mf", "hb")], &[]);
        lib.set_baseline_root(root, false);
        lib.finish_baseline(root).unwrap();
        assert!(!lib.baseline_pending(root));
        assert_eq!(lib.inbox_count().unwrap(), 0, "the baseline is not new");

        // Later: a file in the old folder, and a customer folder with a copy of a known model.
        put(&mut lib, root, root, &[], &[old, kunde]);
        put(&mut lib, root, old, &[("a.stl", "ha"), ("b.3mf", "hb"), ("c.stl", "hc")], &[]);
        put(&mut lib, root, kunde, &[("readme.txt", "ht"), ("kopie.stl", "ha")], &[teile]);
        put(&mut lib, root, teile, &[("deckel.stl", "hd"), ("boden.stl", "he"), ("boden2.stl", "he")], &[]);
        assert_eq!(lib.inbox_count().unwrap(), 3, "c, deckel and boden; copies and texts are not new");
        let groups = lib.inbox(&[root.into()], 99).unwrap();
        let mut got = names(&groups);
        got.sort();
        assert_eq!(
            got,
            [
                ("/lib/alt".to_owned(), vec!["c.stl".to_owned()]),
                ("/lib/kunde".to_owned(), vec!["boden.stl".to_owned(), "deckel.stl".to_owned()]),
            ]
        );
        let kunde_group = groups.iter().find(|g| g.folder == kunde).unwrap();
        assert_eq!(kunde_group.known, 1, "kopie.stl is a known model in the drop");
        assert_eq!(kunde_group.root, root);

        let search = |lib: &Library, q: &str| -> Vec<String> {
            let mut n: Vec<String> = lib.search(&query::parse(q), 99).unwrap().into_iter().map(|e| e.name).collect();
            n.sort();
            n
        };
        assert_eq!(search(&lib, "is:neu"), ["boden.stl", "boden2.stl", "c.stl", "deckel.stl"]);
        lib.tag(&[teile.join("boden.stl")], "Werkstatt").unwrap();
        assert_eq!(search(&lib, "is:neu is:ohne-tags"), ["c.stl", "deckel.stl"], "boden2 is the same content");

        let done = lib.inbox_done(&[teile.join("deckel.stl")], "user").unwrap();
        assert_eq!(done, ["hd"]);
        assert_eq!(lib.inbox_count().unwrap(), 2);
        lib.inbox_reopen(&done).unwrap();
        assert_eq!(lib.inbox_count().unwrap(), 3);
        assert_eq!(lib.inbox_done_all("user").unwrap(), 3);
        assert!(lib.inbox(&[root.into()], 99).unwrap().is_empty());
        // Content once seen never comes back as new, even in another place.
        put(&mut lib, root, old, &[("a.stl", "ha"), ("b.3mf", "hb"), ("c.stl", "hc"), ("d.stl", "hd")], &[]);
        assert_eq!(lib.inbox_count().unwrap(), 0);
    }

    #[test]
    fn libraries_from_before_the_inbox_start_empty() {
        let mut lib = Library::open_in_memory().unwrap();
        let root = Path::new("/lib");
        put(&mut lib, root, root, &[("alt.stl", "h1")], &[]);
        // As a v5 library: the processed files were there before the inbox.
        lib.conn.execute("DELETE FROM inbox", []).unwrap();
        lib.conn.execute("DELETE FROM meta", []).unwrap();
        start(&lib.conn).unwrap();
        start(&lib.conn).unwrap();
        assert_eq!(lib.inbox_count().unwrap(), 0);
        lib.finish_baseline(root).unwrap();
        put(&mut lib, root, root, &[("alt.stl", "h1"), ("kopie.stl", "h1"), ("neu.stl", "h2")], &[]);
        assert_eq!(lib.inbox_count().unwrap(), 1, "only neu.stl");
    }

    #[test]
    fn archives_are_drops_of_their_own() {
        let mut lib = Library::open_in_memory().unwrap();
        let root = Path::new("/lib");
        put(&mut lib, root, root, &[], &[]);
        lib.finish_baseline(root).unwrap();
        let zip = Path::new("/lib/auftrag.zip");
        put(&mut lib, root, root, &[], &[zip]);
        put(&mut lib, root, zip, &[("teil.stl", "hz")], &[]);
        let groups = lib.inbox(&[root.into()], 99).unwrap();
        assert_eq!(names(&groups), [("/lib/auftrag.zip".to_owned(), vec!["teil.stl".to_owned()])]);
    }
}
