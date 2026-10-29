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

/// What one coordinate unit of STL, OBJ and PLY files stands for. These formats carry no unit;
/// 3D-printing files mean millimetres, but some (mostly American) models were drawn in inches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelUnits {
    #[default]
    Millimetres,
    Inches,
}

impl ModelUnits {
    /// Millimetres per unit.
    pub fn scale(self) -> f32 {
        match self {
            ModelUnits::Millimetres => 1.0,
            ModelUnits::Inches => MM_PER_INCH,
        }
    }
}

/// Millimetres in an inch.
pub const MM_PER_INCH: f32 = 25.4;

/// The unit of the STL, OBJ and PLY coordinates of this process. The loaders and the thumbnail
/// cache key read it; the app and the thumbnailer set it from [`Config::model_units`].
static MODEL_INCHES: AtomicBool = AtomicBool::new(false);

pub fn set_model_units(units: ModelUnits) {
    MODEL_INCHES.store(units == ModelUnits::Inches, Ordering::Relaxed);
}

pub fn model_units() -> ModelUnits {
    if MODEL_INCHES.load(Ordering::Relaxed) { ModelUnits::Inches } else { ModelUnits::Millimetres }
}

/// The unit sizes and distances are shown in. Models and the library stay in millimetres.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DisplayUnits {
    #[default]
    Metric,
    Imperial,
}

static DISPLAY_IMPERIAL: AtomicBool = AtomicBool::new(false);

/// Sets the display unit of this process (read by [`display_units`] where sizes are formatted).
pub fn set_display_units(units: DisplayUnits) {
    DISPLAY_IMPERIAL.store(units == DisplayUnits::Imperial, Ordering::Relaxed);
}

pub fn display_units() -> DisplayUnits {
    if DISPLAY_IMPERIAL.load(Ordering::Relaxed) { DisplayUnits::Imperial } else { DisplayUnits::Metric }
}

impl DisplayUnits {
    /// A length in millimetres as a number of this unit.
    pub fn from_mm(self, mm: f32) -> f32 {
        match self {
            DisplayUnits::Metric => mm,
            DisplayUnits::Imperial => mm / MM_PER_INCH,
        }
    }

    /// A length in this unit as millimetres.
    pub fn to_mm(self, value: f32) -> f32 {
        match self {
            DisplayUnits::Metric => value,
            DisplayUnits::Imperial => value * MM_PER_INCH,
        }
    }

    /// "mm" or "in".
    pub fn label(self) -> &'static str {
        match self {
            DisplayUnits::Metric => "mm",
            DisplayUnits::Imperial => "in",
        }
    }
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

/// Tile edge lengths the gallery offers.
pub const TILE_SIZE: std::ops::RangeInclusive<f32> = 96.0..=384.0;

/// Lines of the file name below a tile's picture.
pub const NAME_LINES: std::ops::RangeInclusive<u8> = 0..=2;

/// The marks a gallery tile can carry over its picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TileBadge {
    /// "STL", "3MF" …
    FileType,
    /// The filament colours of a slicer project.
    Filaments,
    /// "2×": the file is in the library more than once.
    Duplicates,
    Tags,
    /// How the model fits the printers and what the printability check found (↻, ✕, ⚠).
    Check,
    /// The favourite star.
    Favorite,
    /// Copies done of a print list, or the print log (✓2, ✗).
    PrintState,
}

impl TileBadge {
    pub const ALL: [TileBadge; 7] = [
        TileBadge::FileType,
        TileBadge::Filaments,
        TileBadge::Duplicates,
        TileBadge::Tags,
        TileBadge::Check,
        TileBadge::Favorite,
        TileBadge::PrintState,
    ];

    /// The name in the settings file.
    pub fn name(self) -> &'static str {
        match self {
            TileBadge::FileType => "file_type",
            TileBadge::Filaments => "filaments",
            TileBadge::Duplicates => "duplicates",
            TileBadge::Tags => "tags",
            TileBadge::Check => "check",
            TileBadge::Favorite => "favorite",
            TileBadge::PrintState => "print_state",
        }
    }

    pub fn from_name(name: &str) -> Option<TileBadge> {
        TileBadge::ALL.into_iter().find(|b| b.name() == name)
    }
}

/// Where on a tile's picture a mark sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TileSlot {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Hidden,
}

impl TileSlot {
    pub const CORNERS: [TileSlot; 4] =
        [TileSlot::TopLeft, TileSlot::TopRight, TileSlot::BottomLeft, TileSlot::BottomRight];

    pub fn is_top(self) -> bool {
        matches!(self, TileSlot::TopLeft | TileSlot::TopRight)
    }

    pub fn is_left(self) -> bool {
        matches!(self, TileSlot::TopLeft | TileSlot::BottomLeft)
    }
}

/// How the filament colours show on a tile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilamentStyle {
    /// A row of dots in the corner.
    #[default]
    Dots,
    /// A thin band along the top or bottom edge of the picture.
    Stripe,
}

/// How the print bed and printability marks read on a tile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStyle {
    /// "↻ 90°", "✕ too big", "⚠ 🖨"
    #[default]
    Text,
    /// Just ↻, ✕ or ⚠ (the tooltip says why).
    Symbol,
}

