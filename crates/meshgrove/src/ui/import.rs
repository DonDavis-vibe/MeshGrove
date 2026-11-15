//! "Import" view: the 3D-print files found in the import folders (a downloads folder), to pick
//! and bring into the library's landing folder.

use std::ops::Range;
use std::path::PathBuf;

use egui::{RichText, Ui};
use meshgrove_core::config::ArchiveImport;
use meshgrove_core::listing::{self, Entry};

use super::duplicates::{TILE_WIDTH, TileClick, tile};
use super::gallery::Gallery;
use super::settings_kit::{self, segmented};

/// What the view knows about one candidate besides its entry.
pub struct ImportItemInfo {
    /// Ticked to be imported.
    pub picked: bool,
    /// A library file with the same content: not imported again.
    pub in_library: Option<PathBuf>,
    /// Still being written, by the look of it.
    pub busy: bool,
    /// A file of this name is in the landing folder already: it gets a number.
    pub name_taken: bool,
    /// For archives: members, and how many of them are models.
    pub archive: Option<(usize, usize)>,
    /// For a folder: how many files to print lie in it. It comes in as a whole.
    pub folder: Option<usize>,
}

impl ImportItemInfo {
    fn importable(&self) -> bool {
        self.in_library.is_none() && !self.busy
    }
}

/// One import folder in the view.
pub struct ImportGroupInfo {
    /// Its files in the view's entry list.
    pub range: Range<usize>,
    pub folder: PathBuf,
    pub title: String,
    pub items: Vec<ImportItemInfo>,
}

pub enum ImportAction {
    Select(usize),
    Open(usize),
    /// Tick or untick one file.
    Toggle(usize),
    /// Tick (true) or untick every file of a folder.
    ToggleGroup(usize, bool),
    Archives(ArchiveImport),
    ImportPicked,
    ImportAll,
    Reload,
    OpenSettings,
}

/// `target`: where the files go ("3D_druck/Import"); `scanning`: the folders are being looked at.
pub fn show(
    ui: &mut Ui,
    entries: &[Entry],
    groups: &[ImportGroupInfo],
    gallery: &Gallery,
    target: &str,
    archives: ArchiveImport,
    scanning: bool,
) -> Option<ImportAction> {
    // One of the kit's forms: its controls, its fields, its label font.
    settings_kit::pane(ui, |ui| import(ui, entries, groups, gallery, target, archives, scanning))
}

