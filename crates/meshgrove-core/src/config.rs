//! User settings in `~/.config/meshgrove/config.toml`.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::Kind;

/// How to start a program with model files, `program [args…] files…`: the slicer or the
/// mesh editor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Program {
    pub name: String,
    pub program: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment; `LD_LIBRARY_PATH` is prepended to the inherited value.
    #[serde(default)]
    pub env: Vec<(String, String)>,
    pub workdir: Option<PathBuf>,
}

/// The slicer models are handed to.
pub type Slicer = Program;

impl Program {
    /// Blender opens only its own files; models go in through its importers.
    pub fn is_blender(&self) -> bool {
        self.program.file_stem().is_some_and(|n| n.to_string_lossy().to_lowercase().contains("blender"))
    }

    /// Whether this program, as the mesh editor, opens models of this kind.
    pub fn opens(&self, kind: Kind) -> bool {
        if self.is_blender() { matches!(kind, Kind::Stl | Kind::Obj | Kind::Ply | Kind::Gltf) } else { kind.is_model() }
    }
}

pub(crate) fn plain(name: &str, program: PathBuf) -> Program {
    Program { name: name.into(), program, args: Vec::new(), env: Vec::new(), workdir: None }
}

#[cfg(windows)]
pub(crate) fn program_folders() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> =
        ["ProgramFiles", "ProgramFiles(x86)"].into_iter().filter_map(std::env::var_os).map(PathBuf::from).collect();
    roots.extend(dirs::data_local_dir().map(|d| d.join("Programs")));
    // Autodesk Fusion installs per user, into %LOCALAPPDATA%\Autodesk\webdeploy\production\<version>\.
    roots.extend(dirs::data_local_dir().map(|d| d.join("Autodesk").join("webdeploy").join("production")));
    roots
}

#[cfg(target_os = "macos")]
pub(crate) fn app_folders() -> Vec<PathBuf> {
    let mut folders = vec![PathBuf::from("/Applications")];
    folders.extend(dirs::home_dir().map(|h| h.join("Applications")));
    folders
}

#[cfg(any(target_os = "linux", windows))]
pub(crate) fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let entries = std::fs::read_dir(dir).into_iter().flatten().flatten();
    entries.map(|e| e.path()).filter(|p| p.is_dir()).collect()
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

/// How much effort the large preview and the model windows put into the picture.
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

/// What a double-click (or Enter) on a model does. Models the chosen program does not open
/// go to a model window instead.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoubleClick {
    /// A window of its own with the 3D viewer and the facts.
    #[default]
    Window,
    /// The large preview over the main window (as Space).
    Preview,
    Slicer,
    Editor,
    /// The system's default application for the file type.
    DefaultApp,
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

/// How tags show on gallery tiles.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TileTags {
    /// Pills with the tag names.
    #[default]
    Names,
    /// Just a dot in each tag's colour (needs coloured tags).
    Dots,
}

/// Which files the gallery shows by their print log.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrintFilter {
    #[default]
    All,
    /// Models never printed successfully.
    Unprinted,
    Printed,
    /// Models whose newest print failed.
    Failed,
}

/// Files as tiles or as a table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GalleryView {
    #[default]
    Grid,
    List,
}

/// The list view's columns, by name: which are hidden and how wide the others were made.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ListColumns {
    pub hidden: Vec<String>,
    pub widths: std::collections::BTreeMap<String, f32>,
}

