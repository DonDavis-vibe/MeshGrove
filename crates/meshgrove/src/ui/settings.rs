//! Settings window: pages listed on the left in groups, each with a line on what it holds, its
//! settings in folding sections of aligned rows (see `settings_kit`), and a button that puts the
//! page back to the defaults. The arrangement follows SolidCraft's preferences.

use std::path::{Path, PathBuf};

use crossbeam_channel::{Receiver, bounded};
use egui::{RichText, Ui};
use meshgrove_core::config::{
    self, ArchiveImport, Config, DisplayUnits, DoubleClick, DuplicatesKeep, FOLDER_PREVIEW_DEPTH, FolderPreviews,
    FolderRule, GltfUnit, ImportAtStart, ModelUnits, PreviewQuality, Program, Slicer, SuggestedTag, TileLayout,
    TileTags, default_properties, default_states, default_themes,
};
use meshgrove_core::printer::{self, DetectedPrinter, Printer};
use meshgrove_core::programs::{Detected, ProgramKind, detect_editors, detect_slicers};
use meshgrove_core::spacemouse::SpaceMouseConfig;
use meshgrove_core::userdata;

use super::settings_kit::{check, hint, row, segmented};
use super::spacemouse::MouseSettings;
use super::tags::{self, Group, PALETTE, TagStyle};
use crate::i18n;
use crate::spacemouse::Snapshot;
use solidcraft_ui_kit as sc;
use solidcraft_ui_kit::prefs_window::{self, HostPage, Labels, Options, Page};

pub enum SettingsChange {
    Language(Option<String>),
    /// The interface theme; `None`: Meshgrove's own look.
    UiTheme(Option<String>),
    /// The interface font: "system", a family, or `None` for the theme's own.
    UiFont(Option<String>),
    /// Who draws the main window's title bar.
    TitleBar(config::TitleBar),
    AddRoot(PathBuf),
    RemoveRoot(PathBuf),
    /// The slicer list and the name of the default one.
    Slicers {
        list: Vec<Slicer>,
        default: Option<String>,
    },
    /// The mesh editor list and the name of the default one.
    Editors {
        list: Vec<Program>,
        default: Option<String>,
    },
    /// The printer list and the name of the main printer.
    Printers {
        list: Vec<Printer>,
        default: Option<String>,
    },
    GltfUnit(GltfUnit),
    /// Free orbit in the 3D views.
    FreeOrbit(bool),
    /// The 3D mouse settings, as a whole.
    SpaceMouse(SpaceMouseConfig),
    /// How STL, OBJ and PLY coordinates are read; inches need a confirmation first.
    ModelUnits(ModelUnits),
    /// Metric or imperial sizes in the interface; imperial needs a confirmation first.
    DisplayUnits(DisplayUnits),
    PreviewQuality(PreviewQuality),
    DoubleClick(DoubleClick),
    FolderPreviews(FolderPreviews),
    LockPanes(bool),
    FoldForPlugins(bool),
    /// The inbox of new files in the sidebar.
    Inbox(bool),
    /// A model both packed and extracted: which copy the duplicate finder keeps.
    DuplicatesKeep(DuplicatesKeep),
    /// The folders looked at for new files to import.
    ImportFolders(Vec<PathBuf>),
    /// The library imported files go to, and the folder in it.
    ImportTarget {
        library: Option<PathBuf>,
        landing: String,
    },
    ImportAtStart(ImportAtStart),
    ImportArchives(ArchiveImport),
    /// Placing files into folders by their tags.
    Placement(bool),
    /// The folder each tag maps to, where it differs from the themes' own names.
    FolderRules(Vec<FolderRule>),
    /// The tags in order of precedence for placing.
    PlacementOrder(Vec<String>),
    AiEnabled(bool),
    /// Make Meshgrove available to the AI apps on this computer.
    AiConnectApps(bool),
    /// Accept the suggestions made from now on without the review (moves excepted).
    AiAutoAccept(bool),
    /// The AI may suggest moves into another library.
    AiCrossLibraryMoves(bool),
    AiCloudAsk(bool),
    AiReviewButton(bool),
    /// Connect the AI apps found now.
    AiConnectNow,
    /// What a library shows to which AI.
    AiLibrary(meshgrove_core::config::LibraryAi),
    /// Deletes the suggestion store and the access log.
    ClearAiData,
    /// The user's own endpoint (`None`: none).
    Endpoint(Option<meshgrove_core::config::EndpointConfig>),
    /// Keeps the endpoint's key in the keyring.
    EndpointKey {
        url: String,
        key: String,
    },
    /// Who answers the chat: the endpoint, or an agent by its id.
    Assistant(String),
    /// What happens when the chat opens: nothing, the last pick connected, or this agent (id).
    AutoConnect(String),
    /// An ACP agent the user added as a command line.
    AddAgent(meshgrove_core::config::AgentConfig),
    RemoveAgent(String),
    ShowAiReview,
    ShowAiHistory,
    /// Installs the plugin in this `.wasm` file (copied into Meshgrove's plugins folder).
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    InstallPlugin(PathBuf),
    /// Asks for a plugin file in the system's file dialog, then installs it.
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    PickPlugin,
    /// Opens Meshgrove's plugins folder in the file manager.
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    OpenPluginFolder,
    /// Switches a plugin on (as its file is now) or off.
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    PluginEnabled {
        id: String,
        enabled: bool,
    },
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    PluginGrants {
        id: String,
        grants: meshgrove_core::config::PluginGrants,
    },
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    RemovePlugin(String),
    /// Tags in their colours, or grey.
    ColoredTags(bool),
    /// How many tags a tile shows at most, and how.
    TileTagLook {
        count: u8,
        style: TileTags,
    },
    /// Which marks the tiles show where, and their names.
    TileLayout(TileLayout),
    /// The tile edge length.
    TileSize(f32),
    /// How many models sent to the slicer "Zuletzt gedruckt" keeps (0: none).
    RecentPrints(u16),
    /// The sidebar lists tags alphabetically, not grouped.
    FlatTagList(bool),
    /// Existing tags stay listed below the tag field while nothing is typed.
    IdleTagChips(bool),
    /// The suggested tags of a group (themes, properties, states); `None`: the built-in set.
    Suggestions(Group, Option<Vec<SuggestedTag>>),
    ResetLayout,
    Export(PathBuf),
    Import(PathBuf),
    OpenBackups,
    ClearCache,
}

fn inbox(ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
    let mut on = config.inbox;
    if check(ui, &mut on, t!("settings.inbox_show"), t!("settings.inbox_hint")) {
        changes.push(SettingsChange::Inbox(on));
    }
}

/// Which copy the duplicate finder keeps when a model is both packed and extracted.
fn duplicates(ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
    row(ui, &t!("settings.duplicates_keep_row"), |ui| {
        let mut keep = config.duplicates_keep;
        let options = [
            (DuplicatesKeep::Extracted, t!("settings.duplicates_extracted").into_owned()),
            (DuplicatesKeep::Packed, t!("settings.duplicates_packed").into_owned()),
        ];
        if segmented(ui, &mut keep, &options) {
            changes.push(SettingsChange::DuplicatesKeep(keep));
        }
        hint(ui, t!("settings.duplicates_hint"));
    });
}

/// How tags are drawn (how they show on the tiles is under Tiles).
fn tags(ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
    let mut colored = config.colored_tags;
    if check(ui, &mut colored, t!("settings.tags_colored"), t!("settings.tags_hint")) {
        changes.push(SettingsChange::ColoredTags(colored));
    }
    let mut flat = config.flat_tag_list;
    if check(ui, &mut flat, t!("settings.tags_flat"), t!("settings.tags_flat_hint")) {
        changes.push(SettingsChange::FlatTagList(flat));
    }
    let mut idle = config.tag_chips_idle;
    if check(ui, &mut idle, t!("settings.tags_idle"), t!("settings.tags_idle_hint")) {
        changes.push(SettingsChange::IdleTagChips(idle));
    }
}

/// How many of the models sent to the slicer the sidebar keeps under "Zuletzt gedruckt".
fn recent_prints(ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
    row(ui, &t!("settings.recent_prints"), |ui| {
        let mut keep = config.recent_prints;
        ui.horizontal(|ui| {
            super::settings_kit::slider(ui, &mut keep, 0..=50, 220.0, None);
            // How many, in words where it is none.
            let shown = if keep == 0 { t!("settings.recent_prints_off").into_owned() } else { keep.to_string() };
            ui.label(shown);
        });
        if keep != config.recent_prints {
            changes.push(SettingsChange::RecentPrints(keep));
        }
        hint(ui, t!("settings.recent_prints_hint"));
    });
}

/// Model pictures on folder tiles.
fn folder_previews(ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
    let mut fp = config.folder_previews;
    check(ui, &mut fp.enabled, t!("settings.folder_previews_enabled"), t!("settings.folder_previews_hint"));
    ui.add_enabled_ui(fp.enabled, |ui| {
        row(ui, &t!("settings.folder_previews_depth"), |ui| {
            super::settings_kit::slider(ui, &mut fp.depth, FOLDER_PREVIEW_DEPTH, 260.0, Some(|v| format!("{v:.0}")));
        });
        sc::checkbox(ui, &t!("settings.folder_previews_cycle"), &mut fp.cycle);
    });
    if fp != config.folder_previews {
        changes.push(SettingsChange::FolderPreviews(fp));
    }
}

/// Picture quality of the large preview and the model windows.
fn preview(ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
    row(ui, &t!("settings.preview_quality"), |ui| {
        let mut quality = config.preview_quality;
        let options = [
            (PreviewQuality::Standard, t!("settings.preview_standard").into_owned()),
            (PreviewQuality::High, t!("settings.preview_high").into_owned()),
            (PreviewQuality::Studio, t!("settings.preview_studio").into_owned()),
        ];
        if segmented(ui, &mut quality, &options) {
            changes.push(SettingsChange::PreviewQuality(quality));
        }
        hint(ui, t!("settings.preview_hint"));
    });
    let mut free = config.free_orbit;
    if check(ui, &mut free, t!("settings.free_orbit"), t!("settings.free_orbit_hint")) {
        changes.push(SettingsChange::FreeOrbit(free));
    }
}

/// How model files without a fixed unit are read.
fn models(ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
    row(ui, &t!("settings.gltf_unit"), |ui| {
        let mut unit = config.gltf_unit;
        let options = [
            (GltfUnit::Metres, t!("settings.gltf_metres").into_owned()),
            (GltfUnit::Millimetres, t!("settings.gltf_millimetres").into_owned()),
        ];
        if segmented(ui, &mut unit, &options) {
            changes.push(SettingsChange::GltfUnit(unit));
        }
        hint(ui, t!("settings.gltf_unit_hint"));
    });
}

/// Units of the model files and of the sizes shown. Inches (imperial) are asked about first.
fn units(ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
    row(ui, &t!("settings.display_units"), |ui| {
        let mut units = config.display_units;
        let options = [
            (DisplayUnits::Metric, t!("settings.units_metric").into_owned()),
            (DisplayUnits::Imperial, t!("settings.units_imperial").into_owned()),
        ];
        if segmented(ui, &mut units, &options) {
            changes.push(SettingsChange::DisplayUnits(units));
        }
        hint(ui, t!("settings.display_units_hint"));
    });
    row(ui, &t!("settings.model_units"), |ui| {
        let mut units = config.model_units;
        let options = [
            (ModelUnits::Millimetres, t!("settings.model_units_mm").into_owned()),
            (ModelUnits::Inches, t!("settings.model_units_inches").into_owned()),
        ];
        if segmented(ui, &mut units, &options) {
            changes.push(SettingsChange::ModelUnits(units));
        }
        hint(ui, t!("settings.model_units_hint"));
    });
}