fn import(
    ui: &mut Ui,
    entries: &[Entry],
    groups: &[ImportGroupInfo],
    gallery: &Gallery,
    target: &str,
    archives: ArchiveImport,
    scanning: bool,
) -> Option<ImportAction> {
    let mut action = None;
    let importable: usize = groups.iter().map(|g| g.items.iter().filter(|i| i.importable()).count()).sum();
    let picked: usize = groups.iter().map(|g| g.items.iter().filter(|i| i.picked).count()).sum();
    let any_archive = groups.iter().any(|g| g.items.iter().any(|i| i.archive.is_some() && i.importable()));
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.horizontal(|ui| {
            let summary = match importable {
                0 if groups.is_empty() => t!("import.no_folders"),
                0 => t!("import.none"),
                n => t!("import.summary", count = n, target = target),
            };
            ui.label(settings_kit::strong(summary));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if settings_kit::action(ui, t!("import.open_settings"), true).clicked() {
                    action = Some(ImportAction::OpenSettings);
                }
                if settings_kit::action(ui, t!("import.reload"), !scanning).clicked() {
                    action = Some(ImportAction::Reload);
                }
                if scanning {
                    settings_kit::busy(ui);
                }
            });
        });
        ui.weak(t!("import.explain"));
        ui.add_space(4.0);
        if importable > 0 {
            ui.horizontal(|ui| {
                let label = t!("import.import_picked", count = picked);
                if settings_kit::primary(ui, label, picked > 0).clicked() {
                    action = Some(ImportAction::ImportPicked);
                }
                if picked < importable
                    && settings_kit::action(ui, t!("import.import_all", count = importable), true).clicked()
                {
                    action = Some(ImportAction::ImportAll);
                }
                if any_archive {
                    ui.add_space(12.0);
                    let mut choice = archives;
                    let options = [
                        (ArchiveImport::Whole, t!("import.archives_whole").into_owned()),
                        (ArchiveImport::Extract, t!("import.archives_extract").into_owned()),
                    ];
                    if segmented(ui, &mut choice, &options) {
                        action = Some(ImportAction::Archives(choice));
                    }
                }
            });
            ui.add_space(8.0);
        }
        for (n, group) in groups.iter().enumerate() {
            if group.items.is_empty() {
                continue;
            }
            let here = group.items.iter().filter(|i| i.importable()).count();
            let here_picked = group.items.iter().filter(|i| i.picked).count();
            settings_kit::card_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let mut all = here > 0 && here_picked == here;
                    let some = here_picked > 0 && !all;
                    let check = settings_kit::check_mark(ui, &mut all, some, here > 0);
                    if check.on_hover_text(t!("import.toggle_group")).changed() {
                        action = Some(ImportAction::ToggleGroup(n, all));
                    }
                    let title = settings_kit::strong(format!("📁 {}", group.title));
                    ui.add(egui::Label::new(title).truncate()).on_hover_text(group.folder.display().to_string());
                    let known = group.items.len() - here;
                    let mut facts = vec![t!("import.new", count = here).into_owned()];
                    if known > 0 {
                        facts.push(t!("import.known", count = known).into_owned());
                    }
                    ui.add(egui::Label::new(RichText::new(facts.join(" · ")).weak()).truncate());
                });
                ui.horizontal_wrapped(|ui| {
                    for (i, info) in group.range.clone().zip(&group.items) {
                        item(ui, &entries[i], i, info, gallery, &mut action);
                    }
                });
            });
            ui.add_space(6.0);
        }
    });
    action
}

/// One candidate: its tick (or why it has none) over the tile. Allocated at its size, so the
/// wrapping row knows when to wrap.
fn item(
    ui: &mut Ui,
    entry: &Entry,
    i: usize,
    info: &ImportItemInfo,
    gallery: &Gallery,
    action: &mut Option<ImportAction>,
) {
    let spacing = ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
    let size = egui::vec2(TILE_WIDTH, TILE_WIDTH + 64.0 + spacing);
    ui.allocate_ui_with_layout(size, egui::Layout::top_down(egui::Align::Min), |ui| {
        ui.set_width(TILE_WIDTH);
        ui.horizontal(|ui| {
            let mut on = info.picked;
            if settings_kit::check_mark(ui, &mut on, false, info.importable()).changed() {
                *action = Some(ImportAction::Toggle(i));
            }
            let note = if let Some(at) = &info.in_library {
                Some((t!("import.in_library").into_owned(), at.display().to_string()))
            } else if info.busy {
                Some((t!("import.busy").into_owned(), String::new()))
            } else if let Some(files) = info.folder {
                Some((t!("import.folder_facts", count = files).into_owned(), String::new()))
            } else if let Some((members, models)) = info.archive {
                Some((t!("import.archive_facts", models = models, members = members).into_owned(), String::new()))
            } else if info.name_taken {
                Some((t!("import.renamed").into_owned(), String::new()))
            } else {
                None
            };
            if let Some((text, tip)) = note {
                let label = egui::Label::new(RichText::new(text).small().weak()).truncate();
                let response = ui.add(label);
                if !tip.is_empty() {
                    response.on_hover_text(tip);
                }
            }
        });
        let (response, click) = ui
            .scope(|ui| {
                if !info.importable() {
                    ui.multiply_opacity(0.45);
                }
                tile(ui, entry, gallery.thumb(i), (None, false), gallery.selected == Some(i), TILE_WIDTH, false)
            })
            .inner;
        match click {
            Some(TileClick::Select) => *action = Some(ImportAction::Select(i)),
            Some(TileClick::Open) => *action = Some(ImportAction::Open(i)),
            None => {}
        }
        if let Some(at) = &info.in_library {
            response.on_hover_text(t!("import.in_library_at", path = listing::file_name(at)));
        }
    });
}
