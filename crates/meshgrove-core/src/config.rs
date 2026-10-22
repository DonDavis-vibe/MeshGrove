//! User settings in `~/.config/meshgrove/config.toml`.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

/// How to start the slicer with model files: `program [args…] files…`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Slicer {
    pub name: String,
    pub program: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment; `LD_LIBRARY_PATH` is prepended to the inherited value.
    #[serde(default)]
    pub env: Vec<(String, String)>,
    pub workdir: Option<PathBuf>,
}

impl Slicer {
    /// Finds an installed slicer: the user's Anycubic Slicer Next build first, then common
    /// Orca/Prusa/Bambu installs (distribution packages and Flatpaks).
    pub fn detect() -> Option<Slicer> {
        detect_slicer()
    }
}

fn plain(name: &str, program: PathBuf) -> Slicer {
    Slicer { name: name.into(), program, args: Vec::new(), env: Vec::new(), workdir: None }
}

#[cfg(target_os = "linux")]
fn detect_slicer() -> Option<Slicer> {
    let home = dirs::home_dir()?;
    let anycubic = home.join("opt/AnycubicSlicer");
    let anycubic_bin = anycubic.join("app/bin/AnycubicSlicerNext");
    if anycubic_bin.is_file() {
        return Some(Slicer {
            name: "Anycubic Slicer Next".into(),
            program: anycubic_bin,
            args: Vec::new(),
            env: vec![
                ("LD_LIBRARY_PATH".into(), anycubic.join("app/lib").to_string_lossy().into_owned()),
                // WebKitGTK renders a blank window on NVIDIA + Wayland without this.
                ("WEBKIT_DISABLE_DMABUF_RENDERER".into(), "1".into()),
            ],
            workdir: Some(anycubic),
        });
    }
    let flatpak = |id: &str| {
        ["/var/lib/flatpak/exports/bin", &format!("{}/.local/share/flatpak/exports/bin", home.display())]
            .iter()
            .map(|dir| Path::new(dir).join(id))
            .find(|p| p.exists())
    };
    let candidates = [
        ("OrcaSlicer", Some(PathBuf::from("/usr/bin/orca-slicer"))),
        ("OrcaSlicer", flatpak("io.github.softfever.OrcaSlicer")),
        ("PrusaSlicer", Some(PathBuf::from("/usr/bin/prusa-slicer"))),
        ("PrusaSlicer", flatpak("com.prusa3d.PrusaSlicer")),
        ("Bambu Studio", flatpak("com.bambulab.BambuStudio")),
    ];
    candidates.into_iter().find_map(|(name, program)| Some(plain(name, program.filter(|p| p.exists())?)))
}

#[cfg_attr(not(windows), allow(dead_code))]
const WINDOWS_PROGRAMS: &[(&str, &str)] = &[
    ("Anycubic Slicer Next", "anycubicslicernext.exe"),
    ("OrcaSlicer", "orca-slicer.exe"),
    ("Bambu Studio", "bambu-studio.exe"),
    ("PrusaSlicer", "prusa-slicer.exe"),
];

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const MACOS_APPS: &[(&str, &str)] = &[
    ("Anycubic Slicer Next", "anycubic"),
    ("OrcaSlicer", "orcaslicer"),
    ("Bambu Studio", "bambustudio"),
    ("PrusaSlicer", "prusaslicer"),
];

#[cfg(windows)]
fn detect_slicer() -> Option<Slicer> {
    let mut roots: Vec<PathBuf> =
        ["ProgramFiles", "ProgramFiles(x86)"].into_iter().filter_map(std::env::var_os).map(PathBuf::from).collect();
    roots.extend(dirs::data_local_dir().map(|d| d.join("Programs")));
    find_program(&roots, WINDOWS_PROGRAMS)
}

#[cfg(target_os = "macos")]
fn detect_slicer() -> Option<Slicer> {
    let mut folders = vec![PathBuf::from("/Applications")];
    folders.extend(dirs::home_dir().map(|h| h.join("Applications")));
    find_app_bundle(&folders, MACOS_APPS)
}

#[cfg_attr(not(windows), allow(dead_code))]
fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let entries = std::fs::read_dir(dir).into_iter().flatten().flatten();
    entries.map(|e| e.path()).filter(|p| p.is_dir()).collect()
}

