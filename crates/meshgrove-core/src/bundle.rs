//! A plugin's bundle as one change of the library: the files it wrote into its staging folder
//! are checked ([`resolve`]) and become the steps of [`crate::changes`] — a copy of the whole
//! folder to a free place, its indexing, then tags, notes and lists ([`ResolvedBundle::ops`]).
//! The staging folder stays until the change is gone from the history, so a redo can copy again.
//!
//! A bundle may instead put its files beside the library files they were made from
//! ([`resolve_beside`]): loose copies under free names, each a step of its own, and only those
//! are indexed and taken back.

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
    /// The staging folder (the plugin's `/out`), copied as a whole, or file by file when the
    /// files land loose ([`ResolvedFile::at`]).
    pub from: PathBuf,
    /// The free place below `root` it is copied to; of loose files, the folder the first lands in.
    pub to: PathBuf,
    /// New content does not wait in the inbox. Only a bundle that lands in the import folder
    /// ([`resolve`] without a folder) is not seen: its files wait there to be put away.
    pub seen: bool,
    /// It waits for the user in the review, whatever they let be accepted by itself: files a
    /// tool's job made after the AI had its answer.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub review: bool,
    /// It goes into a folder the user chose for it ([`resolve_into`]), maybe in another
    /// library than imports go to: the review names the library.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub chosen: bool,
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
    /// Where it lands as a loose file, beside its source ([`resolve_beside`]) or in the folder
    /// the user chose ([`resolve_into`]); none: below the bundle's folder, as `rel` says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<PathBuf>,
    /// The name of the library file it lands beside, for the review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beside: Option<String>,
}

/// A library file a bundle's files are put beside ([`Bundle::next_to`]), found by the host as
/// far as the plugin may read the library.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    /// As the bundle names it.
    pub reference: String,
    /// The library it is in.
    pub root: PathBuf,
    /// The file (also one inside an archive).
    pub path: PathBuf,
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
    if bundle.uses_next_to() {
        return Err(BundleError::Invalid(
            "the bundle puts its files beside library files (next_to), which have to be found first".into(),
        ));
    }
    if bundle.target.is_some() {
        return Err(BundleError::Invalid(
            "the bundle goes into a folder the user chose (target), which has to be found first".into(),
        ));
    }
    if !landing.starts_with(root) {
        return Err(BundleError::Invalid("the import folder is not in the library".into()));
    }
    let checked = check(bundle, out, limits)?;
    let (place, seen) = match &bundle.folder {
        Some(folder) => {
            let place = relative(folder)
                .ok_or_else(|| BundleError::BadPath(folder.clone()))?
                .split('/')
                .fold(root.to_path_buf(), |p, part| p.join(part));
            (place, true)
        }
        None => {
            let name = folder_name(&checked.title);
            if crate::fsops::check_name(&name).is_err() {
                let title = &checked.title;
                return Err(BundleError::Invalid(format!("no folder name can be made of the title {title:?}")));
            }
            // In the import folder the files wait in the inbox like anything imported, to be
            // put away later.
            (landing.join(name), false)
        }
    };
    let mut resolved = checked.placed(bundle, out, root, free_folder(&place));
    resolved.seen = seen;
    Ok(resolved)
}

