//! Facts about where files live that decide which actions make sense.

use std::path::{Path, PathBuf};

/// The mount containing `path` is mounted read-only (e.g. an NTFS drive without write support).
/// Only Linux tells us cheaply; elsewhere a failing move to the trash reports the problem.
#[cfg(target_os = "linux")]
pub fn is_read_only(path: &Path) -> bool {
    let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    let real = existing_ancestor(path).canonicalize().unwrap_or_else(|_| path.to_path_buf());
    mount_is_read_only(&mounts, &real)
}

#[cfg(not(target_os = "linux"))]
pub fn is_read_only(_path: &Path) -> bool {
    false
}

/// Archive members do not exist on disk; their archive does.
#[cfg(target_os = "linux")]
fn existing_ancestor(path: &Path) -> PathBuf {
    path.ancestors().find(|p| p.exists()).unwrap_or(Path::new("/")).to_path_buf()
}

/// `path` spelled the way the library root it lies in is spelled. A path handed to us from
/// outside (the command line, "Open with" in another program) may name the same file in
/// another case (`c:\…` on Windows, where that is the same file) or through a symlink;
/// compared byte by byte it would not count as part of the library. Paths outside every root
/// and paths that do not exist stay as they are.
pub fn spelled_like_root(path: &Path, roots: &[PathBuf]) -> PathBuf {
    if roots.iter().any(|r| path.starts_with(r)) {
        return path.to_path_buf();
    }
    let Ok(real) = path.canonicalize() else { return path.to_path_buf() };
    roots
        .iter()
        .filter_map(|root| real.strip_prefix(root.canonicalize().ok()?).ok().map(|rest| root.join(rest)))
        .max_by_key(|p| p.as_os_str().len())
        .unwrap_or_else(|| without_verbatim(path, real))
}

/// Outside the library, Windows still gets the file system's own spelling (so the file is
/// found among its folder's entries), without the `\\?\` canonical paths carry there.
#[cfg(windows)]
fn without_verbatim(_path: &Path, real: PathBuf) -> PathBuf {
    let s = real.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.starts_with("UNC\\") => PathBuf::from(format!(r"\\{}", &rest[4..])),
        Some(rest) => PathBuf::from(rest),
        None => real,
    }
}

/// Elsewhere canonical paths resolve symlinks, which the user may well have chosen on purpose.
#[cfg(not(windows))]
fn without_verbatim(path: &Path, _real: PathBuf) -> PathBuf {
    path.to_path_buf()
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn mount_is_read_only(mounts: &str, path: &Path) -> bool {
    mounts
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(' ');
            let mount_point = unescape(fields.nth(1)?);
            let options = fields.nth(1)?;
            Some((PathBuf::from(mount_point), options.split(',').any(|o| o == "ro")))
        })
        .filter(|(mount_point, _)| path.starts_with(mount_point))
        .max_by_key(|(mount_point, _)| mount_point.components().count())
        .is_some_and(|(_, ro)| ro)
}

/// /proc/mounts escapes spaces and friends as octal (`\040`).
pub fn unescape(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&raw[i + 1..i + 4], 8)
        {
            out.push(v);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNTS: &str = "\
/dev/sdd3 / btrfs rw,relatime 0 0
/dev/sdc2 /run/media/maker/1A2B3C4D5E6F7A8B fuseblk ro,noatime,user_id=0 0 0
/dev/sdb1 /run/media/maker/My\\040Drive ext4 rw,relatime 0 0
/dev/sdx /mnt/rofs ext4 rw,errors=remount-ro 0 0";

    #[test]
    fn longest_matching_mount_decides() {
        let ro = |p: &str| mount_is_read_only(MOUNTS, Path::new(p));
        assert!(ro("/run/media/maker/1A2B3C4D5E6F7A8B/prints/a.stl"));
        assert!(!ro("/home/maker/model.stl"));
        assert!(!ro("/run/media/maker/My Drive/x.stl"), "escaped spaces in mount points");
        assert!(!ro("/mnt/rofs/x"), "'remount-ro' is not the 'ro' flag");
        assert!(!ro("/run/media/maker/1A2B3C4D5E6F7A8Bx/y"), "sibling with a longer name");
    }

    #[test]
    fn paths_take_the_spelling_of_their_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Library");
        std::fs::create_dir_all(root.join("Sub")).unwrap();
        let inside = root.join("Sub").join("part.stl");
        std::fs::write(&inside, b"solid").unwrap();
        let roots = vec![root.clone()];
        assert_eq!(spelled_like_root(&inside, &roots), inside, "already spelled like the root");
        let missing = dir.path().join("nowhere").join("x.stl");
        assert_eq!(spelled_like_root(&missing, &roots), missing, "nothing to look up");
        let elsewhere = dir.path().join("other.stl");
        std::fs::write(&elsewhere, b"solid").unwrap();
        let outside = spelled_like_root(&elsewhere, &roots);
        assert!(outside.ends_with("other.stl") && !outside.to_string_lossy().contains("?"), "{outside:?}");
        #[cfg(unix)]
        {
            assert_eq!(outside, elsewhere, "outside the library nothing is resolved");
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&root, &link).unwrap();
            assert_eq!(spelled_like_root(&link.join("Sub").join("part.stl"), &roots), inside, "through a symlink");
        }
        #[cfg(windows)]
        {
            let shouted = PathBuf::from(inside.to_string_lossy().to_uppercase());
            assert_eq!(spelled_like_root(&shouted, &roots), inside, "another case of the same file");
        }
    }
}
