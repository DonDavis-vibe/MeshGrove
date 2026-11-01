//! A plugin's bundle as one change of the library: the files it wrote into its staging folder
//! are checked ([`resolve`]) and become the steps of [`crate::changes`] — a copy of the whole
//! folder to a free place, its indexing, then tags, notes and lists ([`ResolvedBundle::ops`]).
//! The staging folder stays until the change is gone from the history, so a redo can copy again.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use meshgrove_plugin_api::bundle::{Bundle, BundleFile, BundleItem, BundleList, ListKind as BundleListKind};

use crate::ai::proposals::Target;
use crate::ai::vocab::valid_tag;
use crate::changes::ApplyOp;
use crate::db::nanos;
use crate::lists::ListKind;

/// What one bundle may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub files: usize,
    pub bytes: u64,
}

impl Limits {
    pub const DEFAULT: Limits = Limits { files: 2000, bytes: 2 << 30 };
}

/// Most tags on one file, lists in one bundle, and copies of one file in a list.
const MAX_TAGS: usize = 8;
const MAX_LISTS: usize = 8;
const MAX_QUANTITY: u32 = 999;

/// Why a bundle cannot go into the library. The texts go to the plugin (and the AI behind it).
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    #[error("{0:?} is not a relative path below /out (no leading '/', no '..', valid names)")]
    BadPath(String),
    #[error("{0:?} is a link; bundles hold plain files and folders only")]
    Link(String),
    #[error("{0:?} is not a file below /out")]
    Missing(String),
    #[error("{0} files, at most {1} fit in one bundle")]
    TooMany(usize, usize),
    #[error("{0} bytes, at most {1} fit in one bundle")]
    TooBig(u64, u64),
    #[error("{0}")]
    Invalid(String),
    #[error("reading the staging folder: {0}")]
    Io(#[from] std::io::Error),
}

