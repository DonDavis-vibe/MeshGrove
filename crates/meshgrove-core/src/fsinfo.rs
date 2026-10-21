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
/dev/sdc2 /run/media/davis/366609AE66096FC3 fuseblk ro,noatime,user_id=0 0 0
/dev/sdb1 /run/media/davis/My\\040Drive ext4 rw,relatime 0 0
/dev/sdx /mnt/rofs ext4 rw,errors=remount-ro 0 0";

    #[test]
    fn longest_matching_mount_decides() {
        let ro = |p: &str| mount_is_read_only(MOUNTS, Path::new(p));
        assert!(ro("/run/media/davis/366609AE66096FC3/3D_druck/a.stl"));
        assert!(!ro("/home/davis/model.stl"));
        assert!(!ro("/run/media/davis/My Drive/x.stl"), "escaped spaces in mount points");
        assert!(!ro("/mnt/rofs/x"), "'remount-ro' is not the 'ro' flag");
        assert!(!ro("/run/media/davis/366609AE66096FC3x/y"), "sibling with a longer name");
    }
}