/// What the gallery tiles show and where: the marks over the picture by corner, each list from
/// the corner inwards, how some of them look, and the name below the picture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TileLayout {
    #[serde(deserialize_with = "known_badges")]
    pub top_left: Vec<TileBadge>,
    #[serde(deserialize_with = "known_badges")]
    pub top_right: Vec<TileBadge>,
    #[serde(deserialize_with = "known_badges")]
    pub bottom_left: Vec<TileBadge>,
    #[serde(deserialize_with = "known_badges")]
    pub bottom_right: Vec<TileBadge>,
    #[serde(deserialize_with = "known_badges")]
    pub hidden: Vec<TileBadge>,
    pub filaments: FilamentStyle,
    pub check: CheckStyle,
    /// Lines of the file name ([`NAME_LINES`]; 0: none).
    pub name_lines: u8,
    /// The name keeps its ".stl", ".3mf" …
    pub name_extension: bool,
}

impl Default for TileLayout {
    /// The tiles as they always looked.
    fn default() -> Self {
        TileLayout {
            top_left: vec![TileBadge::Filaments],
            top_right: vec![TileBadge::Duplicates, TileBadge::Tags],
            bottom_left: vec![TileBadge::FileType],
            bottom_right: vec![TileBadge::Check, TileBadge::Favorite, TileBadge::PrintState],
            hidden: Vec::new(),
            filaments: FilamentStyle::Dots,
            check: CheckStyle::Text,
            name_lines: 2,
            name_extension: true,
        }
    }
}

/// Mark names as written in the file; names this version does not know are skipped.
fn known_badges<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<TileBadge>, D::Error> {
    Ok(Vec::<String>::deserialize(d)?.iter().filter_map(|name| TileBadge::from_name(name)).collect())
}

impl TileLayout {
    /// The marks of a corner (or the hidden ones), from the corner inwards.
    pub fn slot(&self, slot: TileSlot) -> &[TileBadge] {
        match slot {
            TileSlot::TopLeft => &self.top_left,
            TileSlot::TopRight => &self.top_right,
            TileSlot::BottomLeft => &self.bottom_left,
            TileSlot::BottomRight => &self.bottom_right,
            TileSlot::Hidden => &self.hidden,
        }
    }

    fn slot_mut(&mut self, slot: TileSlot) -> &mut Vec<TileBadge> {
        match slot {
            TileSlot::TopLeft => &mut self.top_left,
            TileSlot::TopRight => &mut self.top_right,
            TileSlot::BottomLeft => &mut self.bottom_left,
            TileSlot::BottomRight => &mut self.bottom_right,
            TileSlot::Hidden => &mut self.hidden,
        }
    }

    /// Where a mark sits.
    pub fn slot_of(&self, badge: TileBadge) -> TileSlot {
        TileSlot::CORNERS.into_iter().find(|&s| self.slot(s).contains(&badge)).unwrap_or(TileSlot::Hidden)
    }

    pub fn shows(&self, badge: TileBadge) -> bool {
        self.slot_of(badge) != TileSlot::Hidden
    }

    /// Moves a mark to `index` of `slot` (counted without the mark itself; past the end: last).
    pub fn place(&mut self, badge: TileBadge, slot: TileSlot, index: usize) {
        for s in TileSlot::CORNERS.into_iter().chain([TileSlot::Hidden]) {
            self.slot_mut(s).retain(|&b| b != badge);
        }
        let list = self.slot_mut(slot);
        list.insert(index.min(list.len()), badge);
    }

    /// Every mark exactly once: repeats are dropped, and marks the file does not mention (new in
    /// this version) go where they sit by default.
    pub fn normalize(&mut self) {
        let mut seen = std::collections::HashSet::new();
        for s in TileSlot::CORNERS.into_iter().chain([TileSlot::Hidden]) {
            self.slot_mut(s).retain(|&b| seen.insert(b));
        }
        let default = TileLayout::default();
        for badge in TileBadge::ALL {
            if !seen.contains(&badge) {
                self.slot_mut(default.slot_of(badge)).push(badge);
            }
        }
        self.name_lines = self.name_lines.min(*NAME_LINES.end());
    }
}

/// Optional AI. Meshgrove brings none of its own: the user's assistant reaches the library
/// through `meshgrove-mcp`, or Meshgrove talks to an endpoint the user set up. Off by default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiConfig {
    pub enabled: bool,
    /// What AI may see of each library; a library not listed here is hidden.
    pub libraries: Vec<LibraryAi>,
    /// Days the access log is kept.
    pub log_days: u16,
    /// Days an undecided suggestion waits before it expires.
    pub expire_days: u16,
    /// The user's own endpoint for the chat and the one-click actions; `None`: none set up.
    pub endpoint: Option<EndpointConfig>,
    /// Whether the chat's changes wait for review or apply at once (and can be undone).
    pub chat_mode: ChatMode,
    /// While AI is on, Meshgrove adds itself to the AI assistants installed on this computer
    /// (and takes itself out again when this or AI is turned off).
    pub connect_apps: bool,
    /// Since when (Unix seconds) suggestions are accepted without the review: those of the AI
    /// apps and of the one-click actions, never moves; the chat has its own switch. `None`: all
    /// wait for the user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_accept_since: Option<u64>,
}