/// How many tags a tile can show.
pub const TILE_TAG_COUNT: std::ops::RangeInclusive<u8> = 1..=8;

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
    /// The slicers models can be sent to; `None`: take the ones found on this computer at startup.
    pub slicers: Option<Vec<Slicer>>,
    /// Name of the default slicer (double-click, the first "open in" entry); `None`: the first.
    pub default_slicer: Option<String>,
    /// The single slicer of earlier versions; moved into `slicers` when the file is read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slicer: Option<Slicer>,
    /// Printer for the bed check; `None`: take the one selected in the slicer.
    pub printer: Option<crate::printer::Bed>,
    /// What one glTF unit stands for.
    pub gltf_unit: GltfUnit,
    /// Picture quality of the large preview (Space) and the model windows.
    pub preview_quality: PreviewQuality,
    /// The large preview shows the file name, the viewer's tools and the hints; otherwise just
    /// the model and a button to show them.
    pub preview_controls: bool,
    /// What a double-click (or Enter) on a model does.
    pub double_click: DoubleClick,
    /// Model pictures on folder tiles.
    pub folder_previews: FolderPreviews,
    /// The panes of the main window can only be resized, not moved.
    pub lock_panes: bool,
    /// Mesh editors and CAD programs for "open in …"; `None`: take the ones found on this computer.
    pub editors: Option<Vec<Program>>,
    /// Name of the default editor (double-click); `None`: the first that opens the file.
    pub default_editor: Option<String>,
    /// The single editor of earlier versions; moved into `editors` when the file is read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editor: Option<Program>,
    /// Tags in their palette colours (otherwise grey) on tiles, in the details and the sidebar.
    pub colored_tags: bool,
    /// Tag pills on the gallery tiles.
    pub tags_on_tiles: bool,
    /// Names or colour dots on the tiles.
    pub tile_tags: TileTags,
    /// Tags a tile shows at most (the last row says "+N" when there are more).
    pub tile_tag_count: u8,
    /// Files shown by their print log (View menu).
    pub print_filter: PrintFilter,
    /// Tiles or a table.
    pub gallery_view: GalleryView,
    pub list_columns: ListColumns,
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
            slicers: None,
            default_slicer: None,
            slicer: None,
            printer: None,
            gltf_unit: GltfUnit::Metres,
            preview_quality: PreviewQuality::High,
            preview_controls: false,
            double_click: DoubleClick::Window,
            folder_previews: FolderPreviews::default(),
            lock_panes: false,
            editors: None,
            default_editor: None,
            editor: None,
            colored_tags: true,
            tags_on_tiles: true,
            tile_tags: TileTags::Names,
            tile_tag_count: 3,
            print_filter: PrintFilter::All,
            gallery_view: GalleryView::Grid,
            list_columns: ListColumns::default(),
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
            Ok(text) => {
                let mut config: Config = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
                config.migrate_programs();
                Ok(config)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Earlier versions kept one slicer in `slicer` and one editor in `editor`: they become the
    /// one entry of `slicers` and `editors`.
    fn migrate_programs(&mut self) {
        if let Some(old) = self.slicer.take()
            && self.slicers.is_none()
        {
            self.default_slicer = Some(old.name.clone());
            self.slicers = Some(vec![old]);
        }
        if let Some(old) = self.editor.take()
            && self.editors.is_none()
        {
            self.default_editor = Some(old.name.clone());
            self.editors = Some(vec![old]);
        }
    }

    /// The slicer to use by default among `slicers`.
    pub fn default_slicer_index(slicers: &[Slicer], default: Option<&str>) -> usize {
        default.and_then(|name| slicers.iter().position(|s| s.name == name)).unwrap_or(0)
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
    fn list_view_settings_survive_a_save() {
        let mut config = Config { gallery_view: GalleryView::List, ..Config::default() };
        config.list_columns.hidden.push("type".into());
        config.list_columns.widths.insert("name".into(), 312.5);
        let back: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
        assert_eq!(back, config);
        let old: Config = toml::from_str("tile_size = 120.0\n").unwrap();
        assert_eq!((old.gallery_view, old.list_columns), (GalleryView::Grid, ListColumns::default()));
    }

    #[test]
    fn blender_opens_only_what_it_imports() {
        let blender = plain("Blender", "/usr/bin/blender".into());
        assert!(blender.opens(Kind::Stl) && blender.opens(Kind::Gltf));
        assert!(!blender.opens(Kind::Step) && !blender.opens(Kind::ThreeMf));
        let freecad = plain("FreeCAD", "/usr/bin/freecad".into());
        assert!(!freecad.is_blender() && freecad.opens(Kind::Step) && freecad.opens(Kind::ThreeMf));
        assert!(!freecad.opens(Kind::Image));
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
    fn the_single_program_of_old_files_becomes_the_list() {
        let mut old: Config =
            toml::from_str("[slicer]\nname = \"OrcaSlicer\"\nprogram = \"/usr/bin/orca-slicer\"\n").unwrap();
        old.migrate_programs();
        assert!(old.slicer.is_none());
        let list = old.slicers.clone().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(Config::default_slicer_index(&list, old.default_slicer.as_deref()), 0);
        assert!(!toml::to_string_pretty(&old).unwrap().contains("[slicer]"), "not written again");
        let back: Config = toml::from_str(&toml::to_string_pretty(&old).unwrap()).unwrap();
        assert_eq!(back.slicers, old.slicers);
        let mut two = list.clone();
        two.push(Slicer { name: "PrusaSlicer".into(), ..list[0].clone() });
        assert_eq!(Config::default_slicer_index(&two, Some("PrusaSlicer")), 1);
        assert_eq!(Config::default_slicer_index(&two, Some("gone")), 0);
        assert!(Config::default().slicers.is_none(), "nothing chosen yet: detect at startup");
        let mut old: Config = toml::from_str("[editor]\nname = \"FreeCAD\"\nprogram = \"/usr/bin/freecad\"\n").unwrap();
        old.migrate_programs();
        assert!(old.editor.is_none());
        assert_eq!(old.editors.as_ref().map(Vec::len), Some(1));
        assert_eq!(old.default_editor.as_deref(), Some("FreeCAD"));
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
