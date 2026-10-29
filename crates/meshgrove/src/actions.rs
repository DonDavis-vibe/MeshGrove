//! Handing files to other programs: the slicer, the mesh editor, the file manager, the
//! clipboard, the trash.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use meshgrove_core::config::{Program, Slicer};
use meshgrove_core::{archive, fsinfo};

/// How the slicer got the files.
pub enum SlicerOpened {
    Started,
    /// An already open slicer window took them (Linux only so far).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    HandedOver,
}

/// Gives the model files to the slicer (archive members are extracted first): to a slicer
/// window that is already open if it accepts files from outside, else to a new one.
pub fn open_in_slicer(slicer: &Slicer, files: &[PathBuf]) -> io::Result<SlicerOpened> {
    let files: Vec<PathBuf> = files.iter().map(|f| archive::materialize(f)).collect::<io::Result<_>>()?;
    #[cfg(target_os = "linux")]
    if hand_to_running_slicer(slicer, &files) {
        return Ok(SlicerOpened::HandedOver);
    }
    launch_command(slicer, &files, std::env::var("LD_LIBRARY_PATH").ok()).spawn()?;
    Ok(SlicerOpened::Started)
}

/// Opens the model files in the mesh editor (archive members are extracted first).
pub fn open_in_editor(editor: &Program, files: &[PathBuf]) -> io::Result<()> {
    let files: Vec<PathBuf> = files.iter().map(|f| archive::materialize(f)).collect::<io::Result<_>>()?;
    let mut cmd = if editor.is_blender() {
        blender_command(editor, &files)
    } else {
        launch_command(editor, &files, std::env::var("LD_LIBRARY_PATH").ok())
    };
    cmd.spawn()?;
    Ok(())
}

/// Run by Blender after it started: an empty scene in millimetres (as models for printing are),
/// the files after `--` imported with whichever importer this Blender version has, and the
/// view fitted to them.
const BLENDER_IMPORT: &str = r#"
import bpy, sys
files = sys.argv[sys.argv.index("--") + 1:]
for obj in list(bpy.data.objects):
    bpy.data.objects.remove(obj)
units = bpy.context.scene.unit_settings
units.system, units.scale_length, units.length_unit = "METRIC", 0.001, "MILLIMETERS"
importers = {
    "stl": [("wm", "stl_import"), ("import_mesh", "stl")],
    "obj": [("wm", "obj_import"), ("import_scene", "obj")],
    "ply": [("wm", "ply_import"), ("import_mesh", "ply")],
    "gltf": [("import_scene", "gltf")],
    "glb": [("import_scene", "gltf")],
}
for f in files:
    for group, name in importers.get(f.rsplit(".", 1)[-1].lower(), []):
        try:
            getattr(getattr(bpy.ops, group), name)(filepath=f)
            break
        except (AttributeError, RuntimeError):
            pass
def fit():
    for window in bpy.context.window_manager.windows:
        for area in window.screen.areas:
            if area.type == "VIEW_3D":
                area.spaces.active.clip_end = max(area.spaces.active.clip_end, 100000.0)
                region = next(r for r in area.regions if r.type == "WINDOW")
                with bpy.context.temp_override(window=window, area=area, region=region):
                    bpy.ops.view3d.view_all()
bpy.app.timers.register(fit, first_interval=0.5)
"#;

/// Blender opens only its own files: models go in through [`BLENDER_IMPORT`].
fn blender_command(blender: &Program, files: &[PathBuf]) -> Command {
    let mut cmd = if blender.program.extension().is_some_and(|x| x == "app") {
        // A new instance, so the script runs even when Blender is already open.
        let mut open = Command::new("open");
        open.arg("-n").arg("-a").arg(&blender.program).arg("--args");
        open
    } else {
        Command::new(&blender.program)
    };
    cmd.args(&blender.args).arg("--python-expr").arg(BLENDER_IMPORT).arg("--").args(files);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    for (key, value) in &blender.env {
        cmd.env(key, value);
    }
    if let Some(dir) = &blender.workdir {
        cmd.current_dir(dir);
    }
    cmd
}