impl Default for AiConfig {
    fn default() -> AiConfig {
        AiConfig {
            enabled: false,
            libraries: Vec::new(),
            log_days: 90,
            expire_days: 30,
            endpoint: None,
            chat_mode: ChatMode::Review,
            connect_apps: true,
            auto_accept_since: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatMode {
    /// Suggestions wait in the review.
    #[default]
    Review,
    /// Changes apply at once; the arrow at the user's message takes them back.
    Auto,
}

/// An OpenAI-compatible endpoint the user runs or subscribes to (Ollama, LM Studio, OpenAI,
/// OpenRouter …). Its key is not kept here but in the system keyring or an environment variable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EndpointConfig {
    /// "ollama", "lmstudio", "openai", "openrouter" or "custom": where the URL came from.
    pub preset: String,
    /// Up to and including `/v1`.
    pub base_url: String,
    pub model: String,
    /// It runs on this computer or network (the user says so; guessed from the URL first).
    pub local: bool,
    /// The model understands pictures.
    pub vision: bool,
    /// The model can call tools (search the library itself).
    pub tools: bool,
    /// The key is in the system keyring.
    pub key_in_keyring: bool,
    /// Or in this environment variable.
    pub key_env: String,
    pub timeout_s: u32,
}

impl Default for EndpointConfig {
    fn default() -> EndpointConfig {
        EndpointConfig {
            preset: "ollama".into(),
            base_url: "http://localhost:11434/v1".into(),
            model: String::new(),
            local: true,
            vision: false,
            tools: true,
            key_in_keyring: false,
            key_env: String::new(),
            timeout_s: 120,
        }
    }
}

impl AiConfig {
    /// How far AI may look into the library at `root`.
    pub fn library(&self, root: &Path) -> LibraryAi {
        self.libraries.iter().find(|l| l.root == root).cloned().unwrap_or(LibraryAi {
            root: root.to_path_buf(),
            access: AiAccess::None,
            images: false,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryAi {
    pub root: PathBuf,
    #[serde(default)]
    pub access: AiAccess,
    /// Thumbnails may be sent along (to a model that can see).
    #[serde(default)]
    pub images: bool,
}

/// Which AI may see a library.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiAccess {
    #[default]
    None,
    /// Only AI running on this computer or network.
    LocalOnly,
    /// Also AI services on the internet.
    Cloud,
}

/// Where the AI asking runs: the user says so (`meshgrove-mcp --local`, the endpoint's "local").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessClass {
    Local,
    Cloud,
}

impl AccessClass {
    pub fn as_str(self) -> &'static str {
        match self {
            AccessClass::Local => "local",
            AccessClass::Cloud => "cloud",
        }
    }
}

impl AiAccess {
    pub fn allows(self, class: AccessClass) -> bool {
        match self {
            AiAccess::None => false,
            AiAccess::LocalOnly => class == AccessClass::Local,
            AiAccess::Cloud => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Folders indexed as the library and shown under "Bibliothek" in the sidebar.
    pub library_roots: Vec<PathBuf>,
    pub last_folder: Option<PathBuf>,
    /// Tile edge length in logical pixels.
    pub tile_size: f32,
    /// How many of the models last sent to the slicer the sidebar lists ("Zuletzt gedruckt");
    /// 0: none.
    pub recent_prints: u16,
    pub only_models: bool,
    pub sort_by: SortKey,
    pub sort_descending: bool,
    /// Files Meshgrove shows (models, pictures, PDFs, archives, texts) before the others.
    pub supported_first: bool,
    /// "de" / "en"; `None` follows the system language.
    pub language: Option<String>,
    /// The slicers models can be sent to; `None`: take the ones found on this computer at startup.
    pub slicers: Option<Vec<Slicer>>,
    /// Name of the default slicer (double-click, the first "open in" entry); `None`: the first.
    pub default_slicer: Option<String>,
    /// The single slicer of earlier versions; moved into `slicers` when the file is read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slicer: Option<Slicer>,
    /// The single printer of earlier versions; moved into `printers` when the file is read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub printer: Option<crate::printer::Bed>,
    /// The printers models are checked against; `None`: take the ones the slicers know, at startup.
    pub printers: Option<Vec<crate::printer::Printer>>,
    /// Name of the main printer (shown first, the bed of the 3D view); `None`: the first.
    pub default_printer: Option<String>,
    /// What one coordinate unit of STL, OBJ and PLY files stands for.
    pub model_units: ModelUnits,
    /// Metric or imperial sizes in the interface.
    pub display_units: DisplayUnits,
    /// What one glTF unit stands for.
    pub gltf_unit: GltfUnit,
    /// Free orbit in the 3D views: also turn about the view axis and over the poles.
    pub free_orbit: bool,
    /// 3D mouse (SpaceMouse) navigation.
    pub spacemouse: crate::spacemouse::SpaceMouseConfig,
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
    /// The sidebar lists the inbox of new files.
    pub inbox: bool,
    /// Folders outside the libraries (a downloads folder) whose new 3D-print files Meshgrove
    /// offers to bring in, and where they land.
    pub import: ImportConfig,
    /// Placing files into a folder by their tags.
    pub placement: PlacementConfig,
    /// Optional AI through the user's own assistant (MCP) or endpoint.
    pub ai: AiConfig,
    /// Mesh editors and CAD programs for "open in …"; `None`: take the ones found on this computer.
    pub editors: Option<Vec<Program>>,
    /// Name of the default editor (double-click); `None`: the first that opens the file.
    pub default_editor: Option<String>,
    /// The single editor of earlier versions; moved into `editors` when the file is read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editor: Option<Program>,
    /// Tags in their palette colours (otherwise grey) on tiles, in the details and the sidebar.
    pub colored_tags: bool,
    /// Tag pills on the gallery tiles, as earlier versions wrote it; `tile_layout` says it now.
    #[serde(skip_serializing)]
    pub tags_on_tiles: Option<bool>,
    /// The theme tags offered in the tag menu; `None`: the built-in set ([`default_themes`]).
    pub themes: Option<Vec<SuggestedTag>>,
    /// The print-property tags offered; `None`: [`default_properties`].
    pub properties: Option<Vec<SuggestedTag>>,
    /// The state-and-origin tags offered; `None`: [`default_states`].
    pub states: Option<Vec<SuggestedTag>>,
    /// The sidebar lists all tags alphabetically instead of grouped (themes, properties, state, own).
    pub flat_tag_list: bool,
    /// Sidebar sections and tag groups folded away: "lists", "tags", "devices", "tags:themes" …
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sidebar_collapsed: Vec<String>,
    /// Settings sections folded away: "spacemouse", "ailog" …
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub settings_folded: Vec<String>,
    /// Existing tags stay listed below the tag field while nothing is typed.
    pub tag_chips_idle: bool,
    /// Names or colour dots on the tiles.
    pub tile_tags: TileTags,
    /// Tags a tile shows at most (the last row says "+N" when there are more).
    pub tile_tag_count: u8,
    /// Which marks the tiles show where, and their names.
    pub tile_layout: TileLayout,
    /// Files shown by their print log (View menu).
    pub print_filter: PrintFilter,
    /// Tiles or a table.
    pub gallery_view: GalleryView,
    pub list_columns: ListColumns,
}

/// Bringing new files from outside (a downloads folder) into a library.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportConfig {
    /// The folders looked at (their top level only); none: no import.
    pub folders: Vec<PathBuf>,
    /// The library the files go to; `None`: the first one.
    pub library: Option<PathBuf>,
    /// The folder below the library root they land in.
    pub landing: String,
    pub at_start: ImportAtStart,
    pub archives: ArchiveImport,
}

impl Default for ImportConfig {
    fn default() -> ImportConfig {
        ImportConfig {
            folders: Vec::new(),
            library: None,
            landing: Config::LANDING.to_owned(),
            at_start: ImportAtStart::Ask,
            archives: ArchiveImport::Whole,
        }
    }
}

/// What happens with new files in the import folders when Meshgrove starts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportAtStart {
    Off,
    /// Say how many there are; the user imports them from the import view.
    #[default]
    Ask,
    /// Bring them in without asking.
    Auto,
}

/// What an import does with an archive.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveImport {
    /// Moved as it is; Meshgrove browses inside.
    #[default]
    Whole,
    /// Unpacked into a folder of its name; the archive goes to the trash.
    Extract,
}

/// Placing files into a folder by their tags: a theme's folder is one of its name below the
/// library root unless a rule says otherwise; own tags need a rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlacementConfig {
    pub enabled: bool,
    pub rules: Vec<FolderRule>,
}

impl Default for PlacementConfig {
    fn default() -> PlacementConfig {
        PlacementConfig { enabled: true, rules: Vec::new() }
    }
}

/// The folder (`/`-separated, below the library root) files with `tag` go to; empty: none,
/// which switches a theme's own folder off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderRule {
    pub tag: String,
    pub folder: String,
}

