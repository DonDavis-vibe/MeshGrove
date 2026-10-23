//! Settings window: language, library folders, slicer, printer, backups, cache.

use std::path::{Path, PathBuf};

use crossbeam_channel::{Receiver, bounded};
use egui::{RichText, Ui};
use meshgrove_core::config::{
    self, Config, DisplayUnits, DoubleClick, FOLDER_PREVIEW_DEPTH, FolderPreviews, GltfUnit, ModelUnits,
    PreviewQuality, Program, Slicer, SuggestedTag, TILE_TAG_COUNT, TileTags, default_properties, default_states,
    default_themes,
};
use meshgrove_core::printer::{self, DetectedPrinter, Printer};
use meshgrove_core::programs::{Detected, ProgramKind, detect_editors, detect_slicers};
use meshgrove_core::spacemouse::SpaceMouseConfig;
use meshgrove_core::userdata;

use super::spacemouse::MouseSettings;
use super::tags::{self, Group, PALETTE};
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
    /// Tags in colour; tags on the tiles, how many and how.
    TagDisplay {
        colored: bool,
        on_tiles: bool,
        count: u8,
        style: TileTags,
    },
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

/// Whether the panes of the main window can be moved.
fn panes(ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
    ui.label(RichText::new(t!("settings.panes")).strong());
    let mut locked = config.lock_panes;
    if ui.checkbox(&mut locked, t!("settings.panes_lock")).changed() {
        changes.push(SettingsChange::LockPanes(locked));
    }
    ui.weak(t!("settings.panes_hint"));
    if ui.button(t!("pane.reset")).clicked() {
        changes.push(SettingsChange::ResetLayout);
    }
}

/// How tags are drawn.
fn tags(ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
    let (mut colored, mut on_tiles) = (config.colored_tags, config.tags_on_tiles);
    let (mut count, mut style) = (config.tile_tag_count, config.tile_tags);
    ui.checkbox(&mut colored, t!("settings.tags_colored"));
    ui.checkbox(&mut on_tiles, t!("settings.tags_on_tiles"));
    ui.add_enabled_ui(on_tiles, |ui| {
        ui.indent("tile_tags", |ui| {
            ui.horizontal(|ui| {
                ui.label(t!("settings.tags_count"));
                ui.add(egui::Slider::new(&mut count, TILE_TAG_COUNT));
            });
            ui.horizontal(|ui| {
                ui.label(t!("settings.tags_style"));
                ui.radio_value(&mut style, TileTags::Names, t!("settings.tags_names"));
                ui.add_enabled_ui(colored, |ui| {
                    ui.radio_value(&mut style, TileTags::Dots, t!("settings.tags_dots"))
                        .on_disabled_hover_text(t!("settings.tags_dots_need_colors"));
                });
            });
        });
    });
    ui.weak(t!("settings.tags_hint"));
    let mut flat = config.flat_tag_list;
    if ui.checkbox(&mut flat, t!("settings.tags_flat")).changed() {
        changes.push(SettingsChange::FlatTagList(flat));
    }
    ui.weak(t!("settings.tags_flat_hint"));
    let mut idle = config.tag_chips_idle;
    if ui.checkbox(&mut idle, t!("settings.tags_idle")).changed() {
        changes.push(SettingsChange::IdleTagChips(idle));
    }
    ui.weak(t!("settings.tags_idle_hint"));
    let now = (colored, on_tiles, count, style);
    if now != (config.colored_tags, config.tags_on_tiles, config.tile_tag_count, config.tile_tags) {
        changes.push(SettingsChange::TagDisplay { colored, on_tiles, count, style });
    }
}

/// Model pictures on folder tiles.
fn folder_previews(ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
    ui.label(RichText::new(t!("settings.folder_previews")).strong());
    let mut fp = config.folder_previews;
    ui.checkbox(&mut fp.enabled, t!("settings.folder_previews_enabled"));
    ui.add_enabled_ui(fp.enabled, |ui| {
        ui.horizontal(|ui| {
            ui.label(t!("settings.folder_previews_depth"));
            ui.add(egui::Slider::new(&mut fp.depth, FOLDER_PREVIEW_DEPTH));
        });
        ui.checkbox(&mut fp.cycle, t!("settings.folder_previews_cycle"));
    });
    ui.weak(t!("settings.folder_previews_hint"));
    if fp != config.folder_previews {
        changes.push(SettingsChange::FolderPreviews(fp));
    }
}

