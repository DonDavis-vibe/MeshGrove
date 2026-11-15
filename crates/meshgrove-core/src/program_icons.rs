//! The icon a program of this computer brings itself: for the slicers and editors in the
//! settings, so their rows and menu entries show what the desktop shows for them.
//!
//! On Linux a program says which icon is its own in its desktop entry (`Icon=`), and ships the
//! picture in the `hicolor` icon folders or in `pixmaps`. A Flatpak's entry and icons lie in
//! the Flatpak export folders, named by its application id. Icon themes' redrawn versions
//! (Papirus, Breeze …) are not looked at: the point is the program's own picture.
//!
//! An AppImage has its icon inside itself: its own runtime is asked to unpack just that
//! ([`appimage_icon`]), once, into Meshgrove's cache. Windows and macOS are not looked at yet.

use std::path::{Path, PathBuf};

use crate::config::Program;

/// Where desktop entries and icons are looked for: the data folders, the user's first. A struct
/// so tests can point it at a temporary folder.
#[derive(Debug, Clone)]
pub struct Places {
    pub data_dirs: Vec<PathBuf>,
}

impl Places {
    /// The user's and the system's data folders (`XDG_DATA_HOME`, `XDG_DATA_DIRS`), with the
    /// Flatpak export folders, which are in those lists only inside a desktop session.
    pub fn real() -> Places {
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut add = |dir: PathBuf| {
            if !dir.as_os_str().is_empty() && !dirs.contains(&dir) {
                dirs.push(dir);
            }
        };
        let home = dirs::home_dir();
        if let Some(data) = dirs::data_dir() {
            add(data);
        }
        if let Some(home) = &home {
            add(home.join(".local/share"));
            add(home.join(".local/share/flatpak/exports/share"));
        }
        if let Some(list) = std::env::var_os("XDG_DATA_DIRS") {
            std::env::split_paths(&list).for_each(&mut add);
        }
        for dir in ["/var/lib/flatpak/exports/share", "/usr/local/share", "/usr/share"] {
            add(PathBuf::from(dir));
        }
        Places { data_dirs: dirs }
    }
}

/// One desktop entry, as far as it matters here.
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    /// The entry's file name without `.desktop` ("org.freecad.FreeCAD").
    id: String,
    /// The program its command starts, and how many more words the command has (a viewer
    /// started as `prusa-slicer --gcodeviewer` is not the slicer's own entry).
    command: PathBuf,
    extra_words: usize,
    icon: String,
    hidden: bool,
}

/// The words of an `Exec=` line: quotes taken off, field codes (`%F`) left out.
fn words(exec: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (_, '\\') => {
                if let Some(next) = chars.next() {
                    word.push(next);
                    started = true;
                }
            }
            (Some(q), c) if c == q => quote = None,
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if std::mem::take(&mut started) {
                    out.push(std::mem::take(&mut word));
                }
            }
            (_, c) => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(word);
    }
    out.retain(|w| !(w.len() == 2 && w.starts_with('%')));
    out
}

fn parse(id: &str, text: &str) -> Option<Entry> {
    let (mut exec, mut icon, mut hidden) = (None, None, false);
    let mut in_main = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            // Only the entry itself, not its extra actions ("New window").
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        if let Some(value) = line.strip_prefix("Exec=") {
            exec = Some(value.to_owned());
        } else if let Some(value) = line.strip_prefix("Icon=") {
            icon = Some(value.trim().to_owned());
        } else if line == "NoDisplay=true" || line == "Hidden=true" {
            hidden = true;
        }
    }
    let mut words = words(&exec?);
    // `env NAME=value program …` starts `program`.
    if words.first().is_some_and(|w| w == "env") {
        words.remove(0);
        while words.first().is_some_and(|w| w.contains('=') && !w.starts_with('/')) {
            words.remove(0);
        }
    }
    let command = PathBuf::from(words.first()?);
    let icon = icon.filter(|i| !i.is_empty())?;
    Some(Entry { id: id.to_owned(), command, extra_words: words.len() - 1, icon, hidden })
}

