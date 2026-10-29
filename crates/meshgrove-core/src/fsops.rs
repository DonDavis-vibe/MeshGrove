//! Moving and renaming on disk without ever replacing anything: the target must not exist, or
//! the move fails. Across file systems (a downloads folder on another drive than the library)
//! a file is copied and the copy checked before the source goes. Names are checked against
//! what Windows and FAT accept, so a library on a shared drive stays usable everywhere.

use std::io;
use std::path::{Path, PathBuf};

/// Moves `from` to `to` (a file or a folder with everything in it) if nothing is at `to`. A
/// change of case only ("Rose.STL" → "rose.stl") works on case-insensitive file systems too.
pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    if from == to {
        return Ok(());
    }
    if case_only(from, to) && to.symlink_metadata().is_ok() {
        // The same entry under another case: through a free name in between.
        let between = free_name(from)?;
        imp(from, &between)?;
        return imp(&between, to).inspect_err(|_| {
            let _ = imp(&between, from);
        });
    }
    imp(from, to)
}

/// [`rename_no_replace`], and across file systems too: there the file is copied to a hidden
/// name next to the target, synced, given the source's modification time, checked for size,
/// renamed into place (replacing nothing) and the source removed; a folder goes member by
/// member. Links are refused.
pub fn move_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    match rename_no_replace(from, to) {
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => copy_then_remove(from, to),
        other => other,
    }
}

/// The copy half of [`move_no_replace`], on its own so it can be tested where everything is
/// one file system. Either the whole file arrives at `to` and `from` goes, or `from` stays as
/// it was and nothing is left at `to`.
pub(crate) fn copy_then_remove(from: &Path, to: &Path) -> io::Result<()> {
    let meta = from.symlink_metadata()?;
    if meta.is_symlink() {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "a link is not moved across file systems"));
    }
    if to.symlink_metadata().is_ok() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "the target exists"));
    }
    if meta.is_dir() {
        std::fs::create_dir(to)?;
        let mut members: Vec<_> = std::fs::read_dir(from)?.collect::<io::Result<_>>()?;
        members.sort_by_key(|e| e.file_name());
        for member in members {
            copy_then_remove(&member.path(), &to.join(member.file_name()))?;
        }
        return std::fs::remove_dir(from);
    }
    copy_file(from, to, &meta)?;
    std::fs::remove_file(from).inspect_err(|_| {
        let _ = std::fs::remove_file(to);
    })
}

/// A copy of a file or folder at `to`, which must not exist: the file is copied to a hidden name
/// next to the target, synced, given the source's modification time, checked for size and
/// renamed into place; a folder goes member by member. Links are refused. Should it fail, the
/// source is as it was and nothing is left at `to`.
pub fn copy_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    let meta = from.symlink_metadata()?;
    if meta.is_symlink() {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "a link is not copied"));
    }
    if to.symlink_metadata().is_ok() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "the target exists"));
    }
    if !meta.is_dir() {
        return copy_file(from, to, &meta);
    }
    std::fs::create_dir(to)?;
    let copied = (|| {
        let mut members: Vec<_> = std::fs::read_dir(from)?.collect::<io::Result<_>>()?;
        members.sort_by_key(|e| e.file_name());
        for member in members {
            copy_no_replace(&member.path(), &to.join(member.file_name()))?;
        }
        Ok(())
    })();
    copied.inspect_err(|_| {
        let _ = std::fs::remove_dir_all(to);
    })
}