/// Orca-family slicers listen on the session bus for the files of a second start, even with
/// their own single-instance setting off: (program name contains, bus name prefix, object path
/// prefix); the running instance appends a hash of its executable's path to both.
#[cfg(target_os = "linux")]
const RUNNING_SLICERS: &[(&str, &str, &str)] = &[
    (
        "anycubicslicernext",
        "com.softfever3d.AnycubicSlicerNext.InstanceCheck.Object",
        "/com/softfever3d/AnycubicSlicer/InstanceCheck/Object",
    ),
    ("orca", "com.orcaslicer.OrcaSlicer.InstanceCheck.Object", "/com/orcaslicer/OrcaSlicer/InstanceCheck/Object"),
    // OrcaSlicer 2.x and older.
    ("orca", "com.softfever3d.orca-slicer.InstanceCheck.Object", "/com/softfever3d/OrcaSlicer/InstanceCheck/Object"),
];

/// Sends the files to an open window of this slicer; false if none is open (or it failed).
#[cfg(target_os = "linux")]
fn hand_to_running_slicer(slicer: &Slicer, files: &[PathBuf]) -> bool {
    let program = slicer.program.to_string_lossy().to_lowercase();
    let program_name = program.rsplit('/').next().unwrap_or_default();
    let families: Vec<_> = RUNNING_SLICERS.iter().filter(|(key, _, _)| program_name.contains(key)).collect();
    if families.is_empty() {
        return false;
    }
    let Ok(out) = Command::new("dbus-send")
        .args(["--session", "--print-reply", "--dest=org.freedesktop.DBus", "/org/freedesktop/DBus"])
        .arg("org.freedesktop.DBus.ListNames")
        .stderr(Stdio::null())
        .output()
    else {
        return false;
    };
    let names = bus_names(&String::from_utf8_lossy(&out.stdout));
    // The message is the command line the slicer would have got, as its C-style escaped list.
    let mut args = vec![slicer.program.to_string_lossy().into_owned()];
    args.extend(files.iter().map(|f| f.to_string_lossy().into_owned()));
    let message = escape_strings_cstyle(&args);
    families.iter().any(|&&(_, bus_prefix, path_prefix)| {
        names.iter().filter_map(|n| n.strip_prefix(bus_prefix).map(|hash| (n, hash))).any(|(name, hash)| {
            Command::new("dbus-send")
                .args(handover_args(name, &format!("{path_prefix}{hash}"), &message))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        })
    })
}

/// Arguments for `dbus-send` calling `AnotherInstance(message)` (no reply is sent back).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn handover_args(name: &str, object: &str, message: &str) -> Vec<String> {
    vec![
        "--session".into(),
        "--type=method_call".into(),
        format!("--dest={name}"),
        object.into(),
        format!("{name}.AnotherInstance"),
        format!("string:{message}"),
    ]
}

/// Bus names from `dbus-send --print-reply … ListNames` output (`string "name"` lines).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn bus_names(reply: &str) -> Vec<String> {
    reply.lines().filter_map(|l| l.trim().strip_prefix("string \"")?.strip_suffix('"').map(str::to_owned)).collect()
}

/// string with spaces, tabs, `;`, `\`, `"` or line breaks goes in quotes with `\` escapes.
/// string with spaces, tabs, `;`, `\\`, `"` or line breaks goes in quotes with `\\` escapes.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn escape_strings_cstyle(strings: &[String]) -> String {
    let mut out = String::new();
    for (i, s) in strings.iter().enumerate() {
        if i > 0 {
            out.push(';');
        }
        let quote = (strings.len() == 1 && s.is_empty()) || s.contains([' ', '\t', ';', '\\', '"', '\r', '\n']);
        if !quote {
            out += s;
            continue;
        }
        out.push('"');
        for c in s.chars() {
            match c {
                '\\' | '"' => {
                    out.push('\\');
                    out.push(c);
                }
                '\r' => out += "\\r",
                '\n' => out += "\\n",
                c => out.push(c),
            }
        }
        out.push('"');
    }
    out
}

/// `program [args…] files…` for the slicer or the mesh editor.
fn launch_command(program: &Program, files: &[PathBuf], inherited_lib_path: Option<String>) -> Command {
    // A macOS app bundle starts through `open -a`, which also hands files to a running instance.
    let mut cmd = if program.program.extension().is_some_and(|x| x == "app") {
        let mut open = Command::new("open");
        open.arg("-a").arg(&program.program);
        open
    } else {
        Command::new(&program.program)
    };
    // PrusaSlicer hands the files to an open window itself when asked to.
    let program_name = program.program.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if program_name.contains("prusa") {
        cmd.arg("--single-instance");
    }
    cmd.args(&program.args).args(files).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    for (key, value) in &program.env {
        let value = match (key.as_str(), &inherited_lib_path) {
            // Keep the inherited library path behind the program's own libraries.
            ("LD_LIBRARY_PATH", Some(inherited)) if !inherited.is_empty() => format!("{value}:{inherited}"),
            _ => value.clone(),
        };
        cmd.env(key, value);
    }
    if let Some(dir) = &program.workdir {
        cmd.current_dir(dir);
    }
    cmd
}