fn entries(places: &Places) -> Vec<Entry> {
    let mut out = Vec::new();
    for dir in &places.data_dirs {
        let Ok(files) = std::fs::read_dir(dir.join("applications")) else { continue };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().is_none_or(|x| x != "desktop") {
                continue;
            }
            let Some(id) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else { continue };
            // An entry of the user's hides the system's of the same name.
            if out.iter().any(|e: &Entry| e.id == id) {
                continue;
            }
            // Desktop entries are small; anything else under this name is not read.
            if file.metadata().is_ok_and(|m| m.len() > 256 * 1024) {
                continue;
            }
            if let Some(entry) = std::fs::read_to_string(&path).ok().and_then(|text| parse(&id, &text)) {
                out.push(entry);
            }
        }
    }
    out
}

/// Whether an entry's command is this program: the same file, or a bare name that is the
/// program's file name (the entry leaves the folder to `$PATH`).
fn starts(command: &Path, program: &Path) -> bool {
    if command.is_absolute() {
        return command == program
            || command.canonicalize().ok().zip(program.canonicalize().ok()).is_some_and(|(a, b)| a == b);
    }
    program.file_name().is_some_and(|name| name == command.as_os_str())
}

/// The names `program`'s icon may have: from the desktop entries whose command is the program
/// (the plainest one first, where several start it), then the program's own file name for a
/// Flatpak, whose start script is named by its application id like its icon.
fn icon_names(entries: &[Entry], program: &Program) -> Vec<String> {
    let mut names = Vec::new();
    let mut matching: Vec<&Entry> = entries.iter().filter(|e| starts(&e.command, &program.program)).collect();
    matching.sort_by_key(|e| (e.hidden, e.extra_words, e.id.len()));
    names.extend(matching.into_iter().map(|e| e.icon.clone()));
    if let Some(file) = program.program.file_name().map(|f| f.to_string_lossy().into_owned()) {
        // "org.freecad.FreeCAD": an application id.
        if file.matches('.').count() >= 2 && !names.contains(&file) {
            names.push(file);
        }
    }
    names
}

/// Sizes of the `hicolor` folders in the order they are wanted for a picture drawn at 16 to 32
/// points: large enough to stay sharp on a dense screen, not larger than needed.
const SIZES: [&str; 9] = ["128x128", "256x256", "96x96", "192x192", "64x64", "512x512", "72x72", "48x48", "scalable"];

/// The picture file of the icon `name`: the path itself where the entry gives one, else the
/// program's own picture in `hicolor` or `pixmaps`. PNG before SVG at the same place.
fn icon_path(places: &Places, name: &str) -> Option<PathBuf> {
    let direct = Path::new(name);
    if direct.is_absolute() {
        return direct.is_file().then(|| direct.to_path_buf());
    }
    // An icon name is a plain name: nothing that walks out of the icon folders.
    if name.contains(['/', '\\']) || name.starts_with('.') {
        return None;
    }
    let find = |dir: PathBuf| ["png", "svg"].iter().map(|ext| dir.join(format!("{name}.{ext}"))).find(|p| p.is_file());
    for size in SIZES {
        for dir in &places.data_dirs {
            if let Some(found) = find(dir.join("icons/hicolor").join(size).join("apps")) {
                return Some(found);
            }
        }
    }
    places.data_dirs.iter().find_map(|dir| find(dir.join("pixmaps")))
}

/// Finds programs' icons: the desktop entries are read once, when it is made.
#[derive(Debug, Clone)]
pub struct Finder {
    places: Places,
    entries: Vec<Entry>,
}

impl Finder {
    pub fn new(places: Places) -> Finder {
        let entries = entries(&places);
        Finder { places, entries }
    }

    /// For the programs of this computer. Only Linux is looked at so far: elsewhere nothing
    /// is found.
    pub fn real() -> Finder {
        if cfg!(target_os = "linux") {
            Finder::new(Places::real())
        } else {
            Finder::new(Places { data_dirs: Vec::new() })
        }
    }

    /// The picture file (PNG or SVG) of the icon `program` brings itself, if it has one: by
    /// its own desktop entry, else under one of the names its kind of program is known by
    /// (another install of the same program brought the picture then).
    pub fn icon_file(&self, program: &Program) -> Option<PathBuf> {
        let own = icon_names(&self.entries, program);
        let known = crate::programs::icon_names(&program.name);
        own.iter().map(String::as_str).chain(known).find_map(|name| icon_path(&self.places, name))
    }
}