/// `from` (a plain file with `meta`) copied to `to` by way of a hidden name next to it, so a
/// half-written file never sits under the real name.
fn copy_file(from: &Path, to: &Path, meta: &std::fs::Metadata) -> io::Result<()> {
    let tmp = free_name(to)?;
    let copied = (|| {
        std::fs::copy(from, &tmp)?;
        let file = std::fs::File::options().write(true).open(&tmp)?;
        file.set_modified(meta.modified()?)?;
        file.sync_all()?;
        if file.metadata()?.len() != meta.len() {
            return Err(io::Error::other("the copy is incomplete"));
        }
        drop(file);
        rename_no_replace(&tmp, to)
    })();
    copied.inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// The same name in the same folder, but for upper and lower case.
pub(crate) fn case_only(from: &Path, to: &Path) -> bool {
    from.parent() == to.parent()
        && from.file_name().map(|n| n.to_string_lossy().to_lowercase())
            == to.file_name().map(|n| n.to_string_lossy().to_lowercase())
}

/// A name next to `path` that is not taken.
fn free_name(path: &Path) -> io::Result<PathBuf> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    for i in 0..100 {
        let candidate = path.with_file_name(format!(".{name}.meshgrove-{}-{i}", std::process::id()));
        if candidate.symlink_metadata().is_err() {
            return Ok(candidate);
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free name for renaming"))
}

#[cfg(target_os = "linux")]
fn imp(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let from_c = std::ffi::CString::new(from.as_os_str().as_bytes())?;
    let to_c = std::ffi::CString::new(to.as_os_str().as_bytes())?;
    // The system call itself: the libc wrapper needs a newer glibc than the release targets.
    // SAFETY: both paths are valid NUL-terminated strings for the duration of the call.
    let r = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            from_c.as_ptr(),
            libc::AT_FDCWD,
            to_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if r == 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    match e.raw_os_error() {
        // File systems (some FUSE ones) or kernels without it.
        Some(libc::EINVAL | libc::ENOSYS | libc::EOPNOTSUPP) => fallback(from, to),
        _ => Err(e),
    }
}

#[cfg(target_os = "macos")]
fn imp(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let from_c = std::ffi::CString::new(from.as_os_str().as_bytes())?;
    let to_c = std::ffi::CString::new(to.as_os_str().as_bytes())?;
    // SAFETY: both paths are valid NUL-terminated strings for the duration of the call.
    let r = unsafe { libc::renamex_np(from_c.as_ptr(), to_c.as_ptr(), libc::RENAME_EXCL) };
    if r == 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    match e.raw_os_error() {
        Some(libc::ENOTSUP | libc::EINVAL) => fallback(from, to),
        _ => Err(e),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn imp(from: &Path, to: &Path) -> io::Result<()> {
    fallback(from, to)
}

/// Without a no-replace rename: a file through a hard link (which fails when the name is
/// taken), otherwise a check right before renaming.
fn fallback(from: &Path, to: &Path) -> io::Result<()> {
    if from.symlink_metadata()?.is_file() {
        match std::fs::hard_link(from, to) {
            Ok(()) => {
                return std::fs::remove_file(from).inspect_err(|_| {
                    let _ = std::fs::remove_file(to);
                });
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(e),
            Err(_) => {} // no hard links on this file system
        }
    }
    if to.symlink_metadata().is_ok() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "the target exists"));
    }
    std::fs::rename(from, to)
}

/// Names Windows keeps for devices, with or without an extension.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
    "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Whether `name` works as a file or folder name on every system Meshgrove runs on (NTFS and
/// FAT included); the reason when not, for the AI that suggested it.
pub fn check_name(name: &str) -> Result<(), &'static str> {
    if name.trim().is_empty() || name == "." || name == ".." {
        return Err("the name is empty");
    }
    if name.len() > 255 {
        return Err("the name is longer than 255 bytes");
    }
    if name.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')) {
        return Err("the name holds a character Windows does not allow (/ \\ : * ? \" < > | or a control character)");
    }
    if name.ends_with(' ') || name.ends_with('.') {
        return Err("the name ends with a space or a dot, which Windows does not allow");
    }
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        return Err("the name is reserved on Windows (CON, NUL, COM1 …)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_ever_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.stl"), dir.path().join("b.stl"));
        std::fs::write(&a, "A").unwrap();
        std::fs::write(&b, "B").unwrap();
        let e = rename_no_replace(&a, &b).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "B", "the target is untouched");
        let c = dir.path().join("sub").join("c.stl");
        std::fs::create_dir(c.parent().unwrap()).unwrap();
        rename_no_replace(&a, &c).unwrap();
        assert!(!a.exists() && std::fs::read_to_string(&c).unwrap() == "A");
        // Folders, with what is in them.
        let moved = dir.path().join("neu");
        rename_no_replace(&dir.path().join("sub"), &moved).unwrap();
        assert!(moved.join("c.stl").is_file());
        assert!(rename_no_replace(&moved, &b).is_err(), "not onto a file either");
        assert!(fallback(&b, &moved.join("c.stl")).is_err(), "the fallback does not replace");
    }

    #[test]
    fn the_copy_fallback_keeps_bytes_and_time_and_replaces_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("rose.stl"), dir.path().join("lib").join("rose.stl"));
        std::fs::create_dir(to.parent().unwrap()).unwrap();
        std::fs::write(&from, "solid rose").unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        std::fs::File::options().write(true).open(&from).unwrap().set_modified(old).unwrap();
        copy_then_remove(&from, &to).unwrap();
        assert!(!from.exists(), "the source went");
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "solid rose");
        assert_eq!(std::fs::metadata(&to).unwrap().modified().unwrap(), old, "the time came along");
        let leftovers: Vec<_> =
            std::fs::read_dir(to.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(leftovers.len(), 1, "no temporary file stays: {leftovers:?}");
        // Onto something: refused, both untouched.
        std::fs::write(&from, "another").unwrap();
        let e = copy_then_remove(&from, &to).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert!(from.exists() && std::fs::read_to_string(&to).unwrap() == "solid rose");
        // A folder with a subfolder, member by member.
        let pack = dir.path().join("pack");
        std::fs::create_dir_all(pack.join("sub")).unwrap();
        std::fs::write(pack.join("a.stl"), "a").unwrap();
        std::fs::write(pack.join("sub").join("b.stl"), "b").unwrap();
        let moved = dir.path().join("lib").join("pack");
        copy_then_remove(&pack, &moved).unwrap();
        assert!(!pack.exists() && moved.join("sub").join("b.stl").is_file() && moved.join("a.stl").is_file());
        assert!(move_no_replace(&to, &moved).is_err(), "move_no_replace replaces nothing either");
    }

    #[test]
    fn copies_keep_bytes_and_time_and_replace_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("rose.stl"), dir.path().join("lib").join("rose.stl"));
        std::fs::create_dir(to.parent().unwrap()).unwrap();
        std::fs::write(&from, "solid rose").unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        std::fs::File::options().write(true).open(&from).unwrap().set_modified(old).unwrap();
        copy_no_replace(&from, &to).unwrap();
        assert!(from.is_file(), "the source stays");
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "solid rose");
        assert_eq!(std::fs::metadata(&to).unwrap().modified().unwrap(), old, "the time came along");
        let leftovers: Vec<_> =
            std::fs::read_dir(to.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(leftovers.len(), 1, "no temporary file stays: {leftovers:?}");
        let e = copy_no_replace(&from, &to).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists, "onto something: refused");
        // A folder with a subfolder, member by member.
        let pack = dir.path().join("pack");
        std::fs::create_dir_all(pack.join("sub")).unwrap();
        std::fs::write(pack.join("a.stl"), "a").unwrap();
        std::fs::write(pack.join("sub").join("b.stl"), "b").unwrap();
        let copied = dir.path().join("lib").join("pack");
        copy_no_replace(&pack, &copied).unwrap();
        assert!(pack.join("a.stl").is_file() && copied.join("sub").join("b.stl").is_file());
        #[cfg(unix)]
        {
            let link = dir.path().join("link.stl");
            std::os::unix::fs::symlink(&from, &link).unwrap();
            assert!(copy_no_replace(&link, &dir.path().join("linked.stl")).is_err(), "links are refused");
        }
    }

    #[test]
    fn the_case_of_a_name_can_change() {
        let dir = tempfile::tempdir().unwrap();
        let (upper, lower) = (dir.path().join("Rose.STL"), dir.path().join("rose.stl"));
        std::fs::write(&upper, "x").unwrap();
        rename_no_replace(&upper, &lower).unwrap();
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["rose.stl"]);
    }

    #[test]
    fn names_that_work_everywhere() {
        for ok in ["rose.stl", "Kunde Müller", "Teil (2).3mf", ".hidden", "CONSOLE.stl"] {
            assert_eq!(check_name(ok), Ok(()), "{ok}");
        }
        for bad in
            ["", " ", "..", "a/b", "a\\b", "c:d", "was?.stl", "x.", "x ", "NUL", "com1.stl", "a\nb", &"x".repeat(256)]
        {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
    }
}