/// A bundle checked against its staging folder, with the place it goes: the payload of a
/// `bundle` suggestion and what [`ResolvedBundle::ops`] turns into steps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedBundle {
    /// The plugin that made it (its grants decide whether it may be accepted by itself).
    #[serde(default)]
    pub plugin: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The library it goes into.
    pub root: PathBuf,
    /// The staging folder (the plugin's `/out`), copied as a whole.
    pub from: PathBuf,
    /// The free place below `root` it is copied to.
    pub to: PathBuf,
    /// New content does not wait in the inbox.
    pub seen: bool,
    pub files: Vec<ResolvedFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lists: Vec<ResolvedList>,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedFile {
    /// Below the staging folder and below `to`, `/`-separated.
    pub rel: String,
    pub size: u64,
    pub mtime_ns: i64,
    pub hash: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedList {
    pub name: String,
    pub kind: ListKind,
    /// Files (by `rel`) and how many of each.
    pub items: Vec<(String, u32)>,
}

/// Checks `bundle` against `out` (the staging folder it was written into) and finds its place
/// in library `root`: the bundle's folder below `root`, or one named after the title in
/// `landing`; taken already, it gets a number (" (2)"). Every file below `out` goes along;
/// links, bad names and paths leaving `out` are refused.
pub fn resolve(bundle: &Bundle, out: &Path, root: &Path, landing: &Path) -> Result<ResolvedBundle, BundleError> {
    resolve_with(bundle, out, root, landing, Limits::DEFAULT)
}

/// [`resolve`] with other limits.
pub fn resolve_with(
    bundle: &Bundle,
    out: &Path,
    root: &Path,
    landing: &Path,
    limits: Limits,
) -> Result<ResolvedBundle, BundleError> {
    let title = bundle.title.trim();
    if title.is_empty() || title.chars().count() > 120 {
        return Err(BundleError::Invalid("the title is empty or longer than 120 characters".into()));
    }
    if !landing.starts_with(root) {
        return Err(BundleError::Invalid("the import folder is not in the library".into()));
    }
    let found = files_below(out, limits)?;
    let bytes = found.iter().map(|(_, m)| m.len()).sum();
    // Every path the bundle names must be one of those files.
    for path in bundle.paths() {
        let rel = relative(path).ok_or_else(|| BundleError::BadPath(path.into()))?;
        if !found.iter().any(|(r, _)| *r == rel) {
            return Err(BundleError::Missing(path.into()));
        }
    }
    let tags = |own: &[String]| -> Result<Vec<String>, BundleError> {
        let mut out: Vec<String> = Vec::new();
        for t in bundle.tags.iter().chain(own) {
            if !valid_tag(t) {
                return Err(BundleError::Invalid(format!("{t:?} is not a valid tag")));
            }
            if !out.iter().any(|o| o.eq_ignore_ascii_case(t.trim())) {
                out.push(t.trim().to_owned());
            }
        }
        if out.len() > MAX_TAGS {
            return Err(BundleError::Invalid(format!("more than {MAX_TAGS} tags on one file")));
        }
        Ok(out)
    };
    let mut files = Vec::with_capacity(found.len());
    for (rel, meta) in &found {
        let own = bundle.files.iter().find(|f| relative(&f.path).as_ref() == Some(rel));
        let note = own.and_then(|f| f.note.as_deref()).map(str::trim).filter(|n| !n.is_empty());
        files.push(ResolvedFile {
            rel: rel.clone(),
            size: meta.len(),
            mtime_ns: nanos(meta.modified().ok()),
            hash: crate::dedupe::content_hash(&std::fs::read(out.join(rel))?),
            tags: tags(own.map_or(&[], |f| f.tags.as_slice()))?,
            note: note.map(str::to_owned),
        });
    }
    if bundle.lists.len() > MAX_LISTS {
        return Err(BundleError::Invalid(format!("more than {MAX_LISTS} lists")));
    }
    let mut lists = Vec::new();
    for list in &bundle.lists {
        let name = list.name.trim();
        if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
            return Err(BundleError::Invalid(format!("{:?} is not a valid list name", list.name)));
        }
        let kind = match list.kind {
            BundleListKind::Print => ListKind::Print,
            BundleListKind::Collection => ListKind::Collection,
        };
        let mut items: Vec<(String, u32)> = Vec::new();
        for item in &list.items {
            let rel = relative(&item.path).ok_or_else(|| BundleError::BadPath(item.path.clone()))?;
            let quantity = item.quantity.clamp(1, MAX_QUANTITY);
            match items.iter_mut().find(|(r, _)| *r == rel) {
                Some((_, q)) => *q = (*q + quantity).min(MAX_QUANTITY),
                None => items.push((rel, quantity)),
            }
        }
        lists.push(ResolvedList { name: name.to_owned(), kind, items });
    }
    let place = match &bundle.folder {
        Some(folder) => root.join(relative(folder).ok_or_else(|| BundleError::BadPath(folder.clone()))?),
        None => {
            let name = folder_name(title);
            if crate::fsops::check_name(&name).is_err() {
                return Err(BundleError::Invalid(format!("no folder name can be made of the title {title:?}")));
            }
            landing.join(name)
        }
    };
    Ok(ResolvedBundle {
        plugin: String::new(),
        title: title.to_owned(),
        summary: bundle.summary.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned),
        root: root.to_path_buf(),
        from: out.to_path_buf(),
        to: free_folder(&place),
        seen: true,
        files,
        lists,
        bytes,
    })
}

/// A `/`-separated path below a folder, every part a valid name; `None` otherwise.
fn relative(path: &str) -> Option<String> {
    if path.starts_with('/') || path.starts_with('\\') || path.contains('\\') || path.trim().is_empty() {
        return None;
    }
    let parts: Vec<&str> = path.split('/').collect();
    let ok = parts.iter().all(|p| !p.is_empty() && *p != "." && *p != ".." && crate::fsops::check_name(p).is_ok());
    ok.then(|| parts.join("/"))
}

/// The title as a folder name: characters Windows refuses become spaces.
fn folder_name(title: &str) -> String {
    let name: String =
        title
            .chars()
            .map(|c| {
                if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                    ' '
                } else {
                    c
                }
            })
            .collect();
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    name.trim_end_matches(['.', ' ']).to_owned()
}

