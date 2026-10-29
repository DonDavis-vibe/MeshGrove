//! "Vorschläge prüfen": what an AI suggested, batch by batch, to accept in parts, in full or
//! not at all; and "Verlauf": the changes accepted, to take back or do again.

use std::path::PathBuf;
use std::time::SystemTime;

use egui::{Color32, RichText, Ui};
use meshgrove_core::ai::proposals::{
    ListPayload, MovePayload, NotePayload, Picked, Proposal, ProposalKind, TagsPayload,
};
use meshgrove_core::ai::vocab::TagStatus;
use meshgrove_core::changes::ChangeInfo;
use meshgrove_core::listing::{self, Entry};

use super::duplicates::{TileClick, tile};
use super::gallery::Gallery;
use super::tags::{self, TagStyle};
use crate::i18n;

/// A new tag, which the AI made up.
const NEW_TAG: Color32 = Color32::from_rgb(230, 140, 40);

pub struct ReviewBatch {
    pub id: i64,
    pub uid: String,
    pub title: String,
    /// "Claude Desktop · lokal"
    pub source: String,
    pub created: Option<SystemTime>,
    pub inbox_done: bool,
    /// Made in the chat (its own switch decides whether it applies at once).
    pub chat: bool,
    pub items: Vec<ReviewItem>,
}

impl ReviewBatch {
    fn moves(&self) -> bool {
        self.items.iter().any(|i| i.proposal.kind == ProposalKind::Move)
    }
}

pub struct ReviewItem {
    pub proposal: Proposal,
    /// The file in the view's entries, while it is still in the library.
    pub entry: Option<usize>,
    pub moved_to: Option<PathBuf>,
    pub stale: bool,
    /// Files with the same content: tags and notes go to all of them.
    pub copies: usize,
    /// What the user keeps of the suggestion.
    pub picked: Picked,
    /// The note the file has now (note suggestions).
    pub note_before: Option<String>,
    /// A move whose new place is taken by now.
    pub taken: bool,
}

pub enum ReviewAction {
    Select(usize),
    Open(usize),
    /// Accept the batch: what is picked, or everything.
    Accept {
        batch: usize,
        all: bool,
    },
    Reject(usize),
    History,
}

pub enum HistoryAction {
    Undo(i64),
    Redo(i64),
    Review,
}

pub fn show(
    ui: &mut Ui,
    entries: &[Entry],
    batches: &mut [ReviewBatch],
    gallery: &Gallery,
    style: &TagStyle,
) -> Option<ReviewAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new(t!("review.summary", count = batches.len())).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(t!("review.history")).clicked() {
                action = Some(ReviewAction::History);
            }
        });
    });
    ui.weak(t!("review.explain"));
    ui.add_space(6.0);
    if batches.is_empty() {
        ui.centered_and_justified(|ui| ui.weak(t!("review.empty")));
        return action;
    }
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        for (bi, batch) in batches.iter_mut().enumerate() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(t!("review.reject")).clicked() {
                            action = Some(ReviewAction::Reject(bi));
                        }
                        if ui.button(t!("review.accept_all")).clicked() {
                            action = Some(ReviewAction::Accept { batch: bi, all: true });
                        }
                        let any = batch.items.iter().any(|i| !i.picked.is_empty() && !i.stale);
                        if ui.add_enabled(any, egui::Button::new(t!("review.accept_picked"))).clicked() {
                            action = Some(ReviewAction::Accept { batch: bi, all: false });
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            let picked = batch.items.iter().filter(|i| !i.picked.is_empty()).count();
                            let mut all = picked == batch.items.len();
                            let some = picked > 0 && !all;
                            let check = egui::Checkbox::without_text(&mut all).indeterminate(some);
                            if ui.add(check).on_hover_text(t!("review.toggle_batch")).changed() {
                                for item in &mut batch.items {
                                    item.picked = if all { Picked::all(&item.proposal) } else { Picked::default() };
                                }
                            }
                            ui.add(egui::Label::new(RichText::new(format!("✧ {}", batch.title)).strong()).truncate());
                            let mut facts =
                                vec![batch.source.clone(), t!("review.items", count = batch.items.len()).into_owned()];
                            if let Some(t) = batch.created {
                                facts.push(i18n::datetime(t));
                            }
                            ui.add(egui::Label::new(RichText::new(facts.join(" · ")).weak()).truncate());
                        });
                    })
                });
                if batch.inbox_done {
                    ui.weak(t!("review.inbox_done"));
                }
                if batch.moves() {
                    ui.weak(t!("review.moves_hint"));
                }
                for item in &mut batch.items {
                    ui.separator();
                    if let Some(a) = item_row(ui, entries, item, gallery, style) {
                        action = Some(a);
                    }
                }
            });
            ui.add_space(6.0);
        }
    });
    action
}