/// Opens the file manager with the files selected (Dolphin/Nautilus via the freedesktop
/// FileManager1 D-Bus interface, Finder, Explorer). Archive members show their archive.
pub fn show_in_file_manager(paths: &[PathBuf]) -> io::Result<()> {
    let mut on_disk: Vec<PathBuf> =
        paths.iter().map(|p| archive::split(p).map_or_else(|| p.clone(), |(a, _)| a)).collect();
    on_disk.dedup();
    reveal(&on_disk)
}

#[cfg(target_os = "linux")]
fn reveal(paths: &[PathBuf]) -> io::Result<()> {
    let uris: Vec<String> = paths.iter().map(|p| file_uri(p)).collect();
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
            let folder = paths.first().and_then(|p| p.parent()).unwrap_or(Path::new("/"));
            open_with_default_app(folder)
        }
    }
}

#[cfg(target_os = "macos")]
fn reveal(paths: &[PathBuf]) -> io::Result<()> {
    Command::new("open").arg("-R").args(paths).spawn().map(drop)
}

#[cfg(windows)]
fn reveal(paths: &[PathBuf]) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    // Explorer selects a single file and wants the path quoted right after the comma.
    let Some(first) = paths.first() else { return Ok(()) };
    Command::new("explorer").raw_arg(format!("/select,\"{}\"", first.display())).spawn().map(drop)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn reveal(paths: &[PathBuf]) -> io::Result<()> {
    let folder = paths.first().and_then(|p| p.parent()).unwrap_or(Path::new("/"));
    open_with_default_app(folder)
}

/// Opens a file or folder with the system's default application.
pub fn open_with_default_app(path: &Path) -> io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    Command::new(opener).arg(path).spawn().map(drop)
}

/// Puts the files on the clipboard so they can be pasted into the file manager, a slicer or a
/// chat; `cut` marks them for file managers that move a cut selection (Dolphin, Nautilus).
/// Archive members are extracted first. Blocks while extracting, so call it off the UI thread.
pub fn copy_files(paths: &[PathBuf], cut: bool) -> Result<usize, String> {
    let files: Vec<PathBuf> =
        paths.iter().map(|p| archive::materialize(p)).collect::<io::Result<_>>().map_err(|e| e.to_string())?;
    files_to_clipboard(&files, cut)?;
    Ok(files.len())
}

/// Wayland: `text/uri-list` plus GNOME's variant (which says copy or cut, as KDE's own type
/// does), and the plain paths as text.
#[cfg(target_os = "linux")]
fn files_to_clipboard(files: &[PathBuf], cut: bool) -> Result<(), String> {
    use wl_clipboard_rs::copy::{MimeSource, MimeType, Options, Source};
    let uris: Vec<String> = files.iter().map(|p| file_uri(p)).collect();
    let text: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
    let source =
        |data: String, mime: MimeType| MimeSource { source: Source::Bytes(data.into_bytes().into()), mime_type: mime };
    let how = if cut { "cut" } else { "copy" };
    // Plain text first: the extra text types the library adds fall back to the first text source.
    let mut sources = vec![
        source(text.join("\n"), MimeType::Text),
        source(uris.iter().map(|u| format!("{u}\r\n")).collect(), MimeType::Specific("text/uri-list".into())),
        source(format!("{how}\n{}", uris.join("\n")), MimeType::Specific(GNOME_FILES.into())),
    ];
    if cut {
        sources.push(source("1".into(), MimeType::Specific(KDE_CUT.into())));
    }
    Options::new().copy_multi(sources).map_err(|e| e.to_string())
}

/// GNOME's clipboard type for files: `copy` or `cut`, then the URIs.
#[cfg(target_os = "linux")]
const GNOME_FILES: &str = "x-special/gnome-copied-files";
/// KDE's marker for a cut selection ("1").
#[cfg(target_os = "linux")]
const KDE_CUT: &str = "application/x-kde-cutselection";