/// Windows installers pick different folders, so programs (lower-case file names) are looked up
/// one and two levels below the program folders: `Program Files\OrcaSlicer\orca-slicer.exe`,
/// `Program Files\Prusa3D\PrusaSlicer\prusa-slicer.exe`.
#[cfg_attr(not(windows), allow(dead_code))]
fn find_program(roots: &[PathBuf], programs: &[(&str, &str)]) -> Option<Slicer> {
    let folders: Vec<PathBuf> = roots
        .iter()
        .flat_map(|root| subdirs(root))
        .flat_map(|dir| {
            let below = subdirs(&dir);
            std::iter::once(dir).chain(below)
        })
        .collect();
    programs.iter().find_map(|&(name, exe)| {
        folders.iter().find_map(|folder| {
            let mut entries = std::fs::read_dir(folder).into_iter().flatten().flatten();
            entries.find(|e| e.file_name().to_string_lossy().to_lowercase() == exe).map(|e| plain(name, e.path()))
        })
    })
}

/// macOS apps by bundle name (`OrcaSlicer.app`); `actions` starts a `.app` through `open -a`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn find_app_bundle(folders: &[PathBuf], apps: &[(&str, &str)]) -> Option<Slicer> {
    let bundles: Vec<PathBuf> = folders
        .iter()
        .flat_map(|f| std::fs::read_dir(f).into_iter().flatten().flatten().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "app"))
        .collect();
    apps.iter().find_map(|&(name, key)| {
        bundles.iter().find_map(|bundle| {
            let stem = bundle.file_stem()?.to_string_lossy().to_lowercase().replace([' ', '-', '_'], "");
            stem.contains(key).then(|| plain(name, bundle.clone()))
        })
    })
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn detect_slicer() -> Option<Slicer> {
    None
}

/// What one glTF unit stands for. The glTF standard says metres, but generated models (AI
/// tools, models normalised to ±1) and many hobby exports mean millimetres.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GltfUnit {
    #[default]
    Metres,
    Millimetres,
}

impl GltfUnit {
    /// Millimetres per glTF unit.
    pub fn scale(self) -> f32 {
        match self {
            GltfUnit::Metres => 1000.0,
            GltfUnit::Millimetres => 1.0,
        }
    }
}

/// The glTF unit of this process. The glTF loader and the thumbnail cache key read it; the app
/// and the thumbnailer set it from [`Config::gltf_unit`].
static GLTF_MILLIMETRES: AtomicBool = AtomicBool::new(false);

pub fn set_gltf_unit(unit: GltfUnit) {
    GLTF_MILLIMETRES.store(unit == GltfUnit::Millimetres, Ordering::Relaxed);
}

pub fn gltf_unit() -> GltfUnit {
    if GLTF_MILLIMETRES.load(Ordering::Relaxed) { GltfUnit::Millimetres } else { GltfUnit::Metres }
}

/// Gallery sort order (folders always come first).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    #[default]
    Name,
    Modified,
    Size,
    Type,
}

/// How much effort the large preview puts into the picture.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PreviewQuality {
    /// Drawn like the small viewer.
    Standard,
    /// Rendered at twice the resolution and scaled down: smooth edges.
    #[default]
    High,
    /// High, plus studio lighting with glossy highlights.
    Studio,
}

/// Pictures of the models inside a folder on the folder's tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FolderPreviews {
    /// Look into the folders on disk (costs some reading per folder).
    pub enabled: bool,
    /// How many folder levels down to look for models.
    pub depth: u8,
    /// With more than four models, slowly fade through them.
    pub cycle: bool,
}

impl Default for FolderPreviews {
    fn default() -> Self {
        FolderPreviews { enabled: false, depth: 2, cycle: true }
    }
}

pub const FOLDER_PREVIEW_DEPTH: std::ops::RangeInclusive<u8> = 1..=5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Folders indexed as the library and shown under "Bibliothek" in the sidebar.
    pub library_roots: Vec<PathBuf>,
    pub last_folder: Option<PathBuf>,
    /// Tile edge length in logical pixels.
    pub tile_size: f32,
    pub only_models: bool,
    pub sort_by: SortKey,
    pub sort_descending: bool,
    /// "de" / "en"; `None` follows the system language.
    pub language: Option<String>,
    /// `None`: detect an installed slicer at startup.
    pub slicer: Option<Slicer>,
    /// Printer for the bed check; `None`: take the one selected in the slicer.
    pub printer: Option<crate::printer::Bed>,
    /// What one glTF unit stands for.
    pub gltf_unit: GltfUnit,
    /// Picture quality of the large preview (Space).
    pub preview_quality: PreviewQuality,
    /// Model pictures on folder tiles.
    pub folder_previews: FolderPreviews,
    /// The panes of the main window can only be resized, not moved.
    pub lock_panes: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            library_roots: Vec::new(),
            last_folder: None,
            tile_size: 160.0,
            only_models: false,
            sort_by: SortKey::Name,
            sort_descending: false,
            language: None,
            slicer: None,
            printer: None,
            gltf_unit: GltfUnit::Metres,
            preview_quality: PreviewQuality::High,
            folder_previews: FolderPreviews::default(),
            lock_panes: false,
        }
    }
}

