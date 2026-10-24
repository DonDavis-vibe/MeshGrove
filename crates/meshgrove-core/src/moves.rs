//! Moving files and folders the user accepted: on disk without replacing anything
//! ([`crate::fsops`]), and in the index in place, so hashes, facts, checks and thumbnails stay
//! with them (tags, notes and lists hang on content anyway).

use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension, params};

use crate::ai::proposals::Target;
use crate::cache::ThumbCache;
use crate::db::{Error, Library, entry_from_row, nanos};
use crate::formats::Kind;

/// Why a move cannot be done (any more).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveProblem {
    /// The file or folder is gone, or the file changed since it was suggested.
    Gone,
    /// Something is at the new place already.
    Taken,
}

/// Whether `from` (as it was when suggested) can go to `to` now.
pub fn check_move(from: &Target, to: &Path, folder: bool) -> Result<(), MoveProblem> {
    let Ok(meta) = from.path.symlink_metadata() else { return Err(MoveProblem::Gone) };
    if meta.is_dir() != folder {
        return Err(MoveProblem::Gone);
    }
    // A folder's time changes with what is in it; a file's must be as it was.
    if !folder && (meta.len() != from.size || nanos(meta.modified().ok()) != from.mtime_ns) {
        return Err(MoveProblem::Gone);
    }
    if to.symlink_metadata().is_ok() && !crate::fsops::case_only(&from.path, to) {
        return Err(MoveProblem::Taken);
    }
    Ok(())
}

/// The folders between `root` and `dir` (`dir` included) that do not exist yet, outermost first.
pub fn missing_folders(root: &Path, dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> =
        dir.ancestors().take_while(|a| a.starts_with(root) && *a != root && a.symlink_metadata().is_err()).map(Path::to_path_buf).collect();
    out.reverse();
    out
}

fn text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// `path` (below `from`) with `from` replaced by `to`; `Path::join("")` would add a separator.
fn rebase(path: &Path, from: &Path, to: &Path) -> PathBuf {
    match path.strip_prefix(from) {
        Ok(rel) if !rel.as_os_str().is_empty() => to.join(rel),
        _ => to.to_path_buf(),
    }
}

impl Library {
    /// The index after `from` (a file, an archive or a folder) was moved to `to` below library
    /// `root`: its rows go along and keep what is known about the files; search texts follow the
    /// new place, and so do thumbnails in the cache ([`Library::set_thumb_cache`]).
    pub fn move_index(&mut self, root: &Path, from: &Path, to: &Path) -> Result<(), Error> {
        self.conn.execute_batch("SAVEPOINT move_index")?;
        match self.move_rows(root, from, to) {
            Ok(()) => self.conn.execute_batch("RELEASE move_index"),
            Err(e) => {
                self.conn.execute_batch("ROLLBACK TO move_index; RELEASE move_index")?;
                Err(e)
            }
        }
    }

