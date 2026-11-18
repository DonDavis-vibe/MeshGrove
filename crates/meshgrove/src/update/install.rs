//! Putting a new version in place, where Meshgrove can do that safely: the package is downloaded,
//! its SHA-256 must match the release's `SHA256SUMS.txt`, and only then does anything change.
//!
//! The program keeps its path on every platform: menu entries, pinned launchers, AppImage
//! managers, the Windows shortcuts and the AI apps' MCP settings all point at it, so an update
//! changes what is there, never where it is (SolidCraft renamed its AppImage after the new version
//! and left launchers pointing at nothing). Modelled on SolidCraft's `update/install.rs`
//! (Apache-2.0).

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::kind::InstallKind;
use super::net::{Net, Progress};
use super::release::{Release, SUMS, expected_sha256};

/// What happens once the new version is in place.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Starting this program runs the new version (once Meshgrove has closed).
    Restart(PathBuf),
    /// The installer is running: Meshgrove closes now, and the installer starts it again.
    Installer,
    /// The disk image is open: the user drags the new app over the old one.
    Opened,
}

/// The program files of the Windows ZIP that can take the place of their namesakes.
const PORTABLE_EXE: &str = "meshgrove.exe";

/// Downloads, verifies and puts in place the package `kind` needs from `release`.
pub fn run(net: &Net, release: &Release, kind: &InstallKind, progress: &Progress) -> Result<Outcome, String> {
    let name = kind.asset_name(&release.version).ok_or("no package fits this installation")?;
    let asset = release.asset(&name).ok_or_else(|| format!("{name} is missing from the release"))?;
    let sums = release.asset(SUMS).ok_or_else(|| format!("the release has no {SUMS}"))?;
    let sums = net.text(&sums.url, 64 * 1024).map_err(|e| e.to_string())?;
    let expected = expected_sha256(&sums, &name).ok_or_else(|| format!("{SUMS} lists no {name}"))?;
    let fetch = |to: &Path| -> Result<(), String> {
        let hash = net.download(&asset.url, asset.size, to, progress)?;
        if hash != expected {
            let _ = fs::remove_file(to);
            return Err(format!("{name} does not match its checksum: nothing was changed"));
        }
        Ok(())
    };
    match kind {
        InstallKind::AppImage(path) => {
            let update = sibling(path, ".update");
            fetch(&update)?;
            put_in_place(&update, path)?;
            Ok(Outcome::Restart(path.clone()))
        }
        InstallKind::WindowsSetup { dir, all_users } => {
            let setup = temp_file(&name)?;
            fetch(&setup)?;
            let mut command = Command::new(&setup);
            add_args(&mut command, &installer_args(dir, *all_users));
            command.spawn().map_err(|e| format!("{}: {e}", setup.display()))?;
            Ok(Outcome::Installer)
        }
        InstallKind::WindowsPortable(dir) => {
            let zip = temp_file(&name)?;
            fetch(&zip)?;
            let result = swap_from_zip(&zip, dir);
            let _ = fs::remove_file(&zip);
            result?;
            Ok(Outcome::Restart(dir.join(PORTABLE_EXE)))
        }
        InstallKind::MacApp(_) => {
            let dmg = temp_file(&name)?;
            fetch(&dmg)?;
            Command::new("open").arg(&dmg).spawn().map_err(|e| format!("open {}: {e}", dmg.display()))?;
            Ok(Outcome::Opened)
        }
        _ => Err("Meshgrove cannot update this installation itself".to_owned()),
    }
}

/// `path` with `suffix` added to its name, in the same folder (so a rename stays on one disk).
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// A fresh file in the temporary folder's `meshgrove-update`.
fn temp_file(name: &str) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("meshgrove-update");
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let file = dir.join(name);
    let _ = fs::remove_file(&file);
    Ok(file)
}