/// Windows (file drop list, as Explorer's copy) and macOS (file URLs, as Finder's copy). A cut
/// is Meshgrove's own affair there.
#[cfg(any(windows, target_os = "macos"))]
fn files_to_clipboard(files: &[PathBuf], _cut: bool) -> Result<(), String> {
    use clipboard_rs::{Clipboard, ClipboardContext};
    let clipboard = ClipboardContext::new().map_err(|e| e.to_string())?;
    clipboard.set_files(files.iter().map(|p| p.display().to_string()).collect()).map_err(|e| e.to_string())
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn files_to_clipboard(_files: &[PathBuf], _cut: bool) -> Result<(), String> {
    Err("copying files is not supported on this system".into())
}

/// `file://` URI with everything but unreserved characters and `/` percent-encoded.
#[cfg(target_os = "linux")]
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

/// The files on the clipboard, for pasting into a folder: what a file manager (or Meshgrove)
/// copied, and whether they were cut (a paste moves them then). `text` is the clipboard as
/// text, the fallback where files cannot be read from it (X11, a chat): one path or `file://`
/// URI per line.
pub fn clipboard_files(text: Option<&str>) -> (Vec<PathBuf>, bool) {
    match files_from_clipboard() {
        Ok((files, cut)) if !files.is_empty() => (files, cut),
        Ok(_) => (text.map(paths_from_text).unwrap_or_default(), text.is_some_and(cut_header)),
        Err(e) => {
            log::info!("reading files from the clipboard failed: {e}");
            (text.map(paths_from_text).unwrap_or_default(), text.is_some_and(cut_header))
        }
    }
}

/// Wayland: the `text/uri-list` a file manager (or Meshgrove) put there, and KDE's or GNOME's
/// word on whether it was cut.
#[cfg(target_os = "linux")]
fn files_from_clipboard() -> Result<(Vec<PathBuf>, bool), String> {
    use wl_clipboard_rs::paste::{ClipboardType, MimeType, Seat, get_contents};
    let read = |mime: &str| -> Result<String, String> {
        use std::io::Read;
        let (mut pipe, _) = get_contents(ClipboardType::Regular, Seat::Unspecified, MimeType::Specific(mime))
            .map_err(|e| e.to_string())?;
        let mut text = String::new();
        pipe.read_to_string(&mut text).map_err(|e| e.to_string())?;
        Ok(text)
    };
    let files = paths_from_text(&read("text/uri-list")?);
    let cut = read(KDE_CUT).is_ok_and(|s| s.trim() == "1") || read(GNOME_FILES).is_ok_and(|s| cut_header(&s));
    Ok((files, cut))
}

/// Windows (Explorer's file drop list) and macOS (Finder's file URLs).
#[cfg(any(windows, target_os = "macos"))]
fn files_from_clipboard() -> Result<(Vec<PathBuf>, bool), String> {
    use clipboard_rs::{Clipboard, ClipboardContext};
    let clipboard = ClipboardContext::new().map_err(|e| e.to_string())?;
    let files = clipboard.get_files().map_err(|e| e.to_string())?;
    Ok((files.iter().flat_map(|f| paths_from_text(f)).collect(), false))
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn files_from_clipboard() -> Result<(Vec<PathBuf>, bool), String> {
    Err("reading files from the clipboard is not supported on this system".into())
}

/// GNOME's `cut` header on the first line.
fn cut_header(text: &str) -> bool {
    text.lines().next().is_some_and(|l| l.trim() == "cut")
}

/// Paths from text, one per line: absolute paths or `file://` URIs (a `text/uri-list`, GNOME's
/// `copy`/`cut` header included). Only what exists counts.
pub fn paths_from_text(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(path_from_line)
        .filter(|p| p.is_absolute() && p.symlink_metadata().is_ok())
        .collect()
}

/// A line of a `text/uri-list` (`file:///home/x` or `file://localhost/home/x`) or a plain path.
fn path_from_line(line: &str) -> Option<PathBuf> {
    let Some(rest) = line.strip_prefix("file://") else { return Some(PathBuf::from(line)) };
    let path = match rest.find('/') {
        Some(0) => rest,
        Some(i) if &rest[..i] == "localhost" => &rest[i..],
        _ => return None,
    };
    let bytes = percent_decode(path);
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
    #[cfg(not(unix))]
    {
        let text = String::from_utf8_lossy(&bytes).into_owned();
        // `/C:/…` from a `file:///C:/…` URI.
        let text = match text.strip_prefix('/') {
            Some(drive) if drive.as_bytes().get(1) == Some(&b':') => drive.to_owned(),
            _ => text,
        };
        Some(PathBuf::from(text))
    }
}

/// `%20` and the like back to bytes.
fn percent_decode(s: &str) -> Vec<u8> {
    let hex = |b: Option<&u8>| (*b? as char).to_digit(16).map(|d| d as u8);
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let (Some(high), Some(low)) = (hex(bytes.get(i + 1)), hex(bytes.get(i + 2)))
        {
            out.push(high * 16 + low);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
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

    #[cfg(target_os = "linux")]
    #[test]
    fn paths_come_from_uris_or_plain_lines() {
        let dir = tempfile::tempdir().unwrap();
        let rose = dir.path().join("rosé bush.stl");
        std::fs::write(&rose, "r").unwrap();
        let uri = file_uri(&rose);
        assert!(uri.contains("ros%C3%A9%20bush.stl"), "{uri}");
        let text = format!("copy\n{uri}\r\n# a comment\n{}\n/nowhere/x.stl\nfile://elsewhere/x.stl\n", rose.display());
        assert_eq!(paths_from_text(&text), [rose.clone(), rose]);
        assert_eq!(percent_decode("a%2Fb%zz"), b"a/b%zz");
        assert!(!cut_header(&text) && cut_header("cut\nfile:///x") && !cut_header("/cut/x.stl"));
    }

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
        let cmd = launch_command(
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
    fn blender_imports_the_models() {
        let blender = Program {
            name: "Blender".into(),
            program: "/usr/bin/blender".into(),
            args: Vec::new(),
            env: Vec::new(),
            workdir: None,
        };
        let cmd = blender_command(&blender, &[PathBuf::from("/m/orc.stl"), PathBuf::from("/m/a b.obj")]);
        assert_eq!(cmd.get_program(), "/usr/bin/blender");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args[0], "--python-expr");
        assert_eq!(args[2..], ["--", "/m/orc.stl", "/m/a b.obj"]);
    }

    #[test]
    fn running_slicers_get_their_command_line_over_dbus() {
        let args: Vec<String> =
            ["/opt/a/AnycubicSlicerNext", "/m/orc.stl", "/m/Fidgets & Puzzles/shark v2.stl", r#"a"b\c"#]
                .map(String::from)
                .to_vec();
        assert_eq!(
            escape_strings_cstyle(&args),
            r#"/opt/a/AnycubicSlicerNext;/m/orc.stl;"/m/Fidgets & Puzzles/shark v2.stl";"a\"b\\c""#
        );
        assert_eq!(escape_strings_cstyle(&[String::new()]), r#""""#);

        let reply = "method return time=1 sender=org.freedesktop.DBus -> destination=:1.9 serial=3\n   array [\n      string \"org.freedesktop.DBus\"\n      string \"com.softfever3d.AnycubicSlicerNext.InstanceCheck.Object5886\"\n   ]\n";
        assert_eq!(
            bus_names(reply),
            ["org.freedesktop.DBus", "com.softfever3d.AnycubicSlicerNext.InstanceCheck.Object5886"]
        );
        assert_eq!(
            handover_args("com.x.InstanceCheck.Object7", "/com/x/InstanceCheck/Object7", "a;b"),
            [
                "--session",
                "--type=method_call",
                "--dest=com.x.InstanceCheck.Object7",
                "/com/x/InstanceCheck/Object7",
                "com.x.InstanceCheck.Object7.AnotherInstance",
                "string:a;b"
            ]
        );
    }

    #[test]
    fn prusaslicer_is_asked_for_a_single_instance() {
        let slicer = Slicer {
            name: "PrusaSlicer".into(),
            program: "/usr/bin/prusa-slicer".into(),
            args: Vec::new(),
            env: Vec::new(),
            workdir: None,
        };
        let cmd = launch_command(&slicer, &[PathBuf::from("/m/orc.stl")], None);
        assert_eq!(cmd.get_args().collect::<Vec<_>>(), ["--single-instance", "/m/orc.stl"]);
    }

    #[test]
    fn mac_app_bundles_start_through_open() {
        let slicer = Slicer {
            name: "OrcaSlicer".into(),
            program: "/Applications/OrcaSlicer.app".into(),
            args: Vec::new(),
            env: Vec::new(),
            workdir: None,
        };
        let cmd = launch_command(&slicer, &[PathBuf::from("/m/orc.stl")], None);
        assert_eq!(cmd.get_program(), "open");
        assert_eq!(cmd.get_args().collect::<Vec<_>>(), ["-a", "/Applications/OrcaSlicer.app", "/m/orc.stl"]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn file_uris_are_percent_encoded() {
        assert_eq!(file_uri(Path::new("/a b/Größe,1.stl")), "file:///a%20b/Gr%C3%B6%C3%9Fe%2C1.stl");
        assert_eq!(file_uri(Path::new("/x/plain-name_1.3mf")), "file:///x/plain-name_1.3mf");
    }
}
