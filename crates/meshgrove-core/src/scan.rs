//! Incremental library scan. A folder is only listed again when its modification time
//! changed (files added, removed or renamed); unchanged folders reuse the indexed subfolders,
//! so a startup rescan of a big library on a spinning disk costs one `stat` per folder.
//! Archives are treated as folders: their members are indexed below the archive's path.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::db::{self, Library};
use crate::{archive, listing};

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ScanReport {
    pub dirs_visited: usize,
    /// Folders whose contents changed (or were new).
    pub changed_dirs: Vec<PathBuf>,
}

/// Which folders are listed even though their modification time did not change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Force {
    /// None: the usual incremental scan.
    Nothing,
    /// The start folder (a file in it was rewritten, which leaves the folder's mtime alone).
    Start,
    /// Every folder below the start: "rescan folder". Unchanged files keep their facts and hashes.
    Tree,
}

/// Scans `start` (a library root or a folder inside `root`) and everything below it.
pub fn scan(lib: &mut Library, root: &Path, start: &Path, force: Force, cancel: &AtomicBool) -> io::Result<ScanReport> {
    let mut report = ScanReport::default();
    let mut seen = HashSet::new();
    let mut stack = vec![(start.to_path_buf(), force != Force::Nothing)];
    while let Some((dir, forced)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let meta = match std::fs::metadata(&dir) {
            Ok(m) if m.is_dir() || (m.is_file() && archive::is_archive_file(&dir)) => m,
            _ => {
                lib.remove_tree(&dir).map_err(io::Error::other)?;
                continue;
            }
        };
        // Symlinked folders can form loops; visit every real folder once.
        if !seen.insert(folder_id(&dir, &meta)) {
            continue;
        }
        report.dirs_visited += 1;
        let mtime = db::nanos(meta.modified().ok());
        let subdirs = if !forced && lib.dir_mtime(&dir) == Some(mtime) {
            lib.child_dirs(&dir)
        } else if meta.is_file() {
            // A broken or password-protected archive is indexed as empty rather than failing the scan.
            let members = archive::entries(&dir).unwrap_or_default();
            if lib.update_dir(root, &dir, mtime, &members, &[]).map_err(io::Error::other)? {
                report.changed_dirs.push(dir.clone());
            }
            Vec::new()
        } else {
            let entries = match listing::list_dir(&dir) {
                Ok(entries) => entries,
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => continue,
                Err(e) => return Err(e),
            };
            let subdirs: Vec<PathBuf> = entries
                .iter()
                .filter(|e| e.is_dir() || e.kind == crate::Kind::Archive)
                .map(|e| e.path.clone())
                .collect();
            if lib.update_dir(root, &dir, mtime, &entries, &subdirs).map_err(io::Error::other)? {
                report.changed_dirs.push(dir.clone());
            }
            subdirs
        };
        stack.extend(subdirs.into_iter().map(|d| (d, force == Force::Tree)));
    }
    Ok(report)
}

