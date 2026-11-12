//! Changing archives of the library: unpacking one beside itself, packing files into a new ZIP,
//! adding files to a ZIP. Planned here (checked, the new names freed; nothing is ever replaced),
//! applied as journal steps ([`to_ops`]) by the library's writer: from the context menu at once,
//! or as the AI's suggestion once the user accepted it in the review. Only ZIP is written; 7z,
//! RAR and TAR are unpacked, never changed.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ai::proposals::Target;
use crate::archive;
use crate::changes::ApplyOp;
use crate::db::nanos;
use crate::formats::Kind;
use crate::import::{free_target, stem};
use crate::listing;

/// What is to happen with an archive (the target of the suggestion or action), as the review
/// and the journal see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArchivePayload {
    /// The library it all happens in.
    pub root: PathBuf,
    #[serde(flatten)]
    pub op: ArchiveOp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ArchiveOp {
    /// The archive unpacked into the new folder `into` beside it; the archive stays.
    Unpack { into: PathBuf },
    /// `sources` (files, folders with what is below them) packed into the new ZIP `to`, named
    /// relative to `base`. The target is the first source.
    Pack { sources: Vec<Target>, base: PathBuf, to: PathBuf },
    /// `sources` added to the ZIP below `prefix` (`""`, or `minis/`).
    Add { sources: Vec<Target>, base: PathBuf, prefix: String },
}

impl ArchivePayload {
    /// The folder to list once the change is applied: where the new folder, the new ZIP or the
    /// changed one lies.
    pub fn lands_in(&self, target: &Path) -> PathBuf {
        let at = match &self.op {
            ArchiveOp::Unpack { into } => into.as_path(),
            ArchiveOp::Pack { to, .. } => to.as_path(),
            ArchiveOp::Add { .. } => target,
        };
        at.parent().unwrap_or(&self.root).to_path_buf()
    }

    /// How many files and folders go in (pack, add).
    pub fn sources(&self) -> &[Target] {
        match &self.op {
            ArchiveOp::Unpack { .. } => &[],
            ArchiveOp::Pack { sources, .. } | ArchiveOp::Add { sources, .. } => sources,
        }
    }
}

/// Why an archive operation cannot be planned, worded for the assistant that asked for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    Gone,
    /// Not in the library named.
    Outside,
    /// Inside an archive: members cannot be packed or unpacked by themselves.
    InArchive,
    NotArchive,
    /// Only ZIP archives can be written.
    NotZip,
    ReadOnly,
    /// No sources, or the archive among its own sources.
    Nothing,
    BadName(&'static str),
    /// The folder inside the archive: only plain folder names, nothing that leads out of it.
    BadFolder,
    /// A member of that name is in the archive already, or two sources would get that name.
    Taken(String),
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::Gone => f.write_str("is gone"),
            Problem::Outside => f.write_str("is not in that library"),
            Problem::InArchive => f.write_str("lies inside an archive; unpack the archive instead"),
            Problem::NotArchive => f.write_str("is not an archive"),
            Problem::NotZip => f.write_str("is not a ZIP: only ZIP archives can be written"),
            Problem::ReadOnly => f.write_str("is on a read-only drive"),
            Problem::Nothing => f.write_str("nothing to pack"),
            Problem::BadName(why) => write!(f, "the name will not do: {why}"),
            Problem::BadFolder => {
                f.write_str("the folder inside the archive will not do: plain folder names only, no '.', '..' or '\\'")
            }
            Problem::Taken(name) => write!(f, "{name} is in the archive already, or would be twice"),
        }
    }
}

/// The archive `archive` of library `root` unpacked into a new folder named after it, beside it.
pub fn plan_unpack(root: &Path, archive: &Path) -> Result<(Target, ArchivePayload), Problem> {
    let target = source(root, archive, true)?;
    if Kind::from_file_name(&listing::file_name(archive)) != Kind::Archive || target.size == 0 && archive.is_dir() {
        return Err(Problem::NotArchive);
    }
    let parent = archive.parent().ok_or(Problem::Gone)?;
    if crate::fsinfo::is_read_only(parent) {
        return Err(Problem::ReadOnly);
    }
    let into = free_target(parent, &stem(&listing::file_name(archive)), &[]);
    Ok((target, ArchivePayload { root: root.to_path_buf(), op: ArchiveOp::Unpack { into } }))
}