/// Facts shown in the "about" part.
pub struct AppInfo<'a> {
    pub gpu: &'a str,
    pub db_path: &'a Path,
    /// The 3D mouse as the reader sees it.
    pub mouse: &'a Snapshot,
    /// What the 3D mouse's picture needs to draw a cube.
    pub render: (&'a egui_wgpu::RenderState, &'a meshgrove_render::Renderer),
    /// The tags' colours, for the sample tile.
    pub tag_style: &'a TagStyle,
    /// Connecting the AI apps: allowed in this launch, and going on now.
    pub ai_connect: (bool, bool),
    /// The AI agents found on this computer (`None`: still looking, or AI is off).
    pub agents: Option<&'a crate::assist::agents::Detection>,
    /// The library's tags with their counts, for the placement rules.
    pub tags: &'a [(String, usize)],
    /// The installed plugins.
    pub plugins: &'a [super::settings_plugins::PluginRow],
}

#[derive(Default)]
pub struct SettingsDialog {
    root_input: String,
    root_error: Option<String>,
    /// An import folder being added, and the landing folder's name as typed.
    import_folder_input: String,
    import_folder_error: Option<String>,
    landing_input: String,
    /// The placement rules as typed, in order of precedence, then the library's own tags
    /// without a rule.
    rule_inputs: Vec<RuleRow>,
    /// How many of them can be ordered (the rest have no folder yet).
    rule_ordered: usize,
    /// The rule being dragged by its handle, to a new place in the order.
    rule_drag: Option<usize>,
    /// Name, program and working folder of a slicer being added by hand.
    slicer_form: (String, String, String),
    /// The slicers found on this computer, and the scan that looks for them.
    detected: Vec<Detected>,
    detect_rx: Option<Receiver<Vec<Detected>>>,
    /// Name, program and working folder of an editor being added by hand.
    editor_form: (String, String, String),
    /// Name, X/Y/Z, nozzle and materials of a printer being added or edited, as typed.
    printer_form: [String; 6],
    /// The display unit the X/Y/Z of `printer_form` are in.
    printer_form_units: DisplayUnits,
    printer_error: bool,
    /// The printer the form edits (its name before the edit); `None`: the form adds one.
    printer_editing: Option<String>,
    /// The printers set up in the slicers on this computer.
    found_printers: Vec<DetectedPrinter>,
    /// The "KI & MCP" page.
    ai: super::settings_ai::AiSettings,
    /// The "Plugins" page.
    plugins: super::settings_plugins::PluginSettings,
    /// The kit's preferences window: whether it is open, its page, its search, and the settings
    /// as they were when it opened (what Cancel puts back).
    window: prefs_window::State<Box<Config>>,
    /// The theme picker's cards.
    themes: super::appearance::Picker,
    /// The font list: what is typed into its search, the search its rows were made for, the
    /// rows whose fonts are loaded, and their families.
    font_filter: String,
    font_query: String,
    font_window: super::fonts::PreviewWindow,
    font_previews: std::collections::BTreeSet<String>,
    /// The sample tile of the "Kacheln" page.
    tiles: super::settings_tiles::TileSettings,
    /// A new theme as typed.
    suggest_input: [String; 3],
    import_input: String,
    cache_size: Option<u64>,
    cache_rx: Option<Receiver<u64>>,
    /// The 3D mouse's part: the calibration in progress.
    mouse: MouseSettings,
    /// The category shown on the right.
    category: Category,
}

/// One row of the placement rules: a tag, the folder typed for it, and the theme's own name when
/// the tag is a theme (its folder unless changed).
#[derive(Default)]
struct RuleRow {
    tag: String,
    folder: String,
    own: Option<String>,
}

/// Six dots to grab a row by; `held` while it is being dragged.
fn drag_handle(ui: &mut Ui, held: bool) -> egui::Response {
    let size = egui::vec2(12.0, ui.spacing().interact_size.y);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::drag());
    let response = response.on_hover_cursor(egui::CursorIcon::Grab).on_hover_text(t!("settings.placement_drag"));
    let visuals = ui.visuals();
    let color = if held || response.hovered() { visuals.strong_text_color() } else { visuals.weak_text_color() };
    for (dx, dy) in [(-2.5, -5.0), (2.5, -5.0), (-2.5, 0.0), (2.5, 0.0), (-2.5, 5.0), (2.5, 5.0)] {
        ui.painter().circle_filled(rect.center() + egui::vec2(dx, dy), 1.4, color);
    }
    response
}

/// The slicers, mesh editors and printers, each with the index of the default one.
type Lists<'a> = ((&'a [Slicer], usize), (&'a [Program], usize), (&'a [Printer], usize));

/// The parts of a category, each a folding section with its own title.
#[derive(Debug, Clone, Copy)]
enum Section {
    Language,
    Appearance,
    DoubleClick,
    Panes,
    Library,
    Inbox,
    Duplicates,
    Import,
    FolderPreviews,
    TilePreview,
    TileLook,
    Tags,
    Placement,
    Themes,
    Properties,
    States,
    Preview,
    SpaceMouse,
    Models,
    Units,
    Slicer,
    RecentPrints,
    Editor,
    Printer,
    UserData,
    Storage,
    Ai,
    AiLibraries,
    Mcp,
    Assistant,
    Endpoint,
    AiLog,
    Plugins,
}

impl Section {
    fn title(self) -> String {
        match self {
            Section::Language => t!("settings.language"),
            Section::Appearance => t!("settings.appearance"),
            Section::DoubleClick => t!("settings.double_click"),
            Section::Panes => t!("settings.panes"),
            Section::Library => t!("settings.library"),
            Section::Inbox => t!("settings.inbox"),
            Section::Duplicates => t!("settings.duplicates"),
            Section::Import => t!("settings.import_section"),
            Section::FolderPreviews => t!("settings.folder_previews"),
            Section::TilePreview => t!("settings.tiles_preview"),
            Section::TileLook => t!("settings.tiles_look"),
            Section::Tags => t!("settings.tags_display"),
            Section::Placement => t!("settings.placement"),
            Section::Themes => return Group::Themes.title(),
            Section::Properties => return Group::Properties.title(),
            Section::States => return Group::State.title(),
            Section::Preview => t!("settings.preview"),
            Section::SpaceMouse => t!("mouse.title"),
            Section::Models => t!("settings.models"),
            Section::Units => t!("settings.units"),
            Section::Slicer => t!("settings.slicer"),
            Section::RecentPrints => t!("lists.recent"),
            Section::Editor => t!("settings.editor"),
            Section::Printer => t!("settings.printer"),
            Section::UserData => t!("settings.user_data"),
            Section::Storage => t!("settings.storage"),
            Section::Ai => t!("settings.ai"),
            Section::AiLibraries => t!("settings.ai_libraries"),
            Section::Mcp => t!("settings.mcp"),
            Section::Assistant => t!("assistant.title"),
            Section::Endpoint => t!("endpoint.title"),
            Section::AiLog => t!("settings.ai_log"),
            Section::Plugins => t!("settings.plugins"),
        }
        .into_owned()
    }

    /// The name its folding is kept under in the config.
    fn key(self) -> String {
        format!("{self:?}").to_lowercase()
    }
}

/// The settings' categories, listed on the left of the window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Category {
    #[default]
    General,
    Library,
    Tags,
    Tiles,
    View,
    ModelsUnits,
    Programs,
    Printer,
    Ai,
    Plugins,
    Data,
    About,
}

impl Category {
    const ALL: [Category; 12] = [
        Category::General,
        Category::Library,
        Category::Tags,
        Category::Tiles,
        Category::View,
        Category::ModelsUnits,
        Category::Programs,
        Category::Printer,
        Category::Ai,
        Category::Plugins,
        Category::Data,
        Category::About,
    ];

    /// The caption the category is listed under, from its first category on.
    fn group(self) -> Option<String> {
        match self {
            Category::General | Category::Library | Category::Tags => None,
            Category::Tiles | Category::View | Category::ModelsUnits => Some(t!("settings.group_display")),
            Category::Programs | Category::Printer => Some(t!("settings.group_printing")),
            Category::Ai => Some(t!("settings.group_extend")),
            Category::Plugins => None,
            Category::Data | Category::About => Some(t!("settings.group_app")),
        }
        .map(|s| s.into_owned())
    }

    /// One line under the page's title saying what it holds.
    fn intro(self) -> String {
        match self {
            Category::General => t!("settings.intro_general"),
            Category::Library => t!("settings.intro_library"),
            Category::Tags => t!("settings.intro_tags"),
            Category::Tiles => t!("settings.intro_tiles"),
            Category::View => t!("settings.intro_view"),
            Category::ModelsUnits => t!("settings.intro_models_units"),
            Category::Programs => t!("settings.intro_programs"),
            Category::Printer => t!("settings.intro_printer"),
            Category::Ai => t!("settings.intro_ai"),
            Category::Plugins => t!("settings.intro_plugins"),
            Category::Data => t!("settings.intro_data"),
            Category::About => t!("settings.intro_about"),
        }
        .into_owned()
    }

    /// The changes that put this page's preferences back to how Meshgrove comes (none when they
    /// are). The 3D mouse keeps its calibration: it belongs to the device.
    fn defaults(self, config: &Config) -> Vec<SettingsChange> {
        let d = Config::default();
        let mut out = Vec::new();
        let mut put = |differs: bool, change: SettingsChange| {
            if differs {
                out.push(change);
            }
        };
        match self {
            Category::General => {
                put(config.language.is_some(), SettingsChange::Language(None));
                put(config.ui_theme.is_some(), SettingsChange::UiTheme(None));
                put(config.ui_font.is_some(), SettingsChange::UiFont(None));
                put(config.title_bar != d.title_bar, SettingsChange::TitleBar(d.title_bar));
                put(config.double_click != d.double_click, SettingsChange::DoubleClick(d.double_click));
                put(config.lock_panes != d.lock_panes, SettingsChange::LockPanes(d.lock_panes));
                put(config.fold_for_plugins != d.fold_for_plugins, SettingsChange::FoldForPlugins(d.fold_for_plugins));
            }
            Category::Library => {
                put(config.inbox != d.inbox, SettingsChange::Inbox(d.inbox));
                put(config.duplicates_keep != d.duplicates_keep, SettingsChange::DuplicatesKeep(d.duplicates_keep));
                put(config.import.at_start != d.import.at_start, SettingsChange::ImportAtStart(d.import.at_start));
                put(config.import.archives != d.import.archives, SettingsChange::ImportArchives(d.import.archives));
                put(
                    config.import.landing != d.import.landing,
                    SettingsChange::ImportTarget { library: config.import.library.clone(), landing: d.import.landing },
                );
            }
            Category::Tags => {
                put(config.colored_tags != d.colored_tags, SettingsChange::ColoredTags(d.colored_tags));
                put(config.flat_tag_list != d.flat_tag_list, SettingsChange::FlatTagList(d.flat_tag_list));
                put(config.tag_chips_idle != d.tag_chips_idle, SettingsChange::IdleTagChips(d.tag_chips_idle));
                put(config.themes.is_some(), SettingsChange::Suggestions(Group::Themes, None));
                put(config.properties.is_some(), SettingsChange::Suggestions(Group::Properties, None));
                put(config.states.is_some(), SettingsChange::Suggestions(Group::State, None));
                put(config.placement.enabled != d.placement.enabled, SettingsChange::Placement(d.placement.enabled));
                put(!config.placement.rules.is_empty(), SettingsChange::FolderRules(Vec::new()));
                put(!config.placement.order.is_empty(), SettingsChange::PlacementOrder(Vec::new()));
            }
            Category::Tiles => {
                put(config.tile_layout != d.tile_layout, SettingsChange::TileLayout(d.tile_layout));
                put(
                    (config.tile_tag_count, config.tile_tags) != (d.tile_tag_count, d.tile_tags),
                    SettingsChange::TileTagLook { count: d.tile_tag_count, style: d.tile_tags },
                );
                put(config.folder_previews != d.folder_previews, SettingsChange::FolderPreviews(d.folder_previews));
                put(config.tile_size != d.tile_size, SettingsChange::TileSize(d.tile_size));
            }
            Category::View => {
                put(config.preview_quality != d.preview_quality, SettingsChange::PreviewQuality(d.preview_quality));
                put(config.free_orbit != d.free_orbit, SettingsChange::FreeOrbit(d.free_orbit));
                let mouse =
                    SpaceMouseConfig { calibration: config.spacemouse.calibration, ..SpaceMouseConfig::default() };
                put(config.spacemouse != mouse, SettingsChange::SpaceMouse(mouse));
            }
            Category::ModelsUnits => {
                put(config.gltf_unit != d.gltf_unit, SettingsChange::GltfUnit(d.gltf_unit));
                put(config.display_units != d.display_units, SettingsChange::DisplayUnits(d.display_units));
                put(config.model_units != d.model_units, SettingsChange::ModelUnits(d.model_units));
            }
            _ => {}
        }
        out
    }

