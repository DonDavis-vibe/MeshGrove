//! Directory listing in the order a file manager shows it: folders first, natural name order.

use std::cmp::Ordering;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::formats::Kind;

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub kind: Kind,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Folder
    }
}

/// Dotfiles, Windows system folders and Explorer's own leftovers (`desktop.ini`, `Thumbs.db`),
/// which Dolphin hides as well on NTFS drives.
pub fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
        || name.starts_with('$')
        || name == "System Volume Information"
        || name.eq_ignore_ascii_case("desktop.ini")
        || name.eq_ignore_ascii_case("Thumbs.db")
}

/// Marked hidden by Windows, where that is an attribute rather than a name; Explorer leaves
/// such files out.
#[cfg(windows)]
pub fn has_hidden_attribute(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
}

#[cfg(not(windows))]
pub fn has_hidden_attribute(_meta: &std::fs::Metadata) -> bool {
    false
}

pub fn list_dir(dir: &Path) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in std::fs::read_dir(dir)? {
        let Ok(item) = item else { continue };
        let name = item.file_name().to_string_lossy().into_owned();
        if is_hidden(&name) {
            continue;
        }
        let path = item.path();
        // Follow symlinks like a file manager does; skip dangling ones.
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        if has_hidden_attribute(&meta) {
            continue;
        }
        let kind = if meta.is_dir() { Kind::Folder } else { Kind::from_file_name(&name) };
        entries.push(Entry {
            name,
            path,
            kind,
            size: if meta.is_dir() { 0 } else { meta.len() },
            modified: meta.modified().ok(),
        });
    }
    entries.sort_by(|a, b| b.is_dir().cmp(&a.is_dir()).then_with(|| natural_cmp(&a.name, &b.name)));
    Ok(entries)
}

/// Model files (images only when there are no models) in `dir` and up to `depth` levels below it,
/// for a folder's tile: files closer to the top first, at most `limit`. Reads at most a bounded
/// number of folders and entries, so a huge tree stays cheap.
pub fn scan_previews(dir: &Path, depth: u8, limit: usize) -> Vec<Entry> {
    const MAX_DIRS: usize = 120;
    const MAX_ENTRIES: usize = 4000;
    if let Some(found) = archive_previews(dir, depth, limit) {
        return found;
    }
    let (mut models, mut images) = (Vec::new(), Vec::new());
    let mut level = vec![dir.to_path_buf()];
    let (mut dirs_read, mut seen) = (0, 0);
    for _ in 0..depth {
        let mut next = Vec::new();
        for d in &level {
            if dirs_read >= MAX_DIRS || seen >= MAX_ENTRIES || models.len() >= limit {
                break;
            }
            dirs_read += 1;
            let Ok(entries) = list_dir(d) else { continue };
            seen += entries.len();
            for e in entries {
                match e.kind {
                    Kind::Folder => next.push(e.path),
                    k if k.is_model() => models.push(e),
                    Kind::Image => images.push(e),
                    _ => {}
                }
            }
        }
        if models.len() >= limit || next.is_empty() {
            break;
        }
        level = next;
    }
    let mut found = if models.is_empty() { images } else { models };
    found.truncate(limit);
    found
}

/// [`scan_previews`] for an archive, or a folder inside one: its members up to `depth` levels
/// below, the ones closer to the top first. `None` for a folder on disk.
fn archive_previews(dir: &Path, depth: u8, limit: usize) -> Option<Vec<Entry>> {
    let (archive, prefix) = if crate::archive::is_archive_file(dir) {
        (dir.to_path_buf(), String::new())
    } else {
        crate::archive::split(dir)?
    };
    let below = if prefix.is_empty() { String::new() } else { format!("{prefix}/") };
    let mut found: Vec<(usize, Entry)> = crate::archive::entries(&archive)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| {
            let rest = e.name.strip_prefix(&below)?;
            let level = rest.matches('/').count();
            (level < usize::from(depth.max(1))).then_some((level, e))
        })
        .filter(|(_, e)| e.kind.is_model() || e.kind == Kind::Image)
        .collect();
    found.sort_by_key(|(level, _)| *level);
    let models: Vec<Entry> = found.iter().filter(|(_, e)| e.kind.is_model()).map(|(_, e)| e.clone()).collect();
    let mut found = if models.is_empty() { found.into_iter().map(|(_, e)| e).collect() } else { models };
    found.truncate(limit);
    Some(found)
}