/// The downloaded `update` takes the place of `target`: same path, same permissions. One rename on
/// one disk, so `target` is either the old file or the new one, never missing. On failure the
/// download is removed and `target` stays as it was.
pub(super) fn put_in_place(update: &Path, target: &Path) -> Result<(), String> {
    if let Ok(meta) = fs::metadata(target) {
        let _ = fs::set_permissions(update, meta.permissions());
    }
    fs::rename(update, target).map_err(|e| {
        let _ = fs::remove_file(update);
        format!("{}: {e}", target.display())
    })
}

/// The Windows installer's arguments: quiet, into the folder it is installed in now and for the
/// same users, and start Meshgrove again when done (`meshgrove.iss` reads `/relaunch=1`). The
/// same folder keeps the shortcuts and the AI apps' path to `meshgrove-mcp.exe` right.
pub(super) fn installer_args(dir: &Path, all_users: bool) -> Vec<String> {
    vec![
        "/SILENT".into(),
        "/SUPPRESSMSGBOXES".into(),
        "/NORESTART".into(),
        "/relaunch=1".into(),
        if all_users { "/ALLUSERS" } else { "/CURRENTUSER" }.into(),
        format!("/DIR=\"{}\"", dir.display()),
    ]
}

/// Inno Setup wants `/DIR="C:\Program Files\Meshgrove"` as written.
fn add_args(command: &mut Command, args: &[String]) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        for arg in args {
            command.raw_arg(arg);
        }
    }
    #[cfg(not(windows))]
    command.args(args);
}