/// [`Finder::icon_file`] in these places.
#[cfg(test)]
fn icon_file_in(places: &Places, program: &Program) -> Option<PathBuf> {
    Finder::new(places.clone()).icon_file(program)
}

/// Whether `file` is an AppImage (type 2): a program with the AppImage mark in its header.
pub fn is_appimage(file: &Path) -> bool {
    use std::io::Read;
    let mut header = [0u8; 16];
    std::fs::File::open(file).and_then(|mut f| f.read_exact(&mut header)).is_ok()
        && header.starts_with(b"\x7fELF")
        && &header[8..11] == b"AI\x02"
}

/// How long an AppImage's runtime gets to unpack one file.
const UNPACK_TIME: std::time::Duration = std::time::Duration::from_secs(20);
/// An icon larger than this is not one.
const ICON_SIZE: u64 = 8 << 20;

/// The icon an AppImage carries inside itself, as a picture file in `cache` (unpacked there the
/// first time, and again when the AppImage changed). `None` when it carries none; that is
/// remembered too, so the AppImage is not asked at every start.
///
/// The AppImage's own runtime does the unpacking (`--appimage-extract`), which starts the
/// runtime and not the program in it. This blocks for as long as that takes: call it off the
/// UI thread.
pub fn appimage_icon(file: &Path, cache: &Path) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let meta = file.metadata().ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (file, meta.len(), crate::db::nanos(meta.modified().ok())).hash(&mut hasher);
    let key = format!("{:016x}", hasher.finish());
    for ext in ["png", "svg"] {
        let cached = cache.join(format!("{key}.{ext}"));
        if cached.is_file() {
            return Some(cached);
        }
    }
    let none = cache.join(format!("{key}.none"));
    if none.exists() || !is_appimage(file) {
        return None;
    }
    std::fs::create_dir_all(cache).ok()?;
    let work = cache.join(format!("{key}.tmp"));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).ok()?;
    // The mark that points at the icon, and the pictures at the top that it may point at.
    for pattern in [".DirIcon", "*.png", "*.svg"] {
        unpack(file, &work, pattern);
    }
    let picked = unpacked_icon(&work.join("squashfs-root"));
    let kept = picked.and_then(|(picture, ext)| {
        let to = cache.join(format!("{key}.{ext}"));
        std::fs::copy(&picture, &to).ok().map(|_| to)
    });
    let _ = std::fs::remove_dir_all(&work);
    if kept.is_none() {
        let _ = std::fs::write(&none, "");
    }
    kept
}

/// Has the AppImage's runtime unpack what matches `pattern` into `work`; gives up after
/// [`UNPACK_TIME`].
fn unpack(file: &Path, work: &Path, pattern: &str) {
    use std::process::{Command, Stdio};
    let child = Command::new(file)
        .arg("--appimage-extract")
        .arg(pattern)
        .current_dir(work)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else { return };
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) if started.elapsed() > UNPACK_TIME => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(40)),
        }
    }
}

