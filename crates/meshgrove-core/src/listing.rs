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

/// Dotfiles and Windows system folders, which Dolphin hides as well on NTFS drives.
pub fn is_hidden(name: &str) -> bool {
    name.starts_with('.') || name.starts_with('$') || name == "System Volume Information"
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

/// Visible subfolders only, for the sidebar tree.
pub fn subdirs(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter(|e| !is_hidden(&e.file_name().to_string_lossy()))
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let entries = list_dir(p).unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["zeta", "a.stl", "photo.JPG"]);
        assert_eq!(entries[1].kind, Kind::Stl);
        assert_eq!(entries[1].size, 5);
        assert_eq!(entries[2].kind, Kind::Image);
        assert_eq!(subdirs(p).unwrap(), vec![p.join("zeta")]);
    }
}