    fn icon(self) -> &'static str {
        match self {
            Category::General => "⚙",
            Category::Library => "📚",
            Category::Tags => "🏷",
            Category::Tiles => "⊞",
            Category::View => "🎥",
            Category::ModelsUnits => "📏",
            Category::Programs => "🔧",
            Category::Printer => "🖶",
            Category::Ai => "✨",
            Category::Plugins => "🔌",
            Category::Data => "💾",
            Category::About => "ℹ",
        }
    }

    /// The name test runs use (`MESHGROVE_VIEW=settings:<name>`).
    fn from_name(name: &str) -> Option<Category> {
        Some(match name {
            "general" => Category::General,
            "library" => Category::Library,
            "tags" => Category::Tags,
            "tiles" => Category::Tiles,
            "view" => Category::View,
            "models" => Category::ModelsUnits,
            "programs" => Category::Programs,
            "printer" => Category::Printer,
            "ai" => Category::Ai,
            "plugins" => Category::Plugins,
            "data" => Category::Data,
            "about" => Category::About,
            _ => return None,
        })
    }

    fn title(self) -> String {
        match self {
            Category::General => t!("settings.cat_general"),
            Category::Library => t!("settings.cat_library"),
            Category::Tags => t!("settings.cat_tags"),
            Category::Tiles => t!("settings.cat_tiles"),
            Category::View => t!("settings.cat_view"),
            Category::ModelsUnits => t!("settings.cat_models_units"),
            Category::Programs => t!("settings.cat_programs"),
            Category::Printer => t!("settings.cat_printer"),
            Category::Ai => t!("settings.cat_ai"),
            Category::Plugins => t!("settings.cat_plugins"),
            Category::Data => t!("settings.cat_data"),
            Category::About => t!("settings.cat_about"),
        }
        .into_owned()
    }
}

/// Where the browser keeps rebuildable data.
pub fn cache_dir() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("meshgrove"))
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            match e.metadata() {
                Ok(m) if m.is_dir() => stack.push(e.path()),
                Ok(m) => total += m.len(),
                Err(_) => {}
            }
        }
    }
    total
}

/// What the kit's preferences window works on while it is drawn: the dialog's own state, the
/// settings as they are, and the changes the pages ask for (the app applies them after the frame).
pub struct Host<'a> {
    dialog: &'a mut SettingsDialog,
    config: &'a Config,
    lists: Lists<'a>,
    info: &'a AppInfo<'a>,
    changes: &'a mut Vec<SettingsChange>,
}

impl prefs_window::Host for Host<'_> {
    /// The settings as they were when the window opened, or at the last Apply.
    type Snapshot = Box<Config>;

    fn window(&mut self) -> &mut prefs_window::State<Box<Config>> {
        &mut self.dialog.window
    }

    fn window_ref(&self) -> &prefs_window::State<Box<Config>> {
        &self.dialog.window
    }

    fn take_snapshot(&self) -> Box<Config> {
        Box::new(self.config.clone())
    }

    fn matches(&self, snapshot: &Box<Config>) -> bool {
        changes_towards(self.config, snapshot).is_empty()
    }

    /// Cancel: the same changes the pages make, back towards the snapshot. What acted at once is
    /// not a setting and stays done (a plugin installed or removed, the cache cleared, an export
    /// or import, an assistant connected, the endpoint's key).
    fn restore(&mut self, snapshot: &Box<Config>) -> Result<(), String> {
        self.changes.extend(changes_towards(self.config, snapshot));
        Ok(())
    }

    fn closed(&mut self) {
        self.dialog.font_filter.clear();
        self.dialog.font_window.reset();
        self.dialog.font_previews.clear();
    }
}

/// Draws page `N` of [`Category::ALL`]; a function of its own per page, as the window takes
/// plain functions.
fn draw_page<const N: usize>(ui: &mut Ui, host: &mut Host<'_>) {
    host.dialog.category = Category::ALL[N];
    host.dialog.page(ui, host.config, host.lists, host.info, host.changes);
}

/// Restore defaults for page `N`.
fn page_defaults<const N: usize>(host: &mut Host<'_>) {
    let changes = Category::ALL[N].defaults(host.config);
    host.changes.extend(changes);
}

/// The sign of page `N` in the page list.
fn page_icon<const N: usize>(painter: &egui::Painter, rect: egui::Rect, opacity: f32) {
    let ink = super::appearance::tokens().icon.gamma_multiply(opacity);
    let font = egui::FontId::proportional(rect.height() * 0.9);
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, Category::ALL[N].icon(), font, ink);
}

/// A text kept for the rest of the run, for the window's page list: it wants texts that live as
/// long as the program and reads an unknown id as the text itself. Each text is kept once.
fn kept(text: &str) -> &'static str {
    static TEXTS: std::sync::Mutex<std::collections::BTreeSet<&'static str>> =
        std::sync::Mutex::new(std::collections::BTreeSet::new());
    let mut texts = TEXTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(known) = texts.get(text) {
        return known;
    }
    let text: &'static str = Box::leak(text.to_owned().into_boxed_str());
    texts.insert(text);
    text
}

/// The window's pages: the categories in their order and groups.
fn pages<'a>() -> Vec<Page<Host<'a>>> {
    /// The names the pages are published under (for tests and automation).
    const HANDLES: [&str; 12] = [
        "mg:prefs:page:general",
        "mg:prefs:page:library",
        "mg:prefs:page:tags",
        "mg:prefs:page:tiles",
        "mg:prefs:page:view",
        "mg:prefs:page:models",
        "mg:prefs:page:programs",
        "mg:prefs:page:printer",
        "mg:prefs:page:ai",
        "mg:prefs:page:plugins",
        "mg:prefs:page:data",
        "mg:prefs:page:about",
    ];
    type Draw = for<'h> fn(&mut Ui, &mut Host<'h>);
    type Defaults = for<'h> fn(&mut Host<'h>);
    type Sign = &'static dyn Fn(&egui::Painter, egui::Rect, f32);
    macro_rules! each_page {
        ($($n:literal)*) => {
            [$((draw_page::<$n> as Draw, page_defaults::<$n> as Defaults, &page_icon::<$n> as Sign)),*]
        };
    }
    let functions: [(Draw, Defaults, Sign); 12] = each_page!(0 1 2 3 4 5 6 7 8 9 10 11);
    let mut group = None;
    Category::ALL
        .into_iter()
        .zip(functions)
        .zip(HANDLES)
        .map(|((category, (draw, defaults, sign)), handle)| {
            // A group's caption goes to the first page under it.
            let starts = category.group().filter(|caption| group.as_ref() != Some(caption));
            group = category.group();
            Page::Host(HostPage {
                icon: sc::Icon::Paint(sign),
                label: kept(&category.title()),
                group: starts.as_deref().map(kept),
                handle,
                scroll: true,
                draw,
                defaults,
            })
        })
        .collect()
}

