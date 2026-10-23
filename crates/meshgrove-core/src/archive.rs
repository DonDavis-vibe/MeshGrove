//! Model packs in ZIP / 7z / RAR / TAR archives, read through libarchive. Members are addressed
//! by a path that simply continues below the archive file — `/lib/pack.zip/minis/orc.stl` — so
//! navigation, thumbnails, the viewer and the index treat them like ordinary files.

use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::formats::Kind;
use crate::listing::{self, Entry};

/// Members bigger than this are extracted on demand instead of being kept in memory.
const MAX_CACHED_MEMBER: u64 = 512 << 20;
/// Upper bound for one archive's members held in memory.
const MAX_CACHED_TOTAL: u64 = 1536 << 20;

#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    /// Path inside the archive, `/`-separated, e.g. `minis/orc.stl`.
    pub name: String,
    pub size: u64,
}

fn lossy(bytes: &[u8]) -> compress_tools::Result<String> {
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

fn tool_err(e: compress_tools::Error) -> io::Error {
    io::Error::other(e.to_string())
}

/// Files in the archive (folders and macOS/dot-file clutter left out), in archive order.
pub fn list(archive: &Path) -> io::Result<Vec<Member>> {
    let entries = compress_tools::list_archive_entries_with_encoding(File::open(archive)?, lossy).map_err(tool_err)?;
    Ok(entries
        .into_iter()
        .filter(|e| {
            !e.path.ends_with('/') && !e.path.split('/').any(|part| listing::is_hidden(part) || part == "__MACOSX")
        })
        .map(|e| Member { name: e.path, size: e.size })
        .collect())
}

/// Archive members as directory entries (the archive's own mtime stands in for theirs).
pub fn entries(archive: &Path) -> io::Result<Vec<Entry>> {
    let modified = std::fs::metadata(archive)?.modified().ok();
    let mut entries: Vec<Entry> = list(archive)?
        .into_iter()
        .map(|m| Entry {
            kind: Kind::from_file_name(&m.name),
            path: archive.join(&m.name),
            name: m.name,
            size: m.size,
            modified,
        })
        .collect();
    entries.sort_by(|a, b| listing::natural_cmp(&a.name, &b.name));
    Ok(entries)
}

/// `/lib/pack.zip/minis/orc.stl` → (`/lib/pack.zip`, `minis/orc.stl`); `None` for ordinary paths.
pub fn split(path: &Path) -> Option<(PathBuf, String)> {
    if path.exists() {
        return None; // real files and folders are never inside an archive
    }
    let archive = path.ancestors().skip(1).find(|a| a.is_file())?;
    if Kind::from_file_name(&listing::file_name(archive)) != Kind::Archive {
        return None;
    }
    let member = path.strip_prefix(archive).ok()?.to_str()?.to_owned();
    #[cfg(windows)]
    let member = member.replace('\\', "/"); // members are always `/`-separated
    Some((archive.to_path_buf(), member))
}

pub fn is_archive_file(path: &Path) -> bool {
    Kind::from_file_name(&listing::file_name(path)) == Kind::Archive && path.is_file()
}

/// Bytes of a file or of an archive member.
pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    match split(path) {
        Some((archive, member)) => read_member(&archive, &member),
        None => std::fs::read(path),
    }
}

/// Files models refer to that are no models themselves: OBJ materials, glTF buffers.
fn is_model_resource(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("mtl") || ext.eq_ignore_ascii_case("bin"))
}

/// A file a model refers to (`textures/skin.png`, `../shared.mtl`), relative to the model's
/// folder — next to a plain file or inside the same archive. Absolute paths are refused: a
/// model file has no business pointing anywhere else.
pub fn read_sibling(model: &Path, name: &str) -> Option<Vec<u8>> {
    let name = name.trim().replace('\\', "/");
    if name.is_empty() || name.starts_with('/') || name.contains(':') {
        return None;
    }
    let mut path = model.parent()?.to_path_buf();
    for part in name.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if !path.pop() {
                    return None;
                }
            }
            part => path.push(part),
        }
    }
    read(&path).ok()
}

struct Extracted {
    archive: PathBuf,
    modified: Option<SystemTime>,
    members: HashMap<String, Arc<Vec<u8>>>,
}

/// The most recently unpacked archive. Thumbnails of one archive are requested one after the
/// other, and solid 7z/RAR archives can only be decompressed front to back, so unpacking
/// everything once beats unpacking from the start for every member.
static LAST: Mutex<Option<Extracted>> = Mutex::new(None);

pub fn read_member(archive: &Path, member: &str) -> io::Result<Vec<u8>> {
    let modified = std::fs::metadata(archive)?.modified().ok();
    let mut last = LAST.lock().unwrap_or_else(|p| p.into_inner());
    let fresh = last.as_ref().is_some_and(|e| e.archive == archive && e.modified == modified);
    if !fresh {
        *last = Some(Extracted { archive: archive.to_path_buf(), modified, members: unpack_all(archive)? });
    }
    if let Some(bytes) = last.as_ref().and_then(|e| e.members.get(member)) {
        return Ok(bytes.as_ref().clone());
    }
    drop(last);
    // Too big (or not a previewable kind) to keep around: extract just this one.
    let mut out = Vec::new();
    compress_tools::uncompress_archive_file_with_encoding(File::open(archive)?, &mut out, member, lossy)
        .map_err(tool_err)?;
    Ok(out)
}

