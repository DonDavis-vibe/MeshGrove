//! "Eingang" view: files the library had not seen before, grouped by the drop they came with
//! (a new folder, a customer's ZIP), until they are marked as done.

use std::ops::Range;
use std::path::PathBuf;
use std::time::SystemTime;

use egui::{Key, Modifiers, RichText, Ui};
use meshgrove_core::listing::Entry;

use super::duplicates::{TILE_WIDTH, TileClick, tile};
use super::gallery::{Action, Gallery, MenuContext, entry_menu};
use crate::i18n;

pub struct InboxGroupInfo {
    /// The group's files in the view's entry list.
    pub range: Range<usize>,
    /// The folder (or archive) as the library shows it: relative to the library, or its name.
    pub title: String,
    pub folder: PathBuf,
    pub archive: bool,
    /// When its newest file was seen.
    pub newest: Option<SystemTime>,
    /// Files of the drop whose content was known already.
    pub known: usize,
}

pub enum InboxAction {
    /// A tile was clicked with these modifiers (Ctrl and Shift extend the selection).
    Click(usize, Modifiers),
    /// Ctrl+A: every file of the inbox.
    SelectAll,
    Open(usize),
    /// An entry of a tile's context menu, the gallery's.
    Gallery(Action),
    /// These entries are done.
    Done(Vec<usize>),
    AllDone,
    /// Ask the AI for tags: for this group, or for all.
    Classify(Option<usize>),
    /// Suggest moving the files into the folders of their tags: this group, or all.
    Place(Option<usize>),
}

/// `menu`: what the tiles' context menu offers for the selection; `ai`: the user's AI can be
/// asked for tags; `placement`: files can be placed by their tags.
pub fn show(
    ui: &mut Ui,
    entries: &[Entry],
    groups: &[InboxGroupInfo],
    gallery: &Gallery,
    menu: &MenuContext,
    ai: bool,
    placement: bool,
) -> Option<InboxAction> {
    let mut action = None;
    if groups.is_empty() {
        ui.centered_and_justified(|ui| ui.weak(t!("inbox.empty")));
        return None;
    }
    // Not while a text field (the tag field) has the keyboard.
    if !ui.ctx().egui_wants_keyboard_input() && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::A)) {
        action = Some(InboxAction::SelectAll);
    }
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.horizontal(|ui| {
            let summary = match (entries.len(), groups.len()) {
                (1, _) => t!("inbox.summary_one"),
                (files, 1) => t!("inbox.summary_one_group", files = i18n::count(files)),
                (files, groups) => t!("inbox.summary", files = i18n::count(files), groups = i18n::count(groups)),
            };
            ui.label(RichText::new(summary).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(t!("inbox.all_done")).clicked() {
                    action = Some(InboxAction::AllDone);
                }
                if ai && ui.button(t!("actions.classify_all")).on_hover_text(t!("actions.classify_hint")).clicked() {
                    action = Some(InboxAction::Classify(None));
                }
                if placement && ui.button(t!("placement.inbox_all")).on_hover_text(t!("placement.inbox_hint")).clicked()
                {
                    action = Some(InboxAction::Place(None));
                }
            });
        });
        ui.weak(t!("inbox.explain"));
        ui.add_space(8.0);
        for (n, group) in groups.iter().enumerate() {
            let warned: Vec<bool> = group.range.clone().map(|i| check_flag(gallery, &entries[i], i)).collect();
            let problems = warned.iter().filter(|w| **w).count();
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                // The button first, so a long folder name is shortened instead of hiding it.
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(t!("inbox.done")).on_hover_text(t!("inbox.done_hint")).clicked() {
                            action = Some(InboxAction::Done(group.range.clone().collect()));
                        }
                        if ai && ui.button(t!("actions.classify")).on_hover_text(t!("actions.classify_hint")).clicked()
                        {
                            action = Some(InboxAction::Classify(Some(n)));
                        }
                        if placement
                            && ui
                                .button(t!("placement.inbox_button"))
                                .on_hover_text(t!("placement.inbox_hint"))
                                .clicked()
                        {
                            action = Some(InboxAction::Place(Some(n)));
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            let icon = if group.archive { "📦" } else { "📁" };
                            let title = RichText::new(format!("{icon} {}", group.title)).strong();
                            ui.add(egui::Label::new(title).truncate()).on_hover_text(&group.title);
                            let mut facts = vec![t!("inbox.new", count = group.range.len()).into_owned()];
                            if group.known > 0 {
                                facts.push(t!("inbox.known", count = group.known).into_owned());
                            }
                            if let Some(t) = group.newest {
                                facts.push(t!("inbox.since", time = i18n::datetime(t)).into_owned());
                            }
                            let facts = RichText::new(facts.join(" · ")).weak();
                            ui.add(egui::Label::new(facts).truncate()).on_hover_text(t!("inbox.known_hint"));
                            if problems > 0 {
                                let text = RichText::new(format!("⚠ {}", t!("inbox.problems", count = problems)))
                                    .color(ui.visuals().warn_fg_color);
                                ui.add(egui::Label::new(text).truncate());
                            }
                        });
                    })
                });
                ui.horizontal_wrapped(|ui| {
                    for (i, warn) in group.range.clone().zip(warned) {
                        let (response, click) = tile(
                            ui,
                            &entries[i],
                            gallery.thumb(i),
                            (None, warn),
                            gallery.is_marked(i),
                            TILE_WIDTH,
                            false,
                        );
                        match click {
                            Some(TileClick::Select) => {
                                let modifiers = ui.input(|input| input.modifiers);
                                action = Some(InboxAction::Click(i, modifiers));
                            }
                            Some(TileClick::Open) => action = Some(InboxAction::Open(i)),
                            None => {}
                        }
                        // A right-click outside the selection selects the tile, as in the gallery.
                        if response.secondary_clicked() && !gallery.is_marked(i) {
                            action = Some(InboxAction::Click(i, Modifiers::NONE));
                        }
                        let mut picked = None;
                        entry_menu(&response, i, &entries[i], true, menu, &mut picked);
                        if let Some(a) = picked {
                            action = Some(InboxAction::Gallery(a));
                        }
                    }
                });
            });
            ui.add_space(6.0);
        }
    });
    action
}

/// Whether the printability check found something about this file.
fn check_flag(gallery: &Gallery, entry: &Entry, index: usize) -> bool {
    let (problems, flagged) = gallery.check_problems(entry, index);
    flagged || !problems.is_empty()
}