/// `path`, or `path (2)` (3, 4 …) when that is taken.
fn free_folder(path: &Path) -> PathBuf {
    if path.symlink_metadata().is_err() {
        return path.to_path_buf();
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    (2..)
        .map(|n| path.with_file_name(format!("{name} ({n})")))
        .find(|p| p.symlink_metadata().is_err())
        .expect("a free number")
}

/// Every file below `out` (relative, `/`-separated, sorted) with its metadata; links, bad names
/// and too much refused.
fn files_below(out: &Path, limits: Limits) -> Result<Vec<(String, std::fs::Metadata)>, BundleError> {
    let meta = out.symlink_metadata().map_err(|_| BundleError::Missing("/out".into()))?;
    if !meta.is_dir() {
        return Err(BundleError::Missing("/out".into()));
    }
    let mut found = Vec::new();
    let mut bytes = 0u64;
    let mut stack = vec![String::new()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(out.join(&dir))? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if dir.is_empty() { name.clone() } else { format!("{dir}/{name}") };
            let meta = entry.path().symlink_metadata()?;
            if meta.is_symlink() {
                return Err(BundleError::Link(rel));
            }
            if crate::fsops::check_name(&name).is_err() {
                return Err(BundleError::BadPath(rel));
            }
            if meta.is_dir() {
                stack.push(rel);
            } else if meta.is_file() {
                bytes += meta.len();
                found.push((rel, meta));
                if found.len() > limits.files {
                    return Err(BundleError::TooMany(found.len(), limits.files));
                }
                if bytes > limits.bytes {
                    return Err(BundleError::TooBig(bytes, limits.bytes));
                }
            } else {
                return Err(BundleError::BadPath(rel));
            }
        }
    }
    if found.is_empty() {
        return Err(BundleError::Invalid("the bundle holds no files (write them below /out)".into()));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(found)
}

impl ResolvedBundle {
    /// Where file `rel` lands.
    pub fn path_of(&self, rel: &str) -> PathBuf {
        rel.split('/').fold(self.to.clone(), |p, part| p.join(part))
    }

    fn target(&self, file: &ResolvedFile) -> Target {
        Target {
            path: self.path_of(&file.rel),
            hash: Some(file.hash.clone()),
            size: file.size,
            mtime_ns: file.mtime_ns,
        }
    }

    /// The steps of the change: copy the staging folder, index it, then tags, notes and lists.
    pub fn ops(&self) -> Vec<ApplyOp> {
        let staging = Target { path: self.from.clone(), hash: None, size: 0, mtime_ns: 0 };
        let mut ops = vec![
            ApplyOp::Copy { root: self.root.clone(), from: staging, to: self.to.clone(), folder: true },
            ApplyOp::Index { root: self.root.clone(), dir: self.to.clone(), seen: self.seen },
        ];
        for file in &self.files {
            ops.extend(file.tags.iter().map(|tag| ApplyOp::AddTag { target: self.target(file), tag: tag.clone() }));
            if let Some(note) = &file.note {
                ops.push(ApplyOp::SetNote { target: self.target(file), text: note.clone(), append: false });
            }
        }
        for list in &self.lists {
            let items = list
                .items
                .iter()
                .filter_map(|(rel, q)| self.files.iter().find(|f| f.rel == *rel).map(|f| (self.target(f), *q)))
                .collect();
            ops.push(ApplyOp::List { kind: list.kind, name: list.name.clone(), query: None, items });
        }
        ops
    }

    /// The same bundle at another free place, should its own have been taken since it was
    /// resolved (a suggestion that waited in the review).
    pub fn at_free_place(mut self) -> ResolvedBundle {
        self.to = free_folder(&self.to);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Library;

    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        landing: PathBuf,
        out: PathBuf,
    }

    /// A library (indexed, baseline done) and a staging folder with two STLs and a project file.
    fn fixture() -> (Fixture, Library) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib");
        let out = dir.path().join("staging").join("u1").join("out");
        std::fs::create_dir_all(root.join("Garten")).unwrap();
        std::fs::write(root.join("Garten").join("rose.stl"), "rose").unwrap();
        std::fs::create_dir_all(out.join("parts")).unwrap();
        std::fs::write(out.join("parts").join("bin_1x1.stl"), "solid bin11").unwrap();
        std::fs::write(out.join("parts").join("bin_2x1.stl"), "solid bin21").unwrap();
        std::fs::write(out.join("drawer.gridfinity.json"), "{}").unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        lib.set_baseline_root(&root, true);
        crate::scan::scan(&mut lib, &root, &root, crate::scan::Force::Nothing, &cancel).unwrap();
        lib.set_baseline_root(&root, false);
        lib.finish_baseline(&root).unwrap();
        let landing = root.join("Import");
        (Fixture { _dir: dir, root, landing, out }, lib)
    }

    fn bundle() -> Bundle {
        Bundle {
            title: "Schublade 250×305".into(),
            folder: Some("Gridfinity/Schublade".into()),
            files: vec![BundleFile {
                path: "drawer.gridfinity.json".into(),
                tags: vec!["Projekt".into()],
                note: Some("Belegung".into()),
            }],
            tags: vec!["Gridfinity".into()],
            lists: vec![BundleList {
                name: "Schublade – Teile".into(),
                kind: BundleListKind::Print,
                items: vec![
                    BundleItem { path: "parts/bin_1x1.stl".into(), quantity: 6 },
                    BundleItem { path: "parts/bin_2x1.stl".into(), quantity: 2 },
                    BundleItem { path: "parts/bin_1x1.stl".into(), quantity: 1 },
                ],
            }],
            ..Bundle::default()
        }
    }

    fn tags(lib: &Library, path: &Path) -> Vec<String> {
        let mut tags: Vec<String> = lib.tags_of(&[path.to_path_buf()]).unwrap().into_iter().map(|(t, _)| t).collect();
        tags.sort();
        tags
    }

    fn quantities(lib: &Library, name: &str) -> Vec<(String, u32)> {
        let Some(list) = lib.lists().unwrap().into_iter().find(|l| l.name == name) else { return Vec::new() };
        let mut out: Vec<(String, u32)> =
            lib.list_entries(list.id).unwrap().into_iter().map(|e| (e.entry.name, e.quantity)).collect();
        out.sort();
        out
    }

    #[test]
    fn index_works_inside_a_change() {
        // update_dir used to open a transaction, which SQLite refuses inside apply_change's savepoint.
        let (f, mut lib) = fixture();
        let new = f.root.join("Neu");
        std::fs::create_dir_all(new.join("sub")).unwrap();
        std::fs::write(new.join("sub").join("a.stl"), "a").unwrap();
        let ops = [ApplyOp::Index { root: f.root.clone(), dir: new.clone(), seen: true }];
        let report = lib.apply_change("plugin:test", "Index", &ops).unwrap();
        assert_eq!((report.applied, report.skipped), (1, 0));
        let a = new.join("sub").join("a.stl");
        assert!(lib.entry_of(&a).unwrap().is_some(), "indexed, the new folders on the way included");
        assert_eq!(lib.content_hash_of(&a).unwrap(), Some(crate::dedupe::content_hash(b"a")));
        assert_eq!(lib.inbox_count().unwrap(), 0, "seen: not in the inbox");
        // Not seen: the new content waits in the inbox, and the undo takes that back.
        std::fs::write(new.join("b.stl"), "b").unwrap();
        let ops = [ApplyOp::Index { root: f.root.clone(), dir: new.clone(), seen: false }];
        let report = lib.apply_change("plugin:test", "Index", &ops).unwrap();
        assert_eq!(lib.inbox_count().unwrap(), 1);
        lib.undo_change(report.id).unwrap();
        assert_eq!(lib.inbox_count().unwrap(), 0);
    }

    #[test]
    fn a_bundle_lands_tagged_listed_and_goes_again() {
        let (f, mut lib) = fixture();
        let resolved = resolve(&bundle(), &f.out, &f.root, &f.landing).unwrap();
        assert_eq!(resolved.to, f.root.join("Gridfinity").join("Schublade"));
        assert_eq!(resolved.files.len(), 3, "every file below /out, listed or not");
        assert_eq!(resolved.lists[0].items, [("parts/bin_1x1.stl".into(), 7), ("parts/bin_2x1.stl".into(), 2)]);
        let ops = resolved.ops();
        assert!(matches!(&ops[..2], [ApplyOp::Copy { folder: true, .. }, ApplyOp::Index { seen: true, .. }]));

        let report = lib.apply_change("plugin:bingrove", &resolved.title, &ops).unwrap();
        // The journal names the staging folder (a redo copies from it): cleaning up keeps it.
        let named = serde_json::to_string(&resolved.from).unwrap();
        assert!(lib.journal_mentions(named.trim_matches('"')).unwrap());
        assert!(!lib.journal_mentions("/elsewhere/staging/x").unwrap());
        assert_eq!(report.skipped, 0, "{report:?}");
        let bin = resolved.path_of("parts/bin_1x1.stl");
        assert!(bin.is_file() && f.out.join("parts").join("bin_1x1.stl").is_file(), "copied; staging stays");
        assert_eq!(quantities(&lib, "Schublade – Teile"), [("bin_1x1.stl".into(), 7), ("bin_2x1.stl".into(), 2)]);
        assert_eq!(tags(&lib, &bin), ["Gridfinity"]);
        let project = resolved.path_of("drawer.gridfinity.json");
        assert_eq!(tags(&lib, &project), ["Gridfinity", "Projekt"]);
        assert_eq!(lib.notes_of(std::slice::from_ref(&project)).unwrap().0.as_deref(), Some("Belegung"));
        assert_eq!(lib.inbox_count().unwrap(), 0, "generated files count as seen");

        // The same bundle again: next to it, never onto it.
        let again = resolve(&bundle(), &f.out, &f.root, &f.landing).unwrap();
        assert_eq!(again.to, f.root.join("Gridfinity").join("Schublade (2)"));

        // The user changes one part; the undo keeps it and takes the rest back.
        std::fs::write(resolved.path_of("parts/bin_2x1.stl"), "solid mine").unwrap();
        let undo = lib.undo_change(report.id).unwrap();
        assert!(undo.kept >= 1, "{undo:?}");
        assert!(!bin.exists() && !project.exists(), "the untouched files are gone");
        assert!(resolved.path_of("parts/bin_2x1.stl").is_file(), "the changed one stays");
        assert!(quantities(&lib, "Schublade – Teile").is_empty(), "the list made by the change is gone");
        assert!(lib.all_tags().unwrap().iter().all(|(t, _)| t != "Projekt"));

        // Once the user's file is gone, the redo copies from the staging folder again.
        std::fs::remove_dir_all(f.root.join("Gridfinity")).unwrap();
        let redo = lib.redo_change(report.id).unwrap();
        assert_eq!(redo.skipped, 0, "{redo:?}");
        assert!(bin.is_file() && project.is_file(), "back after the redo");
        assert_eq!(quantities(&lib, "Schublade – Teile"), [("bin_1x1.stl".into(), 7), ("bin_2x1.stl".into(), 2)]);
        assert_eq!(tags(&lib, &project), ["Gridfinity", "Projekt"]);
    }

    #[test]
    fn index_leaves_a_folder_its_copy_could_not_take() {
        let (f, mut lib) = fixture();
        let taken = f.root.join("Taken");
        std::fs::create_dir_all(&taken).unwrap();
        std::fs::write(taken.join("mine.stl"), "mine").unwrap();
        let staging = Target { path: f.out.clone(), hash: None, size: 0, mtime_ns: 0 };
        let ops = [
            ApplyOp::Copy { root: f.root.clone(), from: staging, to: taken.clone(), folder: true },
            ApplyOp::Index { root: f.root.clone(), dir: taken.clone(), seen: true },
        ];
        let report = lib.apply_change("plugin:test", "Taken", &ops).unwrap();
        assert_eq!((report.applied, report.skipped), (0, 2));
        assert!(lib.entry_of(&taken.join("mine.stl")).unwrap().is_none(), "the user's folder is left to the scan");
    }

    #[test]
    fn a_bundle_without_folder_lands_in_the_import_folder() {
        let (f, _lib) = fixture();
        let b = Bundle { title: "Box: 20/30".into(), folder: None, ..bundle() };
        let resolved = resolve(&b, &f.out, &f.root, &f.landing).unwrap();
        assert_eq!(resolved.to, f.landing.join("Box 20 30"));
    }

    #[test]
    fn bad_bundles_are_refused() {
        let (f, _lib) = fixture();
        let check = |b: &Bundle| resolve(b, &f.out, &f.root, &f.landing);
        let with_item = |path: &str| Bundle {
            lists: vec![BundleList {
                name: "L".into(),
                kind: BundleListKind::Print,
                items: vec![BundleItem { path: path.into(), quantity: 1 }],
            }],
            ..bundle()
        };
        assert!(matches!(check(&with_item("../escape.stl")), Err(BundleError::BadPath(_))));
        assert!(matches!(check(&with_item("/out/parts/bin_1x1.stl")), Err(BundleError::BadPath(_))));
        assert!(matches!(check(&with_item("parts/missing.stl")), Err(BundleError::Missing(_))));
        let escape = Bundle { folder: Some("../outside".into()), ..bundle() };
        assert!(matches!(check(&escape), Err(BundleError::BadPath(_))));
        let bad_tag = Bundle { tags: vec!["x".repeat(41)], ..bundle() };
        assert!(matches!(check(&bad_tag), Err(BundleError::Invalid(_))));
        let limits = Limits { files: 2, bytes: 1 << 20 };
        assert!(matches!(resolve_with(&bundle(), &f.out, &f.root, &f.landing, limits), Err(BundleError::TooMany(..))));
        let limits = Limits { files: 100, bytes: 10 };
        assert!(matches!(resolve_with(&bundle(), &f.out, &f.root, &f.landing, limits), Err(BundleError::TooBig(..))));
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(f.root.join("Garten"), f.out.join("garten")).unwrap();
            assert!(matches!(check(&bundle()), Err(BundleError::Link(_))));
        }
    }
}