/// Checks `bundle` against `out` like [`resolve`], for a bundle whose files land loose beside
/// library files ([`Bundle::next_to`], [`crate::bundle::BundleFile::next_to`]): each in the
/// folder of its source (of the archive, for a source inside one) under its own name, with a
/// number where that is taken. `sources` are the files the bundle names, as the host found
/// them. Every file needs a source, the bundle's or its own; all of them are in one library; a
/// folder of its own the bundle cannot have as well.
pub fn resolve_beside(
    bundle: &Bundle,
    out: &Path,
    sources: &[Source],
    limits: Limits,
) -> Result<ResolvedBundle, BundleError> {
    if bundle.folder.is_some() {
        return Err(BundleError::Invalid(
            "next_to and folder do not go together: files beside their source land loose, not in a folder".into(),
        ));
    }
    if bundle.target.is_some() {
        return Err(next_to_and_target());
    }
    let mut checked = check(bundle, out, limits)?;
    let mut root: Option<&Path> = None;
    let mut taken: Vec<PathBuf> = Vec::new();
    for file in &mut checked.files {
        let own = bundle.files.iter().find(|f| relative(&f.path).as_deref() == Some(file.rel.as_str()));
        let reference = own.and_then(|f| f.next_to.as_ref()).or(bundle.next_to.as_ref()).ok_or_else(|| {
            BundleError::Invalid(format!(
                "{:?} has no place: with next_to every file is put beside a library file, its own or the bundle's",
                file.rel
            ))
        })?;
        let source = sources
            .iter()
            .find(|s| s.reference == *reference)
            .ok_or_else(|| BundleError::Invalid(format!("the file to put {:?} beside was not found", file.rel)))?;
        if *root.get_or_insert(&source.root) != source.root {
            return Err(BundleError::Invalid(
                "the files would go into several libraries: hand over one bundle for each library".into(),
            ));
        }
        // The folder the source is in; of a file inside an archive, the archive's.
        let dir = source
            .path
            .ancestors()
            .skip(1)
            .find(|a| a.is_dir())
            .filter(|dir| dir.starts_with(&source.root))
            .ok_or_else(|| BundleError::Invalid(format!("the file to put {:?} beside is gone", file.rel)))?;
        let name = file.rel.rsplit('/').next().unwrap_or(&file.rel);
        let at = crate::import::free_target(dir, name, &taken);
        taken.push(at.clone());
        file.at = Some(at);
        file.beside = Some(crate::listing::file_name(&source.path));
    }
    let root = root.ok_or_else(|| BundleError::Invalid("the bundle holds no files".into()))?.to_path_buf();
    let to = taken.first().and_then(|p| p.parent()).unwrap_or(&root).to_path_buf();
    Ok(checked.placed(bundle, out, &root, to))
}

fn next_to_and_target() -> BundleError {
    BundleError::Invalid(
        "next_to and target do not go together: a file goes beside its source or into the chosen folder".into(),
    )
}

/// Checks `bundle` against `out` like [`resolve`], for a bundle that goes into a folder the user
/// chose ([`Bundle::target`]): `dir` in library `root`, which need not exist yet (copying makes
/// it, the undo takes it away again). With a folder of its own ([`Bundle::folder`]) the bundle
/// lands in that folder below `dir`, as a whole, numbered when taken (" (2)"); without, its files
/// land loose in `dir`, each under its own name (a folder of its own below `out` is dropped) with
/// a number where that is taken.
pub fn resolve_into(
    bundle: &Bundle,
    out: &Path,
    root: &Path,
    dir: &Path,
    limits: Limits,
) -> Result<ResolvedBundle, BundleError> {
    if bundle.uses_next_to() {
        return Err(next_to_and_target());
    }
    if root.as_os_str().is_empty() || !dir.starts_with(root) {
        return Err(BundleError::Invalid("the chosen folder is not in its library".into()));
    }
    let mut checked = check(bundle, out, limits)?;
    if let Some(folder) = &bundle.folder {
        let place = relative(folder)
            .ok_or_else(|| BundleError::BadPath(folder.clone()))?
            .split('/')
            .fold(dir.to_path_buf(), |p, part| p.join(part));
        return Ok(ResolvedBundle { chosen: true, ..checked.placed(bundle, out, root, free_folder(&place)) });
    }
    let mut taken: Vec<PathBuf> = Vec::new();
    for file in &mut checked.files {
        let name = file.rel.rsplit('/').next().unwrap_or(&file.rel);
        let at = crate::import::free_target(dir, name, &taken);
        taken.push(at.clone());
        file.at = Some(at);
    }
    Ok(ResolvedBundle { chosen: true, ..checked.placed(bundle, out, root, dir.to_path_buf()) })
}

/// What of a bundle is the same wherever it goes.
struct Checked {
    title: String,
    files: Vec<ResolvedFile>,
    lists: Vec<ResolvedList>,
    bytes: u64,
}