fn same_tag(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

impl Config {
    /// The landing folder's name when the setting is empty or no file name.
    pub const LANDING: &str = "Import";

    /// Whether suggested tags (and their folders) are given in German.
    pub fn german(&self) -> bool {
        self.language.as_deref().is_none_or(|l| l.starts_with("de"))
    }

    /// The name of the folder imported files land in, below the library root.
    pub fn landing_name(&self) -> &str {
        let name = self.import.landing.trim();
        if name.is_empty() || crate::fsops::check_name(name).is_err() { Config::LANDING } else { name }
    }

    /// The library imports go to.
    pub fn import_root(&self) -> Option<&Path> {
        match &self.import.library {
            Some(l) if self.library_roots.contains(l) => Some(l.as_path()),
            _ => self.library_roots.first().map(PathBuf::as_path),
        }
    }

    /// Where imported files land.
    pub fn landing_dir(&self) -> Option<PathBuf> {
        self.import_root().map(|r| r.join(self.landing_name()))
    }

    /// The landing folder of every library (the files there wait to be placed).
    pub fn landing_folders(&self) -> Vec<PathBuf> {
        self.library_roots.iter().map(|r| r.join(self.landing_name())).collect()
    }

    /// Whether `path` lies in a landing folder (not the folder itself).
    pub fn in_landing(&self, path: &Path) -> bool {
        self.landing_folders().iter().any(|l| path != l && path.starts_with(l))
    }

    /// The folder (`/`-separated, below the library root) files with `tag` go to: by an
    /// explicit rule (an empty one switches the tag off), else a theme's name in the user's
    /// language. `None` for every other tag.
    pub fn folder_for_tag(&self, tag: &str) -> Option<String> {
        let themes = self.themes();
        let theme = themes.iter().find(|t| t.matches(tag.trim()));
        let rule = self
            .placement
            .rules
            .iter()
            .find(|r| same_tag(&r.tag, tag) || theme.is_some_and(|t| t.matches(r.tag.trim())));
        if let Some(rule) = rule {
            let folder = rule.folder.trim();
            return (!folder.is_empty()).then(|| folder.to_owned());
        }
        theme.map(|t| if self.german() { t.de.clone() } else { t.en.clone() })
    }

    /// Every tag with a folder, in the order of the settings: the themes as listed, then the
    /// rules for other tags.
    pub fn folder_rules(&self) -> Vec<(String, String)> {
        let themes = self.themes();
        let mut out = Vec::new();
        for t in &themes {
            let name = if self.german() { &t.de } else { &t.en };
            if let Some(folder) = self.folder_for_tag(name) {
                out.push((name.clone(), folder));
            }
        }
        for r in &self.placement.rules {
            let folder = r.folder.trim();
            if !folder.is_empty() && !themes.iter().any(|t| t.matches(r.tag.trim())) {
                out.push((r.tag.trim().to_owned(), folder.to_owned()));
            }
        }
        out
    }
}

/// A tag the user can pick from the tag menu (a theme, a print property or a state): its name in
/// both languages and its palette slot (`None`: a colour from the name).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestedTag {
    pub de: String,
    pub en: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u8>,
}

