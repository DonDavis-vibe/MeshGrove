//! Settings window: language, library folders, slicer, printer, backups, cache.

use std::path::{Path, PathBuf};

use crossbeam_channel::{Receiver, bounded};
use egui::{RichText, Ui};
use meshgrove_core::config::{
    Config, DoubleClick, FOLDER_PREVIEW_DEPTH, FolderPreviews, GltfUnit, PreviewQuality, Program, Slicer,
    TILE_TAG_COUNT, TileTags,
};
use meshgrove_core::printer::Bed;
use meshgrove_core::userdata;

use crate::i18n;

pub enum SettingsChange {
    Language(Option<String>),
    AddRoot(PathBuf),
    RemoveRoot(PathBuf),
    /// `None`: detect the slicer automatically.
    Slicer(Option<Slicer>),
    /// `None`: detect a mesh editor.
    Editor(Option<Program>),
    /// `None`: take the printer selected in the slicer.
    Printer(Option<Bed>),
    GltfUnit(GltfUnit),
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

/// Facts shown in the "about" part.
pub struct AppInfo<'a> {
    pub gpu: &'a str,
    pub db_path: &'a Path,
}

#[derive(Default)]
pub struct SettingsDialog {
    pub open: bool,
    root_input: String,
    root_error: Option<String>,
    slicer_form: Option<(String, String, String)>,
    /// Name and program as typed.
    editor_form: Option<(String, String)>,
    /// Name and X/Y/Z as typed.
    printer_form: Option<[String; 4]>,
    printer_error: bool,
    import_input: String,
    cache_size: Option<u64>,
    cache_rx: Option<Receiver<u64>>,
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
    Preview,
    Models,
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
    Programs,
    Printer,
    Data,
    About,
}