/// The changes that take the settings from `config` to `target`: what Cancel asks for, and
/// whether anything is left to apply. Only what the pages set and the app keeps in its settings;
/// plugins are left out (switching one acts at once), and a library taken out of the AI's sight
/// stays out.
fn changes_towards(config: &Config, target: &Config) -> Vec<SettingsChange> {
    let (c, t) = (config, target);
    let mut out = Vec::new();
    let mut put = |differs: bool, change: &dyn Fn() -> SettingsChange| {
        if differs {
            out.push(change());
        }
    };
    // General
    put(c.language != t.language, &|| SettingsChange::Language(t.language.clone()));
    put(c.ui_theme != t.ui_theme, &|| SettingsChange::UiTheme(t.ui_theme.clone()));
    put(c.ui_font != t.ui_font, &|| SettingsChange::UiFont(t.ui_font.clone()));
    put(c.title_bar != t.title_bar, &|| SettingsChange::TitleBar(t.title_bar));
    put(c.double_click != t.double_click, &|| SettingsChange::DoubleClick(t.double_click));
    put(c.lock_panes != t.lock_panes, &|| SettingsChange::LockPanes(t.lock_panes));
    put(c.fold_for_plugins != t.fold_for_plugins, &|| SettingsChange::FoldForPlugins(t.fold_for_plugins));
    // Library
    for root in c.library_roots.iter().filter(|root| !t.library_roots.contains(root)) {
        put(true, &|| SettingsChange::RemoveRoot(root.clone()));
    }
    for root in t.library_roots.iter().filter(|root| !c.library_roots.contains(root)) {
        put(true, &|| SettingsChange::AddRoot(root.clone()));
    }
    put(c.inbox != t.inbox, &|| SettingsChange::Inbox(t.inbox));
    put(c.duplicates_keep != t.duplicates_keep, &|| SettingsChange::DuplicatesKeep(t.duplicates_keep));
    put(c.import.folders != t.import.folders, &|| SettingsChange::ImportFolders(t.import.folders.clone()));
    put((&c.import.library, &c.import.landing) != (&t.import.library, &t.import.landing), &|| {
        SettingsChange::ImportTarget { library: t.import.library.clone(), landing: t.import.landing.clone() }
    });
    put(c.import.at_start != t.import.at_start, &|| SettingsChange::ImportAtStart(t.import.at_start));
    put(c.import.archives != t.import.archives, &|| SettingsChange::ImportArchives(t.import.archives));
    // Tags
    put(c.colored_tags != t.colored_tags, &|| SettingsChange::ColoredTags(t.colored_tags));
    put(c.flat_tag_list != t.flat_tag_list, &|| SettingsChange::FlatTagList(t.flat_tag_list));
    put(c.tag_chips_idle != t.tag_chips_idle, &|| SettingsChange::IdleTagChips(t.tag_chips_idle));
    put(c.themes != t.themes, &|| SettingsChange::Suggestions(Group::Themes, t.themes.clone()));
    put(c.properties != t.properties, &|| SettingsChange::Suggestions(Group::Properties, t.properties.clone()));
    put(c.states != t.states, &|| SettingsChange::Suggestions(Group::State, t.states.clone()));
    put(c.placement.enabled != t.placement.enabled, &|| SettingsChange::Placement(t.placement.enabled));
    put(c.placement.rules != t.placement.rules, &|| SettingsChange::FolderRules(t.placement.rules.clone()));
    put(c.placement.order != t.placement.order, &|| SettingsChange::PlacementOrder(t.placement.order.clone()));
    // Tiles
    put(c.tile_layout != t.tile_layout, &|| SettingsChange::TileLayout(t.tile_layout.clone()));
    put((c.tile_tag_count, c.tile_tags) != (t.tile_tag_count, t.tile_tags), &|| SettingsChange::TileTagLook {
        count: t.tile_tag_count,
        style: t.tile_tags,
    });
    put(c.folder_previews != t.folder_previews, &|| SettingsChange::FolderPreviews(t.folder_previews));
    put(c.tile_size != t.tile_size, &|| SettingsChange::TileSize(t.tile_size));
    // 3D view
    put(c.preview_quality != t.preview_quality, &|| SettingsChange::PreviewQuality(t.preview_quality));
    put(c.free_orbit != t.free_orbit, &|| SettingsChange::FreeOrbit(t.free_orbit));
    put(c.spacemouse != t.spacemouse, &|| SettingsChange::SpaceMouse(t.spacemouse.clone()));
    // Models & Units
    put(c.gltf_unit != t.gltf_unit, &|| SettingsChange::GltfUnit(t.gltf_unit));
    put(c.display_units != t.display_units, &|| SettingsChange::DisplayUnits(t.display_units));
    put(c.model_units != t.model_units, &|| SettingsChange::ModelUnits(t.model_units));
    // Programs and printers
    put((&c.slicers, &c.default_slicer) != (&t.slicers, &t.default_slicer), &|| SettingsChange::Slicers {
        list: t.slicers.clone().unwrap_or_default(),
        default: t.default_slicer.clone(),
    });
    put((&c.editors, &c.default_editor) != (&t.editors, &t.default_editor), &|| SettingsChange::Editors {
        list: t.editors.clone().unwrap_or_default(),
        default: t.default_editor.clone(),
    });
    put(c.recent_prints != t.recent_prints, &|| SettingsChange::RecentPrints(t.recent_prints));
    put((&c.printers, &c.default_printer) != (&t.printers, &t.default_printer), &|| SettingsChange::Printers {
        list: t.printers.clone().unwrap_or_default(),
        default: t.default_printer.clone(),
    });
    // AI & MCP
    put(c.ai.enabled != t.ai.enabled, &|| SettingsChange::AiEnabled(t.ai.enabled));
    put(c.ai.connect_apps != t.ai.connect_apps, &|| SettingsChange::AiConnectApps(t.ai.connect_apps));
    put(c.ai.auto_accept_since.is_some() != t.ai.auto_accept_since.is_some(), &|| {
        SettingsChange::AiAutoAccept(t.ai.auto_accept_since.is_some())
    });
    put(c.ai.cloud_ask != t.ai.cloud_ask, &|| SettingsChange::AiCloudAsk(t.ai.cloud_ask));
    put(c.ai.review_button_always != t.ai.review_button_always, &|| {
        SettingsChange::AiReviewButton(t.ai.review_button_always)
    });
    put(c.ai.cross_library_moves != t.ai.cross_library_moves, &|| {
        SettingsChange::AiCrossLibraryMoves(t.ai.cross_library_moves)
    });
    for library in &t.ai.libraries {
        let now = c.ai.libraries.iter().find(|l| l.root == library.root);
        put(now != Some(library), &|| SettingsChange::AiLibrary(library.clone()));
    }
    put(c.ai.endpoint != t.ai.endpoint, &|| SettingsChange::Endpoint(t.ai.endpoint.clone()));
    for agent in c.ai.agents.iter().filter(|agent| !t.ai.agents.iter().any(|a| a.id == agent.id)) {
        put(true, &|| SettingsChange::RemoveAgent(agent.id.clone()));
    }
    for agent in t.ai.agents.iter().filter(|agent| !c.ai.agents.iter().any(|a| a.id == agent.id)) {
        put(true, &|| SettingsChange::AddAgent(agent.clone()));
    }
    put(c.ai.assistant != t.ai.assistant, &|| SettingsChange::Assistant(t.ai.assistant.clone()));
    put(c.ai.auto_connect != t.ai.auto_connect, &|| SettingsChange::AutoConnect(t.ai.auto_connect.clone()));
    out
}

impl SettingsDialog {
    /// The families the open font list shows in themselves; empty while it is closed. The app
    /// loads them as preview fonts.
    pub fn font_previews(&self) -> &std::collections::BTreeSet<String> {
        &self.font_previews
    }

    pub fn is_open(&self) -> bool {
        self.window.open
    }

    pub fn open(&mut self) {
        let page = Category::ALL.iter().position(|category| *category == self.category).unwrap_or(0);
        self.window.open_at(page, Category::ALL.len());
        self.printer_form = printer_form(None);
        self.printer_form_units = config::display_units();
        self.printer_error = false;
        self.printer_editing = None;
        self.found_printers = printer::detect_all();
        self.ai.reset();
        self.landing_input.clear();
        self.rule_inputs.clear();
        self.rule_drag = None;
        if self.import_input.is_empty() {
            self.import_input = export_path().display().to_string();
        }
        self.slicer_form = Default::default();
        self.rescan_slicers();
        self.editor_form = Default::default();
        self.mouse.reset();
        self.refresh_cache_size();
        self.plugins.search_again();
    }

    /// A plugin came or went: the plugin search looks again.
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    pub fn plugins_changed(&mut self) {
        self.plugins.search_again();
    }

    /// Opens the window on the category named as in [`Category::from_name`] (unknown names keep the current one).
    pub fn open_at(&mut self, category: &str) {
        if let Some(category) = Category::from_name(category) {
            self.category = category;
        }
        self.open();
    }

    /// Looks for installed slicers in the background.
    fn rescan_slicers(&mut self) {
        let (tx, rx) = bounded(1);
        std::thread::spawn(move || {
            let mut found = detect_slicers();
            found.extend(detect_editors());
            let _ = tx.send(found);
        });
        self.detect_rx = Some(rx);
    }