/// Visible subfolders only, for the sidebar tree.
pub fn subdirs(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter(|e| !is_hidden(&e.file_name().to_string_lossy()))
        .filter(|e| !cfg!(windows) || !e.metadata().is_ok_and(|m| has_hidden_attribute(&m)))
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort_by(|a, b| natural_cmp(&file_name(a), &file_name(b)));
    Ok(dirs)
}

pub fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Case-insensitive comparison where digit runs compare by value: "part2" < "part10".
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take_num = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (na, nb) = (take_num(&mut a), take_num(&mut b));
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb)).then_with(|| na.len().cmp(&nb.len()));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// The folder's readme, or else its licence: the text that describes what is in it.
pub fn readme_in(dir: &Path) -> io::Result<Option<Entry>> {
    let entries = list_dir(dir)?;
    Ok(entries
        .into_iter()
        .filter(|e| e.kind == crate::Kind::Text)
        .filter_map(|e| Some((readme_rank(&e.name)?, e)))
        .min_by(|(ra, a), (rb, b)| ra.cmp(rb).then(a.name.len().cmp(&b.name.len())).then(a.name.cmp(&b.name)))
        .map(|(_, e)| e))
}

/// Readmes first, then licences; other texts are no folder description.
pub fn readme_rank(name: &str) -> Option<u8> {
    let lower = name.to_lowercase();
    if lower.starts_with("readme") || lower.starts_with("liesmich") {
        Some(0)
    } else if lower.starts_with("license") || lower.starts_with("licence") || lower.starts_with("copying") {
        Some(1)
    } else {
        None
    }
}

/// The start of the folder's readme as text, at most `max_chars` characters.
pub fn readme_excerpt(dir: &Path, max_chars: usize) -> Option<String> {
    let readme = readme_in(dir).ok()??;
    let mut bytes = crate::archive::read(&readme.path).ok()?;
    bytes.truncate(max_chars * 4);
    let text = crate::loaders::text::decode(&bytes);
    Some(text.chars().take(max_chars).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An archive's tile is pictured from its members, from any folder: the model in a subfolder
    /// counts for the archive and for that folder inside it, not for another one.
    #[test]
    fn archive_tiles_are_pictured_from_their_members() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("pack.zip");
        crate::archive::tests::make_zip(&zip);
        let found = scan_previews(&zip, 2, 4);
        assert_eq!(found.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["minis/orc.stl"]);
        assert_eq!(scan_previews(&zip.join("minis"), 1, 4).len(), 1);
        assert!(scan_previews(&zip.join("elsewhere"), 3, 4).is_empty());
        assert!(scan_previews(&zip, 1, 4).is_empty(), "one level deep sees no subfolder");
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["part10.stl", "Part2.stl", "part1.stl", "base.STL", "part02b.stl"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["base.STL", "part1.stl", "Part2.stl", "part02b.stl", "part10.stl"]);
    }

    #[test]
    fn lists_folders_first_and_hides_system_entries() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::create_dir(p.join("zeta")).unwrap();
        std::fs::create_dir(p.join("$RECYCLE.BIN")).unwrap();
        std::fs::write(p.join("a.stl"), b"solid").unwrap();
        std::fs::write(p.join(".hidden"), b"x").unwrap();
        std::fs::write(p.join("photo.JPG"), b"x").unwrap();
        std::fs::write(p.join("Thumbs.db"), b"x").unwrap();
        std::fs::write(p.join("Desktop.ini"), b"x").unwrap();
        let entries = list_dir(p).unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["zeta", "a.stl", "photo.JPG"]);
        assert_eq!(entries[1].kind, Kind::Stl);
        assert_eq!(entries[1].size, 5);
        assert_eq!(entries[2].kind, Kind::Image);
        assert_eq!(subdirs(p).unwrap(), vec![p.join("zeta")]);
    }

    /// Windows marks files hidden with an attribute, whatever their name.
    #[cfg(windows)]
    #[test]
    fn hides_entries_with_the_hidden_attribute() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::write(p.join("shown.stl"), b"solid").unwrap();
        std::fs::write(p.join("hidden.stl"), b"solid").unwrap();
        std::fs::create_dir(p.join("private")).unwrap();
        for name in ["hidden.stl", "private"] {
            let status = std::process::Command::new("attrib").arg("+h").arg(p.join(name)).status().unwrap();
            assert!(status.success());
        }
        let names: Vec<_> = list_dir(p).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["shown.stl"]);
        assert!(subdirs(p).unwrap().is_empty());
    }
}
