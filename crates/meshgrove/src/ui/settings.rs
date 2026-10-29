//! Settings window: pages listed on the left in groups, each with a line on what it holds, its
//! settings in folding sections of aligned rows (see `settings_kit`), and a button that puts the
//! page back to the defaults. The arrangement follows SolidCraft's preferences.

use std::path::{Path, PathBuf};

use crossbeam_channel::{Receiver, bounded};
use egui::{RichText, Ui};
use meshgrove_core::config::{
    self, ArchiveImport, Config, DisplayUnits, DoubleClick, FOLDER_PREVIEW_DEPTH, FolderPreviews, FolderRule, GltfUnit,
    ImportAtStart, ModelUnits, PreviewQuality, Program, Slicer, SuggestedTag, TileLayout, TileTags, default_properties,
    default_states, default_themes,
};
use meshgrove_core::printer::{self, DetectedPrinter, Printer};
use meshgrove_core::programs::{Detected, ProgramKind, detect_editors, detect_slicers};
use meshgrove_core::spacemouse::SpaceMouseConfig;
use meshgrove_core::userdata;

use super::settings_kit::{self as kit, check, hint, row, segmented};
use super::spacemouse::MouseSettings;
use super::tags::{self, Group, PALETTE, TagStyle};
use crate::i18n;
use crate::spacemouse::Snapshot;

pub enum SettingsChange {
    Language(Option<String>),
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
    /// The inbox of new files in the sidebar.
    Inbox(bool),
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
    AiEnabled(bool),
    /// Make Meshgrove available to the AI apps on this computer.
    AiConnectApps(bool),
    /// Accept the suggestions made from now on without the review (moves excepted).
    AiAutoAccept(bool),
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
    ShowAiReview,
    ShowAiHistory,
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
    /// A settings section folded away or opened again.
    FoldSection {
        key: String,
        folded: bool,
    },
    Export(PathBuf),
    Import(PathBuf),
    OpenBackups,
    ClearCache,
}

/// Whether the panes of the main window can be moved.
fn panes(ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
    let mut locked = config.lock_panes;
    if check(ui, &mut locked, t!("settings.panes_lock"), t!("settings.panes_hint")) {
        changes.push(SettingsChange::LockPanes(locked));
    }
    if ui.button(t!("pane.reset")).clicked() {
        changes.push(SettingsChange::ResetLayout);
    }
}

fn inbox(ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
    let mut on = config.inbox;
    if check(ui, &mut on, t!("settings.inbox_show"), t!("settings.inbox_hint")) {
        changes.push(SettingsChange::Inbox(on));
    }
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
        let off = t!("settings.recent_prints_off").into_owned();
        let slider = egui::Slider::new(&mut keep, 0..=50)
            .custom_formatter(move |v, _| if v == 0.0 { off.clone() } else { format!("{v:.0}") });
        if ui.add(slider).changed() {
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
            ui.add(egui::Slider::new(&mut fp.depth, FOLDER_PREVIEW_DEPTH));
        });
        ui.checkbox(&mut fp.cycle, t!("settings.folder_previews_cycle"));
    });
    if fp != config.folder_previews {
        changes.push(SettingsChange::FolderPreviews(fp));
    }
}

/// What a double-click (or Enter) on a model does.
fn double_click(ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
    let actions = [
        (DoubleClick::Window, t!("settings.double_click_window")),
        (DoubleClick::Preview, t!("settings.double_click_preview")),
        (DoubleClick::Slicer, t!("settings.double_click_slicer")),
        (DoubleClick::Editor, t!("settings.double_click_editor")),
        (DoubleClick::DefaultApp, t!("settings.double_click_default_app")),
    ];
    row(ui, &t!("settings.double_click_row"), |ui| {
        let current = actions.iter().find(|(a, _)| *a == config.double_click).map(|(_, l)| l.clone());
        egui::ComboBox::from_id_salt("double_click").selected_text(current.unwrap_or_default()).width(260.0).show_ui(
            ui,
            |ui| {
                for (action, label) in &actions {
                    if ui.selectable_label(config.double_click == *action, label.as_ref()).clicked()
                        && config.double_click != *action
                    {
                        changes.push(SettingsChange::DoubleClick(*action));
                    }
                }
            },
        );
        hint(ui, t!("settings.double_click_hint"));
    });
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
    /// The library's tags with their counts, for the placement rules.
    pub tags: &'a [(String, usize)],
}

