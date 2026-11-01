//! Staging folders: where a tool call, a panel or its jobs write the files of a bundle (the
//! plugin's `/out`). `<base>/<plugin>/staging/<uid>/out`; it stays after the bundle is applied,
//! since a redo copies from it again.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

#[derive(Debug)]
pub struct Staging {
    uid: String,
    out: PathBuf,
    /// Made below the plugin's staging folder (and removed as a whole), not given.
    managed: bool,
}

impl Staging {
    /// A new, empty staging folder of `plugin` below `base`.
    pub fn new(base: &Path, plugin: &str) -> io::Result<Staging> {
        let uid = new_uid();
        let out = base.join(plugin).join("staging").join(&uid).join("out");
        std::fs::create_dir_all(&out)?;
        Ok(Staging { uid, out, managed: true })
    }

    /// The folder `out` (made if missing) as staging folder, for meshgrove-plugin-run's `--out`.
    pub fn at(out: &Path) -> io::Result<Staging> {
        std::fs::create_dir_all(out)?;
        Ok(Staging { uid: new_uid(), out: out.to_path_buf(), managed: false })
    }

    /// A new staging folder holding what is in `from` now (moved, so `from` is empty after).
    /// For a panel, whose `/out` stays open on `from`.
    pub fn take_from(from: &Path, base: &Path, plugin: &str) -> io::Result<Staging> {
        let staging = Staging::new(base, plugin)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            std::fs::rename(entry.path(), staging.out.join(entry.file_name()))?;
        }
        Ok(staging)
    }

    pub fn uid(&self) -> &str {
        &self.uid
    }

    /// The plugin's `/out`.
    pub fn out(&self) -> &Path {
        &self.out
    }

    /// Whether nothing was written into it.
    pub fn is_empty(&self) -> bool {
        std::fs::read_dir(&self.out).map_or(true, |mut d| d.next().is_none())
    }

    /// Removes the folder (one that was made, not given) with everything in it.
    pub fn remove(self) -> io::Result<()> {
        match self.out.parent() {
            Some(dir) if self.managed => std::fs::remove_dir_all(dir),
            _ => Ok(()),
        }
    }

    /// The staging folders of `plugin` there are, by uid.
    pub fn list(base: &Path, plugin: &str) -> Vec<(String, PathBuf)> {
        let Ok(entries) = std::fs::read_dir(base.join(plugin).join("staging")) else { return Vec::new() };
        let mut out: Vec<(String, PathBuf)> =
            entries.flatten().map(|e| (e.file_name().to_string_lossy().into_owned(), e.path().join("out"))).collect();
        out.sort();
        out
    }
}

/// A short id nobody else uses: blake3 of the time, the process and a counter.
pub(crate) fn new_uid() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let seed = format!("{now}:{}:{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed));
    blake3::hash(seed.as_bytes()).to_hex()[..16].to_owned()
}

/// What [`sweep`] removed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Swept {
    pub staging: usize,
    pub leftovers: usize,
}

/// Cleans the plugins folder `base` up: staging folders nothing refers to any more (`referenced`
/// says whether a change in the journal or a waiting suggestion names a folder), and panel,
/// temporary and loading folders a crash left behind. Only what is older than `age`, so work
/// going on now (a tool call, an open panel) keeps its folders.
pub fn sweep(base: &Path, referenced: &dyn Fn(&Path) -> bool, age: std::time::Duration) -> Swept {
    let old =
        |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|e| e >= age));
    let dirs = |p: PathBuf| -> Vec<PathBuf> {
        std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()
    };
    let mut swept = Swept::default();
    let remove = |p: &Path, count: &mut usize| {
        if std::fs::remove_dir_all(p).is_ok() {
            *count += 1;
        }
    };
    for p in dirs(base.join(".load")).iter().filter(|p| old(p)) {
        remove(p, &mut swept.leftovers);
    }
    for plugin in
        dirs(base.to_path_buf()).iter().filter(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
    {
        for p in dirs(plugin.join("staging")).iter().filter(|p| old(p) && !referenced(p)) {
            remove(p, &mut swept.staging);
        }
        for sub in ["panels", "tmp"] {
            for p in dirs(plugin.join(sub)).iter().filter(|p| old(p)) {
                remove(p, &mut swept.leftovers);
            }
        }
    }
    swept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_folders_come_and_go() {
        let dir = tempfile::tempdir().unwrap();
        let s = Staging::new(dir.path(), "hello-box").unwrap();
        assert!(s.out().is_dir() && s.is_empty());
        assert!(s.out().starts_with(dir.path().join("hello-box").join("staging").join(s.uid())));
        std::fs::write(s.out().join("a.stl"), "a").unwrap();
        let panel_out = dir.path().join("panel");
        std::fs::create_dir_all(panel_out.join("sub")).unwrap();
        std::fs::write(panel_out.join("sub").join("b.stl"), "b").unwrap();
        let taken = Staging::take_from(&panel_out, dir.path(), "hello-box").unwrap();
        assert!(taken.out().join("sub").join("b.stl").is_file() && panel_out.read_dir().unwrap().next().is_none());
        assert_eq!(Staging::list(dir.path(), "hello-box").len(), 2);
        let uid = s.uid().to_owned();
        s.remove().unwrap();
        assert!(!dir.path().join("hello-box").join("staging").join(uid).exists());
        let given = Staging::at(&dir.path().join("given")).unwrap();
        given.remove().unwrap();
        assert!(dir.path().join("given").is_dir(), "a given folder stays");
    }

    #[test]
    fn sweeping_keeps_what_is_referenced_or_new() {
        let base = tempfile::tempdir().unwrap();
        let kept = Staging::new(base.path(), "bingrove").unwrap();
        let gone = Staging::new(base.path(), "bingrove").unwrap();
        let panel = base.path().join("bingrove").join("panels").join("x").join("out");
        let tmp = base.path().join("bingrove").join("tmp").join("y");
        let load = base.path().join(".load").join("z");
        for d in [&panel, &tmp, &load] {
            std::fs::create_dir_all(d).unwrap();
        }
        let named = kept.out().parent().unwrap().to_path_buf();
        let referenced = |p: &Path| p == named;
        // Everything is new: nothing goes.
        assert_eq!(sweep(base.path(), &referenced, std::time::Duration::from_secs(3600)), Swept::default());
        let swept = sweep(base.path(), &referenced, std::time::Duration::ZERO);
        assert_eq!(swept, Swept { staging: 1, leftovers: 3 });
        assert!(kept.out().is_dir() && !gone.out().exists());
        assert!(!panel.exists() && !tmp.exists() && !load.exists());
    }
}