/// Unpacks all previewable members (models, images, PDFs, G-code) in one pass, within the memory caps.
fn unpack_all(archive: &Path) -> io::Result<HashMap<String, Arc<Vec<u8>>>> {
    let wanted = |name: &str, size: u64| {
        size <= MAX_CACHED_MEMBER
            && (crate::thumbnail::supported(Kind::from_file_name(name)) || is_model_resource(name))
    };
    let mut iter = compress_tools::ArchiveIteratorBuilder::new(File::open(archive)?)
        .decoder(lossy)
        .filter(move |name, stat| wanted(name, stat.st_size as u64))
        .build()
        .map_err(tool_err)?;
    let mut members = HashMap::new();
    let (mut current, mut buffer, mut total) = (None::<String>, Vec::new(), 0u64);
    for item in &mut iter {
        match item {
            compress_tools::ArchiveContents::StartOfEntry(name, _) => {
                current = Some(name);
                buffer = Vec::new();
            }
            compress_tools::ArchiveContents::DataChunk(chunk) => {
                total += chunk.len() as u64;
                if total > MAX_CACHED_TOTAL {
                    break;
                }
                buffer.extend_from_slice(&chunk);
            }
            compress_tools::ArchiveContents::EndOfEntry => {
                if let Some(name) = current.take() {
                    members.insert(name, Arc::new(std::mem::take(&mut buffer)));
                }
            }
            compress_tools::ArchiveContents::Err(e) => return Err(tool_err(e)),
        }
    }
    Ok(members)
}

/// Extracts an archive member into `~/.cache/meshgrove/extracted/…` so other programs
/// (slicer, image viewer) can open it; plain files are returned as they are.
pub fn materialize(path: &Path) -> io::Result<PathBuf> {
    let base = dirs::cache_dir().ok_or_else(|| io::Error::other("no cache directory"))?;
    materialize_in(path, &base.join("meshgrove").join("extracted"))
}

fn materialize_in(path: &Path, base: &Path) -> io::Result<PathBuf> {
    let Some((archive, member)) = split(path) else { return Ok(path.to_path_buf()) };
    let tag = blake3::hash(archive.to_string_lossy().as_bytes()).to_hex()[..16].to_string();
    let target = base.join(tag).join(&member);
    if !target.exists() {
        std::fs::create_dir_all(target.parent().expect("member path has a parent"))?;
        std::fs::write(&target, read_member(&archive, &member)?)?;
    }
    Ok(target)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    /// A zip with a model in a subfolder, a text file and macOS clutter.
    pub fn make_zip(path: &Path) -> Vec<u8> {
        let stl = crate::loaders::stl::write_binary(&crate::Mesh::cube(5.0), "orc");
        let mut w = zip::ZipWriter::new(File::create(path).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in [
            ("minis/orc.stl", stl.as_slice()),
            ("readme.txt", b"hello".as_slice()),
            ("__MACOSX/minis/._orc.stl", b"junk".as_slice()),
        ] {
            w.start_file(name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.add_directory("empty/", opts).unwrap();
        w.finish().unwrap();
        stl
    }

    #[test]
    fn lists_reads_and_resolves_members() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("pack.zip");
        let stl = make_zip(&zip);

        let members = list(&zip).unwrap();
        assert_eq!(members.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["minis/orc.stl", "readme.txt"]);
        assert_eq!(members[0].size, stl.len() as u64);

        let member_path = zip.join("minis/orc.stl");
        assert_eq!(split(&member_path), Some((zip.clone(), "minis/orc.stl".to_owned())));
        assert_eq!(split(&zip), None);
        assert_eq!(read(&member_path).unwrap(), stl);
        assert_eq!(read(&zip.join("readme.txt")).unwrap(), b"hello");
        assert!(read(&zip.join("missing.stl")).is_err());

        let e = entries(&zip).unwrap();
        assert_eq!(e[0].kind, Kind::Stl);
        assert_eq!(e[0].path, member_path);
    }

    #[test]
    fn siblings_next_to_files_and_inside_archives() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("tex")).unwrap();
        std::fs::write(dir.path().join("tex/skin.png"), b"png").unwrap();
        std::fs::write(dir.path().join("a.mtl"), b"mtl").unwrap();
        let model = dir.path().join("models/a.obj");
        assert_eq!(read_sibling(&model, "../tex/skin.png").as_deref(), Some(&b"png"[..]));
        assert_eq!(read_sibling(&dir.path().join("a.obj"), "./a.mtl").as_deref(), Some(&b"mtl"[..]));
        assert_eq!(read_sibling(&dir.path().join("a.obj"), "tex\\skin.png").as_deref(), Some(&b"png"[..]));
        let absolute = dir.path().join("a.mtl").display().to_string();
        assert_eq!(read_sibling(&dir.path().join("a.obj"), &absolute), None, "absolute paths are refused");

        let zip = dir.path().join("pack3.zip");
        make_zip(&zip);
        let inside = zip.join("minis/orc.obj");
        assert_eq!(read_sibling(&inside, "../readme.txt").as_deref(), Some(&b"hello"[..]));
        assert_eq!(
            read_sibling(&inside, "orc.stl").map(|b| b.len()),
            read(&zip.join("minis/orc.stl")).ok().map(|b| b.len())
        );
    }

    #[test]
    fn materialize_extracts_to_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("pack2.zip");
        let stl = make_zip(&zip);
        let out = dir.path().join("extracted");
        let file = materialize_in(&zip.join("minis/orc.stl"), &out).unwrap();
        assert!(file.starts_with(&out));
        assert_eq!(std::fs::read(&file).unwrap(), stl);
        assert_eq!(materialize_in(&zip, &out).unwrap(), zip, "plain files stay where they are");
    }
}