#[derive(Default)]
pub struct SettingsDialog {
    pub open: bool,
    root_input: String,
    root_error: Option<String>,
    /// An import folder being added, and the landing folder's name as typed.
    import_folder_input: String,
    import_folder_error: Option<String>,
    landing_input: String,
    /// The placement rules as typed: (tag, folder), the themes first.
    rule_inputs: Vec<(String, String)>,
    rule_themes: usize,
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
    /// "Restore defaults" was clicked once.
    confirm_defaults: bool,
}

const CATEGORY_WIDTH: f32 = 190.0;

/// The slicers, mesh editors and printers, each with the index of the default one.
type Lists<'a> = ((&'a [Slicer], usize), (&'a [Program], usize), (&'a [Printer], usize));

/// One entry of the category list: the symbol centred in a column of its own, so every name
/// starts at the same place whatever the symbol's width.
fn category_item(ui: &mut Ui, category: Category, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(CATEGORY_WIDTH, 28.0), egui::Sense::click());
    let visuals = ui.style().interact_selectable(&response, selected);
    if selected || response.hovered() || response.has_focus() {
        ui.painter().rect(
            rect,
            visuals.corner_radius,
            visuals.weak_bg_fill,
            visuals.bg_stroke,
            egui::StrokeKind::Inside,
        );
    }
    let font = egui::TextStyle::Button.resolve(ui.style());
    let painter = ui.painter();
    let middle = rect.center().y;
    painter.text(
        egui::pos2(rect.left() + 16.0, middle),
        egui::Align2::CENTER_CENTER,
        category.icon(),
        font.clone(),
        visuals.text_color(),
    );
    painter.text(
        egui::pos2(rect.left() + 34.0, middle),
        egui::Align2::LEFT_CENTER,
        category.title(),
        font,
        visuals.text_color(),
    );
    response
}

/// The parts of a category, each a folding section with its own title.
#[derive(Debug, Clone, Copy)]
enum Section {
    Language,
    DoubleClick,
    Panes,
    Library,
    Inbox,
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
    Endpoint,
    AiLog,
}