/// `selection` (files and folders, the first of them in library `root`) packed into a new ZIP
/// called `name` in the folder of the first of them; a taken name gets a number.
pub fn plan_pack(root: &Path, selection: &[PathBuf], name: &str) -> Result<(Target, ArchivePayload), Problem> {
    let sources = sources(root, selection)?;
    let base = selection[0].parent().ok_or(Problem::Gone)?.to_path_buf();
    if !base.starts_with(root) {
        return Err(Problem::Outside);
    }
    if crate::fsinfo::is_read_only(&base) {
        return Err(Problem::ReadOnly);
    }
    let name = zip_name(name)?;
    let to = free_target(&base, &name, &[]);
    let paths: Vec<PathBuf> = sources.iter().map(|s| s.path.clone()).collect();
    unique_names(archive::members_of(&paths, &base).map_err(|_| Problem::Gone)?.iter().map(|(_, n)| n.as_str()))?;
    let target = sources[0].clone();
    Ok((target, ArchivePayload { root: root.to_path_buf(), op: ArchiveOp::Pack { sources, base, to } }))
}

/// `selection` (files and folders, from the library or anywhere) added to the ZIP `archive` of
/// library `root` below `prefix` (a folder inside it, or nothing), named relative to the folder
/// of the first of them.
pub fn plan_add(
    root: &Path,
    archive: &Path,
    prefix: &str,
    selection: &[PathBuf],
) -> Result<(Target, ArchivePayload), Problem> {
    let target = source(root, archive, true)?;
    if !archive.is_file() || !is_zip(archive) {
        return Err(Problem::NotZip);
    }
    if crate::fsinfo::is_read_only(archive) {
        return Err(Problem::ReadOnly);
    }
    if selection.iter().any(|s| s == archive || archive.starts_with(s)) {
        return Err(Problem::Nothing);
    }
    let sources = sources(root, selection)?;
    let base = selection[0].parent().ok_or(Problem::Gone)?.to_path_buf();
    let prefix = folder_prefix(prefix)?;
    // Nothing is ever replaced: a name that is there already stops the whole addition, and so
    // do two sources that would get the same name.
    let there = archive::zip_names(archive).map_err(|_| Problem::NotZip)?;
    let paths: Vec<PathBuf> = sources.iter().map(|s| s.path.clone()).collect();
    let names: Vec<String> = archive::members_of(&paths, &base)
        .map_err(|_| Problem::Gone)?
        .into_iter()
        .map(|(_, name)| format!("{prefix}{name}"))
        .collect();
    if let Some(name) = names.iter().find(|n| there.contains(n)) {
        return Err(Problem::Taken(name.clone()));
    }
    unique_names(names.iter().map(String::as_str))?;
    Ok((target, ArchivePayload { root: root.to_path_buf(), op: ArchiveOp::Add { sources, base, prefix } }))
}

/// The folder inside the archive as a member prefix (`minis/`, or nothing for the top): plain
/// folder names between `/`, nothing that could lead out of the archive when another program
/// unpacks it (zip slip), and no backslashes.
fn folder_prefix(folder: &str) -> Result<String, Problem> {
    let folder = folder.trim().trim_matches('/');
    if folder.is_empty() {
        return Ok(String::new());
    }
    if folder.contains('\\') {
        return Err(Problem::BadFolder);
    }
    for part in folder.split('/') {
        if part.is_empty() || part == "." || part == ".." || crate::fsops::check_name(part).is_err() {
            return Err(Problem::BadFolder);
        }
    }
    Ok(format!("{folder}/"))
}

/// Two members may not get the same name (also not differing in case only: Windows would not
/// tell them apart when unpacking).
fn unique_names<'a>(names: impl Iterator<Item = &'a str>) -> Result<(), Problem> {
    let mut seen: Vec<String> = Vec::new();
    for name in names {
        let folded = name.to_lowercase();
        if seen.contains(&folded) {
            return Err(Problem::Taken(name.to_owned()));
        }
        seen.push(folded);
    }
    Ok(())
}