impl SuggestedTag {
    /// A theme with one name for both languages.
    pub fn named(name: &str) -> SuggestedTag {
        SuggestedTag { de: name.to_owned(), en: name.to_owned(), color: None }
    }

    /// Whether `tag` is this theme's German or English name.
    pub fn matches(&self, tag: &str) -> bool {
        self.de.to_lowercase() == tag.to_lowercase() || self.en.to_lowercase() == tag.to_lowercase()
    }
}

/// The built-in themes: (German, English, palette slot).
const DEFAULT_THEMES: &[(&str, &str, u8)] = &[
    ("Haushalt", "Household", 1),
    ("Küche", "Kitchen", 2),
    ("Bad", "Bathroom", 5),
    ("Wohnzimmer", "Living room", 2),
    ("Schlafzimmer", "Bedroom", 9),
    ("Garten", "Garden", 4),
    ("Balkon", "Balcony", 3),
    ("Werkstatt", "Workshop", 1),
    ("Werkzeug", "Tools", 8),
    ("Keller", "Basement", 7),
    ("Aufbewahrung", "Storage", 6),
    ("Büro", "Office", 8),
    ("Elektronik", "Electronics", 7),
    ("Smart Home", "Smart home", 5),
    ("Raspberry Pi", "Raspberry Pi", 0),
    ("Beleuchtung", "Lighting", 3),
    ("Kabelmanagement", "Cable management", 7),
    ("Drucker-Zubehör", "Printer upgrade", 8),
    ("Filament", "Filament", 6),
    ("Kinderzimmer", "Kids' room", 11),
    ("Spielzeug", "Toys", 10),
    ("Hobby", "Hobby", 9),
    ("Modellbau", "Model building", 9),
    ("Tabletop", "Tabletop", 9),
    ("Rollenspiel", "RPG", 10),
    ("Miniatur", "Miniature", 10),
    ("Cosplay", "Cosplay", 11),
    ("Airsoft", "Airsoft", 3),
    ("Musik", "Music", 10),
    ("Sport", "Sport", 4),
    ("Outdoor", "Outdoor", 4),
    ("Camping", "Camping", 3),
    ("Angeln", "Fishing", 6),
    ("Drohne", "Drone", 7),
    ("Haustier", "Pets", 1),
    ("Auto", "Car", 8),
    ("Fahrrad", "Bike", 6),
    ("Deko", "Decor", 10),
    ("Schmuck", "Jewelry", 11),
    ("Geschenk", "Gift", 11),
    ("Weihnachten", "Christmas", 0),
    ("Ostern", "Easter", 3),
    ("Halloween", "Halloween", 1),
    ("Lernen", "Education", 3),
    ("Ersatzteil", "Spare part", 7),
    ("Kalibrierung", "Calibration", 5),
];

/// The built-in print properties: (German, English, palette slot).
const DEFAULT_PROPERTIES: &[(&str, &str, u8)] = &[
    ("mehrfarbig", "multicolour", 10),
    ("Print-in-Place", "print-in-place", 5),
    ("ohne Stützen", "no supports", 4),
    ("braucht Stützen", "needs supports", 0),
    ("mehrteilig", "multi-part", 6),
    ("Funktionsteil", "functional", 8),
    ("beweglich", "articulated", 9),
    ("Gewinde", "threads", 7),
    ("Magnete", "magnets", 2),
    ("Gewindeeinsätze", "heat-set inserts", 1),
    ("Vasenmodus", "vase mode", 6),
    ("PLA", "PLA", 4),
    ("PETG", "PETG", 7),
    ("TPU", "TPU", 10),
    ("ASA", "ASA", 1),
    ("Resin", "resin", 9),
];