impl Category {
    const ALL: [Category; 8] = [
        Category::General,
        Category::Library,
        Category::Tags,
        Category::View,
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
            Category::Programs => "🔧",
            Category::Printer => "🖨",
            Category::Data => "💾",
            Category::About => "ℹ",
        }
    }

    fn title(self) -> String {
        match self {
            Category::General => t!("settings.cat_general"),
            Category::Library => t!("settings.cat_library"),
            Category::Tags => t!("settings.cat_tags"),
            Category::View => t!("settings.cat_view"),
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
    pub fn open(&mut self, active_slicer: Option<&Slicer>, active_editor: Option<&Program>, bed: Option<&Bed>) {
        self.open = true;
        self.printer_form = Some(printer_form(bed));
        self.printer_error = false;
        if self.import_input.is_empty() {
            self.import_input = export_path().display().to_string();
        }
        self.slicer_form = Some(match active_slicer {
            Some(s) => (
                s.name.clone(),
                s.program.display().to_string(),
                s.workdir.as_ref().map(|d| d.display().to_string()).unwrap_or_default(),
            ),
            None => Default::default(),
        });
        self.editor_form = active_editor.map(|e| (e.name.clone(), e.program.display().to_string()));
        self.refresh_cache_size();
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
        slicer: Option<&Slicer>,
        editor: Option<&Program>,
        bed: Option<&Bed>,
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
                                Category::Tags => &[Section::Tags],
                                Category::View => &[Section::Preview, Section::Models],
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
                                    Section::Preview => preview(ui, config, &mut changes),
                                    Section::Models => models(ui, config, &mut changes),
                                    Section::Slicer => self.slicer(ui, config, slicer, &mut changes),
                                    Section::Editor => self.editor(ui, config, editor, &mut changes),
                                    Section::Printer => self.printer(ui, config, bed, &mut changes),
                                    Section::UserData => self.user_data(ui, &mut changes),
                                    Section::Storage => self.storage(ui, &mut changes),
                                }
                            }
                        });
                    });
                });
            });
        self.open = open;
        changes
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

    fn slicer(&mut self, ui: &mut Ui, config: &Config, active: Option<&Slicer>, changes: &mut Vec<SettingsChange>) {
        ui.label(RichText::new(t!("settings.slicer")).strong());
        let status = match (active, &config.slicer) {
            (Some(s), None) => t!("settings.slicer_detected", name = s.name),
            (Some(s), Some(_)) => t!("settings.slicer_custom", name = s.name),
            (None, _) => t!("settings.slicer_none"),
        };
        ui.weak(status);
        let mut detect = false;
        let (name, program, workdir) = self.slicer_form.get_or_insert_with(Default::default);
        egui::Grid::new("slicer_form").num_columns(2).show(ui, |ui| {
            ui.label(t!("settings.slicer_name"));
            ui.add(egui::TextEdit::singleline(name).desired_width(320.0));
            ui.end_row();
            ui.label(t!("settings.slicer_program"));
            ui.add(egui::TextEdit::singleline(program).desired_width(320.0));
            ui.end_row();
            ui.label(t!("settings.slicer_workdir"));
            ui.add(egui::TextEdit::singleline(workdir).desired_width(320.0));
            ui.end_row();
        });
        ui.horizontal(|ui| {
            if ui.button(t!("settings.apply")).clicked() && !program.trim().is_empty() {
                // Keep the environment of a detected slicer with the same program (e.g. Anycubic's libraries).
                let env = active
                    .filter(|s| s.program == Path::new(program.trim()))
                    .map(|s| s.env.clone())
                    .unwrap_or_default();
                changes.push(SettingsChange::Slicer(Some(Slicer {
                    name: if name.trim().is_empty() { "Slicer".into() } else { name.trim().to_owned() },
                    program: program.trim().into(),
                    args: Vec::new(),
                    env,
                    workdir: (!workdir.trim().is_empty()).then(|| workdir.trim().into()),
                })));
            }
            if ui.button(t!("settings.slicer_auto")).clicked() {
                changes.push(SettingsChange::Slicer(None));
                detect = true;
            }
        });
        if detect {
            self.slicer_form = Slicer::detect_slicer().map(|s| {
                (
                    s.name,
                    s.program.display().to_string(),
                    s.workdir.map(|d| d.display().to_string()).unwrap_or_default(),
                )
            });
        }
    }

    fn editor(&mut self, ui: &mut Ui, config: &Config, active: Option<&Program>, changes: &mut Vec<SettingsChange>) {
        ui.label(RichText::new(t!("settings.editor")).strong());
        let status = match (active, &config.editor) {
            (Some(e), None) => t!("settings.slicer_detected", name = e.name),
            (Some(e), Some(_)) => t!("settings.slicer_custom", name = e.name),
            (None, _) => t!("settings.editor_none"),
        };
        ui.weak(status);
        let mut detect = false;
        let (name, program) = self.editor_form.get_or_insert_with(Default::default);
        egui::Grid::new("editor_form").num_columns(2).show(ui, |ui| {
            ui.label(t!("settings.slicer_name"));
            ui.add(egui::TextEdit::singleline(name).desired_width(320.0));
            ui.end_row();
            ui.label(t!("settings.slicer_program"));
            ui.add(egui::TextEdit::singleline(program).desired_width(320.0));
            ui.end_row();
        });
        ui.horizontal(|ui| {
            if ui.button(t!("settings.apply")).clicked() && !program.trim().is_empty() {
                changes.push(SettingsChange::Editor(Some(Program {
                    name: if name.trim().is_empty() { "Editor".into() } else { name.trim().to_owned() },
                    program: program.trim().into(),
                    args: Vec::new(),
                    env: Vec::new(),
                    workdir: None,
                })));
            }
            if ui.button(t!("settings.slicer_auto")).clicked() {
                changes.push(SettingsChange::Editor(None));
                detect = true;
            }
        });
        ui.weak(t!("settings.editor_hint"));
        if detect {
            self.editor_form = Program::detect_editor().map(|e| (e.name, e.program.display().to_string()));
        }
    }

    fn printer(&mut self, ui: &mut Ui, config: &Config, active: Option<&Bed>, changes: &mut Vec<SettingsChange>) {
        ui.label(RichText::new(t!("settings.printer")).strong());
        let status = match (active, &config.printer) {
            (Some(b), None) => t!("settings.printer_detected", name = b.name),
            (Some(b), Some(_)) => t!("settings.printer_custom", name = b.name),
            (None, _) => t!("settings.printer_none"),
        };
        ui.weak(status);
        let mut detect = false;
        let form = self.printer_form.get_or_insert_with(|| printer_form(active));
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
                ui.label("mm");
            });
            ui.end_row();
        });
        ui.horizontal(|ui| {
            if ui.button(t!("settings.apply")).clicked() {
                let mm = |s: &str| s.trim().replace(',', ".").parse::<f32>().ok().filter(|v| *v > 0.0);
                match (mm(&form[1]), mm(&form[2]), mm(&form[3])) {
                    (Some(x), Some(y), Some(z)) => {
                        let name = match form[0].trim() {
                            "" => t!("settings.printer_default_name").into_owned(),
                            name => name.to_owned(),
                        };
                        changes.push(SettingsChange::Printer(Some(Bed { name, x, y, z })));
                        self.printer_error = false;
                    }
                    _ => self.printer_error = true,
                }
            }
            if ui.button(t!("settings.printer_auto")).on_hover_text(t!("settings.printer_auto_hint")).clicked() {
                changes.push(SettingsChange::Printer(None));
                detect = true;
            }
        });
        if self.printer_error {
            ui.colored_label(ui.visuals().error_fg_color, t!("settings.printer_invalid"));
        }
        if detect {
            self.printer_form = Some(printer_form(Bed::detect().as_ref()));
            self.printer_error = false;
        }
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

fn printer_form(bed: Option<&Bed>) -> [String; 4] {
    match bed {
        Some(b) => [b.name.clone(), mm(b.x), mm(b.y), mm(b.z)],
        None => Default::default(),
    }
}

fn mm(value: f32) -> String {
    if value.fract() == 0.0 { format!("{value:.0}") } else { i18n::decimal(value) }
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