impl Section {
    fn title(self) -> String {
        match self {
            Section::Language => t!("settings.language"),
            Section::DoubleClick => t!("settings.double_click"),
            Section::Panes => t!("settings.panes"),
            Section::Library => t!("settings.library"),
            Section::Inbox => t!("settings.inbox"),
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
            Section::Endpoint => t!("endpoint.title"),
            Section::AiLog => t!("settings.ai_log"),
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
    Data,
    About,
}

impl Category {
    const ALL: [Category; 11] = [
        Category::General,
        Category::Library,
        Category::Tags,
        Category::Tiles,
        Category::View,
        Category::ModelsUnits,
        Category::Programs,
        Category::Printer,
        Category::Ai,
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
            Category::Data => t!("settings.intro_data"),
            Category::About => t!("settings.intro_about"),
        }
        .into_owned()
    }

    /// Whether the page has preferences to put back; the lists (library folders, programs,
    /// printers) and what AI may see are the user's data, not preferences.
    fn has_defaults(self) -> bool {
        matches!(
            self,
            Category::General
                | Category::Library
                | Category::Tags
                | Category::Tiles
                | Category::View
                | Category::ModelsUnits
        )
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
                put(config.double_click != d.double_click, SettingsChange::DoubleClick(d.double_click));
                put(config.lock_panes != d.lock_panes, SettingsChange::LockPanes(d.lock_panes));
            }
            Category::Library => {
                put(config.inbox != d.inbox, SettingsChange::Inbox(d.inbox));
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
            Category::Printer => "🖨",
            Category::Ai => "✧",
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

impl SettingsDialog {
    pub fn open(&mut self) {
        self.open = true;
        self.printer_form = printer_form(None);
        self.printer_form_units = config::display_units();
        self.printer_error = false;
        self.printer_editing = None;
        self.found_printers = printer::detect_all();
        self.ai.reset();
        self.landing_input.clear();
        self.rule_inputs.clear();
        if self.import_input.is_empty() {
            self.import_input = export_path().display().to_string();
        }
        self.slicer_form = Default::default();
        self.rescan_slicers();
        self.editor_form = Default::default();
        self.mouse.reset();
        self.refresh_cache_size();
    }

    /// Opens the window on the category named as in [`Category::from_name`] (unknown names keep the current one).
    pub fn open_at(&mut self, category: &str) {
        self.open();
        if let Some(category) = Category::from_name(category) {
            self.category = category;
            self.confirm_defaults = false;
        }
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
        (slicers, default_slicer): (&[Slicer], usize),
        (editors, default_editor): (&[Program], usize),
        (printers, default_printer): (&[Printer], usize),
        info: AppInfo,
    ) -> Vec<SettingsChange> {
        let mut changes = Vec::new();
        if !self.open {
            return changes;
        }
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
        let slicers_editors_printers =
            ((slicers, default_slicer), (editors, default_editor), (printers, default_printer));
        let mut open = self.open;
        let center = ctx.content_rect().center();
        egui::Window::new(t!("settings.title"))
            .open(&mut open)
            .default_width(820.0)
            .default_height(600.0)
            .min_width(640.0)
            .min_height(400.0)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(center)
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                // Categories on the left in their groups, the chosen one's settings on the right.
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(CATEGORY_WIDTH);
                        let mut group = None;
                        for category in Category::ALL {
                            if category.group() != group {
                                group = category.group();
                                if let Some(caption) = &group {
                                    ui.add_space(10.0);
                                    ui.horizontal(|ui| {
                                        ui.add_space(8.0);
                                        ui.label(RichText::new(caption.to_uppercase()).small().strong().weak());
                                    });
                                }
                            }
                            if category_item(ui, category, self.category == category).clicked() {
                                self.category = category;
                                self.confirm_defaults = false;
                            }
                        }
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.add_space(2.0);
                        ui.heading(self.category.title());
                        hint(ui, self.category.intro());
                        let footer = if self.category.has_defaults() { 34.0 } else { 0.0 };
                        egui::ScrollArea::vertical()
                            .auto_shrink(false)
                            .max_height((ui.available_height() - footer).max(80.0))
                            .show(ui, |ui| self.page(ui, config, slicers_editors_printers, &info, &mut changes));
                        if self.category.has_defaults() {
                            self.restore_defaults(ui, config, &mut changes);
                        }
                    });
                });
            });
        self.mouse.calibration_modal(ctx, config, info.mouse, info.render, &mut changes);
        self.open = open;
        changes
    }

    /// The chosen category's sections.
    fn page(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        ((slicers, default_slicer), (editors, default_editor), (printers, default_printer)): Lists,
        info: &AppInfo,
        changes: &mut Vec<SettingsChange>,
    ) {
        let sections: &[Section] = match self.category {
            Category::General => &[Section::Language, Section::DoubleClick, Section::Panes],
            Category::Library => &[Section::Library, Section::Inbox, Section::Import],
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
            Category::Ai => &[Section::Ai, Section::AiLibraries, Section::Mcp, Section::Endpoint, Section::AiLog],
            Category::Data => &[Section::UserData, Section::Storage],
            Category::About => {
                about(ui, info);
                &[]
            }
        };
        for &section in sections {
            let toggled =
                kit::section(ui, &section.key(), &section.title(), &config.settings_folded, |ui| match section {
                    Section::Language => self.language(ui, config, changes),
                    Section::DoubleClick => double_click(ui, config, changes),
                    Section::Panes => panes(ui, config, changes),
                    Section::Library => self.library(ui, config, changes),
                    Section::Inbox => inbox(ui, config, changes),
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
                    Section::Endpoint => self.ai.endpoint(ui, config, changes),
                    Section::AiLog => self.ai.log(ui, changes),
                    Section::UserData => self.user_data(ui, changes),
                    Section::Storage => self.storage(ui, changes),
                });
            if let Some(folded) = toggled {
                changes.push(SettingsChange::FoldSection { key: section.key(), folded });
            }
        }
        ui.add_space(8.0);
    }

    /// The footer: puts the page's preferences back, after a second click to be sure.
    fn restore_defaults(&mut self, ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
        ui.separator();
        let resets = self.category.defaults(config);
        if resets.is_empty() {
            self.confirm_defaults = false;
        }
        ui.horizontal(|ui| {
            let label = if self.confirm_defaults { t!("settings.defaults_confirm") } else { t!("settings.defaults") };
            let button = ui
                .add_enabled(!resets.is_empty(), egui::Button::new(label))
                .on_hover_text(t!("settings.defaults_hint"))
                .on_disabled_hover_text(t!("settings.defaults_none"));
            if button.clicked() {
                if self.confirm_defaults {
                    changes.extend(resets);
                }
                self.confirm_defaults = !self.confirm_defaults;
            }
            if self.confirm_defaults && ui.button(t!("settings.cancel")).clicked() {
                self.confirm_defaults = false;
            }
        });
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
        egui::ScrollArea::vertical().id_salt("list").max_height(260.0).show(ui, |ui| {
            for (i, theme) in themes.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    let (bg, _) = PALETTE[usize::from(tags::theme_slot(theme))];
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
                    ui.add(egui::TextEdit::singleline(name).desired_width(220.0));
                    if linked {
                        let name = if german { theme.de.clone() } else { theme.en.clone() };
                        theme.de = name.clone();
                        theme.en = name;
                    }
                    if ui.small_button("✕").on_hover_text(t!("settings.suggest_remove")).clicked() {
                        remove = Some(i);
                    }
                });
            }
        });
        if let Some(i) = remove {
            themes.remove(i);
        }
        ui.horizontal(|ui| {
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.suggest_input[slot])
                    .hint_text(t!("settings.suggest_new"))
                    .desired_width(220.0),
            );
            let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let name = self.suggest_input[slot].trim().to_owned();
            let can_add = !name.is_empty() && !themes.iter().any(|t| t.matches(&name));
            if (ui.add_enabled(can_add, egui::Button::new("＋")).clicked() || enter) && can_add {
                themes.push(SuggestedTag::named(&name));
                self.suggest_input[slot].clear();
                field.request_focus();
            }
        });
        ui.horizontal(|ui| {
            if ui.button(t!("settings.suggest_missing")).clicked() {
                for theme in defaults() {
                    if !themes.iter().any(|t| t.matches(&theme.de) || t.matches(&theme.en)) {
                        themes.push(theme);
                    }
                }
            }
            if ui.button(t!("settings.suggest_reset")).clicked() {
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

    fn language(&mut self, ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
        row(ui, &t!("settings.language_ui"), |ui| {
            let mut language = config.language.as_deref();
            let options = [
                (None, t!("settings.language_auto").into_owned()),
                (Some("de"), "Deutsch".to_owned()),
                (Some("en"), "English".to_owned()),
            ];
            if segmented(ui, &mut language, &options) {
                changes.push(SettingsChange::Language(language.map(str::to_owned)));
            }
        });
    }

    fn library(&mut self, ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
        hint(ui, t!("settings.library_hint"));
        for root in &config.library_roots {
            ui.horizontal(|ui| {
                if ui.small_button("✕").on_hover_text(t!("sidebar.remove_root")).clicked() {
                    changes.push(SettingsChange::RemoveRoot(root.clone()));
                }
                ui.label(root.display().to_string());
            });
        }
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.root_input)
                    .hint_text(t!("settings.folder_hint"))
                    .desired_width(320.0),
            );
            if ui.button(t!("settings.add")).clicked() {
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
        if self.rule_inputs.is_empty() || self.rule_themes != themes.len() {
            self.rule_inputs = themes
                .iter()
                .map(|t| {
                    let name = theme_name(t);
                    let folder = config.folder_for_tag(&name).unwrap_or_default();
                    (name, folder)
                })
                .collect();
            let known = |tag: &str| themes.iter().chain(&properties).chain(&states).any(|t| t.matches(tag));
            self.rule_inputs.extend(
                tags.iter()
                    .filter(|(t, _)| !known(t))
                    .map(|(t, _)| (t.clone(), config.folder_for_tag(t).unwrap_or_default())),
            );
            self.rule_themes = themes.len();
        }
        hint(ui, t!("settings.placement_rules_hint"));
        let (mut edited, mut bad) = (false, false);
        let valid = |folder: &str| {
            folder
                .split('/')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .all(|s| meshgrove_core::fsops::check_name(s).is_ok())
        };
        egui::Grid::new("placement_rules").num_columns(3).spacing([12.0, 4.0]).show(ui, |ui| {
            for (i, (tag, folder)) in self.rule_inputs.iter_mut().enumerate() {
                if i == themes.len() {
                    ui.label(RichText::new(t!("settings.placement_other_tags")).weak());
                    ui.end_row();
                }
                ui.label(tag.as_str());
                let field = ui.add(
                    egui::TextEdit::singleline(folder).hint_text(t!("settings.placement_none")).desired_width(220.0),
                );
                let ok = valid(folder);
                bad |= !ok;
                edited |= field.lost_focus() && ok;
                if !folder.trim().is_empty()
                    && ui.small_button("✕").on_hover_text(t!("settings.placement_clear")).clicked()
                {
                    folder.clear();
                    edited = true;
                }
                ui.end_row();
            }
        });
        if bad {
            ui.colored_label(ui.visuals().error_fg_color, t!("settings.placement_bad_folder"));
        }
        if edited {
            // Rules only where a tag's folder differs from the theme's own name.
            let mut rules = Vec::new();
            for (i, (tag, folder)) in self.rule_inputs.iter().enumerate() {
                let folder = folder.trim();
                match themes.get(i).map(theme_name) {
                    Some(own) if folder == own => {}
                    Some(_) => rules.push(FolderRule { tag: tag.clone(), folder: folder.to_owned() }),
                    None if !folder.is_empty() => {
                        rules.push(FolderRule { tag: tag.clone(), folder: folder.to_owned() })
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
                if ui.small_button("✕").on_hover_text(t!("settings.import_remove")).clicked() {
                    let kept: Vec<PathBuf> = folders.iter().filter(|f| *f != folder).cloned().collect();
                    changes.push(SettingsChange::ImportFolders(kept));
                }
                ui.label(folder.display().to_string());
            });
        }
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.import_folder_input)
                    .hint_text(t!("settings.folder_hint"))
                    .desired_width(320.0),
            );
            if ui.button(t!("settings.add")).clicked() {
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
            egui::ComboBox::from_id_salt("import_library").selected_text(shown).show_ui(ui, |ui| {
                for root in &config.library_roots {
                    if ui.selectable_label(current.as_deref() == Some(root), listing_name(root)).clicked() {
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
            let field = ui.add(egui::TextEdit::singleline(&mut self.landing_input).desired_width(200.0));
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
                if ui.radio(i == default, "").on_hover_text(t!("settings.slicers_default")).clicked() && i != default {
                    send(programs.to_vec(), Some(program.name.clone()), changes);
                }
                ui.label(RichText::new(&program.name).strong());
                ui.weak(program.program.display().to_string());
                if ui.small_button("✕").on_hover_text(t!("settings.slicers_remove")).clicked() {
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
                if ui.button("＋").on_hover_text(t!("settings.slicers_add")).clicked() {
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
            if ui.add_enabled(self.detect_rx.is_none(), egui::Button::new(t!("settings.slicers_rescan"))).clicked() {
                self.rescan_slicers();
            }
            if self.detect_rx.is_some() {
                ui.spinner();
            }
        });
        let by_hand = if editors { "settings.editors_by_hand" } else { "settings.slicers_by_hand" };
        ui.collapsing(t!(by_hand), |ui| {
            let form = if editors { &mut self.editor_form } else { &mut self.slicer_form };
            let (name, program, workdir) = form;
            egui::Grid::new(("program_form", editors)).num_columns(2).show(ui, |ui| {
                ui.label(t!("settings.slicer_name"));
                ui.add(egui::TextEdit::singleline(name).desired_width(320.0));
                ui.end_row();
                ui.label(t!("settings.slicer_program"));
                ui.add(egui::TextEdit::singleline(program).desired_width(320.0));
                ui.end_row();
                if !editors {
                    ui.label(t!("settings.slicer_workdir"));
                    ui.add(egui::TextEdit::singleline(workdir).desired_width(320.0));
                    ui.end_row();
                }
            });
            if ui.button(t!("settings.slicers_add")).clicked() && !program.trim().is_empty() {
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
                if ui.radio(i == default, "").on_hover_text(t!("settings.printers_default")).clicked() && i != default {
                    send(printers.to_vec(), Some(p.name.clone()), changes);
                }
                ui.label(RichText::new(&p.name).strong());
                ui.weak(printer_summary(p));
                if ui.small_button("✏").on_hover_text(t!("settings.printers_edit")).clicked() {
                    self.printer_form = printer_form(Some(p));
                    self.printer_form_units = units;
                    self.printer_editing = Some(p.name.clone());
                    self.printer_error = false;
                }
                if ui.small_button("✕").on_hover_text(t!("settings.printers_remove")).clicked() {
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
                if ui.button("＋").on_hover_text(t!("settings.printers_add")).clicked() {
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
            if addable.len() > 1 && ui.button(t!("settings.printers_add_all")).clicked() {
                let mut list = printers.to_vec();
                list.extend(addable.iter().map(|d| d.printer.clone()));
                let first = addable.iter().find(|d| d.selected).unwrap_or(&addable[0]).printer.name.clone();
                send(list, default_name.clone().or(Some(first)), changes);
            }
            if ui.button(t!("settings.slicers_rescan")).on_hover_text(t!("settings.printer_auto_hint")).clicked() {
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
                ui.add(egui::TextEdit::singleline(&mut form[0]).desired_width(320.0));
                ui.end_row();
                ui.label(t!("settings.printer_volume"));
                ui.horizontal(|ui| {
                    for (i, axis) in ["X", "Y", "Z"].iter().enumerate() {
                        ui.label(*axis);
                        ui.add(egui::TextEdit::singleline(&mut form[i + 1]).desired_width(56.0));
                    }
                    ui.label(units.label());
                });
                ui.end_row();
                ui.label(t!("settings.printer_nozzle"));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut form[4]).desired_width(56.0));
                    ui.label("mm");
                });
                ui.end_row();
                ui.label(t!("settings.printer_materials"));
                ui.add(egui::TextEdit::singleline(&mut form[5]).hint_text("PLA, PETG").desired_width(320.0));
                ui.end_row();
            });
            ui.horizontal(|ui| {
                let label = if editing { t!("settings.apply") } else { t!("settings.printers_add") };
                if ui.button(label).clicked() {
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
                if editing && ui.button(t!("settings.cancel")).clicked() {
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
            if ui.button(t!("settings.open_folder")).clicked() {
                changes.push(SettingsChange::OpenBackups);
            }
        });
        row(ui, &t!("settings.export_row"), |ui| {
            if ui.button(t!("settings.export")).on_hover_text(export_path().display().to_string()).clicked() {
                changes.push(SettingsChange::Export(export_path()));
            }
        });
        row(ui, &t!("settings.import_row"), |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.import_input).desired_width(280.0));
                if ui.button(t!("settings.import")).on_hover_text(t!("settings.import_hint")).clicked() {
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
                    None => ui.spinner(),
                };
                if ui.button(t!("settings.clear_cache")).on_hover_text(t!("settings.clear_cache_hint")).clicked() {
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
