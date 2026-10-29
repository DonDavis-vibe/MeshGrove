//! Bringing new 3D-print files from outside the library (a downloads folder) in: what is there
//! and worth importing, what each file would become, and the journal steps that do it.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rusqlite::params;

use crate::ai::proposals::Target;
use crate::archive;
use crate::cache::ThumbCache;
use crate::changes::ApplyOp;
use crate::config::ArchiveImport;
use crate::db::{Error, Library, nanos};
use crate::formats::Kind;
use crate::listing::{self, Entry};

/// A file changed more recently than this may still be written by the browser.
const SETTLE: Duration = Duration::from_secs(60);
/// Files up to this size are hashed to find copies of them in the library.
const HASH_LIMIT: u64 = 64 << 20;
/// What browsers and download managers add to a file's name while it is coming in.
const PARTIAL: &[&str] = &[".part", ".crdownload", ".download", ".tmp"];
/// Archive extensions in two parts.
const DOUBLE_EXTENSIONS: &[&str] = &[".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst"];

/// A file in an import folder that could come into the library.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub entry: Entry,
    /// For archives: how many members, and how many of them are models.
    pub archive: Option<(usize, usize)>,
    pub content_hash: Option<String>,
    /// A library file with the same content (for archives: the same name and size).
    pub in_library: Option<PathBuf>,
    /// A file of this name is in the landing folder already.
    pub name_taken: bool,
    /// Still being written, by the look of it.
    pub busy: bool,
}

impl Candidate {
    /// Worth importing: not in the library yet, not still being written.
    pub fn importable(&self) -> bool {
        self.in_library.is_none() && !self.busy
    }
}

/// What one import folder holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Scan {
    pub folder: PathBuf,
    pub candidates: Vec<Candidate>,
}

/// Looks at the top level of `folder` for models, G-code and archives with models in them.
/// `landing` is where they would go (to see whose name is taken), `lib` and `cache` tell
/// which of them the library has already.
pub fn scan(
    folder: &Path,
    landing: &Path,
    lib: Option<&Library>,
    cache: Option<&ThumbCache>,
    now: SystemTime,
) -> std::io::Result<Scan> {
    let entries = listing::list_dir(folder)?;
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    let mut candidates = Vec::new();
    for e in &entries {
        // Links are not moved (the file they point at stays where it is).
        if e.is_dir() || e.path.symlink_metadata().is_ok_and(|m| m.is_symlink()) {
            continue;
        }
        let archive = match e.kind {
            Kind::Archive => match archive::list(&e.path) {
                Ok(members) => {
                    let models = members.iter().filter(|m| Kind::from_file_name(&m.name).is_model()).count();
                    if models == 0 {
                        continue;
                    }
                    Some((members.len(), models))
                }
                Err(_) => continue,
            },
            kind if kind.is_printable() => None,
            _ => continue,
        };
        let fresh = e.modified.is_some_and(|m| now.duration_since(m).is_ok_and(|age| age < SETTLE));
        let partial =
            PARTIAL.iter().any(|ext| names.iter().any(|n| n.len() > e.name.len() && *n == format!("{}{ext}", e.name)));
        let content_hash = if archive.is_some() { None } else { hash_of(e, cache) };
        let in_library = match (lib, &content_hash) {
            (Some(lib), Some(hash)) => lib.paths_with_hash(hash).ok().and_then(|p| p.into_iter().next()),
            (Some(lib), None) if archive.is_some() => lib.file_like(&e.name, e.size).ok().flatten(),
            _ => None,
        };
        candidates.push(Candidate {
            entry: e.clone(),
            archive,
            content_hash,
            in_library,
            name_taken: landing.join(&e.name).symlink_metadata().is_ok(),
            busy: fresh || partial,
        });
    }
    Ok(Scan { folder: folder.to_path_buf(), candidates })
}

/// The content of a file: from its cached thumbnail where the folder was browsed, else hashed.
fn hash_of(e: &Entry, cache: Option<&ThumbCache>) -> Option<String> {
    let key = ThumbCache::key(&e.path, e.size, e.modified, None);
    if let Some(Ok(thumb)) = cache.and_then(|c| c.load(&key))
        && let Some(hash) = thumb.meta.content_hash
    {
        return Some(hash);
    }
    if e.size > HASH_LIMIT {
        return None;
    }
    std::fs::read(&e.path).ok().map(|bytes| crate::dedupe::content_hash(&bytes))
}

impl Library {
    /// A file of this name and size in the library (archives are not hashed).
    fn file_like(&self, name: &str, size: u64) -> Result<Option<PathBuf>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT d.path, f.name FROM files f JOIN dirs d ON d.id = f.dir_id
             WHERE f.name = ?1 COLLATE NOCASE AND f.size = ?2 ORDER BY d.path LIMIT 1",
        )?;
        let found = stmt
            .query_map(params![name, size as i64], |r| {
                Ok(Path::new(&r.get::<_, String>(0)?).join(r.get::<_, String>(1)?))
            })?
            .next()
            .transpose()?;
        Ok(found)
    }
}

