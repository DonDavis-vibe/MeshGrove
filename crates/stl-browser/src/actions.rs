//! Handing files to other programs: the slicer, the file manager, the trash.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use stlb_core::config::Slicer;
use stlb_core::{archive, fsinfo};

/// Starts the slicer with the given model files (archive members are extracted first).
pub fn open_in_slicer(slicer: &Slicer, files: &[PathBuf]) -> io::Result<()> {
    let files: Vec<PathBuf> = files.iter().map(|f| archive::materialize(f)).collect::<io::Result<_>>()?;
    slicer_command(slicer, &files, std::env::var("LD_LIBRARY_PATH").ok()).spawn().map(drop)
}

fn slicer_command(slicer: &Slicer, files: &[PathBuf], inherited_lib_path: Option<String>) -> Command {
    let mut cmd = Command::new(&slicer.program);
    cmd.args(&slicer.args).args(files).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    for (key, value) in &slicer.env {
        let value = match (key.as_str(), &inherited_lib_path) {
            // Keep the inherited library path behind the slicer's own libraries.
            ("LD_LIBRARY_PATH", Some(inherited)) if !inherited.is_empty() => format!("{value}:{inherited}"),
            _ => value.clone(),
        };
        cmd.env(key, value);
    }
    if let Some(dir) = &slicer.workdir {
        cmd.current_dir(dir);
    }
    cmd
}

/// Opens the file manager with the files selected (KDE Dolphin, Nautilus, … via the
/// freedesktop FileManager1 D-Bus interface). Archive members show their archive.
pub fn show_in_file_manager(paths: &[PathBuf]) -> io::Result<()> {
    let mut on_disk: Vec<PathBuf> =
        paths.iter().map(|p| archive::split(p).map_or_else(|| p.clone(), |(a, _)| a)).collect();
    on_disk.dedup();
    let uris: Vec<String> = on_disk.iter().map(|p| file_uri(p)).collect();
    let status = Command::new("dbus-send")
        .args([
            "--session",
            "--print-reply",
            "--dest=org.freedesktop.FileManager1",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
            &format!("array:string:{}", uris.join(",")),
            "string:",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match status {
        Ok(s) if s.success() => Ok(()),
        _ => {
            // No file manager service: at least open the folder.
            let folder = on_disk.first().and_then(|p| p.parent()).unwrap_or(Path::new("/"));
            Command::new("xdg-open").arg(folder).spawn().map(drop)
        }
    }
}

/// `file://` URI with everything but unreserved characters and `/` percent-encoded.
pub fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            uri.push(b as char);
        } else {
            uri += &format!("%{b:02X}");
        }
    }
    uri
}

/// Why `paths` cannot be moved to the trash, as a message for the user.
pub fn trash_blocked(paths: &[PathBuf]) -> Option<String> {
    if paths.is_empty() {
        return Some(String::new());
    }
    if paths.iter().any(|p| archive::split(p).is_some()) {
        return Some(t!("trash.blocked_archive").into_owned());
    }
    if paths.iter().any(|p| fsinfo::is_read_only(p)) {
        return Some(t!("trash.blocked_read_only").into_owned());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slicer_command_line_and_environment() {
        let slicer = Slicer {
            name: "Anycubic Slicer Next".into(),
            program: "/opt/a/app/bin/AnycubicSlicerNext".into(),
            args: vec!["--some-flag".into()],
            env: vec![
                ("LD_LIBRARY_PATH".into(), "/opt/a/app/lib".into()),
                ("WEBKIT_DISABLE_DMABUF_RENDERER".into(), "1".into()),
            ],
            workdir: Some("/opt/a".into()),
        };
        let cmd = slicer_command(
            &slicer,
            &[PathBuf::from("/m/orc.stl"), PathBuf::from("/m/plate.3mf")],
            Some("/usr/lib/x".into()),
        );
        assert_eq!(cmd.get_program(), "/opt/a/app/bin/AnycubicSlicerNext");
        assert_eq!(cmd.get_args().collect::<Vec<_>>(), ["--some-flag", "/m/orc.stl", "/m/plate.3mf"]);
        let envs: Vec<_> = cmd.get_envs().map(|(k, v)| (k.to_owned(), v.map(|v| v.to_owned()))).collect();
        assert!(envs.contains(&("LD_LIBRARY_PATH".into(), Some("/opt/a/app/lib:/usr/lib/x".into()))));
        assert!(envs.contains(&("WEBKIT_DISABLE_DMABUF_RENDERER".into(), Some("1".into()))));
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/opt/a")));
    }

    #[test]
    fn file_uris_are_percent_encoded() {
        assert_eq!(file_uri(Path::new("/a b/Größe,1.stl")), "file:///a%20b/Gr%C3%B6%C3%9Fe%2C1.stl");
        assert_eq!(file_uri(Path::new("/x/plain-name_1.3mf")), "file:///x/plain-name_1.3mf");
    }
}