/// The project's name before it became Meshgrove; its folders are taken over once.
const LEGACY_NAME: &str = "stl-browser";

/// Moves settings, library index, backups and caches from the folders of the former name
/// (`~/.config/stl-browser`, …) to Meshgrove's, unless Meshgrove's already exist.
pub fn migrate_legacy_dirs() {
    for base in [dirs::config_dir(), dirs::data_dir(), dirs::cache_dir()].into_iter().flatten() {
        migrate_dir(&base.join(LEGACY_NAME), &base.join("meshgrove"));
    }
}

fn migrate_dir(old: &Path, new: &Path) {
    if old.is_dir() && !new.exists() {
        // Another process (e.g. a thumbnailer) may win the race; then there is nothing left to do.
        let _ = std::fs::rename(old, new);
    }
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        Some(dirs::config_dir()?.join("meshgrove").join("config.toml"))
    }

    /// Missing file → defaults. A broken file is reported so the caller can warn instead of overwriting silently.
    pub fn load() -> Result<Config, String> {
        let Some(path) = Self::path() else {
            return Ok(Config::default());
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    pub fn save(&self) -> io::Result<()> {
        let path = Self::path().ok_or_else(|| io::Error::other("no config directory"))?;
        std::fs::create_dir_all(path.parent().expect("config file has a parent"))?;
        std::fs::write(path, toml::to_string_pretty(self).map_err(io::Error::other)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_windows_programs_and_mac_bundles() {
        let dir = tempfile::tempdir().unwrap();
        let programs = dir.path().join("Program Files");
        std::fs::create_dir_all(programs.join("Prusa3D/PrusaSlicer")).unwrap();
        std::fs::write(programs.join("Prusa3D/PrusaSlicer/prusa-slicer.exe"), b"").unwrap();
        std::fs::create_dir_all(programs.join("OrcaSlicer")).unwrap();
        std::fs::write(programs.join("OrcaSlicer/Orca-Slicer.exe"), b"").unwrap();
        let found = find_program(std::slice::from_ref(&programs), WINDOWS_PROGRAMS).unwrap();
        assert_eq!(found.name, "OrcaSlicer", "earlier entries win, file names compare case-insensitively");
        assert_eq!(found.program, programs.join("OrcaSlicer/Orca-Slicer.exe"));

        let apps = dir.path().join("Applications");
        std::fs::create_dir_all(apps.join("Bambu Studio.app")).unwrap();
        std::fs::create_dir_all(apps.join("Safari.app")).unwrap();
        let found = find_app_bundle(std::slice::from_ref(&apps), MACOS_APPS).unwrap();
        assert_eq!((found.name.as_str(), found.program), ("Bambu Studio", apps.join("Bambu Studio.app")));
    }

    #[test]
    fn legacy_folders_move_once() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("stl-browser"), dir.path().join("meshgrove"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("library.db"), b"db").unwrap();
        migrate_dir(&old, &new);
        assert_eq!(std::fs::read(new.join("library.db")).unwrap(), b"db");
        assert!(!old.exists());
        // An existing Meshgrove folder is never replaced.
        std::fs::create_dir_all(&old).unwrap();
        migrate_dir(&old, &new);
        assert!(old.exists() && new.join("library.db").exists());
    }

    #[test]
    fn partial_files_fill_in_defaults() {
        let cfg: Config = toml::from_str("library_roots = [\"/data/3D_druck\"]\n").unwrap();
        assert_eq!(cfg.library_roots, vec![PathBuf::from("/data/3D_druck")]);
        assert_eq!(cfg.tile_size, 160.0);
        assert_eq!(cfg.gltf_unit, GltfUnit::Metres);
        let mm: Config = toml::from_str("gltf_unit = \"millimetres\"\n").unwrap();
        assert_eq!(mm.gltf_unit.scale(), 1.0);
        let text = toml::to_string_pretty(&cfg).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), cfg);
    }
}
