//! Pasting files: copies of files or folders – from this library, another one or wherever the
//! clipboard points – into a folder, as a journalled change like a move. Names that are taken
//! get a number; nothing is ever replaced.

use std::path::{Path, PathBuf};

use crate::ai::proposals::Target;
use crate::changes::ApplyOp;
use crate::db::nanos;
use crate::import::free_target;

/// One copy: the source as it is now, and where the copy goes.
#[derive(Debug, Clone)]
pub struct Item {
    pub from: Target,
    pub to: PathBuf,
    pub folder: bool,
}

/// A source that is not copied, and why: "gone", "link" or "into_itself".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub path: PathBuf,
    pub why: &'static str,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub root: PathBuf,
    pub dir: PathBuf,
    pub items: Vec<Item>,
    pub skipped: Vec<Skipped>,
}

/// Copies of `sources` into `dir`, below `root` (the library, or `dir` itself elsewhere).
/// Members of archives are unpacked to the cache first, so their copies are plain files.
pub fn plan(sources: &[PathBuf], root: &Path, dir: &Path) -> Plan {
    let (mut items, mut skipped) = (Vec::new(), Vec::new());
    let mut used: Vec<PathBuf> = Vec::new();
    for source in sources {
        let mut skip = |why| skipped.push(Skipped { path: source.clone(), why });
        let Some(name) = source.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            skip("gone");
            continue;
        };
        let Ok(path) = crate::archive::materialize(source) else {
            skip("gone");
            continue;
        };
        let Ok(meta) = path.symlink_metadata() else {
            skip("gone");
            continue;
        };
        if meta.is_symlink() {
            skip("link");
            continue;
        }
        if meta.is_dir() && dir.starts_with(&path) {
            skip("into_itself");
            continue;
        }
        let to = free_target(dir, &name, &used);
        used.push(to.clone());
        let from = Target { path, hash: None, size: meta.len(), mtime_ns: nanos(meta.modified().ok()) };
        items.push(Item { from, to, folder: meta.is_dir() });
    }
    Plan { root: root.to_path_buf(), dir: dir.to_path_buf(), items, skipped }
}

/// The journal steps of a plan.
pub fn to_ops(plan: &Plan) -> Vec<ApplyOp> {
    plan.items
        .iter()
        .map(|i| ApplyOp::Copy { root: plan.root.clone(), from: i.from.clone(), to: i.to.clone(), folder: i.folder })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_get_free_names_and_nothing_goes_into_itself() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib");
        let garten = root.join("Garten");
        std::fs::create_dir_all(&garten).unwrap();
        std::fs::write(garten.join("rose.stl"), "r").unwrap();
        std::fs::write(root.join("rose.stl"), "r2").unwrap();
        let sources = [root.join("rose.stl"), garten.join("rose.stl"), root.clone(), root.join("missing.stl")];
        let plan = plan(&sources, &root, &garten);
        let names: Vec<String> =
            plan.items.iter().map(|i| i.to.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["rose (2).stl", "rose (3).stl"], "the folder's own rose.stl keeps its name");
        assert!(plan.items.iter().all(|i| !i.folder && i.from.size > 0));
        assert_eq!(
            plan.skipped,
            [
                Skipped { path: root.clone(), why: "into_itself" },
                Skipped { path: root.join("missing.stl"), why: "gone" }
            ]
        );
        assert_eq!(to_ops(&plan).len(), 2);
    }
}
