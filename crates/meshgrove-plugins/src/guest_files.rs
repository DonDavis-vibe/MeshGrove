//! Files a plugin names in its own folders (`/tmp/…`, `/data/…`), found on the host: for the
//! host functions that take such a path beside library ids.
//!
//! The plugin can only name what it could read itself. A path with `..` is refused, and so is
//! one that leads through a link: a plugin may make links in its folders, and one pointing out
//! of them must not make the host read for it what the sandbox keeps it from.

use std::path::{Path, PathBuf};

use meshgrove_core::{Kind, Mesh, loaders};

/// Where the folders of an instance are on this computer.
#[derive(Debug, Clone)]
pub(crate) struct GuestDirs {
    /// `/data`
    pub data: PathBuf,
    /// `/tmp`; a tool call's is gone once it has answered.
    pub tmp: Option<PathBuf>,
}

/// Whether `reference` is meant as a path in the plugin's folders (library ids never start
/// with a slash).
pub(crate) fn is_guest_path(reference: &str) -> bool {
    reference.starts_with('/')
}

/// The parts of a `/`-separated path below a folder, if it is a plain one: no `..`, no empty
/// parts, nothing a system reads as more than a name.
pub(crate) fn plain_parts(path: &str) -> Option<Vec<&str>> {
    let parts: Vec<&str> = path.split('/').collect();
    let plain = |p: &&str| !p.is_empty() && *p != "." && *p != ".." && !p.contains(['\\', '\0', ':']);
    parts.iter().all(plain).then_some(parts)
}

/// Opens the file `parts` lead to below `base`: a plain file reached through plain folders.
/// `shown` is the path as the plugin wrote it, for what it is told.
pub(crate) fn open_plain(base: &Path, parts: &[&str], shown: &str) -> Result<std::fs::File, String> {
    let missing = || format!("{shown} is not a file");
    let mut path = base.to_path_buf();
    let mut meta = None;
    for part in parts {
        path.push(part);
        let found = path.symlink_metadata().map_err(|_| missing())?;
        if found.file_type().is_symlink() {
            return Err(format!("{shown} leads through a link"));
        }
        meta = Some(found);
    }
    let meta = meta.filter(std::fs::Metadata::is_file).ok_or_else(missing)?;
    let file = std::fs::File::open(&path).map_err(|e| format!("{shown}: {e}"))?;
    // Another instance of the plugin may have put a link there since the look above: what
    // is open now must be the very file that was looked at.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata().map_err(|e| format!("{shown}: {e}"))?;
        if (opened.dev(), opened.ino()) != (meta.dev(), meta.ino()) {
            return Err(format!("{shown} changed while it was read"));
        }
    }
    #[cfg(not(unix))]
    let _ = meta;
    Ok(file)
}

/// The bytes of the file `parts` lead to below `base` ([`open_plain`]).
pub(crate) fn read_plain(base: &Path, parts: &[&str], shown: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    open_plain(base, parts, shown)?.read_to_end(&mut bytes).map_err(|e| format!("{shown}: {e}"))?;
    Ok(bytes)
}

impl GuestDirs {
    /// The folder on this computer and the parts of the path below it.
    fn split<'a>(&self, guest: &'a str) -> Result<(&Path, Vec<&'a str>), String> {
        let (base, rest) = match guest.strip_prefix("/tmp/") {
            Some(rest) => (self.tmp.as_deref().ok_or("there is no /tmp to read from here")?, rest),
            None => match guest.strip_prefix("/data/") {
                Some(rest) => (self.data.as_path(), rest),
                None => return Err(format!("{guest:?} is neither a library id nor a file below /tmp or /data")),
            },
        };
        let parts = plain_parts(rest)
            .ok_or_else(|| format!("{guest:?} is not a plain path below /tmp or /data (no '..', no empty parts)"))?;
        Ok((base, parts))
    }

    /// The bytes of the file behind `guest`.
    pub fn read(&self, guest: &str) -> Result<Vec<u8>, String> {
        let (base, parts) = self.split(guest)?;
        read_plain(base, &parts, guest)
    }

    /// The model in the file behind `guest`, in millimetres. Files it refers to (glTF buffers,
    /// OBJ materials) are read from beside it, by the same rules.
    pub fn load(&self, guest: &str) -> Result<Mesh, String> {
        let bytes = self.read(guest)?;
        let name = file_name(guest);
        let kind = Kind::from_file_name(name);
        if kind == Kind::Step {
            return self.load_step(&bytes);
        }
        let folder = &guest[..guest.len() - name.len()];
        let sibling = |name: &str| self.read(&format!("{folder}{}", name.trim().replace('\\', "/"))).ok();
        loaders::load_mesh_with(kind, &bytes, &sibling).map_err(|e| e.to_string())
    }

    /// STEP is meshed by a program that wants a path: it gets a copy only the host writes,
    /// named by its content and with a fixed time, so the same file meshes once.
    fn load_step(&self, bytes: &[u8]) -> Result<Mesh, String> {
        let hash = blake3::hash(bytes).to_hex();
        // Beside `/data`: the plugin's temporary folders, which Meshgrove clears when it starts.
        let dir = self.data.parent().unwrap_or(&self.data).join("tmp").join(format!("step-{}", &hash[..32]));
        let copy = dir.join("model.step");
        let write = || -> std::io::Result<()> {
            if copy.is_file() {
                return Ok(());
            }
            std::fs::create_dir_all(&dir)?;
            let part = dir.join(format!("model.{}.part", crate::staging::new_uid()));
            std::fs::write(&part, bytes)?;
            std::fs::File::options().write(true).open(&part)?.set_modified(std::time::UNIX_EPOCH)?;
            std::fs::rename(&part, &copy)
        };
        write().map_err(|e| format!("a copy for meshing: {e}"))?;
        loaders::step::load(&copy).map_err(|e| e.to_string())
    }
}