/// What the import does with one candidate.
#[derive(Debug, Clone, PartialEq)]
pub enum PlanItem {
    Move { from: Target, to: PathBuf },
    Extract { archive: Target, into: PathBuf },
}

/// An import as it will be done.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub root: PathBuf,
    pub landing: PathBuf,
    pub items: Vec<PlanItem>,
    /// Archives to put in the trash once they are unpacked.
    pub trash: Vec<PathBuf>,
}

/// The candidates `picked` (by index) of `scan` into `landing` below `root`; names already
/// taken get a number. (A file the library has already is not a candidate to import.)
pub fn plan(scan: &Scan, root: &Path, landing: &Path, archives: ArchiveImport, picked: &[usize]) -> Plan {
    let mut plan =
        Plan { root: root.to_path_buf(), landing: landing.to_path_buf(), items: Vec::new(), trash: Vec::new() };
    let mut used: Vec<PathBuf> = Vec::new();
    for &i in picked {
        let Some(c) = scan.candidates.get(i) else { continue };
        if !c.importable() {
            continue;
        }
        let e = &c.entry;
        let target =
            Target { path: e.path.clone(), hash: c.content_hash.clone(), size: e.size, mtime_ns: nanos(e.modified) };
        if c.archive.is_some() && archives == ArchiveImport::Extract {
            let into = free_target(landing, &stem(&e.name), &used);
            used.push(into.clone());
            plan.trash.push(e.path.clone());
            plan.items.push(PlanItem::Extract { archive: target, into });
            continue;
        }
        let to = free_target(landing, &e.name, &used);
        used.push(to.clone());
        plan.items.push(PlanItem::Move { from: target, to });
    }
    plan
}

/// The journal steps of a plan.
pub fn to_ops(plan: &Plan) -> Vec<ApplyOp> {
    plan.items
        .iter()
        .map(|item| match item {
            PlanItem::Move { from, to } => {
                ApplyOp::Import { root: plan.root.clone(), from: from.clone(), to: to.clone(), folder: false }
            }
            PlanItem::Extract { archive, into } => {
                ApplyOp::Extract { root: plan.root.clone(), archive: archive.clone(), into: into.clone() }
            }
        })
        .collect()
}

/// `name` split into its stem and extension, "pack.tar.gz" and "rose (2).stl" included.
fn split_name(name: &str) -> (&str, &str) {
    let lower = name.to_ascii_lowercase();
    if let Some(ext) = DOUBLE_EXTENSIONS.iter().find(|ext| lower.ends_with(*ext) && name.len() > ext.len()) {
        return name.split_at(name.len() - ext.len());
    }
    match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    }
}

/// `name` without its extension.
pub fn stem(name: &str) -> String {
    split_name(name).0.to_owned()
}