fn item_row(
    ui: &mut Ui,
    entries: &[Entry],
    item: &mut ReviewItem,
    gallery: &Gallery,
    style: &TagStyle,
) -> Option<ReviewAction> {
    let mut action = None;
    ui.horizontal_top(|ui| {
        if let Some(i) = item.entry {
            let (_, click) =
                tile(ui, &entries[i], gallery.thumb(i), (None, false), gallery.selected == Some(i), 120.0, false);
            match click {
                Some(TileClick::Select) => action = Some(ReviewAction::Select(i)),
                Some(TileClick::Open) => action = Some(ReviewAction::Open(i)),
                None => {}
            }
        }
        ui.vertical(|ui| {
            let p = &item.proposal;
            match p.kind {
                ProposalKind::Tags => tags_row(ui, item, style),
                ProposalKind::Note => note_row(ui, item),
                ProposalKind::List => list_row(ui, item),
                ProposalKind::Move => move_row(ui, item),
            }
            let p = &item.proposal;
            if let Some(reason) = &p.reason {
                ui.label(RichText::new(reason).italics().weak());
            }
            if item.taken {
                ui.colored_label(ui.visuals().warn_fg_color, t!("review.move_taken"));
            } else if item.stale {
                ui.colored_label(ui.visuals().warn_fg_color, t!("review.stale"));
            } else if let Some(to) = &item.moved_to {
                ui.weak(t!("review.moved", path = to.display().to_string()));
            }
            if item.copies > 1 && !matches!(p.kind, ProposalKind::List | ProposalKind::Move) {
                ui.weak(t!("review.copies", count = item.copies - 1));
            }
        });
    });
    action
}

fn target_name(item: &ReviewItem) -> String {
    item.proposal.target.as_ref().map(|t| listing::file_name(&t.path)).unwrap_or_default()
}

fn tags_row(ui: &mut Ui, item: &mut ReviewItem, style: &TagStyle) {
    ui.horizontal(|ui| {
        // The file as a whole: all its tags, or none.
        let mut on = !item.picked.is_empty();
        if ui.checkbox(&mut on, "").on_hover_text(t!("review.toggle_file")).changed() {
            item.picked = if on { Picked::all(&item.proposal) } else { Picked::default() };
        }
        ui.label(RichText::new(target_name(item)).strong());
    });
    let Ok(payload) = serde_json::from_value::<TagsPayload>(item.proposal.payload.clone()) else { return };
    ui.horizontal_wrapped(|ui| {
        for tag in &payload.add {
            let on = item.picked.add.contains(&tag.name);
            let label = format!("{} +{}", if on { "☑" } else { "☐" }, tag.name);
            let response = chip(ui, style, &tag.name, &label, on);
            if tag.status == TagStatus::New {
                ui.colored_label(NEW_TAG, t!("review.new_tag")).on_hover_text(t!("review.new_tag_hint"));
            }
            if response.on_hover_text(t!("review.toggle")).clicked() {
                toggle(&mut item.picked.add, &tag.name);
            }
        }
        for tag in &payload.remove {
            let on = item.picked.remove.contains(tag);
            let label = format!("{} −{tag}", if on { "☑" } else { "☐" });
            if chip(ui, style, tag, &label, on).on_hover_text(t!("review.toggle")).clicked() {
                toggle(&mut item.picked.remove, tag);
            }
        }
    });
}

/// A tag of the suggestion, faded while it is left out.
fn chip(ui: &mut Ui, style: &TagStyle, tag: &str, label: &str, on: bool) -> egui::Response {
    ui.scope(|ui| {
        if !on {
            ui.multiply_opacity(0.4);
        }
        tags::chip(ui, style, tag, label)
    })
    .inner
}

fn toggle(list: &mut Vec<String>, tag: &str) {
    match list.iter().position(|t| t == tag) {
        Some(i) => {
            list.remove(i);
        }
        None => list.push(tag.to_owned()),
    }
}

fn note_row(ui: &mut Ui, item: &mut ReviewItem) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut item.picked.whole, "");
        ui.label(RichText::new(target_name(item)).strong());
    });
    let Ok(note) = serde_json::from_value::<NotePayload>(item.proposal.payload.clone()) else { return };
    if let Some(before) = item.note_before.as_deref().filter(|b| !b.is_empty()) {
        ui.label(RichText::new(t!("review.note_before", text = before)).weak().strikethrough());
    }
    let label = if note.append { t!("review.note_append") } else { t!("review.note_new") };
    ui.label(format!("{label} {}", note.text));
}