/// What a double-click (or Enter) on a model does.
fn double_click(ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
    ui.label(RichText::new(t!("settings.double_click")).strong());
    for (action, label) in [
        (DoubleClick::Window, t!("settings.double_click_window")),
        (DoubleClick::Preview, t!("settings.double_click_preview")),
        (DoubleClick::Slicer, t!("settings.double_click_slicer")),
        (DoubleClick::Editor, t!("settings.double_click_editor")),
        (DoubleClick::DefaultApp, t!("settings.double_click_default_app")),
    ] {
        if ui.radio(config.double_click == action, label).clicked() && config.double_click != action {
            changes.push(SettingsChange::DoubleClick(action));
        }
    }
    ui.weak(t!("settings.double_click_hint"));
}

/// Picture quality of the large preview and the model windows.
fn preview(ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
    ui.label(RichText::new(t!("settings.preview")).strong());
    ui.horizontal(|ui| {
        ui.label(t!("settings.preview_quality"));
        for (quality, label) in [
            (PreviewQuality::Standard, t!("settings.preview_standard")),
            (PreviewQuality::High, t!("settings.preview_high")),
            (PreviewQuality::Studio, t!("settings.preview_studio")),
        ] {
            if ui.radio(config.preview_quality == quality, label).clicked() && config.preview_quality != quality {
                changes.push(SettingsChange::PreviewQuality(quality));
            }
        }
    });
    ui.weak(t!("settings.preview_hint"));
    ui.add_space(4.0);
    let mut free = config.free_orbit;
    if ui.checkbox(&mut free, t!("settings.free_orbit")).changed() {
        changes.push(SettingsChange::FreeOrbit(free));
    }
    ui.weak(t!("settings.free_orbit_hint"));
}

/// How model files without a fixed unit are read.
fn models(ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
    ui.label(RichText::new(t!("settings.models")).strong());
    ui.horizontal(|ui| {
        ui.label(t!("settings.gltf_unit"));
        for (unit, label) in
            [(GltfUnit::Metres, t!("settings.gltf_metres")), (GltfUnit::Millimetres, t!("settings.gltf_millimetres"))]
        {
            if ui.radio(config.gltf_unit == unit, label).clicked() && config.gltf_unit != unit {
                changes.push(SettingsChange::GltfUnit(unit));
            }
        }
    });
    ui.weak(t!("settings.gltf_unit_hint"));
}

/// Units of the model files and of the sizes shown. Inches (imperial) are asked about first.
fn units(ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
    ui.label(RichText::new(t!("settings.units")).strong());
    ui.horizontal(|ui| {
        ui.label(t!("settings.display_units"));
        for (units, label) in [
            (DisplayUnits::Metric, t!("settings.units_metric")),
            (DisplayUnits::Imperial, t!("settings.units_imperial")),
        ] {
            if ui.radio(config.display_units == units, label).clicked() && config.display_units != units {
                changes.push(SettingsChange::DisplayUnits(units));
            }
        }
    });
    ui.weak(t!("settings.display_units_hint"));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(t!("settings.model_units"));
        for (units, label) in [
            (ModelUnits::Millimetres, t!("settings.model_units_mm")),
            (ModelUnits::Inches, t!("settings.model_units_inches")),
        ] {
            if ui.radio(config.model_units == units, label).clicked() && config.model_units != units {
                changes.push(SettingsChange::ModelUnits(units));
            }
        }
    });
    ui.weak(t!("settings.model_units_hint"));
}

/// Facts shown in the "about" part.
pub struct AppInfo<'a> {
    pub gpu: &'a str,
    pub db_path: &'a Path,
    /// The 3D mouse as the reader sees it.
    pub mouse: &'a Snapshot,
    /// What the 3D mouse's picture needs to draw a cube.
    pub render: (&'a egui_wgpu::RenderState, &'a meshgrove_render::Renderer),
}