/// The journal steps of an archive operation on `target`.
pub fn to_ops(target: &Target, payload: &ArchivePayload) -> Vec<ApplyOp> {
    let root = payload.root.clone();
    match &payload.op {
        ArchiveOp::Unpack { into } => vec![
            ApplyOp::Extract { root: root.clone(), archive: target.clone(), into: into.clone() },
            ApplyOp::Index { root, dir: into.clone(), seen: false, only: Vec::new() },
        ],
        ArchiveOp::Pack { sources, base, to } => {
            vec![ApplyOp::Pack { root, sources: sources.clone(), base: base.clone(), to: to.clone() }]
        }
        ArchiveOp::Add { sources, base, prefix } => vec![ApplyOp::AddToArchive {
            root,
            archive: target.clone(),
            sources: sources.clone(),
            base: base.clone(),
            prefix: prefix.clone(),
        }],
    }
}

/// Whether `path` is a ZIP by name (`.zip`, any case).
pub fn is_zip(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip"))
}

/// A name for a new ZIP: `name` with `.zip` once, checked like any file name.
fn zip_name(name: &str) -> Result<String, Problem> {
    let name = name.trim();
    let stem = name.strip_suffix(".zip").or_else(|| name.strip_suffix(".ZIP")).unwrap_or(name).trim_end_matches('.');
    if stem.trim().is_empty() {
        return Err(Problem::BadName("empty"));
    }
    let name = format!("{stem}.zip");
    crate::fsops::check_name(&name).map_err(Problem::BadName)?;
    Ok(name)
}

/// A file or folder as it is now, outside any archive; `inside`: it has to lie in `root`.
fn source(root: &Path, path: &Path, inside: bool) -> Result<Target, Problem> {
    if path == root || (inside && !path.starts_with(root)) {
        return Err(Problem::Outside);
    }
    if archive::split(path).is_some() {
        return Err(Problem::InArchive);
    }
    let meta = path.symlink_metadata().map_err(|_| Problem::Gone)?;
    let size = if meta.is_dir() { 0 } else { meta.len() };
    Ok(Target { path: path.to_path_buf(), hash: None, size, mtime_ns: nanos(meta.modified().ok()) })
}