/// The icon among what was unpacked into `root`, and its kind: what `.DirIcon` is or points at
/// (a file beside it, nothing outside), else the largest picture at the top.
fn unpacked_icon(root: &Path) -> Option<(PathBuf, &'static str)> {
    let kind = |file: &Path| -> Option<&'static str> {
        use std::io::Read;
        let meta = file.metadata().ok()?;
        if !meta.is_file() || meta.len() == 0 || meta.len() > ICON_SIZE {
            return None;
        }
        let mut start = [0u8; 256];
        let read = std::fs::File::open(file).and_then(|mut f| f.read(&mut start)).ok()?;
        let start = &start[..read];
        if start.starts_with(b"\x89PNG") {
            Some("png")
        } else if String::from_utf8_lossy(start).contains("<svg") || file.extension().is_some_and(|x| x == "svg") {
            Some("svg")
        } else {
            None
        }
    };
    let mark = root.join(".DirIcon");
    let pointed = match std::fs::read_link(&mark) {
        // Only a plain name: a picture at the top of the AppImage, which was unpacked too.
        Ok(target) if target.components().count() == 1 => Some(root.join(target)),
        Ok(_) => None,
        Err(_) => Some(mark),
    };
    if let Some(found) = pointed.and_then(|file| kind(&file).map(|k| (file, k))) {
        return Some(found);
    }
    let mut pictures: Vec<(u64, PathBuf, &'static str)> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter_map(|item| {
            let path = item.path();
            // Not through a link: the picture itself.
            let meta = path.symlink_metadata().ok().filter(|m| m.is_file())?;
            kind(&path).map(|k| (meta.len(), path, k))
        })
        .collect();
    pictures.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    pictures.into_iter().next().map(|(_, path, kind)| (path, kind))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(path: &str) -> Program {
        Program { name: "x".into(), program: PathBuf::from(path), args: Vec::new(), env: Vec::new(), workdir: None }
    }

    /// A data folder with these files (path below it, content).
    fn data(files: &[(&str, &str)]) -> (tempfile::TempDir, Places) {
        let dir = tempfile::tempdir().unwrap();
        for (path, content) in files {
            let file = dir.path().join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, content).unwrap();
        }
        let places = Places { data_dirs: vec![dir.path().to_path_buf()] };
        (dir, places)
    }

    #[test]
    fn exec_lines_are_taken_apart() {
        assert_eq!(words("prusa-slicer %F"), ["prusa-slicer"]);
        assert_eq!(words(r#""/opt/My App/app" --new-window %U"#), ["/opt/My App/app", "--new-window"]);
        assert_eq!(words("env GDK_BACKEND=x11 cura %F"), ["env", "GDK_BACKEND=x11", "cura"]);
        let entry = parse("cura", "[Desktop Entry]\nExec=env GDK_BACKEND=x11 cura %F\nIcon=cura-icon\n").unwrap();
        assert_eq!((entry.command, entry.extra_words, entry.icon.as_str()), (PathBuf::from("cura"), 0, "cura-icon"));
        // An action's own command and icon are not the entry's.
        let with_action = "[Desktop Entry]\nExec=app\nIcon=app\n[Desktop Action New]\nExec=other\nIcon=other\n";
        assert_eq!(parse("app", with_action).unwrap().icon, "app");
        assert!(parse("empty", "[Desktop Entry]\nExec=app\n").is_none(), "no icon named");
    }

    /// The program's own entry wins over a viewer started with the same program, and its
    /// picture is taken from `hicolor`, a size that stays sharp, PNG before SVG.
    #[test]
    fn a_native_program_finds_its_own_icon() {
        let (dir, places) = data(&[
            (
                "applications/PrusaGcodeviewer.desktop",
                "[Desktop Entry]\nExec=prusa-slicer --gcodeviewer %F\nIcon=viewer\n",
            ),
            ("applications/PrusaSlicer.desktop", "[Desktop Entry]\nExec=prusa-slicer %F\nIcon=PrusaSlicer\n"),
            ("icons/hicolor/32x32/apps/PrusaSlicer.png", "small"),
            ("icons/hicolor/192x192/apps/PrusaSlicer.png", "large"),
            ("icons/hicolor/scalable/apps/PrusaSlicer.svg", "svg"),
            ("icons/hicolor/192x192/apps/viewer.png", "viewer"),
            // A theme's redrawn version is not the program's own.
            ("icons/Papirus/128x128/apps/PrusaSlicer.png", "theme"),
        ]);
        let found = icon_file_in(&places, &program("/usr/bin/prusa-slicer")).unwrap();
        assert_eq!(found, dir.path().join("icons/hicolor/192x192/apps/PrusaSlicer.png"));
    }

    #[test]
    fn a_flatpak_is_found_by_its_application_id_and_an_svg_will_do() {
        let (dir, places) = data(&[("icons/hicolor/scalable/apps/org.blender.Blender.svg", "svg")]);
        let flatpak = program("/var/lib/flatpak/exports/bin/org.blender.Blender");
        assert_eq!(
            icon_file_in(&places, &flatpak).unwrap(),
            dir.path().join("icons/hicolor/scalable/apps/org.blender.Blender.svg")
        );
    }

    /// An entry may name its picture by its path. Desktop entries are a Unix matter: their
    /// commands are Unix paths, which Windows does not take for whole paths (no drive).
    #[cfg(unix)]
    #[test]
    fn an_entry_may_name_its_picture_by_its_path() {
        let (dir, places) =
            data(&[("applications/solid.desktop", "[Desktop Entry]\nExec=/opt/solid/solid %f\nIcon=ICON\n")]);
        let picture = dir.path().join("solid.png");
        std::fs::write(&picture, "png").unwrap();
        let entry = dir.path().join("applications/solid.desktop");
        let text = std::fs::read_to_string(&entry).unwrap().replace("ICON", &picture.display().to_string());
        std::fs::write(&entry, text).unwrap();
        assert_eq!(icon_file_in(&places, &program("/opt/solid/solid")).unwrap(), picture);
    }

    /// A start script of the user's has no desktop entry of its own: it shows the icon the
    /// same program's Flatpak brought, found by the program's name in the settings.
    #[test]
    fn an_install_without_an_entry_takes_the_icon_of_its_kind() {
        let (dir, places) = data(&[("icons/hicolor/128x128/apps/com.anycubic.AnycubicSlicer.png", "png")]);
        let script =
            Program { name: "Anycubic Slicer Next".into(), ..program("/home/u/.local/bin/AnycubicSlicerNext") };
        assert_eq!(
            icon_file_in(&places, &script).unwrap(),
            dir.path().join("icons/hicolor/128x128/apps/com.anycubic.AnycubicSlicer.png")
        );
        let unknown = Program { name: "My own tool".into(), ..program("/home/u/bin/tool") };
        assert_eq!(icon_file_in(&places, &unknown), None);
    }

    /// What an AppImage's runtime unpacked: `.DirIcon` points at the picture, or is it, or
    /// there is only a picture; a link out of the folder is not followed.
    #[test]
    fn the_icon_is_picked_from_what_an_appimage_unpacked() {
        let png = b"\x89PNG\r\n\x1a\nrest";
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("squashfs-root");
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(unpacked_icon(&root), None, "nothing unpacked");
        std::fs::write(root.join("small.png"), png).unwrap();
        std::fs::write(root.join("App.png"), [png.as_slice(), &[0; 64]].concat()).unwrap();
        assert_eq!(unpacked_icon(&root), Some((root.join("App.png"), "png")), "the largest picture");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("small.png", root.join(".DirIcon")).unwrap();
            assert_eq!(unpacked_icon(&root), Some((root.join("small.png"), "png")), "what the mark points at");
            std::fs::remove_file(root.join(".DirIcon")).unwrap();
            std::os::unix::fs::symlink("../../secret.png", root.join(".DirIcon")).unwrap();
            std::fs::write(dir.path().join("secret.png"), png).unwrap();
            assert_eq!(unpacked_icon(&root), Some((root.join("App.png"), "png")), "never out of the folder");
            std::fs::remove_file(root.join(".DirIcon")).unwrap();
        }
        std::fs::write(root.join(".DirIcon"), "<svg xmlns='http://www.w3.org/2000/svg'/>").unwrap();
        assert_eq!(unpacked_icon(&root), Some((root.join(".DirIcon"), "svg")), "the mark is the picture");
        // Not an AppImage: nothing is started, nothing found.
        let plain = dir.path().join("tool");
        std::fs::write(&plain, "#!/bin/sh\n").unwrap();
        assert!(!is_appimage(&plain));
        assert_eq!(appimage_icon(&plain, &dir.path().join("cache")), None);
        let mut header = b"\x7fELF".to_vec();
        header.extend_from_slice(&[2, 1, 1, 0]);
        header.extend_from_slice(b"AI\x02");
        header.extend_from_slice(&[0; 8]);
        let marked = dir.path().join("App.AppImage");
        std::fs::write(&marked, header).unwrap();
        assert!(is_appimage(&marked));
    }

    /// A wrapper that only has the program among its arguments (a terminal that starts an
    /// AppImage in a container) is not the program's entry: its icon is the wrapper's.
    #[test]
    fn a_wrappers_entry_is_not_the_programs() {
        let (_dir, places) = data(&[
            (
                "applications/slicer-Orca.desktop",
                "[Desktop Entry]\nExec=/usr/bin/distrobox-enter -n slicer -- /home/u/Apps/Orca.AppImage\nIcon=utilities-terminal\n",
            ),
            ("icons/hicolor/128x128/apps/utilities-terminal.png", "terminal"),
        ]);
        assert_eq!(icon_file_in(&places, &program("/home/u/Apps/Orca.AppImage")), None);
        // And a name that tries to leave the icon folders finds nothing.
        assert_eq!(icon_path(&places, "../../etc/passwd"), None);
    }
}