#[derive(Default)]
pub struct SettingsDialog {
    pub open: bool,
    root_input: String,
    root_error: Option<String>,
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

const CATEGORY_WIDTH: f32 = 190.0;

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

/// The parts of a category, each with its own heading.
#[derive(Clone, Copy)]
enum Section {
    Language,
    DoubleClick,
    Panes,
    Library,
    FolderPreviews,
    Tags,
    Themes,
    Properties,
    States,
    Preview,
    SpaceMouse,
    Models,
    Units,
    Slicer,
    Editor,
    Printer,
    UserData,
    Storage,
}

/// The settings' categories, listed on the left of the window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Category {
    #[default]
    General,
    Library,
    Tags,
    View,
    ModelsUnits,
    Programs,
    Printer,
    Data,
    About,
}

impl Category {
    const ALL: [Category; 9] = [
        Category::General,
        Category::Library,
        Category::Tags,
        Category::View,
        Category::ModelsUnits,
        Category::Programs,
        Category::Printer,
        Category::Data,
        Category::About,
    ];

    fn icon(self) -> &'static str {
        match self {
            Category::General => "⚙",
            Category::Library => "📚",
            Category::Tags => "🏷",
            Category::View => "🎥",
            Category::ModelsUnits => "📏",
            Category::Programs => "🔧",
            Category::Printer => "🖨",
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
            "view" => Category::View,
            "models" => Category::ModelsUnits,
            "programs" => Category::Programs,
            "printer" => Category::Printer,
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
            Category::View => t!("settings.cat_view"),
            Category::ModelsUnits => t!("settings.cat_models_units"),
            Category::Programs => t!("settings.cat_programs"),
            Category::Printer => t!("settings.cat_printer"),
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
                // Categories on the left, the chosen one's settings on the right.
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(CATEGORY_WIDTH);
                        for category in Category::ALL {
                            if category_item(ui, category, self.category == category).clicked() {
                                self.category = category;
                            }
                        }
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.add_space(2.0);
                        ui.heading(self.category.title());
                        ui.add_space(6.0);
                        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                            let sections: &[Section] = match self.category {
                                Category::General => &[Section::Language, Section::DoubleClick, Section::Panes],
                                Category::Library => &[Section::Library, Section::FolderPreviews],
                                Category::Tags => {
                                    &[Section::Tags, Section::Themes, Section::Properties, Section::States]
                                }
                                Category::View => &[Section::Preview, Section::SpaceMouse],
                                Category::ModelsUnits => &[Section::Models, Section::Units],
                                Category::Programs => &[Section::Slicer, Section::Editor],
                                Category::Printer => &[Section::Printer],
                                Category::Data => &[Section::UserData, Section::Storage],
                                Category::About => {
                                    about(ui, &info);
                                    &[]
                                }
                            };
                            for (i, section) in sections.iter().enumerate() {
                                if i > 0 {
                                    ui.add_space(4.0);
                                    ui.separator();
                                    ui.add_space(4.0);
                                }
                                match section {
                                    Section::Language => self.language(ui, config, &mut changes),
                                    Section::DoubleClick => double_click(ui, config, &mut changes),
                                    Section::Panes => panes(ui, config, &mut changes),
                                    Section::Library => self.library(ui, config, &mut changes),
                                    Section::FolderPreviews => folder_previews(ui, config, &mut changes),
                                    Section::Tags => tags(ui, config, &mut changes),
                                    Section::Themes => self.suggestions(ui, config, Group::Themes, &mut changes),
                                    Section::Properties => {
                                        self.suggestions(ui, config, Group::Properties, &mut changes)
                                    }
                                    Section::States => self.suggestions(ui, config, Group::State, &mut changes),
                                    Section::Preview => preview(ui, config, &mut changes),
                                    Section::SpaceMouse => {
                                        self.mouse.section(ui, config, info.mouse, info.render, &mut changes)
                                    }
                                    Section::Models => models(ui, config, &mut changes),
                                    Section::Units => units(ui, config, &mut changes),
                                    Section::Slicer => {
                                        self.program_list(ui, false, slicers, default_slicer, &mut changes)
                                    }
                                    Section::Editor => {
                                        self.program_list(ui, true, editors, default_editor, &mut changes)
                                    }
                                    Section::Printer => self.printers(ui, printers, default_printer, &mut changes),
                                    Section::UserData => self.user_data(ui, &mut changes),
                                    Section::Storage => self.storage(ui, &mut changes),
                                }
                            }
                        });
                    });
                });
            });
        self.mouse.calibration_modal(ctx, config, info.mouse, info.render, &mut changes);
        self.open = open;
        changes
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
        ui.label(RichText::new(group.title()).strong());
        ui.weak(t!("settings.suggest_hint", group = group.title()));
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
        ui.label(RichText::new(t!("settings.language")).strong());
        ui.horizontal(|ui| {
            for (value, label) in
                [(None, t!("settings.language_auto")), (Some("de"), "Deutsch".into()), (Some("en"), "English".into())]
            {
                if ui.radio(config.language.as_deref() == value, label).clicked() {
                    changes.push(SettingsChange::Language(value.map(str::to_owned)));
                }
            }
        });
    }

    fn library(&mut self, ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
        ui.label(RichText::new(t!("settings.library")).strong());
        ui.weak(t!("settings.library_hint"));
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
        let (title, none, hint) = if editors {
            ("settings.editor", "settings.editor_none", "settings.editor_hint")
        } else {
            ("settings.slicer", "settings.slicer_none", "settings.slicers_hint")
        };
        ui.label(RichText::new(t!(title)).strong());
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
        ui.weak(t!(hint));
    }

    /// The printers models are checked against: the added ones (one is the main printer), the
    /// ones the slicers know that can be added with a click, and a form to add or edit one.
    fn printers(&mut self, ui: &mut Ui, printers: &[Printer], default: usize, changes: &mut Vec<SettingsChange>) {
        ui.label(RichText::new(t!("settings.printer")).strong());
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
        ui.label(RichText::new(t!("settings.user_data")).strong());
        ui.weak(t!("settings.user_data_hint"));
        let backups = userdata::backup_dir().map(|d| userdata::backups(&d)).unwrap_or_default();
        ui.horizontal(|ui| {
            match backups.last().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()) {
                Some(last) => ui.label(t!(
                    "settings.backups",
                    count = backups.len(),
                    last = last.trim_start_matches("user-data-")
                )),
                None => ui.label(t!("settings.backups_none")),
            };
            if ui.button(t!("settings.open_folder")).clicked() {
                changes.push(SettingsChange::OpenBackups);
            }
        });
        ui.horizontal(|ui| {
            if ui.button(t!("settings.export")).on_hover_text(export_path().display().to_string()).clicked() {
                changes.push(SettingsChange::Export(export_path()));
            }
        });
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.import_input).desired_width(320.0));
            if ui.button(t!("settings.import")).on_hover_text(t!("settings.import_hint")).clicked() {
                changes.push(SettingsChange::Import(PathBuf::from(self.import_input.trim())));
            }
        });
    }

    fn storage(&mut self, ui: &mut Ui, changes: &mut Vec<SettingsChange>) {
        ui.label(RichText::new(t!("settings.storage")).strong());
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
    }
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
    egui::Grid::new("about").num_columns(2).show(ui, |ui| {
        let row = |ui: &mut Ui, k: std::borrow::Cow<str>, v: String| {
            ui.weak(k);
            ui.label(v);
            ui.end_row();
        };
        ui.weak(t!("settings.developer"));
        ui.horizontal(|ui| {
            ui.label(t!("settings.developer_value"));
            ui.hyperlink_to("German Redneck Engineering", env!("CARGO_PKG_HOMEPAGE"));
        });
        ui.end_row();
        ui.weak(t!("settings.source"));
        ui.hyperlink_to(env!("CARGO_PKG_REPOSITORY"), env!("CARGO_PKG_REPOSITORY"));
        ui.end_row();
        row(ui, t!("settings.license"), env!("CARGO_PKG_LICENSE").to_owned());
        row(ui, t!("settings.version"), env!("CARGO_PKG_VERSION").to_owned());
        row(ui, t!("settings.gpu"), info.gpu.to_owned());
        row(ui, t!("settings.config_file"), Config::path().map(|p| p.display().to_string()).unwrap_or_default());
        row(ui, t!("settings.index_file"), info.db_path.display().to_string());
        row(ui, t!("settings.cache_dir"), cache_dir().map(|p| p.display().to_string()).unwrap_or_default());
    });
}