/// Old and new place, relative to the library; folders the move makes.
fn move_row(ui: &mut Ui, item: &mut ReviewItem) {
    let Ok(m) = serde_json::from_value::<MovePayload>(item.proposal.payload.clone()) else { return };
    let Some(from) = item.proposal.target.as_ref().map(|t| t.path.clone()) else { return };
    let rel = |p: &std::path::Path| p.strip_prefix(&m.root).unwrap_or(p).display().to_string();
    let rename = from.parent() == m.to.parent();
    ui.horizontal(|ui| {
        ui.checkbox(&mut item.picked.whole, "");
        let icon = if m.folder { "📁" } else { "📄" };
        let what = if rename { t!("review.rename") } else { t!("review.move") };
        ui.label(RichText::new(format!("{icon} {what}")).strong());
    });
    let chosen = item.picked.to.clone().unwrap_or_else(|| m.to.clone());
    let (old, new) =
        if rename { (listing::file_name(&from), listing::file_name(&chosen)) } else { (rel(&from), rel(&chosen)) };
    ui.label(RichText::new(old).weak().strikethrough());
    ui.label(RichText::new(format!("→ {new}")).strong());
    if !m.alternatives.is_empty() {
        // Several folders fit the file's tags: the suggested one first, the others to pick.
        let folder_label = |p: &std::path::Path| {
            let dir = p.parent().unwrap_or(p);
            if dir.is_dir() { format!("{} ({})", rel(dir), t!("review.folder_exists")) } else { rel(dir) }
        };
        ui.horizontal(|ui| {
            ui.weak(t!("review.also_fits"));
            egui::ComboBox::from_id_salt(("move_to", item.proposal.id)).selected_text(folder_label(&chosen)).show_ui(
                ui,
                |ui| {
                    for option in std::iter::once(&m.to).chain(&m.alternatives) {
                        if ui.selectable_label(*option == chosen, folder_label(option)).clicked() {
                            item.picked.to = (*option != m.to).then(|| option.clone());
                        }
                    }
                },
            );
        });
    }
    let made = meshgrove_core::moves::missing_folders(&m.root, chosen.parent().unwrap_or(&m.root));
    if let Some(last) = made.last() {
        ui.weak(t!("review.new_folder", path = rel(last)));
    }
}

fn list_row(ui: &mut Ui, item: &mut ReviewItem) {
    let Ok(list) = serde_json::from_value::<ListPayload>(item.proposal.payload.clone()) else { return };
    ui.horizontal(|ui| {
        ui.checkbox(&mut item.picked.whole, "");
        let kind = if list.kind == "print" { t!("review.print_list") } else { t!("review.collection") };
        ui.label(RichText::new(format!("{kind} „{}“", list.name)).strong());
    });
    if let Some(q) = &list.query {
        ui.weak(t!("review.saved_search", query = q));
    }
    let names: Vec<String> = list
        .items
        .iter()
        .take(12)
        .map(|i| {
            let name = listing::file_name(&i.target.path);
            if i.quantity > 1 { format!("{name} ×{}", i.quantity) } else { name }
        })
        .collect();
    if !names.is_empty() {
        let more = list.items.len().saturating_sub(names.len());
        let mut text = names.join(", ");
        if more > 0 {
            text += &format!(" {}", t!("review.more", count = more));
        }
        ui.label(text);
    }
}

pub fn show_history(ui: &mut Ui, changes: &[ChangeInfo]) -> Option<HistoryAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new(t!("history.title")).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(t!("history.review")).clicked() {
                action = Some(HistoryAction::Review);
            }
        });
    });
    ui.weak(t!("history.explain"));
    ui.add_space(6.0);
    if changes.is_empty() {
        ui.centered_and_justified(|ui| ui.weak(t!("history.empty")));
        return action;
    }
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        egui::Grid::new("history").num_columns(4).striped(true).spacing([14.0, 6.0]).show(ui, |ui| {
            for c in changes {
                let at = SystemTime::UNIX_EPOCH + std::time::Duration::from_nanos(c.applied_ns.max(0) as u64);
                ui.weak(i18n::datetime(at));
                let title = if c.undone_ns.is_some() {
                    RichText::new(&c.title).strikethrough().weak()
                } else {
                    RichText::new(&c.title)
                };
                ui.label(title);
                ui.weak(t!("history.steps", count = c.steps));
                if c.undone_ns.is_some() {
                    if ui.button(t!("history.redo")).on_hover_text(t!("history.redo_hint")).clicked() {
                        action = Some(HistoryAction::Redo(c.id));
                    }
                } else if ui.button(t!("history.undo")).on_hover_text(t!("history.undo_hint")).clicked() {
                    action = Some(HistoryAction::Undo(c.id));
                }
                ui.end_row();
            }
        });
    });
    action
}
