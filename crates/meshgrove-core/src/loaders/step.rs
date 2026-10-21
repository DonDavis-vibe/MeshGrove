//! STEP CAD files: tessellated by the separate `meshgrove-step-mesher` (OpenCascade) and cached as
//! binary STL in `~/.cache/meshgrove/step`, since meshing a big assembly takes seconds.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{LoadError, stl};
use crate::mesh::Mesh;

const TIMEOUT: Duration = Duration::from_secs(180);
/// Bump when the mesher's settings change, to re-tessellate cached files.
const MESHER_VERSION: u32 = 1;

/// The mesher next to our own executable (target dir, ~/.local/bin), else from `PATH`.
/// `MESHGROVE_STEP_MESHER` overrides it.
pub fn mesher_path() -> PathBuf {
    if let Some(p) = std::env::var_os("MESHGROVE_STEP_MESHER") {
        return p.into();
    }
    let sibling = |exe: PathBuf| exe.parent().map(|d| d.join("meshgrove-step-mesher")).filter(|p| p.is_file());
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            // Test binaries live one level deeper (target/<profile>/deps).
            sibling(exe.clone()).or_else(|| sibling(exe.parent()?.to_path_buf()))
        })
        .unwrap_or_else(|| "meshgrove-step-mesher".into())
}

pub fn load(path: &Path) -> Result<Mesh, LoadError> {
    let base = dirs::cache_dir().ok_or_else(|| LoadError::Malformed("no cache directory".into()))?;
    load_with(path, &base.join("meshgrove").join("step"), &mesher_path())
}

fn load_with(path: &Path, cache: &Path, mesher: &Path) -> Result<Mesh, LoadError> {
    let source = crate::archive::materialize(path)?;
    let meta = std::fs::metadata(&source)?;
    let mtime =
        meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    let key =
        blake3::hash(format!("{MESHER_VERSION}\0{}\0{}\0{mtime}", path.display(), meta.len()).as_bytes()).to_hex();
    let cached = cache.join(format!("{}.stl", &key[..32]));
    if let Ok(bytes) = std::fs::read(&cached) {
        return stl::parse(&bytes);
    }
    std::fs::create_dir_all(cache)?;
    let tmp = cached.with_extension(format!("tmp{}", std::process::id()));
    run(mesher, &source, &tmp)?;
    std::fs::rename(&tmp, &cached)?;
    stl::parse(&std::fs::read(&cached)?)
}

fn run(mesher: &Path, input: &Path, output: &Path) -> Result<(), LoadError> {
    let mut child = Command::new(mesher)
        .arg(input)
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| LoadError::Malformed(format!("STEP support missing ({}): {e}", mesher.display())))?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return match status.code() {
                Some(0) => Ok(()),
                Some(10) => Err(LoadError::Malformed("meshgrove-step-mesher was built without OpenCascade".into())),
                Some(code) => Err(LoadError::Malformed(format!("STEP file could not be tessellated (code {code})"))),
                None => Err(LoadError::Malformed("STEP mesher crashed".into())),
            };
        }
        if started.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(output);
            return Err(LoadError::Malformed("STEP tessellation timed out".into()));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs the mesher built with OpenCascade (`cargo build -p meshgrove-step`); skipped otherwise.
    #[test]
    fn step_cube_is_tessellated_and_cached() {
        let mesher = mesher_path();
        let dir = tempfile::tempdir().unwrap();
        let step = dir.path().join("cube.step");
        let made = Command::new(&mesher).arg("--write-test-cube").arg(&step).stdout(Stdio::null()).status();
        if !made.is_ok_and(|s| s.success()) {
            eprintln!("skipping: {} unavailable or without OpenCascade", mesher.display());
            return;
        }
        let cache = dir.path().join("cache");
        let mesh = load_with(&step, &cache, &mesher).unwrap();
        assert_eq!(mesh.triangle_count(), 12);
        assert_eq!(mesh.bounds().unwrap().size(), glam::Vec3::splat(10.0));
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1, "result cached");
        // A cache hit no longer needs the mesher.
        assert_eq!(load_with(&step, &cache, Path::new("/nonexistent/mesher")).unwrap().triangle_count(), 12);
        std::fs::write(&step, b"ISO-10303-21; broken").unwrap();
        assert!(load_with(&step, &cache, &mesher).is_err(), "changed file is re-tessellated and fails");
    }
}