impl Checked {
    fn placed(self, bundle: &Bundle, out: &Path, root: &Path, to: PathBuf) -> ResolvedBundle {
        ResolvedBundle {
            plugin: String::new(),
            title: self.title,
            summary: bundle.summary.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned),
            root: root.to_path_buf(),
            from: out.to_path_buf(),
            to,
            seen: true,
            review: false,
            chosen: false,
            files: self.files,
            lists: self.lists,
            bytes: self.bytes,
        }
    }
}

/// The title, files (with tags and notes) and lists of `bundle`, checked against what is in `out`.
fn check(bundle: &Bundle, out: &Path, limits: Limits) -> Result<Checked, BundleError> {
    let title = bundle.title.trim();
    if title.is_empty() || title.chars().count() > 120 {
        return Err(BundleError::Invalid("the title is empty or longer than 120 characters".into()));
    }
    if bundle.job.is_some() {
        return Err(BundleError::Invalid(
            "this Meshgrove does not start a bundle's job yet: write the files first".into(),
        ));
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
            at: None,
            beside: None,
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
    Ok(Checked { title: title.to_owned(), files, lists, bytes })
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
    /// Whether the files land loose beside library files, not in a folder of their own.
    pub fn is_loose(&self) -> bool {
        self.files.iter().any(|f| f.at.is_some())
    }

    /// Where file `rel` lands.
    pub fn path_of(&self, rel: &str) -> PathBuf {
        match self.files.iter().find(|f| f.rel == rel).and_then(|f| f.at.clone()) {
            Some(at) => at,
            None => rel.split('/').fold(self.to.clone(), |p, part| p.join(part)),
        }
    }

    fn target(&self, file: &ResolvedFile) -> Target {
        Target {
            path: self.path_of(&file.rel),
            hash: Some(file.hash.clone()),
            size: file.size,
            mtime_ns: file.mtime_ns,
        }
    }

    /// The steps of the change: copy the staging folder and index it (or, of loose files, copy
    /// each beside its source and index just those), then tags, notes and lists.
    pub fn ops(&self) -> Vec<ApplyOp> {
        let mut ops = if self.is_loose() {
            self.loose_copies()
        } else {
            let staging = Target { path: self.from.clone(), hash: None, size: 0, mtime_ns: 0 };
            vec![
                ApplyOp::Copy { root: self.root.clone(), from: staging, to: self.to.clone(), folder: true },
                ApplyOp::Index { root: self.root.clone(), dir: self.to.clone(), seen: self.seen, only: Vec::new() },
            ]
        };
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

    /// One copy for each loose file, then one index step for each folder they land in, which
    /// looks at those files only: the folder is the user's.
    fn loose_copies(&self) -> Vec<ApplyOp> {
        let mut ops = Vec::new();
        let mut folders: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
        for file in &self.files {
            let Some(at) = &file.at else { continue };
            let staged = file.rel.split('/').fold(self.from.clone(), |p, part| p.join(part));
            let from = Target { path: staged, hash: Some(file.hash.clone()), size: file.size, mtime_ns: file.mtime_ns };
            ops.push(ApplyOp::Copy { root: self.root.clone(), from, to: at.clone(), folder: false });
            let dir = at.parent().unwrap_or(&self.root).to_path_buf();
            match folders.iter_mut().find(|(d, _)| *d == dir) {
                Some((_, files)) => files.push(at.clone()),
                None => folders.push((dir, vec![at.clone()])),
            }
        }
        ops.extend(folders.into_iter().map(|(dir, only)| ApplyOp::Index {
            root: self.root.clone(),
            dir,
            seen: self.seen,
            only,
        }));
        ops
    }

    /// The same bundle at another free place, should its own have been taken since it was
    /// resolved (a suggestion that waited in the review).
    pub fn at_free_place(mut self) -> ResolvedBundle {
        if !self.is_loose() {
            self.to = free_folder(&self.to);
            return self;
        }
        let mut taken: Vec<PathBuf> = Vec::new();
        for file in &mut self.files {
            let Some(at) = &file.at else { continue };
            let name = file.rel.rsplit('/').next().unwrap_or(&file.rel);
            let free = crate::import::free_target(at.parent().unwrap_or(&self.root), name, &taken);
            taken.push(free.clone());
            file.at = Some(free);
        }
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
                ..BundleFile::default()
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
        let ops = [ApplyOp::Index { root: f.root.clone(), dir: new.clone(), seen: true, only: Vec::new() }];
        let report = lib.apply_change("plugin:test", "Index", &ops).unwrap();
        assert_eq!((report.applied, report.skipped), (1, 0));
        let a = new.join("sub").join("a.stl");
        assert!(lib.entry_of(&a).unwrap().is_some(), "indexed, the new folders on the way included");
        assert_eq!(lib.content_hash_of(&a).unwrap(), Some(crate::dedupe::content_hash(b"a")));
        assert_eq!(lib.inbox_count().unwrap(), 0, "seen: not in the inbox");
        // Not seen: the new content waits in the inbox, and the undo takes that back.
        std::fs::write(new.join("b.stl"), "b").unwrap();
        let ops = [ApplyOp::Index { root: f.root.clone(), dir: new.clone(), seen: false, only: Vec::new() }];
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
            ApplyOp::Index { root: f.root.clone(), dir: taken.clone(), seen: true, only: Vec::new() },
        ];
        let report = lib.apply_change("plugin:test", "Taken", &ops).unwrap();
        assert_eq!((report.applied, report.skipped), (0, 2));
        assert!(lib.entry_of(&taken.join("mine.stl")).unwrap().is_none(), "the user's folder is left to the scan");
    }

    #[test]
    fn a_bundle_without_folder_lands_in_the_import_folder() {
        let (f, mut lib) = fixture();
        let b = Bundle { title: "Box: 20/30".into(), folder: None, ..bundle() };
        let resolved = resolve(&b, &f.out, &f.root, &f.landing).unwrap();
        assert_eq!(resolved.to, f.landing.join("Box 20 30"));
        // There its files wait in the inbox to be put away, like anything imported; the undo
        // takes them out again.
        assert!(!resolved.seen);
        let report = lib.apply_change("plugin:bingrove", &resolved.title, &resolved.ops()).unwrap();
        assert_eq!(report.skipped, 0, "{report:?}");
        assert_eq!(lib.inbox_count().unwrap(), 2, "the two models; the project file is no import");
        lib.undo_change(report.id).unwrap();
        assert_eq!(lib.inbox_count().unwrap(), 0);
    }

    /// The library of [`fixture`] with a second folder, and a staging folder of two textured
    /// models: one for the rose, one (in a folder of its own there) for the pot.
    fn beside() -> (Fixture, Library, Vec<Source>) {
        let (mut f, mut lib) = fixture();
        std::fs::create_dir_all(f.root.join("Töpfe")).unwrap();
        std::fs::write(f.root.join("Töpfe").join("topf.stl"), "topf").unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        crate::scan::scan(&mut lib, &f.root, &f.root, crate::scan::Force::Nothing, &cancel).unwrap();
        f.out = f.out.parent().unwrap().parent().unwrap().join("u2").join("out");
        std::fs::create_dir_all(f.out.join("sub")).unwrap();
        std::fs::write(f.out.join("rose – Holz.3mf"), "rose with wood").unwrap();
        std::fs::write(f.out.join("sub").join("topf – Holz.stl"), "solid topf with wood").unwrap();
        let source = |id: &str, rel: &[&str]| Source {
            reference: id.into(),
            root: f.root.clone(),
            path: rel.iter().fold(f.root.clone(), |p, part| p.join(part)),
        };
        let sources = vec![
            source("lib/Garten/rose.stl", &["Garten", "rose.stl"]),
            source("lib/Töpfe/topf.stl", &["Töpfe", "topf.stl"]),
        ];
        (f, lib, sources)
    }

    /// One bundle for two sources: the rose's file beside the rose (the bundle's source), the
    /// pot's beside the pot (its own).
    fn textured() -> Bundle {
        Bundle {
            title: "Holz auf 2 Modellen".into(),
            next_to: Some("lib/Garten/rose.stl".into()),
            files: vec![
                BundleFile {
                    path: "rose – Holz.3mf".into(),
                    note: Some("Holz, 0,4 mm tief".into()),
                    ..BundleFile::default()
                },
                BundleFile {
                    path: "sub/topf – Holz.stl".into(),
                    next_to: Some("lib/Töpfe/topf.stl".into()),
                    ..BundleFile::default()
                },
            ],
            tags: vec!["Texturiert".into()],
            ..Bundle::default()
        }
    }

    #[test]
    fn files_land_loose_beside_their_sources_and_only_they_go_again() {
        let (f, mut lib, sources) = beside();
        let (rose, topf) = (f.root.join("Garten").join("rose.stl"), f.root.join("Töpfe").join("topf.stl"));
        // A name that is taken: the user's file stays, the copy gets a number.
        let mine = f.root.join("Garten").join("rose – Holz.3mf");
        std::fs::write(&mine, "my own").unwrap();
        let resolved = resolve_beside(&textured(), &f.out, &sources, Limits::DEFAULT).unwrap();
        assert!(resolved.is_loose());
        let (rose_copy, topf_copy) = (resolved.path_of("rose – Holz.3mf"), resolved.path_of("sub/topf – Holz.stl"));
        assert_eq!(rose_copy, f.root.join("Garten").join("rose – Holz (2).3mf"));
        assert_eq!(topf_copy, f.root.join("Töpfe").join("topf – Holz.stl"), "loose: without its folder in /out");
        let beside: Vec<_> = resolved.files.iter().map(|file| file.beside.as_deref()).collect();
        assert_eq!(beside, [Some("rose.stl"), Some("topf.stl")]);
        assert_eq!((&resolved.root, &resolved.to), (&f.root, &f.root.join("Garten")));

        let ops = resolved.ops();
        assert!(matches!(&ops[..2], [ApplyOp::Copy { folder: false, .. }, ApplyOp::Copy { folder: false, .. }]));
        let indexed: Vec<_> = ops
            .iter()
            .filter_map(|op| match op {
                ApplyOp::Index { dir, only, seen: true, .. } => Some((dir.clone(), only.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            indexed,
            [(f.root.join("Garten"), vec![rose_copy.clone()]), (f.root.join("Töpfe"), vec![topf_copy.clone()])],
            "each folder is looked at for the copies only"
        );

        let report = lib.apply_change("plugin:texgrove", &resolved.title, &ops).unwrap();
        assert_eq!(report.skipped, 0, "{report:?}");
        assert_eq!(std::fs::read(&rose_copy).unwrap(), b"rose with wood");
        assert_eq!(std::fs::read(&mine).unwrap(), b"my own", "never replaced");
        assert_eq!(tags(&lib, &rose_copy), ["Texturiert"]);
        assert_eq!(tags(&lib, &topf_copy), ["Texturiert"]);
        assert_eq!(lib.notes_of(std::slice::from_ref(&rose_copy)).unwrap().0.as_deref(), Some("Holz, 0,4 mm tief"));
        assert!(lib.entry_of(&topf_copy).unwrap().is_some());
        // The files around them were not touched: not hashed, not tagged, not in the inbox.
        assert_eq!(lib.content_hash_of(&rose).unwrap(), None, "the source was left alone");
        assert_eq!(lib.content_hash_of(&mine).unwrap(), None);
        assert!(tags(&lib, &rose).is_empty() && tags(&lib, &topf).is_empty());
        assert_eq!(lib.inbox_count().unwrap(), 0, "made files count as seen");

        // Undo takes the copies away and nothing else.
        let undo = lib.undo_change(report.id).unwrap();
        assert_eq!(undo.kept, 0, "{undo:?}");
        assert!(!rose_copy.exists() && !topf_copy.exists());
        assert!(rose.is_file() && topf.is_file() && mine.is_file() && f.root.join("Töpfe").is_dir());
        assert!(lib.entry_of(&rose_copy).unwrap().is_none() && lib.entry_of(&rose).unwrap().is_some());
        assert!(lib.all_tags().unwrap().iter().all(|(t, _)| t != "Texturiert"));

        // Redo copies again; a copy the user changed since stays at the next undo.
        let redo = lib.redo_change(report.id).unwrap();
        assert_eq!(redo.skipped, 0, "{redo:?}");
        assert!(rose_copy.is_file() && topf_copy.is_file());
        std::fs::write(&topf_copy, "solid mine now").unwrap();
        let undo = lib.undo_change(redo.id).unwrap();
        assert!(undo.kept >= 1, "{undo:?}");
        assert!(!rose_copy.exists() && topf_copy.is_file());
    }

    #[test]
    fn two_files_of_one_name_and_a_source_in_an_archive() {
        let (f, _lib, mut sources) = beside();
        // Two files called the same land beside one source; and a source inside an archive
        // has its copy beside the archive.
        std::fs::create_dir_all(f.out.join("a")).unwrap();
        std::fs::write(f.out.join("a").join("rose – Holz.3mf"), "another").unwrap();
        std::fs::write(f.root.join("Garten").join("pack.zip"), "PK").unwrap();
        let member = f.root.join("Garten").join("pack.zip").join("minis").join("orc.stl");
        sources.push(Source {
            reference: "lib/Garten/pack.zip/minis/orc.stl".into(),
            root: f.root.clone(),
            path: member,
        });
        let mut bundle = textured();
        bundle.files[1].next_to = Some("lib/Garten/pack.zip/minis/orc.stl".into());
        let resolved = resolve_beside(&bundle, &f.out, &sources, Limits::DEFAULT).unwrap();
        let names: Vec<String> =
            resolved.files.iter().map(|file| crate::listing::file_name(file.at.as_ref().unwrap())).collect();
        assert_eq!(names, ["rose – Holz.3mf", "rose – Holz (2).3mf", "topf – Holz.stl"]);
        assert!(resolved.files.iter().all(|file| file.at.as_ref().unwrap().parent() == Some(&f.root.join("Garten"))));
        assert_eq!(resolved.files[2].beside.as_deref(), Some("orc.stl"));
        // A place taken while the bundle waited in the review ("… (2)"): the next free name.
        std::fs::write(resolved.path_of("rose – Holz.3mf"), "taken since").unwrap();
        let later = resolved.clone().at_free_place();
        let names: Vec<String> =
            later.files.iter().map(|file| crate::listing::file_name(file.at.as_ref().unwrap())).collect();
        assert_eq!(names, ["rose – Holz.3mf", "rose – Holz (3).3mf", "topf – Holz.stl"]);
        // What was resolved before there were loose files still reads.
        let old = serde_json::json!({"title": "T", "root": "/r", "from": "/s", "to": "/r/T", "seen": true, "bytes": 1,
            "files": [{"rel": "a.stl", "size": 1, "mtime_ns": 2, "hash": "h"}]});
        let old: ResolvedBundle = serde_json::from_value(old).unwrap();
        assert!(!old.is_loose() && old.path_of("a.stl") == Path::new("/r/T/a.stl"));
        let index: ApplyOp =
            serde_json::from_str(r#"{"op": "index", "root": "/r", "dir": "/r/T", "seen": true}"#).unwrap();
        assert!(matches!(index, ApplyOp::Index { only, .. } if only.is_empty()));
    }

    #[test]
    fn a_copy_whose_name_was_taken_in_between_is_left_out() {
        let (f, mut lib, sources) = beside();
        let resolved = resolve_beside(&textured(), &f.out, &sources, Limits::DEFAULT).unwrap();
        // Someone puts a file where the rose's copy was to go.
        let theirs = resolved.path_of("rose – Holz.3mf");
        std::fs::write(&theirs, "theirs").unwrap();
        let report = lib.apply_change("plugin:texgrove", &resolved.title, &resolved.ops()).unwrap();
        assert_eq!(std::fs::read(&theirs).unwrap(), b"theirs");
        assert!(tags(&lib, &theirs).is_empty(), "their file is not ours to tag");
        assert_eq!(lib.content_hash_of(&theirs).unwrap(), None, "nor to index");
        let topf_copy = resolved.path_of("sub/topf – Holz.stl");
        assert_eq!(tags(&lib, &topf_copy), ["Texturiert"], "the other file went through");
        // copy, index, tag and note of the rose's file did nothing
        assert_eq!((report.applied, report.skipped), (3, 4), "{report:?}");
        lib.undo_change(report.id).unwrap();
        assert!(theirs.is_file() && !topf_copy.exists());
    }

    #[test]
    fn bundles_beside_their_sources_are_checked() {
        let (f, _lib, sources) = beside();
        let check = |b: &Bundle, sources: &[Source]| match resolve_beside(b, &f.out, sources, Limits::DEFAULT) {
            Err(BundleError::Invalid(why)) => why,
            other => panic!("{other:?}"),
        };
        let with_folder = Bundle { folder: Some("Texturen".into()), ..textured() };
        assert!(check(&with_folder, &sources).contains("next_to and folder do not go together"));
        let mut no_place = textured();
        no_place.next_to = None;
        assert!(check(&no_place, &sources).contains("has no place"), "the rose's file names no source");
        assert!(check(&textured(), &sources[..1]).contains("was not found"), "a source the host did not find");
        let mut elsewhere = sources.clone();
        elsewhere[1].root = f.root.parent().unwrap().join("other");
        assert!(check(&textured(), &elsewhere).contains("several libraries"));
        let mut outside = sources.clone();
        outside[0].path = f.root.parent().unwrap().join("rose.stl");
        assert!(check(&textured(), &outside).contains("is gone"), "a source outside its library");
        let mut gone = sources.clone();
        gone[0].path = f.root.join("Weg").join("rose.stl");
        std::fs::remove_dir_all(f.root.join("Garten")).unwrap();
        // The folder is gone: the next one up that is there takes the copy.
        assert_eq!(
            resolve_beside(&textured(), &f.out, &gone, Limits::DEFAULT).unwrap().path_of("rose – Holz.3mf"),
            f.root.join("rose – Holz.3mf")
        );
        // The folder kind of bundle does not take next_to, and no bundle takes a job yet.
        let err = resolve(&textured(), &f.out, &f.root, &f.landing).unwrap_err();
        assert!(matches!(&err, BundleError::Invalid(why) if why.contains("have to be found first")), "{err}");
        let job = Bundle {
            job: Some(meshgrove_plugin_api::BundleJob { kind: "export".into(), ..Default::default() }),
            ..textured()
        };
        assert!(check(&job, &sources).contains("does not start a bundle's job yet"));
        // The usual checks hold here too.
        let mut escape = textured();
        escape.files[0].path = "../rose.3mf".into();
        assert!(matches!(resolve_beside(&escape, &f.out, &sources, Limits::DEFAULT), Err(BundleError::BadPath(_))));
    }

    #[test]
    fn a_bundle_lands_in_the_chosen_folder_as_a_whole_or_loose() {
        let (f, mut lib) = fixture();
        let chosen = f.root.join("Garten");
        let into = |b: &Bundle| resolve_into(b, &f.out, &f.root, &chosen, Limits::DEFAULT);
        let target = |folder: Option<&str>| Bundle {
            target: Some("3f9a0c11d2e4b567".into()),
            folder: folder.map(str::to_owned),
            ..bundle()
        };

        // With a folder of its own: that folder below the chosen one, as a whole.
        let whole = into(&target(Some("Schublade"))).unwrap();
        assert_eq!(whole.to, chosen.join("Schublade"));
        assert!(!whole.is_loose() && whole.chosen);
        assert!(!resolve(&bundle(), &f.out, &f.root, &f.landing).unwrap().chosen);
        std::fs::create_dir_all(chosen.join("Schublade")).unwrap();
        assert_eq!(into(&target(Some("Schublade"))).unwrap().to, chosen.join("Schublade (2)"), "never onto it");
        std::fs::remove_dir(chosen.join("Schublade")).unwrap();

        // Without: loose in the chosen folder, own folders below /out dropped, taken names numbered.
        std::fs::write(chosen.join("bin_1x1.stl"), "mine").unwrap();
        let loose = into(&target(None)).unwrap();
        assert!(loose.is_loose());
        assert_eq!(loose.to, chosen);
        assert_eq!(loose.path_of("parts/bin_1x1.stl"), chosen.join("bin_1x1 (2).stl"));
        assert_eq!(loose.path_of("parts/bin_2x1.stl"), chosen.join("bin_2x1.stl"));
        assert_eq!(loose.path_of("drawer.gridfinity.json"), chosen.join("drawer.gridfinity.json"));
        assert!(loose.files.iter().all(|file| file.beside.is_none()), "no source to name");

        let report = lib.apply_change("plugin:bingrove", &loose.title, &loose.ops()).unwrap();
        assert_eq!(report.skipped, 0, "{report:?}");
        assert_eq!(std::fs::read_to_string(chosen.join("bin_1x1.stl")).unwrap(), "mine", "the user's file stays");
        assert!(chosen.join("bin_1x1 (2).stl").is_file() && chosen.join("rose.stl").is_file());
        assert_eq!(quantities(&lib, "Schublade – Teile"), [("bin_1x1 (2).stl".into(), 7), ("bin_2x1.stl".into(), 2)]);
        let project = chosen.join("drawer.gridfinity.json");
        assert_eq!(tags(&lib, &project), ["Gridfinity", "Projekt"]);
        lib.undo_change(report.id).unwrap();
        assert!(!project.exists() && !chosen.join("bin_1x1 (2).stl").exists(), "the undo takes them back");
        assert!(chosen.join("bin_1x1.stl").is_file() && chosen.join("rose.stl").is_file(), "and only them");
    }

    #[test]
    fn a_new_chosen_folder_is_made_and_goes_again_with_the_undo() {
        let (f, mut lib) = fixture();
        let chosen = f.root.join("Werkstatt").join("Boxen");
        for folder in [None, Some("Schublade 160".to_owned())] {
            let b = Bundle { target: Some("t".into()), folder, ..bundle() };
            let resolved = resolve_into(&b, &f.out, &f.root, &chosen, Limits::DEFAULT).unwrap();
            let report = lib.apply_change("plugin:bingrove", &resolved.title, &resolved.ops()).unwrap();
            assert_eq!(report.skipped, 0, "{report:?}");
            let bin = resolved.path_of("parts/bin_1x1.stl");
            assert!(bin.is_file() && bin.starts_with(&chosen), "{bin:?}");
            assert!(lib.entry_of(&bin).unwrap().is_some(), "indexed");
            lib.undo_change(report.id).unwrap();
            assert!(!f.root.join("Werkstatt").exists(), "the folders the change made are gone again");
        }
    }

    #[test]
    fn a_chosen_folder_is_checked() {
        let (f, _lib) = fixture();
        let chosen = f.root.join("Garten");
        let b = Bundle { target: Some("t".into()), folder: None, ..bundle() };
        let outside = resolve_into(&b, &f.out, &f.root, Path::new("/elsewhere"), Limits::DEFAULT).unwrap_err();
        assert!(outside.to_string().contains("not in its library"), "{outside}");
        let beside = Bundle { next_to: Some("lib/Garten/rose.stl".into()), ..b.clone() };
        let both = resolve_into(&beside, &f.out, &f.root, &chosen, Limits::DEFAULT).unwrap_err();
        assert!(both.to_string().contains("next_to and target"), "{both}");
        let source =
            Source { reference: "lib/Garten/rose.stl".into(), root: f.root.clone(), path: chosen.join("rose.stl") };
        let both = resolve_beside(&beside, &f.out, &[source], Limits::DEFAULT).unwrap_err();
        assert!(both.to_string().contains("next_to and target"), "{both}");
        // Not found first: the import folder would take it silently.
        let unresolved = resolve(&b, &f.out, &f.root, &f.landing).unwrap_err();
        assert!(unresolved.to_string().contains("chose (target)"), "{unresolved}");
        let escape = Bundle { folder: Some("../outside".into()), ..b };
        assert!(matches!(
            resolve_into(&escape, &f.out, &f.root, &chosen, Limits::DEFAULT),
            Err(BundleError::BadPath(_))
        ));
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