fn sources(root: &Path, selection: &[PathBuf]) -> Result<Vec<Target>, Problem> {
    if selection.is_empty() {
        return Err(Problem::Nothing);
    }
    selection.iter().map(|p| source(root, p, false)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib");
        std::fs::create_dir_all(root.join("Garten").join("sub")).unwrap();
        std::fs::write(root.join("Garten").join("rose.stl"), "rose").unwrap();
        std::fs::write(root.join("Garten").join("sub").join("leaf.stl"), "leaf").unwrap();
        archive::tests::make_zip(&root.join("pack.zip"));
        (dir, root)
    }

    #[test]
    fn unpacking_goes_beside_the_archive_under_a_free_name() {
        let (_dir, root) = library();
        let zip = root.join("pack.zip");
        let (target, payload) = plan_unpack(&root, &zip).unwrap();
        assert_eq!(target.path, zip);
        assert!(target.size > 0);
        assert_eq!(payload.op, ArchiveOp::Unpack { into: root.join("pack") });
        assert_eq!(payload.lands_in(&zip), root);
        std::fs::create_dir(root.join("pack")).unwrap();
        let (_, payload) = plan_unpack(&root, &zip).unwrap();
        assert_eq!(payload.op, ArchiveOp::Unpack { into: root.join("pack (2)") });
        let ops = to_ops(&target, &payload);
        assert!(
            matches!(&ops[..], [ApplyOp::Extract { .. }, ApplyOp::Index { dir, .. }] if dir == &root.join("pack (2)"))
        );

        assert_eq!(plan_unpack(&root, &root.join("Garten").join("rose.stl")).unwrap_err(), Problem::NotArchive);
        assert_eq!(plan_unpack(&root, &root.join("missing.zip")).unwrap_err(), Problem::Gone);
        assert_eq!(plan_unpack(&root, Path::new("/elsewhere/pack.zip")).unwrap_err(), Problem::Outside);
        assert_eq!(plan_unpack(&root, &zip.join("minis").join("orc.stl")).unwrap_err(), Problem::InArchive);
    }

    #[test]
    fn packing_names_the_zip_after_the_wish_and_frees_it() {
        let (_dir, root) = library();
        let garten = root.join("Garten");
        let selection = [garten.join("rose.stl"), garten.join("sub")];
        let (target, payload) = plan_pack(&root, &selection, "Rosen").unwrap();
        assert_eq!(target.path, garten.join("rose.stl"));
        let ArchiveOp::Pack { sources, base, to } = &payload.op else { panic!("a pack") };
        assert_eq!((sources.len(), base, to), (2, &garten, &garten.join("Rosen.zip")));
        assert_eq!(sources[1].size, 0, "a folder");
        assert_eq!(payload.lands_in(&target.path), garten);
        assert!(matches!(&to_ops(&target, &payload)[..], [ApplyOp::Pack { .. }]));

        let (_, payload) = plan_pack(&root, &selection, " rosen.ZIP ").unwrap();
        assert!(matches!(payload.op, ArchiveOp::Pack { to, .. } if to == garten.join("rosen.zip")));
        std::fs::write(garten.join("Rosen.zip"), "x").unwrap();
        let (_, payload) = plan_pack(&root, &selection, "Rosen.zip").unwrap();
        assert!(matches!(payload.op, ArchiveOp::Pack { to, .. } if to == garten.join("Rosen (2).zip")));

        assert_eq!(plan_pack(&root, &selection, "  ").unwrap_err(), Problem::BadName("empty"));
        assert!(matches!(plan_pack(&root, &selection, "a/b").unwrap_err(), Problem::BadName(_)));
        assert_eq!(plan_pack(&root, &[], "x").unwrap_err(), Problem::Nothing);
        assert_eq!(plan_pack(&root, &[root.join("pack.zip").join("readme.txt")], "x").unwrap_err(), Problem::InArchive);
    }

    #[test]
    fn adding_goes_only_into_zips_and_never_over_a_member() {
        let (_dir, root) = library();
        let zip = root.join("pack.zip");
        let garten = root.join("Garten");
        let (target, payload) = plan_add(&root, &zip, "/minis/", &[garten.join("rose.stl")]).unwrap();
        assert_eq!(target.path, zip);
        let ArchiveOp::Add { sources, base, prefix } = &payload.op else { panic!("an add") };
        assert_eq!((sources.len(), base, prefix.as_str()), (1, &garten, "minis/"));
        assert_eq!(payload.lands_in(&zip), root);
        assert!(matches!(&to_ops(&target, &payload)[..], [ApplyOp::AddToArchive { prefix, .. }] if prefix == "minis/"));

        // readme.txt is in there already, at the top.
        std::fs::write(garten.join("readme.txt"), "mine").unwrap();
        assert_eq!(
            plan_add(&root, &zip, "", &[garten.join("readme.txt")]).unwrap_err(),
            Problem::Taken("readme.txt".into())
        );
        assert!(plan_add(&root, &zip, "docs", &[garten.join("readme.txt")]).is_ok(), "elsewhere inside it is fine");
        // The folder inside the archive leads nowhere else.
        for bad in ["../..", "..", "a/../b", "./a", "a//b", "a\\b", "con"] {
            assert_eq!(
                plan_add(&root, &zip, bad, &[garten.join("rose.stl")]).unwrap_err(),
                Problem::BadFolder,
                "{bad}"
            );
        }
        assert!(matches!(plan_add(&root, &zip, " /minis/sub/ ", &[garten.join("rose.stl")]).unwrap().1.op,
            ArchiveOp::Add { prefix, .. } if prefix == "minis/sub/"));
        // Two sources that would get the same name: files of other folders carry their name alone.
        std::fs::create_dir_all(root.join("other")).unwrap();
        std::fs::write(root.join("other").join("Rose.stl"), "r2").unwrap();
        let clash = [garten.join("rose.stl"), root.join("other").join("Rose.stl")];
        assert_eq!(plan_add(&root, &zip, "", &clash).unwrap_err(), Problem::Taken("Rose.stl".into()));
        assert_eq!(plan_pack(&root, &clash, "x").unwrap_err(), Problem::Taken("Rose.stl".into()));
        assert_eq!(plan_add(&root, &zip, "", std::slice::from_ref(&zip)).unwrap_err(), Problem::Nothing);
        assert_eq!(
            plan_add(&root, &zip, "", std::slice::from_ref(&root)).unwrap_err(),
            Problem::Nothing,
            "its own folder"
        );
        std::fs::write(root.join("pack.7z"), "not a zip").unwrap();
        assert_eq!(
            plan_add(&root, &root.join("pack.7z"), "", &[garten.join("rose.stl")]).unwrap_err(),
            Problem::NotZip
        );
        assert_eq!(plan_add(&root, &garten, "", &[garten.join("rose.stl")]).unwrap_err(), Problem::NotZip);
    }
}