    fn refresh_cache_size(&mut self) {
        let (tx, rx) = bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send(cache_dir().map(|d| dir_size(&d)).unwrap_or(0));
        });
        self.cache_size = None;
        self.cache_rx = Some(rx);
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        config: &Config,
        slicers: (&[Slicer], usize),
        editors: (&[Program], usize),
        printers: (&[Printer], usize),
        info: AppInfo,
    ) -> Vec<SettingsChange> {
        let mut changes = Vec::new();
        if self.window.open {
            if let Some(rx) = &self.cache_rx
                && let Ok(size) = rx.try_recv()
            {
                self.cache_size = Some(size);
                self.cache_rx = None;
            }
            if let Some(rx) = &self.detect_rx
                && let Ok(found) = rx.try_recv()
            {
                self.detected = found;
                self.detect_rx = None;
            }
        }
        // The kit's preferences window: the pages in a list on the left with a search over all of
        // them, Restore defaults for the page shown, and OK, Cancel and Apply.
        let texts = [
            t!("settings.title"),
            t!("settings.ok"),
            t!("settings.cancel"),
            t!("settings.apply"),
            t!("settings.search"),
            t!("settings.no_results"),
            t!("settings.defaults"),
            t!("settings.defaults_hint"),
        ];
        let [title, ok, cancel, apply, search, no_results, restore_defaults, restore_defaults_hint] = &texts;
        let labels = Labels {
            title,
            icon: "settings",
            ok,
            cancel,
            apply,
            search,
            no_results,
            restore_defaults,
            restore_defaults_hint,
        };
        let options = Options {
            id: egui::Id::unique("settings_window"),
            prefix: "mg:prefs",
            labels,
            sidebar_footer: None,
            above_page: None,
        };
        let pages = pages();
        let mut host =
            Host { dialog: self, config, lists: (slicers, editors, printers), info: &info, changes: &mut changes };
        prefs_window::show(ctx, &mut host, &pages, &options, |_| {});
        // The page the window shows now is the one opened again next time.
        self.category = Category::ALL.get(self.window.section).copied().unwrap_or_default();
        self.mouse.calibration_modal(ctx, config, info.mouse, info.render, &mut changes);
        changes
    }

    /// The chosen category's sections.
    fn page(&mut self, ui: &mut Ui, config: &Config, lists: Lists, info: &AppInfo, changes: &mut Vec<SettingsChange>) {
        if self.category == Category::General {
            return self.general_page(ui, config, changes);
        }
        let category = self.category;
        sc::settings_page(ui, |ui| {
            // Lines of text and small buttons are as tall as the kit's small buttons, so those
            // sit on the line and not below it.
            ui.spacing_mut().interact_size.y = solidcraft_theme::Tokens::get().form_close_size;
            // Plain labels wrap where they must and are otherwise written out: the kit's window
            // cuts them short, which empties the columns of a table.
            ui.style_mut().wrap_mode = None;
            sc::settings_heading(ui, &category.title(), &category.intro());
            self.sections(ui, config, lists, info, changes);
        });
    }

    /// The sections of the page shown, each under its heading that folds it away.
    fn sections(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        ((slicers, default_slicer), (editors, default_editor), (printers, default_printer)): Lists,
        info: &AppInfo,
        changes: &mut Vec<SettingsChange>,
    ) {
        let sections: &[Section] = match self.category {
            Category::General => &[Section::Language, Section::Appearance, Section::DoubleClick, Section::Panes],
            Category::Library => &[Section::Library, Section::Inbox, Section::Duplicates, Section::Import],
            Category::Tags => {
                &[Section::Tags, Section::Placement, Section::Themes, Section::Properties, Section::States]
            }
            Category::Tiles => &[Section::TilePreview, Section::TileLook, Section::FolderPreviews],
            Category::View => &[Section::Preview, Section::SpaceMouse],
            Category::ModelsUnits => &[Section::Models, Section::Units],
            Category::Programs => &[Section::Slicer, Section::RecentPrints, Section::Editor],
            Category::Printer => &[Section::Printer],
            // With AI off, only the switch: the rest would say "AI is off" four times.
            Category::Ai if !config.ai.enabled => &[Section::Ai],
            Category::Ai => &[
                Section::Ai,
                Section::AiLibraries,
                Section::Assistant,
                Section::Endpoint,
                Section::Mcp,
                Section::AiLog,
            ],
            Category::Plugins => &[Section::Plugins],
            Category::Data => &[Section::UserData, Section::Storage],
            Category::About => {
                about(ui, info);
                &[]
            }
        };
        for &section in sections {
            sc::settings_section(ui, &section.key(), &section.title(), |ui| match section {
                // The General page is drawn by `general_page`.
                Section::Appearance | Section::Language | Section::DoubleClick | Section::Panes => {}
                Section::Library => self.library(ui, config, changes),
                Section::Inbox => inbox(ui, config, changes),
                Section::Duplicates => duplicates(ui, config, changes),
                Section::Import => self.import(ui, config, changes),
                Section::FolderPreviews => folder_previews(ui, config, changes),
                Section::TilePreview => self.tiles.preview(ui, config, info.tag_style, changes),
                Section::TileLook => self.tiles.look(ui, config, changes),
                Section::Tags => tags(ui, config, changes),
                Section::Placement => self.placement(ui, config, info.tags, changes),
                Section::Themes => self.suggestions(ui, config, Group::Themes, changes),
                Section::Properties => self.suggestions(ui, config, Group::Properties, changes),
                Section::States => self.suggestions(ui, config, Group::State, changes),
                Section::Preview => preview(ui, config, changes),
                Section::SpaceMouse => self.mouse.section(ui, config, info.mouse, info.render, changes),
                Section::Models => models(ui, config, changes),
                Section::Units => units(ui, config, changes),
                Section::Slicer => self.program_list(ui, false, slicers, default_slicer, changes),
                Section::RecentPrints => recent_prints(ui, config, changes),
                Section::Editor => self.program_list(ui, true, editors, default_editor, changes),
                Section::Printer => self.printers(ui, printers, default_printer, changes),
                Section::Ai => self.ai.main(ui, config, changes),
                Section::AiLibraries => self.ai.libraries(ui, config, changes),
                Section::Mcp => self.ai.mcp(ui, config, info.ai_connect, changes),
                Section::Assistant => self.ai.assistant(ui, config, info.agents, changes),
                Section::Endpoint => self.ai.endpoint(ui, config, changes),
                Section::AiLog => self.ai.log(ui, changes),
                Section::Plugins => self.plugins.page(ui, config, info.plugins, changes),
                Section::UserData => self.user_data(ui, changes),
                Section::Storage => self.storage(ui, changes),
            });
        }
    }

    /// The suggested tags of one group (themes, print properties, states) offered in the tag menu:
    /// rename, recolour, remove, add.
    fn suggestions(&mut self, ui: &mut Ui, config: &Config, group: Group, changes: &mut Vec<SettingsChange>) {
        let (slot, defaults) = match group {
            Group::Themes => (0, default_themes as fn() -> Vec<SuggestedTag>),
            Group::Properties => (1, default_properties as fn() -> Vec<SuggestedTag>),
            _ => (2, default_states as fn() -> Vec<SuggestedTag>),
        };
        let current = match group {
            Group::Themes => config.themes(),
            Group::Properties => config.properties(),
            _ => config.states(),
        };
        ui.push_id(slot, |ui| self.suggestion_list(ui, group, current, defaults, slot, changes));
    }

    fn suggestion_list(
        &mut self,
        ui: &mut Ui,
        group: Group,
        original: Vec<SuggestedTag>,
        defaults: fn() -> Vec<SuggestedTag>,
        slot: usize,
        changes: &mut Vec<SettingsChange>,
    ) {
        hint(ui, t!("settings.suggest_hint", group = group.title()));
        let german = rust_i18n::locale().starts_with("de");
        let mut themes = original.clone();
        let mut remove = None;
        // All of them on the page, two to a row: the page scrolls and the section folds, a list
        // that scrolls in itself was too low to work in.
        const GAP: f32 = 16.0;
        // Never wider than the page: a row that asks for more makes the window grow.
        let column = ((ui.available_width() - GAP) / 2.0).floor().max(120.0);
        let rows = themes.len().div_ceil(2);
        for row in 0..rows {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (side, i) in [row, row + rows].into_iter().enumerate() {
                    if side == 1 {
                        ui.add_space(GAP);
                    }
                    let Some(theme) = themes.get_mut(i) else { continue };
                    ui.allocate_ui_with_layout(
                        egui::vec2(column, ui.spacing().interact_size.y),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_width(column);
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.push_id(i, |ui| {
                                let (bg, _) = PALETTE[usize::from(tags::theme_slot(theme))];
                                // A round dot, though the line is taller than it.
                                ui.spacing_mut().interact_size.y = 20.0;
                                let swatch = egui::Button::new("")
                                    .fill(egui::Color32::from_rgb(bg[0], bg[1], bg[2]))
                                    .min_size(egui::vec2(20.0, 20.0))
                                    .corner_radius(10.0);
                                egui::containers::menu::MenuButton::from_button(swatch).ui(ui, |ui| {
                                    ui.set_max_width(6.0 * 26.0);
                                    ui.horizontal_wrapped(|ui| {
                                        for (slot, (bg, _)) in PALETTE.iter().enumerate() {
                                            let fill = egui::Color32::from_rgb(bg[0], bg[1], bg[2]);
                                            let pick = egui::Button::new("")
                                                .fill(fill)
                                                .min_size(egui::vec2(20.0, 20.0))
                                                .corner_radius(10.0);
                                            if ui.add(pick).clicked() {
                                                theme.color = Some(slot as u8);
                                                ui.close();
                                            }
                                        }
                                    });
                                });
                                let linked = theme.de == theme.en;
                                let name = if german { &mut theme.de } else { &mut theme.en };
                                // The field takes what the dot and the cross leave (its own
                                // margins and the cross with its gap come off).
                                let field = (ui.available_width() - 44.0).max(40.0);
                                super::settings_kit::text_field(ui, name, "", field);
                                if linked {
                                    let name = if german { theme.de.clone() } else { theme.en.clone() };
                                    theme.de = name.clone();
                                    theme.en = name;
                                }
                                if super::settings_kit::small(ui, "✖", &t!("settings.suggest_remove")).clicked() {
                                    remove = Some(i);
                                }
                            });
                        },
                    );
                }
            });
        }
        if let Some(i) = remove {
            themes.remove(i);
        }
        ui.horizontal(|ui| {
            let field =
                super::settings_kit::text_field(ui, &mut self.suggest_input[slot], &t!("settings.suggest_new"), 220.0);
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let name = self.suggest_input[slot].trim().to_owned();
            let can_add = !name.is_empty() && !themes.iter().any(|t| t.matches(&name));
            if (super::settings_kit::button_if(ui, can_add, "+").clicked() || enter) && can_add {
                themes.push(SuggestedTag::named(&name));
                self.suggest_input[slot].clear();
                field.request_focus();
            }
        });
        ui.horizontal(|ui| {
            if sc::button(ui, t!("settings.suggest_missing"), sc::Button::Normal).clicked() {
                for theme in defaults() {
                    if !themes.iter().any(|t| t.matches(&theme.de) || t.matches(&theme.en)) {
                        themes.push(theme);
                    }
                }
            }
            if sc::button(ui, t!("settings.suggest_reset"), sc::Button::Normal).clicked() {
                themes = defaults();
                if themes == original {
                    return;
                }
                changes.push(SettingsChange::Suggestions(group, None));
                themes = original.clone();
            }
        });
        if themes != original {
            changes.push(SettingsChange::Suggestions(group, Some(themes)));
        }
    }

    /// The General page on the kit's settings controls: sections that fold, rows with their label
    /// in a column, joined buttons, lists that open, switches with their note under them.
    fn general_page(&mut self, ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
        /// The label column of the page's rows.
        const LABELS: f32 = 190.0;
        /// A note under what it explains.
        fn note(ui: &mut Ui, text: &str) {
            // Closer to its control than the page's rhythm puts the next row.
            ui.add_space(-solidcraft_theme::Tokens::get().space_6);
            sc::text(ui, text, sc::Text::Note(text));
        }
        sc::settings_page(ui, |ui| {
            sc::settings_heading(ui, &Category::General.title(), &Category::General.intro());
            sc::settings_section(ui, "language", &t!("settings.language"), |ui| {
                sc::settings_form(ui, LABELS, |ui| {
                    sc::row(ui, &t!("settings.language_ui"), |ui| {
                        let automatic = t!("settings.language_auto");
                        let (options, codes) = ([&*automatic, "Deutsch", "English"], [None, Some("de"), Some("en")]);
                        let mut index = codes.iter().position(|code| *code == config.language.as_deref()).unwrap_or(0);
                        if sc::settings_segments(ui, &t!("settings.language_ui"), &mut index, &options) {
                            changes.push(SettingsChange::Language(codes[index].map(str::to_owned)));
                        }
                    });
                });
            });
            sc::settings_section(ui, "appearance", &t!("settings.appearance"), |ui| {
                note(ui, &t!("settings.appearance_hint"));
                // Not among search results: the cards are pictures, and their room would stay
                // empty above the rows the search found.
                if !sc::settings_search::querying(ui) {
                    let labels = [&*t!("settings.appearance_dark"), &*t!("settings.appearance_light")];
                    if let Some(theme) = self.themes.show(ui, labels) {
                        changes.push(SettingsChange::UiTheme(Some(theme)));
                    }
                }
                sc::settings_block(ui, &t!("settings.font"), LABELS, |ui| {
                    if let Some(font) = self.font_picker(ui, config.ui_font.as_deref()) {
                        changes.push(SettingsChange::UiFont(font));
                    }
                    note(ui, &t!("settings.font_hint"));
                });
                // On macOS the system's title bar stays: its window buttons belong to it.
                if cfg!(not(target_os = "macos")) {
                    sc::settings_block(ui, &t!("settings.title_bar"), LABELS, |ui| {
                        let (app, system) = (t!("settings.title_bar_app"), t!("settings.title_bar_system"));
                        let mut index = usize::from(config.title_bar == config::TitleBar::System);
                        if sc::settings_segments(ui, &t!("settings.title_bar"), &mut index, &[&*app, &*system]) {
                            let title_bar = if index == 1 { config::TitleBar::System } else { config::TitleBar::App };
                            changes.push(SettingsChange::TitleBar(title_bar));
                        }
                        note(ui, &t!("settings.title_bar_hint"));
                    });
                }
            });
            sc::settings_section(ui, "double_click", &t!("settings.double_click"), |ui| {
                sc::settings_block(ui, &t!("settings.double_click_row"), LABELS, |ui| {
                    let actions = [
                        (DoubleClick::Window, t!("settings.double_click_window")),
                        (DoubleClick::Preview, t!("settings.double_click_preview")),
                        (DoubleClick::Slicer, t!("settings.double_click_slicer")),
                        (DoubleClick::Editor, t!("settings.double_click_editor")),
                        (DoubleClick::DefaultApp, t!("settings.double_click_default_app")),
                    ];
                    let current = actions.iter().find(|(action, _)| *action == config.double_click);
                    let current = current.map(|(_, label)| label.to_string()).unwrap_or_default();
                    sc::settings_control_width(ui, 260.0, |ui| {
                        sc::dropdown_menu(ui, "settings_double_click", &current, |ui| {
                            for (action, label) in &actions {
                                let chosen = config.double_click == *action;
                                if sc::list_picker(ui, &format!("{action:?}"), label, chosen).clicked() && !chosen {
                                    changes.push(SettingsChange::DoubleClick(*action));
                                }
                            }
                        });
                    });
                    note(ui, &t!("settings.double_click_hint"));
                });
            });
            sc::settings_section(ui, "panes", &t!("settings.panes"), |ui| {
                let mut locked = config.lock_panes;
                sc::checkbox(ui, &t!("settings.panes_lock"), &mut locked);
                if locked != config.lock_panes {
                    changes.push(SettingsChange::LockPanes(locked));
                }
                note(ui, &t!("settings.panes_hint"));
                let mut fold = config.fold_for_plugins;
                sc::checkbox(ui, &t!("settings.panes_fold"), &mut fold);
                if fold != config.fold_for_plugins {
                    changes.push(SettingsChange::FoldForPlugins(fold));
                }
                note(ui, &t!("settings.panes_fold_hint"));
                if sc::button(ui, t!("pane.reset"), sc::Button::Normal).clicked() {
                    changes.push(SettingsChange::ResetLayout);
                }
            });
        });
    }

    /// The interface font as a list that opens: the theme's font and the desktop's on top, then
    /// every installed family, with a search. Gives the choice as the setting takes it.
    fn font_picker(&mut self, ui: &mut Ui, setting: Option<&str>) -> Option<Option<String>> {
        use super::fonts::{self, Choice};
        let (theme, system) = (t!("settings.font_theme"), t!("settings.font_system"));
        let current = match Choice::of(setting) {
            Choice::Theme => theme.to_string(),
            Choice::System => system.to_string(),
            Choice::Family(family) => family.to_owned(),
        };
        let families = fonts::families(ui.ctx());
        let mut open = false;
        let mut chosen = None;
        ui.allocate_ui(egui::vec2(260.0_f32.min(ui.available_width()), 0.0), |ui| {
            let close = egui::PopupCloseBehavior::CloseOnClickOutside;
            sc::dropdown_menu_with_close_behavior(ui, "settings_ui_font", &current, close, |ui| {
                open = true;
                sc::hint_field(ui, &mut self.font_filter, &t!("settings.font_search"));
                let query = self.font_filter.trim().to_lowercase();
                if query != self.font_query {
                    self.font_query = query.clone();
                    self.font_window.reset();
                }
                let matches = |name: &str| query.is_empty() || name.to_lowercase().contains(&query);
                // The two fixed choices, then the families: what a click sets, the label, and
                // the family the row is written in.
                let fixed =
                    [(None, &*theme, fonts::theme_family()), (Some(fonts::SYSTEM), &*system, fonts::system_family())];
                let rows: Vec<(Option<&str>, &str, egui::FontFamily)> = fixed
                    .into_iter()
                    .filter(|(_, label, _)| matches(label))
                    .chain(
                        families
                            .into_iter()
                            .flatten()
                            .filter(|family| matches(family))
                            .map(|family| (Some(family.as_str()), family.as_str(), fonts::preview_family(family))),
                    )
                    .collect();
                let tokens = solidcraft_theme::Tokens::get();
                let row_height = sc::row_height();
                egui::ScrollArea::vertical()
                    .id_salt("settings_ui_font_list")
                    .max_height(tokens.space_24 * 10.0)
                    .min_scrolled_height(tokens.space_24 * 10.0)
                    .auto_shrink([false, false])
                    .show_rows(ui, row_height, rows.len(), |ui, visible| {
                        if self.font_window.follow(visible.clone(), rows.len()) {
                            self.font_previews = rows[self.font_window.rows()]
                                .iter()
                                .filter_map(|(value, _, _)| value.filter(|v| *v != fonts::SYSTEM))
                                .map(str::to_owned)
                                .collect();
                        }
                        for (value, label, family) in &rows[visible] {
                            let mut text = RichText::new(*label);
                            if fonts::is_loaded(ui.ctx(), family) {
                                text = text.family(family.clone()).size(tokens.font_body + 1.0);
                            }
                            let size = egui::vec2(ui.available_width(), row_height);
                            if ui.add_sized(size, egui::Button::selectable(current == *label, text)).clicked() {
                                chosen = Some(value.map(str::to_owned));
                                ui.close();
                            }
                        }
                    });
                if families.is_none() {
                    ui.label(RichText::new(t!("settings.font_listing")).weak());
                }
            });
        });
        if !open {
            self.font_window.reset();
            self.font_previews.clear();
        }
        if chosen.is_some() {
            self.font_filter.clear();
        }
        chosen
    }

    fn library(&mut self, ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
        hint(ui, t!("settings.library_hint"));
        for root in &config.library_roots {
            ui.horizontal(|ui| {
                if super::settings_kit::small(ui, "✖", &t!("sidebar.remove_root")).clicked() {
                    changes.push(SettingsChange::RemoveRoot(root.clone()));
                }
                ui.label(root.display().to_string());
            });
        }
        ui.horizontal(|ui| {
            super::settings_kit::text_field(ui, &mut self.root_input, &t!("settings.folder_hint"), 320.0);
            if sc::button(ui, t!("settings.add"), sc::Button::Normal).clicked() {
                let path = PathBuf::from(self.root_input.trim());
                if path.is_dir() {
                    changes.push(SettingsChange::AddRoot(path));
                    self.root_input.clear();
                    self.root_error = None;
                } else {
                    self.root_error = Some(t!("settings.not_a_folder").into_owned());
                }
            }
        });
        if let Some(e) = &self.root_error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
    }

    /// Placing files by tag: the switch, and the folder each theme (and any other tag of the
    /// library) goes to.
    fn placement(&mut self, ui: &mut Ui, config: &Config, tags: &[(String, usize)], changes: &mut Vec<SettingsChange>) {
        let mut on = config.placement.enabled;
        if check(ui, &mut on, t!("settings.placement_enable"), t!("settings.placement_hint")) {
            changes.push(SettingsChange::Placement(on));
        }
        if !on {
            return;
        }
        let themes = config.themes();
        let (properties, states) = (config.properties(), config.states());
        let german = config.german();
        let theme_name = |t: &SuggestedTag| if german { t.de.clone() } else { t.en.clone() };
        let ordered = config.placement_tags();
        if self.rule_inputs.is_empty() || self.rule_ordered != ordered.len() {
            let row = |tag: &str| RuleRow {
                tag: tag.to_owned(),
                folder: config.folder_for_tag(tag).unwrap_or_default(),
                own: themes.iter().find(|t| t.matches(tag)).map(theme_name),
            };
            self.rule_inputs = ordered.iter().map(|t| row(t)).collect();
            let known = |tag: &str| {
                ordered.iter().any(|o| o.eq_ignore_ascii_case(tag))
                    || themes.iter().chain(&properties).chain(&states).any(|t| t.matches(tag))
            };
            self.rule_inputs.extend(tags.iter().filter(|(t, _)| !known(t)).map(|(t, _)| row(t)));
            self.rule_ordered = ordered.len();
        }
        hint(ui, t!("settings.placement_rules_hint"));
        let (mut edited, mut bad, mut moved) = (false, false, None);
        let valid = |folder: &str| {
            folder
                .split('/')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .all(|s| meshgrove_core::fsops::check_name(s).is_ok())
        };
        let ordered_rows = self.rule_ordered;
        // Rows of the order can be dragged by their handle only, so typing in a folder field or
        // clicking a button never moves anything.
        let dragging = self.rule_drag;
        let (mut row_rects, mut drag_start, mut dropped) = (Vec::new(), None, false);
        let grid =
            egui::Grid::new("placement_rules").num_columns(5).min_col_width(12.0).spacing([12.0, 4.0]).show(ui, |ui| {
                for (i, row) in self.rule_inputs.iter_mut().enumerate() {
                    if i == ordered_rows {
                        ui.label("");
                        ui.label(RichText::new(t!("settings.placement_other_tags")).weak());
                        ui.end_row();
                    }
                    if i < ordered_rows {
                        let handle = drag_handle(ui, dragging == Some(i));
                        if handle.drag_started() {
                            drag_start = Some(i);
                        }
                        dropped |= handle.drag_stopped();
                        row_rects.push(handle.rect);
                    } else {
                        ui.label("");
                    }
                    ui.label(row.tag.as_str());
                    let field =
                        super::settings_kit::text_field(ui, &mut row.folder, &t!("settings.placement_none"), 220.0);
                    let ok = valid(&row.folder);
                    bad |= !ok;
                    edited |= field.lost_focus() && ok;
                    ui.horizontal(|ui| {
                        if i < ordered_rows {
                            if super::settings_kit::small_if(ui, i > 0, "▲", &t!("settings.placement_up")).clicked() {
                                moved = Some((i, i - 1));
                            }
                            if super::settings_kit::small_if(
                                ui,
                                i + 1 < ordered_rows,
                                "▼",
                                &t!("settings.placement_down"),
                            )
                            .clicked()
                            {
                                moved = Some((i, i + 1));
                            }
                        }
                        if !row.folder.trim().is_empty()
                            && super::settings_kit::small(ui, "✖", &t!("settings.placement_clear")).clicked()
                        {
                            row.folder.clear();
                            edited = true;
                        }
                    });
                    ui.end_row();
                }
            });
        if bad {
            ui.colored_label(ui.visuals().error_fg_color, t!("settings.placement_bad_folder"));
        }
        if drag_start.is_some() {
            self.rule_drag = drag_start;
        }
        if let Some(from) = self.rule_drag {
            // Where the row would land: before the first row whose middle is below the pointer.
            let pointer = ui.ctx().pointer_interact_pos();
            let slot = pointer.map_or(from, |p| row_rects.iter().filter(|r| r.center().y < p.y).count());
            let width = grid.response.rect.x_range();
            let y = match row_rects.get(slot) {
                Some(r) => r.top() - 2.0,
                None => row_rects.last().map_or(grid.response.rect.top(), |r| r.bottom() + 2.0),
            };
            if let Some(r) = row_rects.get(from) {
                let band = egui::Rect::from_x_y_ranges(width, r.y_range());
                ui.painter().rect_filled(band, 3.0, ui.visuals().selection.bg_fill.gamma_multiply(0.25));
            }
            ui.painter().hline(width, y, egui::Stroke::new(2.0, ui.visuals().selection.stroke.color));
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            // Near the edge of the page, the page scrolls along.
            let clip = ui.clip_rect();
            if let Some(p) = pointer {
                let edge = 40.0;
                let speed = if p.y < clip.top() + edge {
                    (clip.top() + edge - p.y).min(edge) * 0.4
                } else if p.y > clip.bottom() - edge {
                    -(p.y - (clip.bottom() - edge)).min(edge) * 0.4
                } else {
                    0.0
                };
                if speed != 0.0 {
                    ui.scroll_with_delta(egui::vec2(0.0, speed));
                }
            }
            ui.ctx().request_repaint();
            if dropped || !ui.input(|i| i.pointer.any_down()) {
                self.rule_drag = None;
                // Taken out above its slot, everything below moves up by one.
                let to = if slot > from { slot - 1 } else { slot };
                if to != from && from < ordered_rows {
                    moved = Some((from, to));
                }
            }
        }
        if let Some((from, to)) = moved {
            let row = self.rule_inputs.remove(from);
            self.rule_inputs.insert(to, row);
            let order = self.rule_inputs[..ordered_rows].iter().map(|r| r.tag.clone()).collect();
            changes.push(SettingsChange::PlacementOrder(order));
        }
        if edited {
            // Rules only where a tag's folder differs from the theme's own name.
            let mut rules = Vec::new();
            for row in &self.rule_inputs {
                let folder = row.folder.trim();
                match &row.own {
                    Some(own) if folder == own => {}
                    Some(_) => rules.push(FolderRule { tag: row.tag.clone(), folder: folder.to_owned() }),
                    None if !folder.is_empty() => {
                        rules.push(FolderRule { tag: row.tag.clone(), folder: folder.to_owned() })
                    }
                    None => {}
                }
            }
            changes.push(SettingsChange::FolderRules(rules));
        }
    }

    /// The import folders, where their files go, and when they are looked at.
    fn import(&mut self, ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
        hint(ui, t!("settings.import_folders_hint"));
        let folders = &config.import.folders;
        for folder in folders {
            ui.horizontal(|ui| {
                if super::settings_kit::small(ui, "✖", &t!("settings.import_remove")).clicked() {
                    let kept: Vec<PathBuf> = folders.iter().filter(|f| *f != folder).cloned().collect();
                    changes.push(SettingsChange::ImportFolders(kept));
                }
                ui.label(folder.display().to_string());
            });
        }
        ui.horizontal(|ui| {
            super::settings_kit::text_field(ui, &mut self.import_folder_input, &t!("settings.folder_hint"), 320.0);
            if sc::button(ui, t!("settings.add"), sc::Button::Normal).clicked() {
                let path = PathBuf::from(self.import_folder_input.trim());
                if path.is_dir() {
                    let mut all = folders.clone();
                    if !all.contains(&path) {
                        all.push(path);
                    }
                    changes.push(SettingsChange::ImportFolders(all));
                    self.import_folder_input.clear();
                    self.import_folder_error = None;
                } else {
                    self.import_folder_error = Some(t!("settings.not_a_folder").into_owned());
                }
            }
            if let Some(downloads) = dirs::download_dir().filter(|d| !folders.contains(d) && d.is_dir())
                && ui
                    .button(t!("settings.import_add_downloads"))
                    .on_hover_text(downloads.display().to_string())
                    .clicked()
            {
                let mut all = folders.clone();
                all.push(downloads);
                changes.push(SettingsChange::ImportFolders(all));
            }
        });
        if let Some(e) = &self.import_folder_error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        if config.library_roots.is_empty() {
            hint(ui, t!("settings.import_no_library"));
            return;
        }
        ui.add_space(4.0);
        row(ui, &t!("settings.import_library"), |ui| {
            let current = config.import_root().map(|r| r.to_path_buf());
            let shown = current.as_deref().map(listing_name).unwrap_or_default();
            super::settings_kit::dropdown(ui, "import_library", &shown, 220.0, |ui| {
                for root in &config.library_roots {
                    if super::settings_kit::choice_row(ui, current.as_deref() == Some(root), &listing_name(root)) {
                        changes.push(SettingsChange::ImportTarget {
                            library: Some(root.clone()),
                            landing: config.import.landing.clone(),
                        });
                    }
                }
            });
        });
        row(ui, &t!("settings.import_landing"), |ui| {
            if self.landing_input.is_empty() {
                self.landing_input = config.import.landing.clone();
            }
            let field = super::settings_kit::text_field(ui, &mut self.landing_input, "", 200.0);
            let name = self.landing_input.trim();
            let valid = !name.is_empty() && meshgrove_core::fsops::check_name(name).is_ok();
            if field.lost_focus() && valid && name != config.import.landing {
                changes.push(SettingsChange::ImportTarget {
                    library: config.import.library.clone(),
                    landing: name.to_owned(),
                });
            }
            if !valid {
                ui.colored_label(ui.visuals().error_fg_color, t!("settings.import_bad_name"));
            }
            hint(ui, t!("settings.import_landing_hint"));
        });
        row(ui, &t!("settings.import_at_start"), |ui| {
            let mut choice = config.import.at_start;
            let options = [
                (ImportAtStart::Off, t!("settings.import_at_start_off").into_owned()),
                (ImportAtStart::Ask, t!("settings.import_at_start_ask").into_owned()),
                (ImportAtStart::Auto, t!("settings.import_at_start_auto").into_owned()),
            ];
            if segmented(ui, &mut choice, &options) {
                changes.push(SettingsChange::ImportAtStart(choice));
            }
            hint(ui, t!("settings.import_at_start_hint"));
        });
        row(ui, &t!("settings.import_archives"), |ui| {
            let mut choice = config.import.archives;
            let options = [
                (ArchiveImport::Whole, t!("import.archives_whole").into_owned()),
                (ArchiveImport::Extract, t!("import.archives_extract").into_owned()),
            ];
            if segmented(ui, &mut choice, &options) {
                changes.push(SettingsChange::ImportArchives(choice));
            }
            hint(ui, t!("settings.import_archives_hint"));
        });
    }

    /// The slicers (or, with `editors`, the mesh editors) models are sent to: the added ones (one
    /// is the default), the installed ones that can be added with a click, and a form for any
    /// other.
    fn program_list(
        &mut self,
        ui: &mut Ui,
        editors: bool,
        programs: &[Program],
        default: usize,
        changes: &mut Vec<SettingsChange>,
    ) {
        let (none, explained) = if editors {
            ("settings.editor_none", "settings.editor_hint")
        } else {
            ("settings.slicer_none", "settings.slicers_hint")
        };
        let send = move |list: Vec<Program>, default: Option<String>, changes: &mut Vec<SettingsChange>| {
            changes.push(if editors {
                SettingsChange::Editors { list, default }
            } else {
                SettingsChange::Slicers { list, default }
            });
        };
        let default_name = programs.get(default).map(|s| s.name.clone());
        if programs.is_empty() {
            ui.weak(t!(none));
        }
        for (i, program) in programs.iter().enumerate() {
            ui.horizontal(|ui| {
                let path = program.program.display().to_string();
                let tip = t!("settings.slicers_default");
                if super::settings_kit::choice_tile(ui, i == default, &program.name, &path, &tip, 34.0) && i != default
                {
                    send(programs.to_vec(), Some(program.name.clone()), changes);
                }
                if super::settings_kit::small(ui, "✖", &t!("settings.slicers_remove")).clicked() {
                    let list: Vec<Program> = programs.iter().filter(|s| s.name != program.name).cloned().collect();
                    send(list, default_name.clone().filter(|n| *n != program.name), changes);
                }
            });
        }
        ui.add_space(6.0);
        ui.label(RichText::new(t!("settings.slicers_found")).small().weak());
        let addable: Vec<&Detected> = self
            .detected
            .iter()
            .filter(|d| d.kind.is_slicer() != editors)
            .filter(|d| !programs.iter().any(|s| s.program == d.program.program))
            .collect();
        for d in &addable {
            ui.horizontal(|ui| {
                if sc::button(ui, "+", sc::Button::Normal).on_hover_text(t!("settings.slicers_add")).clicked() {
                    let mut list = programs.to_vec();
                    list.push(d.program.clone());
                    send(list, default_name.clone().or_else(|| Some(d.program.name.clone())), changes);
                }
                ui.label(&d.name);
                match d.kind {
                    ProgramKind::Fdm => ui.weak(t!("settings.slicers_fdm")),
                    ProgramKind::Sla => ui.weak(t!("settings.slicers_sla")),
                    ProgramKind::Editor => ui.weak(""),
                };
            });
        }
        if addable.is_empty() && self.detect_rx.is_none() {
            ui.weak(t!("settings.slicers_nothing_more"));
        }
        ui.horizontal(|ui| {
            if super::settings_kit::button_if(ui, self.detect_rx.is_none(), t!("settings.slicers_rescan")).clicked() {
                self.rescan_slicers();
            }
            if self.detect_rx.is_some() {
                crate::ui::loading::mark(ui);
            }
        });
        let by_hand = if editors { "settings.editors_by_hand" } else { "settings.slicers_by_hand" };
        ui.collapsing(t!(by_hand), |ui| {
            let form = if editors { &mut self.editor_form } else { &mut self.slicer_form };
            let (name, program, workdir) = form;
            egui::Grid::new(("program_form", editors)).num_columns(2).show(ui, |ui| {
                ui.label(t!("settings.slicer_name"));
                super::settings_kit::text_field(ui, name, "", 320.0);
                ui.end_row();
                ui.label(t!("settings.slicer_program"));
                super::settings_kit::text_field(ui, program, "", 320.0);
                ui.end_row();
                if !editors {
                    ui.label(t!("settings.slicer_workdir"));
                    super::settings_kit::text_field(ui, workdir, "", 320.0);
                    ui.end_row();
                }
            });
            if sc::button(ui, t!("settings.slicers_add"), sc::Button::Normal).clicked() && !program.trim().is_empty() {
                // Keep the environment of a detected program with the same path (e.g. Anycubic's libraries).
                let env = self
                    .detected
                    .iter()
                    .find(|d| d.program.program == Path::new(program.trim()))
                    .map(|d| d.program.env.clone())
                    .unwrap_or_default();
                let fallback = if editors { "Editor" } else { "Slicer" };
                let new = Program {
                    name: if name.trim().is_empty() { fallback.into() } else { name.trim().to_owned() },
                    program: program.trim().into(),
                    args: Vec::new(),
                    env,
                    workdir: (!workdir.trim().is_empty()).then(|| workdir.trim().into()),
                };
                let mut list: Vec<Program> = programs.iter().filter(|s| s.name != new.name).cloned().collect();
                let first = list.is_empty();
                let new_name = new.name.clone();
                list.push(new);
                send(list, if first { Some(new_name) } else { default_name.clone() }, changes);
                *form = Default::default();
            }
        });
        hint(ui, t!(explained));
    }

    /// The printers models are checked against: the added ones (one is the main printer), the
    /// ones the slicers know that can be added with a click, and a form to add or edit one.
    fn printers(&mut self, ui: &mut Ui, printers: &[Printer], default: usize, changes: &mut Vec<SettingsChange>) {
        let default_name = printers.get(default).map(|p| p.name.clone());
        let send = |list: Vec<Printer>, default: Option<String>, changes: &mut Vec<SettingsChange>| {
            changes.push(SettingsChange::Printers { list, default });
        };
        if printers.is_empty() {
            ui.weak(t!("settings.printer_none"));
        }
        let units = config::display_units();
        for (i, p) in printers.iter().enumerate() {
            ui.horizontal(|ui| {
                let tip = t!("settings.printers_default");
                if super::settings_kit::choice_tile(ui, i == default, &p.name, &printer_summary(p), &tip, 68.0)
                    && i != default
                {
                    send(printers.to_vec(), Some(p.name.clone()), changes);
                }
                if super::settings_kit::small(ui, "✏", &t!("settings.printers_edit")).clicked() {
                    self.printer_form = printer_form(Some(p));
                    self.printer_form_units = units;
                    self.printer_editing = Some(p.name.clone());
                    self.printer_error = false;
                }
                if super::settings_kit::small(ui, "✖", &t!("settings.printers_remove")).clicked() {
                    let list: Vec<Printer> = printers.iter().filter(|q| q.name != p.name).cloned().collect();
                    send(list, default_name.clone().filter(|n| *n != p.name), changes);
                }
            });
        }
        ui.add_space(6.0);
        ui.label(RichText::new(t!("settings.printers_found")).small().weak());
        let addable: Vec<DetectedPrinter> = self
            .found_printers
            .iter()
            .filter(|d| !printers.iter().any(|p| p.name.eq_ignore_ascii_case(&d.printer.name)))
            .cloned()
            .collect();
        for d in &addable {
            ui.horizontal(|ui| {
                if sc::button(ui, "+", sc::Button::Normal).on_hover_text(t!("settings.printers_add")).clicked() {
                    let mut list = printers.to_vec();
                    list.push(d.printer.clone());
                    send(list, default_name.clone().or_else(|| Some(d.printer.name.clone())), changes);
                }
                ui.label(&d.printer.name);
                ui.weak(format!("{} · {}", printer_summary(&d.printer), d.slicer));
            });
        }
        if addable.is_empty() {
            ui.weak(t!("settings.printers_nothing_more"));
        }
        ui.horizontal(|ui| {
            if addable.len() > 1 && sc::button(ui, t!("settings.printers_add_all"), sc::Button::Normal).clicked() {
                let mut list = printers.to_vec();
                list.extend(addable.iter().map(|d| d.printer.clone()));
                let first = addable.iter().find(|d| d.selected).unwrap_or(&addable[0]).printer.name.clone();
                send(list, default_name.clone().or(Some(first)), changes);
            }
            if sc::button(ui, t!("settings.slicers_rescan"), sc::Button::Normal)
                .on_hover_text(t!("settings.printer_auto_hint"))
                .clicked()
            {
                self.found_printers = printer::detect_all();
            }
        });

        // The display unit changed since the form was filled: convert what is typed, so Apply
        // does not read millimetres as inches (or the other way round).
        let form = &mut self.printer_form;
        if self.printer_form_units != units {
            let before = self.printer_form_units;
            for value in &mut form[1..4] {
                if let Ok(v) = value.trim().replace(',', ".").parse::<f32>() {
                    *value = mm(units.from_mm(before.to_mm(v)));
                }
            }
        }
        self.printer_form_units = units;
        let heading = match &self.printer_editing {
            Some(name) => t!("settings.printers_editing", name = name),
            None => t!("settings.printers_by_hand"),
        };
        let editing = self.printer_editing.is_some();
        egui::CollapsingHeader::new(heading).id_salt("printer_form").open(editing.then_some(true)).show(ui, |ui| {
            egui::Grid::new("printer_form").num_columns(2).show(ui, |ui| {
                ui.label(t!("settings.printer_name"));
                super::settings_kit::text_field(ui, &mut form[0], "", 320.0);
                ui.end_row();
                ui.label(t!("settings.printer_volume"));
                ui.horizontal(|ui| {
                    for (i, axis) in ["X", "Y", "Z"].iter().enumerate() {
                        ui.label(*axis);
                        super::settings_kit::text_field(ui, &mut form[i + 1], "", 56.0);
                    }
                    ui.label(units.label());
                });
                ui.end_row();
                ui.label(t!("settings.printer_nozzle"));
                ui.horizontal(|ui| {
                    super::settings_kit::text_field(ui, &mut form[4], "", 56.0);
                    ui.label("mm");
                });
                ui.end_row();
                ui.label(t!("settings.printer_materials"));
                super::settings_kit::text_field(ui, &mut form[5], "PLA, PETG", 320.0);
                ui.end_row();
            });
            ui.horizontal(|ui| {
                let label = if editing { t!("settings.apply") } else { t!("settings.printers_add") };
                if sc::button(ui, label, sc::Button::Normal).clicked() {
                    let length = |s: &str| {
                        s.trim().replace(',', ".").parse::<f32>().ok().filter(|v| *v > 0.0).map(|v| units.to_mm(v))
                    };
                    match (length(&form[1]), length(&form[2]), length(&form[3])) {
                        (Some(x), Some(y), Some(z)) => {
                            let name = match form[0].trim() {
                                "" => t!("settings.printer_default_name").into_owned(),
                                name => name.to_owned(),
                            };
                            let old = self.printer_editing.take();
                            let kept = old.as_ref().and_then(|o| printers.iter().find(|p| p.name == *o));
                            let new = Printer {
                                name: name.clone(),
                                x,
                                y,
                                z,
                                nozzle: form[4].trim().replace(',', ".").parse().ok().filter(|v: &f32| *v > 0.0),
                                materials: form[5]
                                    .split(',')
                                    .map(str::trim)
                                    .filter(|m| !m.is_empty())
                                    .map(str::to_owned)
                                    .collect(),
                                gcode_names: kept.map(|p| p.gcode_names.clone()).unwrap_or_default(),
                            };
                            let replaced = |p: &&Printer| p.name == name || Some(&p.name) == old.as_ref();
                            let mut list: Vec<Printer> = printers.to_vec();
                            match list.iter().position(|p| replaced(&p)) {
                                Some(i) => list[i] = new,
                                None => list.push(new),
                            }
                            let default = match &default_name {
                                Some(d) if Some(d) == old.as_ref() => Some(name),
                                Some(d) => Some(d.clone()),
                                None => Some(name),
                            };
                            send(list, default, changes);
                            *form = printer_form(None);
                            self.printer_error = false;
                        }
                        _ => self.printer_error = true,
                    }
                }
                if editing && sc::button(ui, t!("settings.cancel"), sc::Button::Normal).clicked() {
                    self.printer_editing = None;
                    *form = printer_form(None);
                    self.printer_error = false;
                }
            });
            if self.printer_error {
                ui.colored_label(ui.visuals().error_fg_color, t!("settings.printer_invalid"));
            }
        });
        ui.weak(t!("settings.printers_hint"));
    }

    fn user_data(&mut self, ui: &mut Ui, changes: &mut Vec<SettingsChange>) {
        hint(ui, t!("settings.user_data_hint"));
        ui.add_space(2.0);
        let backups = userdata::backup_dir().map(|d| userdata::backups(&d)).unwrap_or_default();
        row(ui, &t!("settings.backups_row"), |ui| {
            // The text on a line of its own: beside the button it would not wrap and widen the window.
            let text = match backups.last().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()) {
                Some(last) => {
                    t!("settings.backups", count = backups.len(), last = last.trim_start_matches("user-data-"))
                }
                None => t!("settings.backups_none"),
            };
            ui.add(egui::Label::new(text).wrap());
            if sc::button(ui, t!("settings.open_folder"), sc::Button::Normal).clicked() {
                changes.push(SettingsChange::OpenBackups);
            }
        });
        row(ui, &t!("settings.export_row"), |ui| {
            if sc::button(ui, t!("settings.export"), sc::Button::Normal)
                .on_hover_text(export_path().display().to_string())
                .clicked()
            {
                changes.push(SettingsChange::Export(export_path()));
            }
        });
        row(ui, &t!("settings.import_row"), |ui| {
            ui.horizontal(|ui| {
                super::settings_kit::text_field(ui, &mut self.import_input, "", 280.0);
                if sc::button(ui, t!("settings.import"), sc::Button::Normal)
                    .on_hover_text(t!("settings.import_hint"))
                    .clicked()
                {
                    changes.push(SettingsChange::Import(PathBuf::from(self.import_input.trim())));
                }
            });
        });
    }

    fn storage(&mut self, ui: &mut Ui, changes: &mut Vec<SettingsChange>) {
        row(ui, &t!("settings.cache_row"), |ui| {
            ui.horizontal(|ui| {
                match self.cache_size {
                    Some(size) => ui.label(t!("settings.cache_size", size = i18n::file_size(size))),
                    None => crate::ui::loading::mark(ui),
                };
                if sc::button(ui, t!("settings.clear_cache"), sc::Button::Normal)
                    .on_hover_text(t!("settings.clear_cache_hint"))
                    .clicked()
                {
                    changes.push(SettingsChange::ClearCache);
                    self.refresh_cache_size();
                }
            });
        });
    }
}