/// The built-in states and origins: (German, English, palette slot).
const DEFAULT_STATES: &[(&str, &str, u8)] = &[
    ("eigenes Design", "own design", 4),
    ("Remix", "remix", 9),
    ("gekauft", "bought", 2),
    ("kostenlos", "free", 5),
    ("getestet", "tested", 5),
    ("muss angepasst werden", "needs tweaking", 1),
    ("fehlerhaft", "broken", 0),
];

fn suggested_tags(list: &[(&str, &str, u8)]) -> Vec<SuggestedTag> {
    list.iter().map(|&(de, en, slot)| SuggestedTag { de: de.into(), en: en.into(), color: Some(slot) }).collect()
}

/// The built-in set of themes.
pub fn default_themes() -> Vec<SuggestedTag> {
    suggested_tags(DEFAULT_THEMES)
}

/// The built-in set of print properties.
pub fn default_properties() -> Vec<SuggestedTag> {
    suggested_tags(DEFAULT_PROPERTIES)
}

/// The built-in set of states and origins.
pub fn default_states() -> Vec<SuggestedTag> {
    suggested_tags(DEFAULT_STATES)
}

impl Config {
    /// The themes in use: the user's own list, or the built-in set.
    pub fn themes(&self) -> Vec<SuggestedTag> {
        self.themes.clone().unwrap_or_else(default_themes)
    }

    /// The print properties in use.
    pub fn properties(&self) -> Vec<SuggestedTag> {
        self.properties.clone().unwrap_or_else(default_properties)
    }

    /// The states and origins in use.
    pub fn states(&self) -> Vec<SuggestedTag> {
        self.states.clone().unwrap_or_else(default_states)
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            library_roots: Vec::new(),
            last_folder: None,
            tile_size: 160.0,
            recent_prints: 10,
            only_models: false,
            sort_by: SortKey::Name,
            sort_descending: false,
            supported_first: true,
            language: None,
            slicers: None,
            default_slicer: None,
            slicer: None,
            printer: None,
            printers: None,
            default_printer: None,
            gltf_unit: GltfUnit::Metres,
            free_orbit: false,
            spacemouse: Default::default(),
            model_units: ModelUnits::Millimetres,
            display_units: DisplayUnits::Metric,
            preview_quality: PreviewQuality::High,
            preview_controls: false,
            double_click: DoubleClick::Window,
            folder_previews: FolderPreviews::default(),
            lock_panes: false,
            inbox: true,
            import: ImportConfig::default(),
            placement: PlacementConfig::default(),
            ai: AiConfig::default(),
            editors: None,
            default_editor: None,
            editor: None,
            colored_tags: true,
            tags_on_tiles: None,
            themes: None,
            properties: None,
            states: None,
            flat_tag_list: false,
            sidebar_collapsed: Vec::new(),
            settings_folded: Vec::new(),
            tag_chips_idle: true,
            tile_tags: TileTags::Names,
            tile_tag_count: 3,
            tile_layout: TileLayout::default(),
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
        match Self::path() {
            Some(path) => Self::load_from(&path),
            None => Ok(Config::default()),
        }
    }