/// What makes a folder the same folder when reached through different (symlinked) paths.
#[cfg(unix)]
fn folder_id(_dir: &Path, meta: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

#[cfg(not(unix))]
fn folder_id(dir: &Path, _meta: &std::fs::Metadata) -> PathBuf {
    dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query;
    use std::fs;

    fn names(lib: &Library, q: &str) -> Vec<String> {
        lib.search(&query::parse(q), 100).unwrap().into_iter().map(|e| e.name).collect()
    }

    #[test]
    fn initial_and_incremental_scans() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("lib");
        fs::create_dir_all(root.join("minis/orcs")).unwrap();
        fs::write(root.join("minis/orcs/boss.stl"), b"x").unwrap();
        fs::write(root.join("minis/elf.3mf"), b"x").unwrap();
        fs::write(root.join("notes.txt"), b"x").unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        let no = AtomicBool::new(false);

        let first = scan(&mut lib, &root, &root, Force::Nothing, &no).unwrap();
        assert_eq!(first.dirs_visited, 3);
        assert_eq!(first.changed_dirs.len(), 3);
        assert_eq!(names(&lib, "orcs"), ["boss.stl"]);

        // Nothing changed: no folder is listed again.
        let again = scan(&mut lib, &root, &root, Force::Nothing, &no).unwrap();
        assert_eq!(again.dirs_visited, 3);
        assert!(again.changed_dirs.is_empty());

        // Adding a file changes only that folder's mtime; removing a folder prunes it.
        fs::write(root.join("minis/orcs/grunt.stl"), b"x").unwrap();
        fs::remove_file(root.join("minis/elf.3mf")).unwrap();
        fs::create_dir(root.join("minis/elves")).unwrap();
        let third = scan(&mut lib, &root, &root, Force::Nothing, &no).unwrap();
        let mut changed: Vec<_> =
            third.changed_dirs.iter().map(|d| d.strip_prefix(&root).unwrap().to_owned()).collect();
        changed.sort();
        assert_eq!(changed, [Path::new("minis"), Path::new("minis/elves"), Path::new("minis/orcs")]);
        assert_eq!(names(&lib, "minis"), ["boss.stl", "grunt.stl"]);

        fs::remove_dir_all(root.join("minis/orcs")).unwrap();
        scan(&mut lib, &root, &root, Force::Nothing, &no).unwrap();
        assert!(names(&lib, "boss").is_empty());
        assert_eq!(lib.counts().unwrap().files, 1);
    }

    #[test]
    fn archive_members_are_indexed_like_folder_contents() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let zip = root.join("pack.zip");
        crate::archive::tests::make_zip(&zip);
        let mut lib = Library::open_in_memory().unwrap();
        let no = AtomicBool::new(false);
        let report = scan(&mut lib, &root, &root, Force::Nothing, &no).unwrap();
        assert_eq!(report.changed_dirs.len(), 2, "root and the archive");
        let hits = lib.search(&query::parse("orc"), 9).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, zip.join("minis/orc.stl"));
        assert_eq!(lib.folder_preview(&zip, 4).unwrap().len(), 1);
        assert!(
            scan(&mut lib, &root, &root, Force::Nothing, &no).unwrap().changed_dirs.is_empty(),
            "unchanged archive is not re-read"
        );
        std::fs::remove_file(&zip).unwrap();
        scan(&mut lib, &root, &root, Force::Nothing, &no).unwrap();
        assert!(names(&lib, "orc").is_empty());
    }

    #[test]
    fn forced_rescan_of_one_folder_and_vanished_start() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        fs::create_dir(root.join("a")).unwrap();
        fs::write(root.join("a/part.stl"), b"x").unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        let no = AtomicBool::new(false);
        scan(&mut lib, &root, &root, Force::Nothing, &no).unwrap();

        // Rewriting a file leaves its folder's mtime alone; only a forced scan notices.
        fs::write(root.join("a/part.stl"), b"xy").unwrap();
        let forced = scan(&mut lib, &root, &root.join("a"), Force::Start, &no).unwrap();
        assert_eq!(forced.changed_dirs, [root.join("a")]);
        let again = scan(&mut lib, &root, &root.join("a"), Force::Start, &no).unwrap();
        assert!(again.changed_dirs.is_empty(), "listed again, but nothing changed");

        fs::remove_dir_all(root.join("a")).unwrap();
        scan(&mut lib, &root, &root.join("a"), Force::Nothing, &no).unwrap();
        assert_eq!(lib.counts().unwrap().files, 0);
    }

    #[test]
    fn rescanning_a_tree_ignores_folder_times() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("a/b/part.stl"), b"x").unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        let no = AtomicBool::new(false);
        scan(&mut lib, &root, &root, Force::Nothing, &no).unwrap();
        fs::write(root.join("a/b/part.stl"), b"xyz").unwrap();
        assert!(scan(&mut lib, &root, &root, Force::Start, &no).unwrap().changed_dirs.is_empty());
        let tree = scan(&mut lib, &root, &root.join("a"), Force::Tree, &no).unwrap();
        assert_eq!((tree.dirs_visited, tree.changed_dirs), (2, vec![root.join("a/b")]));
        assert_eq!(lib.search(&query::parse("part"), 9).unwrap()[0].size, 3);
    }
}