/// A library root as the sidebar names it.
fn listing_name(root: &Path) -> String {
    meshgrove_core::listing::file_name(root)
}

fn printer_form(printer: Option<&Printer>) -> [String; 6] {
    match printer {
        Some(p) => {
            let units = config::display_units();
            [
                p.name.clone(),
                mm(units.from_mm(p.x)),
                mm(units.from_mm(p.y)),
                mm(units.from_mm(p.z)),
                p.nozzle.map(i18n::decimal).unwrap_or_default(),
                p.materials.join(", "),
            ]
        }
        None => Default::default(),
    }
}

/// "250 × 250 × 250 mm · Düse 0,4 mm · PLA, PETG"
fn printer_summary(p: &Printer) -> String {
    let mut parts = vec![i18n::dimensions(glam::Vec3::new(p.x, p.y, p.z))];
    if let Some(n) = p.nozzle {
        parts.push(t!("settings.printer_nozzle_short", size = i18n::decimal(n)).into_owned());
    }
    if !p.materials.is_empty() {
        parts.push(p.materials.join(", "));
    }
    parts.join(" · ")
}

fn mm(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else if config::display_units() == config::DisplayUnits::Imperial {
        i18n::fixed(value, 2)
    } else {
        i18n::decimal(value)
    }
}