/// The last part of a guest path.
pub(crate) fn file_name(guest: &str) -> &str {
    guest.rsplit('/').next().unwrap_or(guest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(base: &Path) -> GuestDirs {
        let dirs = GuestDirs { data: base.join("data"), tmp: Some(base.join("tmp")) };
        std::fs::create_dir_all(dirs.data.join("picked")).unwrap();
        std::fs::create_dir_all(dirs.tmp.as_ref().unwrap()).unwrap();
        dirs
    }

    fn stl() -> Vec<u8> {
        meshgrove_plugin_api::MeshData::cuboid([10.0, 20.0, 30.0]).to_stl("box")
    }

    #[test]
    fn files_in_the_plugins_folders_are_read() {
        let base = tempfile::tempdir().unwrap();
        let dirs = dirs(base.path());
        std::fs::write(dirs.data.join("picked").join("box.stl"), stl()).unwrap();
        std::fs::write(base.path().join("tmp").join("a.stl"), stl()).unwrap();
        assert!(is_guest_path("/data/picked/box.stl") && !is_guest_path("Eigene/box.stl"));
        assert_eq!(dirs.read("/data/picked/box.stl").unwrap(), stl());
        assert_eq!(dirs.load("/tmp/a.stl").unwrap().triangle_count(), 12);
        assert_eq!(dirs.load("/data/picked/box.stl").unwrap().bounds().unwrap().size().to_array(), [10.0, 20.0, 30.0]);
        assert!(dirs.read("/data/picked").unwrap_err().contains("not a file"), "a folder");
        assert!(dirs.read("/data/picked/missing.stl").unwrap_err().contains("not a file"));
        let no_tmp = GuestDirs { tmp: None, ..dirs };
        assert!(no_tmp.read("/tmp/a.stl").unwrap_err().contains("no /tmp"));
    }

    #[test]
    fn paths_that_leave_the_folders_are_refused() {
        let base = tempfile::tempdir().unwrap();
        let dirs = dirs(base.path());
        std::fs::write(base.path().join("secret.stl"), stl()).unwrap();
        std::fs::write(dirs.data.join("box.stl"), stl()).unwrap();
        for path in [
            "/data/../secret.stl",
            "/data/picked/../../secret.stl",
            "/tmp/../data/box.stl",
            "/data//box.stl",
            "/data/./box.stl",
            "/data/picked\\..\\..\\secret.stl",
            "/data/",
            "/tmp",
        ] {
            let err = dirs.read(path).unwrap_err();
            assert!(err.contains("plain path") || err.contains("neither"), "{path}: {err}");
        }
        for path in ["/out/box.stl", "/etc/passwd", "/", "//data/box.stl"] {
            assert!(dirs.read(path).unwrap_err().contains("neither a library id"), "{path}");
        }
    }

    /// A plugin may make links in its folders; the host does not follow them out.
    #[cfg(unix)]
    #[test]
    fn links_are_not_followed() {
        use std::os::unix::fs::symlink;
        let base = tempfile::tempdir().unwrap();
        let dirs = dirs(base.path());
        let outside = base.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.stl"), stl()).unwrap();
        symlink(outside.join("secret.stl"), dirs.data.join("file-link.stl")).unwrap();
        symlink("../outside", dirs.data.join("folder-link")).unwrap();
        symlink("picked", dirs.data.join("inside-link")).unwrap();
        std::fs::write(dirs.data.join("picked").join("box.stl"), stl()).unwrap();
        for path in ["/data/file-link.stl", "/data/folder-link/secret.stl", "/data/inside-link/box.stl"] {
            let err = dirs.read(path).unwrap_err();
            assert!(err.contains("leads through a link"), "{path}: {err}");
            assert!(dirs.load(path).is_err());
        }
        // A file beside a model (a glTF buffer, say) is read by the same rules.
        let gltf = r#"{"asset": {"version": "2.0"}, "buffers": [{"uri": "../file-link.stl", "byteLength": 4}]}"#;
        std::fs::write(dirs.data.join("picked").join("m.gltf"), gltf).unwrap();
        assert!(dirs.load("/data/picked/m.gltf").is_err());
    }
}