/// The files right inside the ZIP's one folder take the place of their namesakes in `dir`: each is
/// written beside its namesake as `.update`, then swapped in. A running program cannot be
/// replaced on Windows but can be renamed: it becomes `.old`, removed at the next start
/// ([`clean_up`]). If one swap fails, the ones done are taken back.
pub(super) fn swap_from_zip(zip: &Path, dir: &Path) -> Result<(), String> {
    let file = File::open(zip).map_err(|e| format!("{}: {e}", zip.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", zip.display()))?;
    let mut updates: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut unpack = || -> Result<(), String> {
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            let Some(inner) = entry.enclosed_name().filter(|_| entry.is_file()) else { continue };
            if inner.components().count() != 2 {
                continue;
            }
            let Some(name) = inner.file_name().and_then(|n| n.to_str()).map(str::to_owned) else { continue };
            let update = dir.join(format!("{name}.update"));
            let mut out = File::create(&update).map_err(|e| format!("{}: {e}", update.display()))?;
            updates.push((update.clone(), dir.join(&name)));
            std::io::copy(&mut entry, &mut out).map_err(|e| format!("{}: {e}", update.display()))?;
        }
        Ok(())
    };
    let unpacked = unpack();
    let has_program =
        updates.iter().any(|(_, target)| target.file_name().is_some_and(|n| n.eq_ignore_ascii_case(PORTABLE_EXE)));
    if let Err(e) =
        unpacked.and_then(|()| if has_program { Ok(()) } else { Err(format!("the package holds no {PORTABLE_EXE}")) })
    {
        for (update, _) in &updates {
            let _ = fs::remove_file(update);
        }
        return Err(e);
    }
    let mut swapped: Vec<PathBuf> = Vec::new();
    for (update, target) in &updates {
        if let Err(e) = swap(update, target) {
            for done in swapped.iter().rev() {
                unswap(done);
            }
            for (update, _) in &updates {
                let _ = fs::remove_file(update);
            }
            return Err(format!("{}: {e}", target.display()));
        }
        swapped.push(target.clone());
    }
    Ok(())
}

fn swap(update: &Path, target: &Path) -> std::io::Result<()> {
    let old = sibling(target, ".old");
    let _ = fs::remove_file(&old);
    let had = target.exists();
    if had {
        fs::rename(target, &old)?;
    }
    if let Err(e) = fs::rename(update, target) {
        if had {
            let _ = fs::rename(&old, target);
        }
        return Err(e);
    }
    Ok(())
}

fn unswap(target: &Path) {
    let old = sibling(target, ".old");
    if old.exists() && fs::remove_file(target).is_ok() {
        let _ = fs::rename(&old, target);
    }
}

/// After a portable update: the old programs and texts renamed to `.old` go. Only names a
/// Meshgrove package brings are touched.
pub fn clean_up(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(original) = name.strip_suffix(".old") else { continue };
        let ours = original.to_ascii_lowercase().starts_with("meshgrove")
            || ["README.md", "LICENSE.txt", "THIRD-PARTY-LICENSES.md"].contains(&original);
        if ours && dir.join(original).exists() {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Starts `program` (the new version) on its own: after an update, once Meshgrove has saved and
/// closed. An AppImage's runtime variables of this process are not handed on.
pub fn relaunch(program: &Path) {
    let mut command = Command::new(program);
    for var in ["APPDIR", "APPIMAGE", "ARGV0", "OWD"] {
        command.env_remove(var);
    }
    if let Some(dir) = program.parent() {
        command.current_dir(dir);
    }
    if let Err(e) = command.spawn() {
        log::warn!("update: could not start {}: {e}", program.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::net::tests::serve;
    use crate::update::release::Asset;
    use std::io::Write;

    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> =
            fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    fn sha(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// A release whose AppImage and checksum list come from a local server.
    fn release(url: &str, package: &[u8]) -> Release {
        let name = "Meshgrove-9.0.0-x86_64.AppImage";
        Release {
            version: semver::Version::new(9, 0, 0),
            notes: String::new(),
            page: String::new(),
            assets: vec![
                Asset { name: name.into(), url: format!("{url}/{name}"), size: package.len() as u64 },
                Asset { name: SUMS.into(), url: format!("{url}/{SUMS}"), size: 0 },
            ],
        }
    }

    #[test]
    fn update_keeps_the_program_path() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Meshgrove-0.12.0-x86_64.AppImage");
        fs::write(&app, b"old program").unwrap();
        fs::write(dir.path().join("notes.txt"), b"mine").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&app, fs::Permissions::from_mode(0o750)).unwrap();
        }
        let before = files(dir.path());
        let package = b"new program".to_vec();
        let sums = format!("{}  Meshgrove-9.0.0-x86_64.AppImage\n", sha(&package));
        let (url, _) = serve(vec![(200, vec![], sums.clone().into_bytes()), (200, vec![], package.clone())]);
        let outcome =
            run(&Net::new(true), &release(&url, &package), &InstallKind::AppImage(app.clone()), &Progress::default());
        assert_eq!(outcome, Ok(Outcome::Restart(app.clone())));
        // The same path, with the new program, the old permissions, and nothing else changed:
        // the name still says 0.12.0, so every launcher pointing at it keeps working.
        assert_eq!(fs::read(&app).unwrap(), package);
        assert_eq!(files(dir.path()), before);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&app).unwrap().permissions().mode() & 0o777, 0o750);
        }
    }

    #[test]
    fn a_package_that_does_not_match_its_checksum_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Meshgrove.AppImage");
        fs::write(&app, b"old program").unwrap();
        let package = b"tampered".to_vec();
        let sums = format!("{}  Meshgrove-9.0.0-x86_64.AppImage\n", "0".repeat(64));
        let (url, _) = serve(vec![(200, vec![], sums.clone().into_bytes()), (200, vec![], package.clone())]);
        let outcome =
            run(&Net::new(true), &release(&url, &package), &InstallKind::AppImage(app.clone()), &Progress::default());
        assert!(outcome.unwrap_err().contains("checksum"));
        assert_eq!(fs::read(&app).unwrap(), b"old program");
        assert_eq!(files(dir.path()), ["Meshgrove.AppImage"]);
    }

    #[test]
    fn no_checksum_list_no_update() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Meshgrove.AppImage");
        fs::write(&app, b"old").unwrap();
        let mut r = release("http://127.0.0.1:1", b"x");
        r.assets.retain(|a| a.name != SUMS);
        let outcome = run(&Net::new(true), &r, &InstallKind::AppImage(app.clone()), &Progress::default());
        assert!(outcome.unwrap_err().contains(SUMS));
        assert_eq!(fs::read(&app).unwrap(), b"old");
    }

    #[test]
    fn a_failed_put_in_place_keeps_the_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let update = dir.path().join("x.update");
        fs::write(&update, b"new").unwrap();
        // The target's folder does not exist: the rename fails.
        let target = dir.path().join("missing").join("x");
        assert!(put_in_place(&update, &target).is_err());
        assert!(!update.exists(), "the download is removed");
    }

    #[test]
    fn the_installer_goes_into_the_same_folder_for_the_same_users() {
        let dir = Path::new(r"C:\Program Files\Meshgrove");
        let args = installer_args(dir, true);
        assert!(args.contains(&r#"/DIR="C:\Program Files\Meshgrove""#.to_owned()), "{args:?}");
        assert!(args.contains(&"/ALLUSERS".to_owned()) && args.contains(&"/relaunch=1".to_owned()));
        let args = installer_args(Path::new(r"C:\Users\u\AppData\Local\Programs\Meshgrove"), false);
        assert!(args.contains(&"/CURRENTUSER".to_owned()) && !args.contains(&"/ALLUSERS".to_owned()));
    }

    fn portable_zip(path: &Path, files: &[(&str, &[u8])]) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (name, data) in files {
            zip.start_file(*name, options).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn the_portable_programs_keep_their_names() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Meshgrove");
        fs::create_dir(&folder).unwrap();
        for (name, data) in [
            ("meshgrove.exe", "old gui"),
            ("meshgrove-mcp.exe", "old mcp"),
            ("README.md", "old"),
            ("my-notes.txt", "mine"),
        ] {
            fs::write(folder.join(name), data).unwrap();
        }
        let zip = dir.path().join("p.zip");
        let top = "Meshgrove-9.0.0-windows-x64-portable";
        portable_zip(
            &zip,
            &[
                (&format!("{top}/meshgrove.exe"), b"new gui"),
                (&format!("{top}/meshgrove-mcp.exe"), b"new mcp"),
                (&format!("{top}/README.md"), b"new"),
                ("stray-at-top.txt", b"no"),
            ],
        );
        swap_from_zip(&zip, &folder).unwrap();
        assert_eq!(fs::read_to_string(folder.join("meshgrove.exe")).unwrap(), "new gui");
        assert_eq!(fs::read_to_string(folder.join("meshgrove-mcp.exe")).unwrap(), "new mcp");
        assert_eq!(fs::read_to_string(folder.join("my-notes.txt")).unwrap(), "mine");
        assert!(!folder.join("stray-at-top.txt").exists());
        // The old ones wait as .old until the next start removes them.
        assert!(folder.join("meshgrove.exe.old").exists());
        clean_up(&folder);
        assert_eq!(files(&folder), ["README.md", "meshgrove-mcp.exe", "meshgrove.exe", "my-notes.txt"]);
    }

    #[test]
    fn a_zip_without_the_program_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("meshgrove.exe"), "old").unwrap();
        let zip = dir.path().join("p.zip");
        portable_zip(&zip, &[("top/README.md", b"new")]);
        assert!(swap_from_zip(&zip, dir.path()).is_err());
        assert_eq!(files(dir.path()), ["meshgrove.exe", "p.zip"]);
        assert_eq!(fs::read_to_string(dir.path().join("meshgrove.exe")).unwrap(), "old");
    }

    #[test]
    fn clean_up_leaves_other_old_files_alone() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["meshgrove.exe", "meshgrove.exe.old", "photo.jpg.old", "orphan-meshgrove.exe.old"] {
            fs::write(dir.path().join(name), "x").unwrap();
        }
        clean_up(dir.path());
        assert_eq!(files(dir.path()), ["meshgrove.exe", "orphan-meshgrove.exe.old", "photo.jpg.old"]);
    }
}