/// `~/Dokumente/meshgrove-tags-notizen-2026-10-21.json` (the XDG documents folder).
pub fn export_path() -> PathBuf {
    let dir = dirs::document_dir().or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("."));
    let today = userdata::date(std::time::SystemTime::now());
    dir.join(format!("{}-{today}.json", t!("settings.export_file")))
}

fn about(ui: &mut Ui, info: &AppInfo) {
    ui.add_space(10.0);
    let fact = |ui: &mut Ui, k: std::borrow::Cow<str>, v: String| {
        row(ui, &k, |ui| ui.add(egui::Label::new(v).wrap()));
    };
    row(ui, &t!("settings.developer"), |ui| {
        ui.horizontal(|ui| {
            ui.label(t!("settings.developer_value"));
            ui.hyperlink_to("German Redneck Engineering", env!("CARGO_PKG_HOMEPAGE"));
        });
    });
    row(ui, &t!("settings.source"), |ui| ui.hyperlink_to(env!("CARGO_PKG_REPOSITORY"), env!("CARGO_PKG_REPOSITORY")));
    fact(ui, t!("settings.license"), env!("CARGO_PKG_LICENSE").to_owned());
    fact(ui, t!("settings.version"), env!("CARGO_PKG_VERSION").to_owned());
    fact(ui, t!("settings.gpu"), info.gpu.to_owned());
    fact(ui, t!("settings.config_file"), Config::path().map(|p| p.display().to_string()).unwrap_or_default());
    fact(ui, t!("settings.index_file"), info.db_path.display().to_string());
    fact(ui, t!("settings.cache_dir"), cache_dir().map(|p| p.display().to_string()).unwrap_or_default());
    let log = crate::logging::log_file().map(|p| p.display().to_string()).unwrap_or_default();
    fact(ui, t!("settings.log_file"), log);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_asks_for_exactly_what_differs() {
        let config = Config::default();
        assert!(changes_towards(&config, &config).is_empty());
        let mut changed = config.clone();
        changed.language = Some("de".into());
        changed.ui_theme = Some("nord".into());
        changed.lock_panes = !config.lock_panes;
        changed.library_roots.push("/models".into());
        changed.recent_prints = config.recent_prints + 1;
        // Back from the changed settings to the ones the window opened with.
        let back = changes_towards(&changed, &config);
        assert_eq!(back.len(), 5);
        assert!(back.iter().any(|c| matches!(c, SettingsChange::Language(None))));
        assert!(back.iter().any(|c| matches!(c, SettingsChange::UiTheme(None))));
        assert!(back.iter().any(|c| matches!(c, SettingsChange::RemoveRoot(root) if root == Path::new("/models"))));
        // What only the view changes is no setting of the window: Cancel leaves it.
        let mut browsed = config.clone();
        browsed.last_folder = Some("/somewhere".into());
        browsed.sort_descending = !config.sort_descending;
        assert!(changes_towards(&browsed, &config).is_empty());
    }

    #[test]
    fn the_window_has_a_page_for_every_category() {
        assert_eq!(pages().len(), Category::ALL.len());
    }
}