    /// [`Config::load`] from another file (tests, `meshgrove-mcp --config`).
    pub fn load_from(path: &Path) -> Result<Config, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let mut config: Config = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
                config.migrate_programs();
                config.migrate_tile_tags();
                config.tile_layout.normalize();
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
        if let Some(old) = self.printer.take()
            && self.printers.is_none()
        {
            self.default_printer = Some(old.name.clone());
            self.printers = Some(vec![old.into()]);
        }
    }

    /// Earlier versions switched the tags on tiles off with `tags_on_tiles = false`: they become
    /// a hidden mark.
    fn migrate_tile_tags(&mut self) {
        if self.tags_on_tiles.take() == Some(false) {
            self.tile_layout.place(TileBadge::Tags, TileSlot::Hidden, usize::MAX);
        }
    }

    /// The configured printers with the main one first (the order the views and the AI use).
    pub fn printers_in_order(&self) -> Vec<crate::printer::Printer> {
        let mut printers = self.printers.clone().unwrap_or_default();
        let main = Self::default_printer_index(&printers, self.default_printer.as_deref());
        if main < printers.len() {
            let p = printers.remove(main);
            printers.insert(0, p);
        }
        printers
    }

    /// The main printer's place among `printers`.
    pub fn default_printer_index(printers: &[crate::printer::Printer], default: Option<&str>) -> usize {
        default.and_then(|name| printers.iter().position(|p| p.name == name)).unwrap_or(0)
    }

    /// The slicer to use by default among `slicers`.
    pub fn default_slicer_index(slicers: &[Slicer], default: Option<&str>) -> usize {
        default.and_then(|name| slicers.iter().position(|s| s.name == name)).unwrap_or(0)
    }

    pub fn save(&self) -> io::Result<()> {
        self.save_to(&Self::path().ok_or_else(|| io::Error::other("no config directory"))?)
    }

    /// Replaces the file in one step: `meshgrove-mcp` reads it at any time.
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        crate::cache::write_atomic(path, toml::to_string_pretty(self).map_err(io::Error::other)?.as_bytes())
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
    fn the_single_printer_of_earlier_versions_becomes_the_list() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[printer]\nname = \"Kobra S1\"\nx = 250.0\ny = 250.0\nz = 250.0\n").unwrap();
        let config = Config::load_from(&path).unwrap();
        let printers = config.printers.as_deref().unwrap();
        assert_eq!((printers.len(), printers[0].name.as_str(), printers[0].z), (1, "Kobra S1", 250.0));
        assert_eq!(config.default_printer.as_deref(), Some("Kobra S1"));
        config.save_to(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[[printers]]") && !text.contains("[printer]"), "{text}");
        assert_eq!(Config::load_from(&path).unwrap(), config);
    }

    #[test]
    fn saving_replaces_the_file_in_one_step() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meshgrove").join("config.toml");
        assert_eq!(Config::load_from(&path).unwrap(), Config::default(), "missing file → defaults");
        let config = Config { tile_size: 200.0, ..Config::default() };
        config.save_to(&path).unwrap();
        Config { tile_size: 300.0, ..Config::default() }.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path).unwrap().tile_size, 300.0);
        let names: Vec<_> =
            std::fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["config.toml"], "no temporary file left behind");
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
    fn units_default_to_metric_and_convert() {
        let config = Config::default();
        assert_eq!((config.model_units, config.display_units), (ModelUnits::Millimetres, DisplayUnits::Metric));
        assert_eq!(ModelUnits::Inches.scale(), 25.4);
        assert_eq!(DisplayUnits::Imperial.from_mm(25.4), 1.0);
        assert_eq!(DisplayUnits::Imperial.to_mm(2.0), 50.8);
        assert_eq!(DisplayUnits::Metric.from_mm(12.5), 12.5);
        let config = Config { model_units: ModelUnits::Inches, display_units: DisplayUnits::Imperial, ..config };
        let back: Config = toml::from_str(&toml::to_string_pretty(&config).unwrap()).unwrap();
        assert_eq!(back, config);
        let old: Config = toml::from_str("tile_size = 120.0\n").unwrap();
        assert_eq!((old.model_units, old.display_units), (ModelUnits::Millimetres, DisplayUnits::Metric));
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
    fn themes_default_to_the_built_in_set_and_survive_a_save() {
        let mut config = Config::default();
        assert_eq!(config.themes(), default_themes());
        let mut seen = std::collections::HashSet::new();
        for t in default_themes().into_iter().chain(default_properties()).chain(default_states()) {
            assert!(t.color.is_some_and(|c| c < 12));
            assert!(seen.insert(t.de.to_lowercase()), "{} twice", t.de);
        }
        config.themes = Some(vec![
            SuggestedTag::named("Drachen"),
            SuggestedTag { de: "Burg".into(), en: "Castle".into(), color: Some(3) },
        ]);
        config.flat_tag_list = true;
        config.sidebar_collapsed = vec!["tags:themes".into(), "devices".into()];
        let back: Config = toml::from_str(&toml::to_string_pretty(&config).unwrap()).unwrap();
        assert_eq!(back, config);
        assert!(back.themes()[1].matches("castle"));
        config.themes = Some(Vec::new());
        let back: Config = toml::from_str(&toml::to_string_pretty(&config).unwrap()).unwrap();
        assert!(back.themes().is_empty(), "an emptied list stays empty");
        let old: Config = toml::from_str("tile_size = 200.0\n").unwrap();
        assert!(
            old.themes.is_none()
                && old.properties.is_none()
                && old.states.is_none()
                && !old.flat_tag_list
                && old.sidebar_collapsed.is_empty()
                && old.tag_chips_idle
        );
        assert_eq!(old.properties().len(), 16);
        assert_eq!(old.states().len(), 7);
    }

    #[test]
    fn tile_layout_starts_as_tiles_always_looked_and_survives_a_save() {
        let layout = TileLayout::default();
        assert_eq!(layout.top_left, [TileBadge::Filaments]);
        assert_eq!(layout.top_right, [TileBadge::Duplicates, TileBadge::Tags]);
        assert_eq!(layout.bottom_left, [TileBadge::FileType]);
        assert_eq!(layout.bottom_right, [TileBadge::Check, TileBadge::Favorite, TileBadge::PrintState]);
        assert!(layout.hidden.is_empty() && layout.name_lines == 2 && layout.name_extension);
        for badge in TileBadge::ALL {
            assert_eq!(toml::Value::try_from(badge).unwrap().as_str(), Some(badge.name()));
        }
        let mut config = Config::default();
        config.tile_layout.place(TileBadge::Tags, TileSlot::BottomLeft, 0);
        config.tile_layout.place(TileBadge::Favorite, TileSlot::Hidden, 0);
        config.tile_layout.filaments = FilamentStyle::Stripe;
        config.tile_layout.check = CheckStyle::Symbol;
        config.tile_layout.name_lines = 1;
        config.tile_layout.name_extension = false;
        let text = toml::to_string_pretty(&config).unwrap();
        assert!(!text.contains("tags_on_tiles"), "{text}");
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back, config);
        assert_eq!(back.tile_layout.bottom_left, [TileBadge::Tags, TileBadge::FileType]);
        assert_eq!(back.tile_layout.slot_of(TileBadge::Favorite), TileSlot::Hidden);
        let old: Config = toml::from_str("tile_size = 120.0\n").unwrap();
        assert_eq!(old.tile_layout, TileLayout::default());
    }

    #[test]
    fn tile_marks_move_and_what_is_read_gets_cleaned_up() {
        let mut layout = TileLayout::default();
        // Within its own corner the index is counted without the mark itself.
        layout.place(TileBadge::Check, TileSlot::BottomRight, 1);
        assert_eq!(layout.bottom_right, [TileBadge::Favorite, TileBadge::Check, TileBadge::PrintState]);
        layout.place(TileBadge::Duplicates, TileSlot::BottomRight, usize::MAX);
        assert_eq!(layout.top_right, [TileBadge::Tags]);
        assert_eq!(layout.bottom_right.last(), Some(&TileBadge::Duplicates));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "[tile_layout]\ntop_left = [\"tags\", \"sparkles\", \"tags\"]\nbottom_right = [\"favorite\"]\nname_lines = 7\n";
        std::fs::write(&path, text).unwrap();
        let layout = Config::load_from(&path).unwrap().tile_layout;
        assert_eq!(layout.top_left, [TileBadge::Tags, TileBadge::Filaments], "unknown and repeated names dropped");
        assert_eq!(layout.top_right, [TileBadge::Duplicates]);
        assert_eq!(layout.bottom_right, [TileBadge::Favorite, TileBadge::Check, TileBadge::PrintState]);
        assert_eq!(layout.name_lines, 2);
        let mut all: Vec<_> = TileSlot::CORNERS
            .into_iter()
            .chain([TileSlot::Hidden])
            .flat_map(|s| layout.slot(s).to_vec())
            .map(TileBadge::name)
            .collect();
        all.sort_unstable();
        let mut every: Vec<_> = TileBadge::ALL.map(TileBadge::name).to_vec();
        every.sort_unstable();
        assert_eq!(all, every, "every mark exactly once");
    }

    #[test]
    fn tags_switched_off_on_tiles_become_a_hidden_mark() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "tags_on_tiles = false\n").unwrap();
        let config = Config::load_from(&path).unwrap();
        assert_eq!(config.tile_layout.slot_of(TileBadge::Tags), TileSlot::Hidden);
        assert!(config.tags_on_tiles.is_none());
        config.save_to(&path).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("tags_on_tiles"));
        assert_eq!(Config::load_from(&path).unwrap(), config);
        std::fs::write(&path, "tags_on_tiles = true\n").unwrap();
        assert_eq!(Config::load_from(&path).unwrap().tile_layout, TileLayout::default());
    }

    #[test]
    fn import_and_placement_follow_the_settings() {
        let mut config = Config { library_roots: vec!["/lib".into(), "/lib2".into()], ..Default::default() };
        assert_eq!(config.landing_dir(), Some(PathBuf::from("/lib/Import")));
        assert!(config.in_landing(Path::new("/lib2/Import/rose.stl")));
        assert!(!config.in_landing(Path::new("/lib/Import")) && !config.in_landing(Path::new("/lib/Garten/rose.stl")));
        config.import.landing = "Neu?".into();
        assert_eq!(config.landing_name(), "Import", "no file name: the usual one");
        config.import.landing = " Eingang ".into();
        config.import.library = Some("/lib2".into());
        assert_eq!(config.landing_dir(), Some(PathBuf::from("/lib2/Eingang")));
        config.import.library = Some("/nowhere".into());
        assert_eq!(config.import_root(), Some(Path::new("/lib")), "an unknown library falls back to the first");
        // Themes have a folder of their name in the user's language, whichever spelling the tag has.
        assert_eq!(config.folder_for_tag("garten").as_deref(), Some("Garten"));
        assert_eq!(config.folder_for_tag("Garden").as_deref(), Some("Garten"));
        config.language = Some("en".into());
        assert_eq!(config.folder_for_tag("Garten").as_deref(), Some("Garden"));
        config.language = None;
        assert_eq!(config.folder_for_tag("Kunde Müller"), None, "own tags need a rule");
        config.placement.rules = vec![
            FolderRule { tag: "Kunde Müller".into(), folder: "Kunden/Müller".into() },
            FolderRule { tag: "Garden".into(), folder: String::new() },
            FolderRule { tag: "werkstatt".into(), folder: "Werkstatt/Teile".into() },
        ];
        assert_eq!(config.folder_for_tag("kunde müller").as_deref(), Some("Kunden/Müller"));
        assert_eq!(config.folder_for_tag("Garten"), None, "an empty rule switches a theme off");
        assert_eq!(config.folder_for_tag("Werkstatt").as_deref(), Some("Werkstatt/Teile"));
        let rules = config.folder_rules();
        assert!(!rules.iter().any(|(t, _)| t == "Garten"));
        assert_eq!(rules.iter().find(|(t, _)| t == "Werkstatt").map(|(_, f)| f.as_str()), Some("Werkstatt/Teile"));
        assert_eq!(rules.last().map(|(t, f)| (t.as_str(), f.as_str())), Some(("Kunde Müller", "Kunden/Müller")));
        let back: Config = toml::from_str(&toml::to_string_pretty(&config).unwrap()).unwrap();
        assert_eq!(back, config);
        let old: Config = toml::from_str("library_roots = []").unwrap();
        assert!(old.import.folders.is_empty() && old.import.at_start == ImportAtStart::Ask && old.placement.enabled);
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
