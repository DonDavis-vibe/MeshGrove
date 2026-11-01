//! Looking for plugins to install: `.wasm` files directly in a few folders (the plugins folder,
//! Downloads, the import folders, where the user last picked one). Each is checked the way an
//! install would check it (compiled, its API version, its manifest and tools); nothing of it
//! runs beyond that, and nothing is installed.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::{Plugin, Settings};

/// Larger `.wasm` files are left alone (a plugin is a few MiB).
pub const MAX_FILE: u64 = 128 << 20;

/// A folder to look in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub path: PathBuf,
    /// Report files that are no usable plugin (where plugins are put on purpose); elsewhere
    /// (Downloads) other `.wasm` files are passed over in silence.
    pub report_problems: bool,
}

/// What a plugin file says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
}

/// A `.wasm` file found: a plugin, or why it is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    /// blake3 of the file, as [`Plugin::hash`] gives it.
    pub hash: String,
    pub plugin: Result<Offer, String>,
}

/// The plugin files directly in `folders` (no subfolders). Files whose content is installed
/// already (`installed`: blake3 hashes) or was found before are left out; plugins come first,
/// by name.
pub fn discover(folders: &[Folder], settings: &Settings, installed: &[String]) -> Vec<Found> {
    let mut seen_paths = HashSet::new();
    let mut seen_hashes: HashSet<String> = installed.iter().map(|h| h.to_ascii_lowercase()).collect();
    let mut found = Vec::new();
    for folder in folders {
        for path in wasm_files(&folder.path) {
            if !seen_paths.insert(path.canonicalize().unwrap_or_else(|_| path.clone())) {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let hash = blake3::hash(&bytes).to_hex().to_string();
            if !seen_hashes.insert(hash.clone()) {
                continue;
            }
            let plugin = Plugin::from_bytes(&bytes, settings.clone()).map(|p| {
                let m = p.manifest();
                Offer {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    version: m.version.clone(),
                    description: m.description.clone(),
                }
            });
            match plugin {
                Ok(offer) => found.push(Found { path, hash, plugin: Ok(offer) }),
                Err(e) if folder.report_problems => found.push(Found { path, hash, plugin: Err(e.to_string()) }),
                Err(_) => {}
            }
        }
    }
    found.sort_by(|a, b| match (&a.plugin, &b.plugin) {
        (Ok(x), Ok(y)) => x.name.to_lowercase().cmp(&y.name.to_lowercase()).then_with(|| a.path.cmp(&b.path)),
        (Ok(_), Err(_)) => std::cmp::Ordering::Less,
        (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
        (Err(_), Err(_)) => a.path.cmp(&b.path),
    });
    found
}

/// The `.wasm` files directly in `dir` (links to files count), by name.
fn wasm_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("wasm")))
        .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() <= MAX_FILE))
        .collect();
    files.sort();
    files
}

/// Whether version `a` is older than `b`: dotted numbers compared as numbers, anything after a
/// `-` or `+` ignored; versions that are no such numbers are never older.
pub fn older(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Option<Vec<u64>> {
        let core = v.trim().split(['-', '+']).next().unwrap_or_default();
        core.split('.').map(|p| p.parse().ok()).collect()
    };
    match (parts(a), parts(b)) {
        (Some(mut a), Some(mut b)) => {
            let len = a.len().max(b.len());
            a.resize(len, 0);
            b.resize(len, 0);
            a < b
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_wasm_files_directly_inside_and_not_too_big() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.wasm"), b"x").unwrap();
        std::fs::write(dir.path().join("B.WASM"), b"x").unwrap();
        std::fs::write(dir.path().join("readme.txt"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("sub.wasm")).unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("deep.wasm"), b"x").unwrap();
        let names: Vec<String> =
            wasm_files(dir.path()).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["B.WASM", "a.wasm"]);
        assert!(wasm_files(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn non_plugins_are_reported_only_where_plugins_are_put() {
        let base = tempfile::tempdir().unwrap();
        let (put, downloads) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(put.path().join("broken.wasm"), b"no component").unwrap();
        std::fs::write(downloads.path().join("game.wasm"), b"something else").unwrap();
        // The same bytes twice: one report.
        std::fs::write(downloads.path().join("broken copy.wasm"), b"no component").unwrap();
        let folders = [
            Folder { path: put.path().to_path_buf(), report_problems: true },
            Folder { path: downloads.path().to_path_buf(), report_problems: false },
        ];
        let found = discover(&folders, &Settings::new(base.path()), &[]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, put.path().join("broken.wasm"));
        assert!(found[0].plugin.is_err());
        // Installed content is no news.
        let hash = blake3::hash(b"no component").to_hex().to_string();
        assert!(discover(&folders, &Settings::new(base.path()), &[hash.to_uppercase()]).is_empty());
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert!(older("0.9.0", "0.10.0"));
        assert!(older("1.2", "1.2.1"));
        assert!(older("1.2.0-beta", "1.3.0"));
        assert!(!older("1.2.0", "1.2"));
        assert!(!older("2.0.0", "1.9.9"));
        assert!(!older("dev", "1.0.0"));
    }
}