    /// The index row of folder `dir`, made (with those above it up to `root`) when missing.
    fn dir_id(&mut self, root: &Path, dir: &Path) -> Result<i64, Error> {
        if let Some(id) = self.conn.query_row("SELECT id FROM dirs WHERE path = ?1", [text(dir)], |r| r.get(0)).optional()? {
            return Ok(id);
        }
        let parent = match dir.parent() {
            Some(p) if dir != root && p.starts_with(root) => {
                self.dir_id(root, p)?;
                Some(text(p))
            }
            _ => None,
        };
        let mtime = nanos(dir.symlink_metadata().ok().and_then(|m| m.modified().ok()));
        self.conn.execute("INSERT INTO dirs (path, parent, mtime_ns) VALUES (?1, ?2, ?3)", params![text(dir), parent, mtime])?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Folder rows at `top` and below it: (id, path, parent).
    fn dirs_below(&self, top: &Path) -> Result<Vec<(i64, PathBuf, Option<String>)>, Error> {
        let prefix = text(top);
        let mut stmt = self.conn.prepare("SELECT id, path, parent FROM dirs WHERE substr(path, 1, ?2) = ?1")?;
        let rows = stmt.query_map(params![prefix, prefix.chars().count() as i64], |r| {
            Ok((r.get::<_, i64>(0)?, PathBuf::from(r.get::<_, String>(1)?), r.get::<_, Option<String>>(2)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let row = row?;
            // "/lib/Garten2" is not below "/lib/Garten".
            if row.1.starts_with(top) {
                out.push(row);
            }
        }
        Ok(out)
    }

    fn move_rows(&mut self, root: &Path, from: &Path, to: &Path) -> Result<(), Error> {
        let (Some(to_dir), Some(to_name)) = (to.parent(), to.file_name()) else { return Ok(()) };
        let to_name = to_name.to_string_lossy().into_owned();
        let new_dir = self.dir_id(root, to_dir)?;
        // Whatever the index still has at the new place is out of date.
        for (id, _, _) in self.dirs_below(to)? {
            self.conn.execute("DELETE FROM dirs WHERE id = ?1", [id])?;
        }
        if !crate::fsops::case_only(from, to) {
            self.conn.execute("DELETE FROM files WHERE dir_id = ?1 AND name = ?2", params![new_dir, to_name])?;
        }
        // The file (or the archive) itself.
        if let (Some(dir), Some(name)) = (from.parent(), from.file_name()) {
            self.conn.execute(
                "UPDATE files SET dir_id = ?1, name = ?2 WHERE name = ?3 AND dir_id = (SELECT id FROM dirs WHERE path = ?4)",
                params![new_dir, to_name, name.to_string_lossy(), text(dir)],
            )?;
        }
        // A folder (or an archive's inside) with everything below it.
        for (id, path, parent) in self.dirs_below(from)? {
            let new_path = rebase(&path, from, to);
            let new_parent = if path == from {
                (to != root).then(|| text(to_dir))
            } else {
                parent.map(|p| text(&rebase(Path::new(&p), from, to)))
            };
            self.conn.execute("UPDATE dirs SET path = ?2, parent = ?3 WHERE id = ?1", params![id, text(&new_path), new_parent])?;
        }
        // The moved files: search text for the new folder, cache entries under the new path.
        let mut moved = Vec::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT d.path, f.name, f.kind, f.size, f.mtime_ns, f.id, f.meta_key FROM files f JOIN dirs d ON d.id = f.dir_id
                 WHERE (f.dir_id = ?1 AND f.name = ?2) OR substr(d.path, 1, ?4) = ?3",
            )?;
            let rows = stmt.query_map(params![new_dir, to_name, text(to), text(to).chars().count() as i64], |r| {
                Ok((entry_from_row(r)?, r.get::<_, i64>(5)?, r.get::<_, Option<String>>(6)?))
            })?;
            for row in rows {
                let row = row?;
                if row.0.path.starts_with(to) {
                    moved.push(row);
                }
            }
        }
        for (entry, id, meta_key) in moved {
            let dir = entry.path.parent().unwrap_or(root);
            let rel = dir.strip_prefix(root).unwrap_or(dir).to_string_lossy().to_lowercase();
            let mut key = meta_key;
            let old_path = rebase(&entry.path, to, from);
            let old_key = ThumbCache::key(&old_path, entry.size, entry.modified, None);
            if key.as_deref() == Some(old_key.as_str()) {
                let new_key = ThumbCache::key(&entry.path, entry.size, entry.modified, None);
                if let Some(cache) = &self.thumbs {
                    cache.rekey(&old_key, &new_key);
                }
                key = Some(new_key);
            }
            if entry.kind == Kind::Step && crate::archive::split(&entry.path).is_none() {
                crate::loaders::step::rekey(&old_path, &entry.path, entry.size, nanos(entry.modified) as u128);
            }
            self.conn.execute(
                "UPDATE files SET search = ?2, meta_key = ?3 WHERE id = ?1",
                params![id, format!("{}\n{rel}", entry.name.to_lowercase()), key],
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::ThumbMeta;
    use crate::listing::Entry;

    fn entry(dir: &Path, name: &str) -> Entry {
        let modified = std::fs::metadata(dir.join(name)).ok().and_then(|m| m.modified().ok());
        Entry { name: name.into(), path: dir.join(name), kind: Kind::from_file_name(name), size: 3, modified }
    }

    #[test]
    fn rows_keep_their_facts_and_follow_the_move() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib");
        let garten = root.join("Garten");
        let sub = garten.join("Rosen");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(garten.join("zaun.stl"), "abc").unwrap();
        std::fs::write(sub.join("rose.stl"), "abc").unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        lib.finish_baseline(&root).unwrap();
        lib.set_thumb_cache(ThumbCache::new(dir.path().join("thumbs")));
        lib.update_dir(&root, &root, 1, &[], std::slice::from_ref(&garten)).unwrap();
        lib.update_dir(&root, &garten, 1, &[entry(&garten, "zaun.stl")], std::slice::from_ref(&sub)).unwrap();
        lib.update_dir(&root, &sub, 1, &[entry(&sub, "rose.stl")], &[]).unwrap();
        let rose = sub.join("rose.stl");
        let e = entry(&sub, "rose.stl");
        let key = ThumbCache::key(&rose, e.size, e.modified, None);
        lib.store_meta(&rose, &key, &ThumbMeta { content_hash: Some("h-rose".into()), ..Default::default() }).unwrap();
        std::fs::create_dir_all(dir.path().join("thumbs").join(&key[..2])).unwrap();
        std::fs::write(dir.path().join("thumbs").join(&key[..2]).join(format!("{key}.png")), "png").unwrap();
        lib.tag(std::slice::from_ref(&rose), "Rose").unwrap();

        // The folder goes into a new one.
        let target = root.join("Draußen").join("Garten");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        crate::fsops::rename_no_replace(&garten, &target).unwrap();
        lib.move_index(&root, &garten, &target).unwrap();
        let moved = target.join("Rosen").join("rose.stl");
        let row = lib.entry_of(&moved).unwrap().expect("found at the new place");
        assert_eq!(row.size, 3);
        assert!(lib.entry_of(&rose).unwrap().is_none(), "not at the old place");
        assert_eq!(lib.tags_of(std::slice::from_ref(&moved)).unwrap()[0].0, "Rose", "tags are on the content");
        let found = lib.search(&crate::query::parse("draußen"), 10).unwrap();
        assert_eq!(found.len(), 2, "search texts follow the folder: {found:?}");
        let new_key = ThumbCache::key(&moved, e.size, e.modified, None);
        assert!(dir.path().join("thumbs").join(&new_key[..2]).join(format!("{new_key}.png")).exists(), "the thumbnail moved");
        assert!(lib.needing_meta().unwrap().iter().all(|(e, _)| e.path != moved), "nothing to render again");
        let parent: Option<String> = lib
            .conn
            .query_row("SELECT parent FROM dirs WHERE path = ?1", [text(&target.join("Rosen"))], |r| r.get(0))
            .unwrap();
        assert_eq!(parent, Some(text(&target)));

        // A file is renamed in its folder.
        let renamed = target.join("gartenzaun.stl");
        crate::fsops::rename_no_replace(&target.join("zaun.stl"), &renamed).unwrap();
        lib.move_index(&root, &target.join("zaun.stl"), &renamed).unwrap();
        assert!(lib.entry_of(&renamed).unwrap().is_some());
    }

    /// lib "Werkstatt" (cloud): Kunde/teil.stl, Kunde/alt/deckel.stl, pack.zip (an archive),
    /// lib "Andere": nothing; both in a temporary folder.
    struct Setup {
        _dir: tempfile::TempDir,
        root: PathBuf,
        other: PathBuf,
        config: crate::config::Config,
        lib: Library,
    }

    fn setup() -> Setup {
        use crate::config::{AiAccess, LibraryAi};
        let dir = tempfile::tempdir().unwrap();
        let (root, other) = (dir.path().join("Werkstatt"), dir.path().join("Andere"));
        let kunde = root.join("Kunde");
        std::fs::create_dir_all(kunde.join("alt")).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(kunde.join("teil.stl"), "abc").unwrap();
        std::fs::write(kunde.join("alt").join("deckel.stl"), "abc").unwrap();
        let mut zip = zip::ZipWriter::new(std::fs::File::create(root.join("pack.zip")).unwrap());
        zip.start_file("orc.stl", zip::write::SimpleFileOptions::default()).unwrap();
        std::io::Write::write_all(&mut zip, b"abc").unwrap();
        zip.finish().unwrap();
        let mut config = crate::config::Config { library_roots: vec![root.clone(), other.clone()], ..Default::default() };
        config.ai.enabled = true;
        config.ai.libraries = [&root, &other]
            .iter()
            .map(|r| LibraryAi { root: (*r).clone(), access: AiAccess::Cloud, images: false })
            .collect();
        let mut lib = Library::open_in_memory().unwrap();
        lib.finish_baseline(&root).unwrap();
        lib.update_dir(&root, &root, 1, &[entry(&root, "pack.zip")], std::slice::from_ref(&kunde)).unwrap();
        lib.update_dir(&root, &kunde, 1, &[entry(&kunde, "teil.stl")], &[kunde.join("alt")]).unwrap();
        lib.update_dir(&root, &kunde.join("alt"), 1, &[entry(&kunde.join("alt"), "deckel.stl")], &[]).unwrap();
        for (path, hash) in [(kunde.join("teil.stl"), "h-teil"), (kunde.join("alt").join("deckel.stl"), "h-deckel")] {
            lib.store_meta(&path, "k", &ThumbMeta { content_hash: Some(hash.into()), ..Default::default() }).unwrap();
        }
        Setup { _dir: dir, root, other, config, lib }
    }

    fn propose(s: &Setup, items: &[(&str, &str)]) -> crate::ai::ToolResult<(crate::ai::proposals::Receipt, Vec<crate::changes::ApplyOp>)> {
        use crate::ai::proposals::*;
        let cx = crate::ai::Context::new(&s.lib, &s.config, crate::config::AccessClass::Cloud, None)?;
        let mut store = ProposalStore::open_in_memory()?;
        let origin = Origin { source: "mcp".into(), client: None, class: crate::config::AccessClass::Cloud };
        let req = ProposeMoves {
            title: "Aufräumen".into(),
            items: items.iter().map(|(id, to)| MoveItem { id: (*id).into(), to: (*to).into(), reason: None }).collect(),
        };
        let receipt = propose_moves(&cx, &mut store, &origin, &req)?;
        let batch = store.batch_by_uid(&receipt.batch)?.unwrap();
        let ops = store.proposals(batch)?.iter().flat_map(|p| to_ops(p, &Picked::all(p), false)).collect();
        Ok((receipt, ops))
    }

    #[test]
    fn moves_are_checked_when_suggested() {
        let s = setup();
        let (receipt, _) = propose(
            &s,
            &[
                ("Werkstatt/Kunde/teil.stl", "Werkstatt/Kunde/alt/deckel.stl"), // taken
                ("Werkstatt/Kunde/teil.stl", "Andere/teil.stl"),                // other library
                ("Werkstatt/pack.zip/orc.stl", "Werkstatt/orc.stl"),            // inside an archive
                ("Werkstatt/Kunde/teil.stl", "Werkstatt/Kunde/te?l.stl"),       // bad name
                ("Werkstatt/Kunde", "Werkstatt/Kunde/alt/Kunde"),               // into itself
                ("Werkstatt", "Werkstatt/x"),                                   // the library
                ("Werkstatt/Kunde/teil.stl", "Werkstatt/Teile/teil.stl"),       // fine, makes Teile
                ("Werkstatt/Kunde/alt/deckel.stl", "Werkstatt/Teile/teil.stl"), // same target as before
                ("Werkstatt/pack.zip", "Werkstatt/Archiv/"),                    // a whole archive is fine
            ],
        )
        .unwrap();
        assert_eq!(receipt.items, 2, "{:?}", receipt.rejected);
        let why: Vec<&str> = receipt.rejected.iter().map(|r| r.why.as_str()).collect();
        assert_eq!(why.len(), 7, "{why:?}");
        assert!(why[0].contains("there already") && why[1].contains("one library") && why[2].contains("archive"), "{why:?}");
        assert!(why[3].contains("Windows") && why[4].contains("itself") && why[5].contains("library itself"), "{why:?}");
        assert!(why[6].contains("another move"), "{why:?}");
        assert!(propose(&s, &[("Werkstatt/Kunde/teil.stl", "Werkstatt/Kunde/alt/deckel.stl")]).is_err(), "nothing usable");
        assert!(s.other.read_dir().unwrap().next().is_none(), "suggesting moves nothing");
    }

    #[test]
    fn accepted_moves_go_and_come_back() {
        let mut s = setup();
        let kunde = s.root.join("Kunde");
        let (_, ops) = propose(
            &s,
            &[("Werkstatt/Kunde/teil.stl", "Werkstatt/Teile/Kunde/teil-oben.stl"), ("Werkstatt/Kunde/alt", "Werkstatt/Archiv/")],
        )
        .unwrap();
        let report = s.lib.apply_change("proposal:m", "Aufräumen", &ops).unwrap();
        assert_eq!(report.applied, 2);
        let (teil, alt) = (s.root.join("Teile").join("Kunde").join("teil-oben.stl"), s.root.join("Archiv").join("alt"));
        assert!(teil.is_file() && alt.join("deckel.stl").is_file() && !kunde.join("teil.stl").exists());
        assert_eq!(s.lib.entry_of(&alt.join("deckel.stl")).unwrap().map(|e| e.size), Some(3), "the index followed");
        let row = s.lib.resolve_target(&Target { path: teil.clone(), hash: Some("h-teil".into()), size: 3, mtime_ns: 0 });
        assert_eq!(row.unwrap(), crate::changes::TargetState::Here, "the hash went along");

        let undo = s.lib.undo_change(report.id).unwrap();
        assert_eq!(undo.undone, 2);
        assert!(kunde.join("teil.stl").is_file() && kunde.join("alt").join("deckel.stl").is_file());
        assert!(!s.root.join("Teile").exists() && !s.root.join("Archiv").exists(), "the folders made for it are gone");
        assert!(s.lib.entry_of(&kunde.join("teil.stl")).unwrap().is_some());

        // Again, then the user changes the file: undo leaves it where it is.
        s.lib.redo_change(report.id).unwrap();
        assert!(teil.is_file());
        let id = s.lib.changes(1).unwrap()[0].id;
        std::fs::write(&teil, "geändert").unwrap();
        let undo = s.lib.undo_change(id).unwrap();
        assert_eq!((undo.undone, undo.kept), (1, 1), "the folder went back, the changed file stays");
        assert!(teil.is_file() && kunde.join("alt").is_dir());

        // Something appeared at the new place before accepting: skipped, nothing replaced.
        let (_, ops) = propose(&s, &[("Werkstatt/Kunde/alt/deckel.stl", "Werkstatt/Kunde/deckel.stl")]).unwrap();
        std::fs::write(kunde.join("deckel.stl"), "fremd").unwrap();
        let report = s.lib.apply_change("proposal:n", "Spät", &ops).unwrap();
        assert_eq!((report.applied, report.skipped), (0, 1));
        assert_eq!(std::fs::read_to_string(kunde.join("deckel.stl")).unwrap(), "fremd");
    }

    #[test]
    fn a_move_is_checked_against_what_is_there_now() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.stl"), dir.path().join("b.stl"));
        std::fs::write(&a, "abc").unwrap();
        std::fs::write(&b, "x").unwrap();
        let m = std::fs::metadata(&a).unwrap();
        let t = Target { path: a.clone(), hash: None, size: m.len(), mtime_ns: nanos(m.modified().ok()) };
        assert_eq!(check_move(&t, &dir.path().join("c.stl"), false), Ok(()));
        assert_eq!(check_move(&t, &b, false), Err(MoveProblem::Taken));
        assert_eq!(check_move(&Target { size: 9, ..t.clone() }, &dir.path().join("c.stl"), false), Err(MoveProblem::Gone));
        assert_eq!(check_move(&t, &dir.path().join("c"), true), Err(MoveProblem::Gone), "no folder");
        let deep = dir.path().join("x").join("y");
        assert_eq!(missing_folders(dir.path(), &deep), [dir.path().join("x"), deep.clone()]);
    }
}