/// `dir/name`, or `dir/name (2)` (3, 4 …) when that is taken on disk or by `used`.
pub fn free_target(dir: &Path, name: &str, used: &[PathBuf]) -> PathBuf {
    let taken = |p: &Path| p.symlink_metadata().is_ok() || used.iter().any(|u| u == p);
    let first = dir.join(name);
    if !taken(&first) {
        return first;
    }
    let (stem, ext) = split_name(name);
    (2..).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|p| !taken(p)).expect("some number is free")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::ThumbMeta;

    fn backdate(path: &Path) {
        let old = SystemTime::now() - Duration::from_secs(600);
        std::fs::File::options().write(true).open(path).unwrap().set_modified(old).unwrap();
    }

    fn entry(dir: &Path, name: &str) -> Entry {
        let meta = dir.join(name).metadata().unwrap();
        Entry {
            name: name.into(),
            path: dir.join(name),
            kind: Kind::from_file_name(name),
            size: meta.len(),
            modified: meta.modified().ok(),
        }
    }

    /// Downloads with one of everything; a library whose `Import` holds a copy of `old.gcode`
    /// (by name only) and knows the content of `a.stl` under another name.
    fn scene() -> (tempfile::TempDir, PathBuf, PathBuf, Library) {
        let dir = tempfile::tempdir().unwrap();
        let downloads = dir.path().join("Downloads");
        let root = dir.path().join("lib");
        let landing = root.join("Import");
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::create_dir_all(&landing).unwrap();
        for (name, content) in
            [("a.stl", "abc"), ("old.gcode", ";gcode"), ("notes.txt", "hi"), ("x.stl", "x"), ("x.stl.part", "")]
        {
            std::fs::write(downloads.join(name), content).unwrap();
            backdate(&downloads.join(name));
        }
        std::fs::write(downloads.join("busy.stl"), "fresh").unwrap();
        crate::archive::tests::make_zip(&downloads.join("pack.zip"));
        backdate(&downloads.join("pack.zip"));
        let mut w = zip::ZipWriter::new(std::fs::File::create(downloads.join("empty.zip")).unwrap());
        w.start_file("readme.txt", zip::write::SimpleFileOptions::default()).unwrap();
        std::io::Write::write_all(&mut w, b"only text").unwrap();
        w.finish().unwrap();
        backdate(&downloads.join("empty.zip"));
        std::fs::write(landing.join("old.gcode"), ";other gcode").unwrap();
        std::fs::write(root.join("known.stl"), "abc").unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        lib.finish_baseline(&root).unwrap();
        lib.update_dir(&root, &root, 1, &[entry(&root, "known.stl")], std::slice::from_ref(&landing)).unwrap();
        lib.update_dir(&root, &landing, 1, &[entry(&landing, "old.gcode")], &[]).unwrap();
        let abc = crate::dedupe::content_hash(b"abc");
        lib.store_meta(&root.join("known.stl"), "k", &ThumbMeta { content_hash: Some(abc), ..Default::default() })
            .unwrap();
        let other = crate::dedupe::content_hash(b";other gcode");
        lib.store_meta(&landing.join("old.gcode"), "k", &ThumbMeta { content_hash: Some(other), ..Default::default() })
            .unwrap();
        (dir, downloads, root, lib)
    }

    #[test]
    fn candidates_in_a_downloads_folder() {
        let (_dir, downloads, root, lib) = scene();
        let landing = root.join("Import");
        let scan = scan(&downloads, &landing, Some(&lib), None, SystemTime::now()).unwrap();
        let names: Vec<&str> = scan.candidates.iter().map(|c| c.entry.name.as_str()).collect();
        assert_eq!(
            names,
            ["a.stl", "busy.stl", "old.gcode", "pack.zip", "x.stl"],
            "no text, no archive without a model"
        );
        let by = |name: &str| scan.candidates.iter().find(|c| c.entry.name == name).unwrap();
        assert_eq!(by("a.stl").in_library.as_deref(), Some(root.join("known.stl").as_path()), "known by content");
        assert!(by("busy.stl").busy && by("x.stl").busy, "fresh, or with a .part next to it");
        assert!(by("old.gcode").name_taken && by("old.gcode").in_library.is_none(), "same name, other content");
        assert_eq!(by("pack.zip").archive, Some((2, 1)));
        assert_eq!(scan.candidates.iter().filter(|c| c.importable()).count(), 2, "old.gcode and pack.zip");
    }

    #[test]
    fn names_get_a_number_and_duplicates_are_reported() {
        let (_dir, downloads, root, lib) = scene();
        let landing = root.join("Import");
        let scan = scan(&downloads, &landing, Some(&lib), None, SystemTime::now()).unwrap();
        let all: Vec<usize> = (0..scan.candidates.len()).collect();
        let plan = plan(&scan, &root, &landing, ArchiveImport::Whole, &all);
        let moves: Vec<(String, String)> = plan
            .items
            .iter()
            .filter_map(|i| match i {
                PlanItem::Move { from, to } => Some((
                    from.path.file_name().unwrap().to_string_lossy().into_owned(),
                    to.strip_prefix(&landing).unwrap().to_string_lossy().into_owned(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(
            moves,
            [("old.gcode".to_owned(), "old (2).gcode".to_owned()), ("pack.zip".to_owned(), "pack.zip".to_owned())]
        );
        assert!(plan.trash.is_empty());
        assert_eq!(to_ops(&plan).len(), 2);

        // Unpacking instead; a file the landing folder holds with the same content is known.
        std::fs::write(landing.join("x.stl"), "x").unwrap();
        std::fs::remove_file(downloads.join("x.stl.part")).unwrap();
        backdate(&downloads.join("x.stl"));
        let mut lib = lib;
        lib.update_dir(&root, &landing, 2, &[entry(&landing, "old.gcode"), entry(&landing, "x.stl")], &[]).unwrap();
        let x = crate::dedupe::content_hash(b"x");
        lib.store_meta(&landing.join("x.stl"), "k", &ThumbMeta { content_hash: Some(x), ..Default::default() })
            .unwrap();
        let scan = super::scan(&downloads, &landing, Some(&lib), None, SystemTime::now()).unwrap();
        let all: Vec<usize> = (0..scan.candidates.len()).collect();
        let x = scan.candidates.iter().find(|c| c.entry.name == "x.stl").unwrap();
        assert_eq!(x.in_library.as_deref(), Some(landing.join("x.stl").as_path()), "a copy, not imported");
        let plan = super::plan(&scan, &root, &landing, ArchiveImport::Extract, &all);
        assert!(
            plan.items.iter().any(|i| matches!(i, PlanItem::Extract { into, .. } if into == &landing.join("pack")))
        );
        assert_eq!(plan.trash, [downloads.join("pack.zip")]);
        assert_eq!(plan.items.len(), 2, "old.gcode and the archive; x.stl stays out");

        assert_eq!(
            free_target(&landing, "pack.tar.gz", &[landing.join("pack.tar.gz")]),
            landing.join("pack (2).tar.gz")
        );
        assert_eq!(stem("pack.tar.gz"), "pack");
        assert_eq!(free_target(&landing, "old.gcode", &[landing.join("old (2).gcode")]), landing.join("old (3).gcode"));
    }
}
