//! "Vorschläge prüfen": what an AI suggested, batch by batch, to accept in parts, in full or
//! not at all, and changed on the way: tags added or left out, a note rewritten, a new place
//! typed, a list renamed and trimmed; and "Verlauf": the changes accepted, to take back or do
//! again.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use egui::{Color32, Id, Rect, RichText, Shape, Ui, vec2};
use meshgrove_core::ai::proposals::{
    DeletePayload, ListPayload, MovePayload, NotePayload, Picked, Proposal, ProposalKind, TagsPayload,
};
use meshgrove_core::ai::vocab::TagStatus;
use meshgrove_core::archives::{ArchiveOp, ArchivePayload};
use meshgrove_core::bundle::ResolvedBundle;
use meshgrove_core::changes::ChangeInfo;
use meshgrove_core::listing::{self, Entry};
use meshgrove_core::lists::ListKind;

use super::duplicates::{TileClick, tile};
use super::gallery::Gallery;
use solidcraft_ui_kit as sc;

use super::suggest::{self, Candidate};
use super::settings_kit::{self, good};
use super::tags::{self, TagStyle};
use crate::i18n;

/// A new tag, which the AI made up.
const NEW_TAG: Color32 = Color32::from_rgb(230, 140, 40);
/// Space under each card.
const GAP: f32 = 6.0;
/// How long the green sweep takes to catch up with the library, and how long a finished card
/// takes to fade away.
const SWEEP_TIME: f32 = 0.25;
const FADE_TIME: f32 = 0.45;
/// The green of a card being applied, at its strongest.
const TINT: f32 = 0.35;

/// What a batch goes through after one of its buttons was clicked. The card stays where it is
/// (the next batch's buttons come to where the clicked ones were) until the library has
/// recorded the decisions, then fades away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flight {
    /// Sent to the library; `reject`: nothing is applied, only recorded.
    Applying { reject: bool },
    /// Recorded (and what was picked applied): the card fades.
    Done,
}

pub struct ReviewBatch {
    pub id: i64,
    pub uid: String,
    pub title: String,
    /// "Claude Desktop · lokal"
    pub source: String,
    pub created: Option<SystemTime>,
    pub inbox_done: bool,
    /// Made in the chat (its own switch decides whether it applies at once), or in a plugin's
    /// panel for the user to look at: never accepted by itself.
    pub chat: bool,
    pub items: Vec<ReviewItem>,
    /// After a click on one of its buttons, until the card has gone.
    pub flight: Option<Flight>,
}

impl ReviewBatch {
    fn sweep_id(&self) -> Id {
        Id::unique(("review_sweep", self.id))
    }

    fn fade_id(&self) -> Id {
        Id::unique(("review_fade", self.id))
    }

    /// How far the green sweep across the card should be.
    fn sweep_target(&self, applying: Option<(i64, usize, usize)>) -> f32 {
        match self.flight {
            Some(Flight::Applying { reject: true }) | None => 0.0,
            Some(Flight::Applying { reject: false }) => match applying {
                Some((batch, done, total)) if batch == self.id && total > 0 => (done as f32 / total as f32).max(0.08),
                // Waiting in the library's queue: a sliver says it is on its way.
                _ => 0.08,
            },
            Some(Flight::Done) => 1.0,
        }
    }
    fn moves(&self) -> bool {
        self.items.iter().any(|i| i.proposal.kind == ProposalKind::Move)
    }

    fn deletes(&self) -> bool {
        self.items.iter().any(|i| i.proposal.kind == ProposalKind::Delete)
    }

    fn archives(&self) -> bool {
        self.items.iter().any(|i| i.proposal.kind == ProposalKind::Archive)
    }
}

/// What the user is typing for one suggestion.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemDraft {
    /// A tag of their own, before Enter adds it.
    pub tag: String,
    /// A move: the new place as typed, relative to the library, and why it will not do.
    pub path: Option<String>,
    pub path_error: Option<String>,
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
    /// What the user is typing for it.
    pub draft: ItemDraft,
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
    /// A finished batch has faded away: take it out of the list.
    Remove(usize),
    History,
    /// Choose the folder a move (the proposal of this id) goes to, in the system's dialog.
    PickFolder(i64),
}

/// The folder the user chose in the system's dialog for a move: the file keeps its name and
/// goes there. A folder outside the move's library says so under the field instead.
pub fn move_into(item: &mut ReviewItem, folder: &std::path::Path) {
    let Ok(m) = serde_json::from_value::<MovePayload>(item.proposal.payload.clone()) else { return };
    let Some(from) = item.proposal.target.as_ref().map(|t| t.path.clone()) else { return };
    let chosen = item.picked.to.clone().unwrap_or_else(|| m.to.clone());
    let (Some(name), Ok(below)) = (chosen.file_name(), folder.strip_prefix(&m.root)) else {
        item.draft.path_error = Some(t!("review.place_outside", library = listing::file_name(&m.root)).into_owned());
        return;
    };
    let to = folder.join(name);
    // As the field shows a place: below the library, with the library's name for another one.
    let shown = below.join(name).display().to_string();
    let cross = !from.starts_with(&m.root);
    item.draft.path = Some(if cross { format!("{}/{shown}", listing::file_name(&m.root)) } else { shown });
    item.draft.path_error = None;
    item.picked.to = (to != m.to).then_some(to);
}

/// The scroll area's pending offset, set by a click and read the next frame.
fn jump_id() -> Id {
    Id::unique("review_scroll_jump")
}

/// The offset that brings batch `clicked + 1` to where batch `clicked` is: its buttons land
/// under the pointer, so the user can go on clicking. `rects` are the cards as drawn at
/// `offset`. None for the last batch.
pub(crate) fn jump_to_next(rects: &[Rect], clicked: usize, offset: f32) -> Option<f32> {
    let next = rects.get(clicked + 1)?;
    Some(offset + (next.min.y - rects[clicked].min.y))
}

/// How much the scroll offset shrinks when the card `removed` goes, so that what is below it
/// stays where it is on screen: the card's height when it starts above the viewport (what the
/// user looks at would move up otherwise), nothing when it is in view or below.
pub(crate) fn compensation(removed: Rect, viewport: Rect) -> f32 {
    if removed.min.y < viewport.min.y + 1.0 { removed.height() + GAP } else { 0.0 }
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
    (style, tags): (&TagStyle, &[(String, usize)]),
    applying: Option<(i64, usize, usize)>,
) -> Option<ReviewAction> {
    // One of the kit's forms: its controls, its fields, its label font.
    settings_kit::pane(ui, |ui| review(ui, entries, batches, gallery, (style, tags), applying))
}

fn review(
    ui: &mut Ui,
    entries: &[Entry],
    batches: &mut [ReviewBatch],
    gallery: &Gallery,
    (style, tags): (&TagStyle, &[(String, usize)]),
    applying: Option<(i64, usize, usize)>,
) -> Option<ReviewAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.label(settings_kit::strong(t!("review.summary", count = batches.len())));
        if let Some((_, done, total)) = applying {
            ui.weak(t!("review.applying", done = done, total = total));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if settings_kit::action(ui, t!("review.history"), true).clicked() {
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
    let ctx = ui.ctx().clone();
    let mut area = egui::ScrollArea::vertical().id_salt("review").auto_shrink(false);
    if let Some(target) = ui.data_mut(|d| d.remove_temp::<f32>(jump_id())) {
        area = area.vertical_scroll_offset(target);
    }
    // The cards as drawn this frame, for the scrolling below.
    let mut rects: Vec<Rect> = Vec::with_capacity(batches.len());
    let mut clicked = None;
    let mut remove = None;
    let shown = area.show(ui, |ui| {
        let viewport_h = ui.clip_rect().height();
        let rounding = egui::CornerRadius::same(solidcraft_theme::Tokens::get().radius_lg);
        for (bi, batch) in batches.iter_mut().enumerate() {
            let busy = batch.flight.is_some();
            // Painted once the card's place is known: behind it.
            let background = ui.painter().add(Shape::Noop);
            let sweep = ctx.animate_value_with_time(batch.sweep_id(), batch.sweep_target(applying), SWEEP_TIME);
            let fade = match batch.flight {
                Some(Flight::Done) => ctx.animate_value_with_time(batch.fade_id(), 1.0, FADE_TIME),
                _ => 0.0,
            };
            let card = ui.scope(|ui| {
                if fade > 0.0 {
                    ui.multiply_opacity(1.0 - fade);
                }
                settings_kit::card_frame().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if settings_kit::action(ui, t!("review.reject"), !busy).clicked() {
                                action = Some(ReviewAction::Reject(bi));
                                clicked = Some(bi);
                            }
                            if settings_kit::primary(ui, t!("review.accept_all"), !busy).clicked() {
                                action = Some(ReviewAction::Accept { batch: bi, all: true });
                                clicked = Some(bi);
                            }
                            let any = batch.items.iter().any(|i| !i.picked.is_empty() && !i.stale);
                            if settings_kit::action(ui, t!("review.accept_picked"), any && !busy).clicked() {
                                action = Some(ReviewAction::Accept { batch: bi, all: false });
                                clicked = Some(bi);
                            }
                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                let picked = batch.items.iter().filter(|i| !i.picked.is_empty()).count();
                                let mut all = picked == batch.items.len();
                                let some = picked > 0 && !all;
                                let check = settings_kit::check_mark(ui, &mut all, some, !busy);
                                if check.on_hover_text(t!("review.toggle_batch")).changed() {
                                    for item in &mut batch.items {
                                        item.picked = if all { Picked::all(&item.proposal) } else { Picked::default() };
                                    }
                                }
                                ui.add(
                                    egui::Label::new(settings_kit::strong(&batch.title)).truncate(),
                                );
                                let mut facts = vec![
                                    batch.source.clone(),
                                    t!("review.items", count = batch.items.len()).into_owned(),
                                ];
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
                    if batch.deletes() {
                        ui.weak(t!("review.deletes_hint"));
                    }
                    if batch.archives() {
                        ui.weak(t!("review.archives_hint"));
                    }
                    ui.add_enabled_ui(!busy, |ui| {
                        for item in &mut batch.items {
                            ui.separator();
                            if let Some(a) = item_row(ui, entries, item, gallery, (style, tags)) {
                                action = Some(a);
                            }
                        }
                    });
                })
            });
            let rect = card.inner.response.rect;
            if busy && sweep > 0.0 {
                // Green from the left, as far as the library got; gone with the card.
                let tint = Rect::from_min_size(rect.min, vec2(rect.width() * sweep, rect.height()));
                let color = good().gamma_multiply(TINT * (1.0 - fade));
                ui.painter().set(background, Shape::rect_filled(tint, rounding, color));
            }
            if fade >= 0.999 && remove.is_none() {
                remove = Some(bi);
            }
            rects.push(rect);
            ui.add_space(GAP);
        }
        // Room below the last card, so any card can be scrolled up to where a clicked one was.
        if let Some(last) = rects.last() {
            ui.add_space((viewport_h - last.height() - GAP).max(0.0));
        }
    });
    let offset = shown.state.clamped_offset().y;
    let mut jump = None;
    if let Some(bi) = clicked {
        // The sweep and the fade start from nothing, whenever the answer comes.
        ctx.animate_value_with_time(batches[bi].sweep_id(), 0.0, SWEEP_TIME);
        ctx.animate_value_with_time(batches[bi].fade_id(), 0.0, FADE_TIME);
        jump = jump_to_next(&rects, bi, offset);
    }
    if let Some(bi) = remove {
        action = Some(ReviewAction::Remove(bi));
        // On top of a click's jump of the same frame: the cards below the removed one come up
        // by its height either way.
        let back = compensation(rects[bi], shown.inner_rect);
        if back > 0.0 {
            jump = Some((jump.unwrap_or(offset) - back).max(0.0));
        }
    }
    if let Some(target) = jump {
        ui.data_mut(|d| d.insert_temp(jump_id(), target));
        ctx.request_repaint();
    }
    if batches.iter().any(|b| b.flight.is_some()) {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
    action
}

fn item_row(
    ui: &mut Ui,
    entries: &[Entry],
    item: &mut ReviewItem,
    gallery: &Gallery,
    (style, tags): (&TagStyle, &[(String, usize)]),
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
                ProposalKind::Tags => tags_row(ui, item, style, tags),
                ProposalKind::Note => note_row(ui, item),
                ProposalKind::List => list_row(ui, item),
                ProposalKind::Move => {
                    if let Some(a) = move_row(ui, item) {
                        action = Some(a);
                    }
                }
                ProposalKind::Bundle => bundle_row(ui, item),
                ProposalKind::Delete => delete_row(ui, item),
                ProposalKind::Archive => archive_row(ui, item),
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
            if item.copies > 1
                && !matches!(
                    p.kind,
                    ProposalKind::List
                        | ProposalKind::Move
                        | ProposalKind::Bundle
                        | ProposalKind::Delete
                        | ProposalKind::Archive
                )
            {
                ui.weak(t!("review.copies", count = item.copies - 1));
            }
        });
    });
    action
}

fn target_name(item: &ReviewItem) -> String {
    item.proposal.target.as_ref().map(|t| listing::file_name(&t.path)).unwrap_or_default()
}

fn tags_row(ui: &mut Ui, item: &mut ReviewItem, style: &TagStyle, tags: &[(String, usize)]) {
    ui.horizontal(|ui| {
        // The file as a whole: all its tags, or none.
        let mut on = !item.picked.is_empty();
        if sc::checkbox(ui, "", &mut on).on_hover_text(t!("review.toggle_file")).changed() {
            item.picked = if on { Picked::all(&item.proposal) } else { Picked::default() };
        }
        ui.label(settings_kit::strong(target_name(item)));
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
        // Tags the user added themselves, each with its ✖.
        let own: Vec<String> =
            item.picked.add.iter().filter(|t| !payload.add.iter().any(|a| &a.name == *t)).cloned().collect();
        for tag in own {
            if chip(ui, style, &tag, &format!("+{tag} ✖"), true).on_hover_text(t!("review.own_tag_remove")).clicked()
            {
                toggle(&mut item.picked.add, &tag);
            }
        }
        let hint = t!("review.add_tag_hint");
        let field =
            settings_kit::text_field_named(ui, ("own_tag", item.proposal.id), &mut item.draft.tag, &hint, 130.0);
        // The library's tags that match what is typed, in a list floating under the field:
        // Up and Down pick one, Enter or a click takes it; Enter with none picked takes what
        // is typed, as before.
        let needle = item.draft.tag.trim().to_lowercase();
        let held_id = field.id.with("list-held");
        let held = ui.data(|d| d.get_temp::<bool>(held_id)).unwrap_or(false);
        let active = field.has_focus() || field.lost_focus() || held;
        let wanted = |tag: &str| !item.picked.add.iter().any(|t| t.eq_ignore_ascii_case(tag));
        let known: Vec<Candidate<usize>> = if active && !needle.is_empty() {
            let candidate =
                |tag: String, files: usize| Candidate { label: tag, detail: String::new(), existing: files > 0, value: files };
            let mut all: Vec<Candidate<usize>> =
                tags.iter().filter(|(tag, _)| wanted(tag)).map(|(tag, files)| candidate(tag.clone(), *files)).collect();
            // And the suggested tags of the settings that no file carries yet, as the info pane
            // offers them.
            for group in tags::Group::SUGGESTED {
                for tag in tags::suggested(group, style) {
                    if wanted(&tag) && !all.iter().any(|c| c.label.eq_ignore_ascii_case(&tag)) {
                        all.push(candidate(tag, 0));
                    }
                }
            }
            suggest::rank(&needle, all, 8)
        } else {
            Vec::new()
        };
        let lit = suggest::highlight(ui, field.id.with("pick"), &needle, known.len(), field.has_focus());
        let mut take = None;
        let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if entered {
            take = Some(match lit.and_then(|i| known.get(i)) {
                Some(picked) => picked.label.clone(),
                None => item.draft.tag.trim().to_owned(),
            });
        }
        let mut down = false;
        if !known.is_empty() {
            let about = |files: usize| match files {
                0 => t!("review.tag_suggested").into_owned(),
                n => t!("review.tag_files", count = n).into_owned(),
            };
            let rows: Vec<suggest::Offer> = known
                .iter()
                .map(|c| suggest::Offer { name: c.label.clone(), about: about(c.value), dot: Some(style.colors(&c.label).0) })
                .collect();
            let place = (field.rect.left_bottom() + vec2(0.0, 2.0), egui::Align2::LEFT_TOP);
            let (clicked, on_list) = suggest::floating_list(ui, field.id.with("tags"), place, 240.0, &rows, lit);
            down = on_list;
            if let Some(i) = clicked {
                take = Some(known[i].label.clone());
            }
        }
        ui.data_mut(|d| d.insert_temp(held_id, down));
        if let Some(tag) = take {
            item.draft.tag.clear();
            if !tag.is_empty() && !item.picked.add.iter().any(|t| t.eq_ignore_ascii_case(&tag)) {
                // A removal of the same tag makes no sense any more.
                item.picked.remove.retain(|t| !t.eq_ignore_ascii_case(&tag));
                item.picked.add.push(tag);
            }
            field.request_focus();
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

/// The note as suggested, to rewrite before accepting; and whether it replaces the old one.
fn note_row(ui: &mut Ui, item: &mut ReviewItem) {
    ui.horizontal(|ui| {
        sc::checkbox(ui, "", &mut item.picked.whole);
        ui.label(settings_kit::strong(target_name(item)));
    });
    let Ok(note) = serde_json::from_value::<NotePayload>(item.proposal.payload.clone()) else { return };
    let has_old = item.note_before.as_deref().is_some_and(|b| !b.is_empty());
    let append = item.picked.append.get_or_insert(note.append);
    if let Some(before) = item.note_before.as_deref().filter(|b| !b.is_empty()) {
        let text = RichText::new(t!("review.note_before", text = before)).weak();
        ui.label(if *append { text } else { text.strikethrough() });
    }
    let text = item.picked.note.get_or_insert_with(|| note.text.clone());
    // The kit has one line fields; several lines are egui's in the same fill.
    ui.add(
        egui::TextEdit::multiline(text)
            .desired_rows(2)
            .desired_width(f32::INFINITY)
            .background_color(solidcraft_theme::Tokens::get().input_fill())
            .id_salt(("note_text", item.proposal.id)),
    )
    .on_hover_text(t!("review.note_edit_hint"));
    if has_old {
        settings_kit::checkbox(ui, &t!("review.note_append_check"), append);
    }
    if text.trim().is_empty() {
        ui.colored_label(ui.visuals().warn_fg_color, t!("review.note_empty"));
    }
}

/// Old and new place, relative to the library; folders the move makes.
fn move_row(ui: &mut Ui, item: &mut ReviewItem) -> Option<ReviewAction> {
    let mut action = None;
    let m = serde_json::from_value::<MovePayload>(item.proposal.payload.clone()).ok()?;
    let from = item.proposal.target.as_ref().map(|t| t.path.clone())?;
    // The new place is told from the library it goes to; a move into another library names it.
    let cross = !from.starts_with(&m.root);
    let rel = |p: &std::path::Path| match p.strip_prefix(&m.root) {
        Ok(rel) if cross => format!("{}/{}", listing::file_name(&m.root), rel.display()),
        Ok(rel) => rel.display().to_string(),
        Err(_) => p.display().to_string(),
    };
    let rename = !cross && from.parent() == m.to.parent();
    ui.horizontal(|ui| {
        sc::checkbox(ui, "", &mut item.picked.whole);
        let icon = if m.folder { "📁" } else { "📄" };
        let what = if rename { t!("review.rename") } else { t!("review.move") };
        ui.label(settings_kit::strong(format!("{icon} {what}")));
    });
    let mut chosen = item.picked.to.clone().unwrap_or_else(|| m.to.clone());
    let old = if rename { listing::file_name(&from) } else { rel(&from) };
    ui.label(RichText::new(old).weak().strikethrough());
    if cross {
        ui.weak(t!("review.cross_library", library = listing::file_name(&m.root)));
    }
    // The new place, to change: the name for a rename, the whole place below the library else.
    let shown = |p: &std::path::Path| if rename { listing::file_name(p) } else { rel(p) };
    let typed = item.draft.path.get_or_insert_with(|| shown(&chosen));
    ui.horizontal(|ui| {
        ui.label(settings_kit::strong("→"));
        // The system's folder dialog, for a place that is easier shown than typed: a button
        // inside the field. Not for a rename (the file stays in its folder), and only where
        // the dialog is built in.
        let picks = !rename && cfg!(feature = "plugins");
        let room = if picks { settings_kit::field_button_room() } else { 0.0 };
        let name = ("move_to_text", item.proposal.id);
        let field = settings_kit::text_field_named_reserving(ui, name, typed, "", 320.0 + room, room);
        if field.changed() {
            match place_from(&m.root, &from, rename, typed) {
                Ok(to) => {
                    item.draft.path_error = None;
                    chosen = to.clone();
                    item.picked.to = (to != m.to).then_some(to);
                }
                Err(why) => item.draft.path_error = Some(why),
            }
        }
        if picks && settings_kit::field_button(ui, &field, "folder", &t!("review.pick_folder")).clicked() {
            action = Some(ReviewAction::PickFolder(item.proposal.id));
        }
    });
    if let Some(why) = &item.draft.path_error {
        ui.colored_label(ui.visuals().warn_fg_color, why);
    } else if chosen != from && chosen.symlink_metadata().is_ok() && !same_place(&chosen, &from) {
        ui.colored_label(ui.visuals().warn_fg_color, t!("review.place_taken"));
    }
    if !m.alternatives.is_empty() {
        // Several folders fit the file's tags: the suggested one first, the others to pick.
        let folder_label = |p: &std::path::Path| {
            let dir = p.parent().unwrap_or(p);
            if dir.is_dir() { format!("{} ({})", rel(dir), t!("review.folder_exists")) } else { rel(dir) }
        };
        ui.horizontal(|ui| {
            ui.weak(t!("review.also_fits"));
            let width = ui.available_width().clamp(120.0, 320.0);
            settings_kit::dropdown(ui, ("move_to", item.proposal.id), &folder_label(&chosen), width, |ui| {
                for option in std::iter::once(&m.to).chain(&m.alternatives) {
                    if settings_kit::choice_row(ui, *option == chosen, &folder_label(option)) {
                        item.picked.to = (*option != m.to).then(|| option.clone());
                        item.draft.path = Some(shown(option));
                        item.draft.path_error = None;
                    }
                }
            });
        });
    }
    let made = meshgrove_core::moves::missing_folders(&m.root, chosen.parent().unwrap_or(&m.root));
    if let Some(last) = made.last() {
        ui.weak(t!("review.new_folder", path = rel(last)));
    }
    action
}

/// The place the user typed for a move: a name beside the old place (a rename), or a path below
/// the library; never outside it, never empty.
fn place_from(root: &std::path::Path, from: &std::path::Path, rename: bool, typed: &str) -> Result<PathBuf, String> {
    let typed = typed.trim().trim_matches(['/', '\\']);
    if typed.is_empty() {
        return Err(t!("review.place_empty").into_owned());
    }
    let parts: Vec<&str> = typed.split(['/', '\\']).collect();
    if parts.iter().any(|p| p.is_empty() || *p == "." || *p == ".." || p.chars().any(char::is_control)) {
        return Err(t!("review.place_invalid").into_owned());
    }
    if rename && parts.len() > 1 {
        // A rename became a move: fine, the path is taken below the library.
    }
    let to = if rename && parts.len() == 1 {
        from.parent().unwrap_or(root).join(parts[0])
    } else {
        parts.iter().fold(root.to_path_buf(), |p, part| p.join(part))
    };
    if !to.starts_with(root) || to == root {
        return Err(t!("review.place_invalid").into_owned());
    }
    Ok(to)
}

/// The same file under another spelling of its name (a case-only rename).
fn same_place(a: &std::path::Path, b: &std::path::Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

/// A file, archive or folder for the trash.
fn delete_row(ui: &mut Ui, item: &mut ReviewItem) {
    let Ok(d) = serde_json::from_value::<DeletePayload>(item.proposal.payload.clone()) else { return };
    let Some(path) = item.proposal.target.as_ref().map(|t| t.path.clone()) else { return };
    let rel = path.strip_prefix(&d.root).unwrap_or(&path).display().to_string();
    ui.horizontal(|ui| {
        sc::checkbox(ui, "", &mut item.picked.whole);
        let icon = if d.folder { "📁" } else { "📄" };
        ui.label(settings_kit::strong(format!("🗑 {icon} {}", t!("review.delete"))));
    });
    ui.label(RichText::new(rel).strikethrough());
    ui.weak(t!("review.delete_hint"));
}

/// An archive unpacked, files packed into a new ZIP, or added to one: what goes where.
fn archive_row(ui: &mut Ui, item: &mut ReviewItem) {
    let Ok(a) = serde_json::from_value::<ArchivePayload>(item.proposal.payload.clone()) else { return };
    let Some(path) = item.proposal.target.as_ref().map(|t| t.path.clone()) else { return };
    let rel = |p: &std::path::Path| p.strip_prefix(&a.root).unwrap_or(p).display().to_string();
    let name = listing::file_name(&path);
    let text = match &a.op {
        ArchiveOp::Unpack { into } => t!("review.archive_unpack", name = name, into = rel(into)),
        ArchiveOp::Pack { sources, to, .. } => t!("review.archive_pack", count = sources.len(), to = rel(to)),
        ArchiveOp::Add { sources, .. } => t!("review.archive_add", name = name, count = sources.len()),
    };
    ui.horizontal(|ui| {
        sc::checkbox(ui, "", &mut item.picked.whole);
        ui.label(settings_kit::strong(format!("📦 {text}")));
    });
    if let ArchiveOp::Add { prefix, .. } = &a.op
        && !prefix.is_empty()
    {
        ui.weak(t!("review.archive_into", folder = prefix.trim_end_matches('/')));
    }
    let sources = a.sources();
    for s in sources.iter().take(8) {
        ui.weak(rel(&s.path));
    }
    if sources.len() > 8 {
        ui.weak(t!("review.more", count = sources.len() - 8));
    }
}

/// A list as suggested: its name and kind to change, each item to leave out, the copies to set.
fn list_row(ui: &mut Ui, item: &mut ReviewItem) {
    let Ok(list) = serde_json::from_value::<ListPayload>(item.proposal.payload.clone()) else { return };
    let id = item.proposal.id;
    let kind = item.picked.list_kind.get_or_insert_with(|| list.kind.clone());
    let name = item.picked.list_name.get_or_insert_with(|| list.name.clone());
    ui.horizontal(|ui| {
        sc::checkbox(ui, "", &mut item.picked.whole);
        if list.query.is_none() {
            let mut print = kind == "print";
            let options = [(true, t!("review.print_list").into_owned()), (false, t!("review.collection").into_owned())];
            if settings_kit::segmented(ui, &mut print, &options) {
                *kind = if print { "print".into() } else { "collection".into() };
            }
        } else {
            ui.label(settings_kit::strong(t!("review.collection")));
        }
        settings_kit::text_field_named(ui, ("list_name", id), name, "", 220.0)
            .on_hover_text(t!("review.list_name_hint"));
    });
    if let Some(q) = &list.query {
        ui.weak(t!("review.saved_search", query = q));
    }
    if list.items.is_empty() {
        return;
    }
    let print = kind == "print";
    let kept = item
        .picked
        .list_items
        .get_or_insert_with(|| list.items.iter().enumerate().map(|(i, it)| (i, it.quantity)).collect());
    ui.weak(t!("review.list_items_hint"));
    for (i, entry) in list.items.iter().enumerate() {
        ui.horizontal(|ui| {
            let at = kept.iter().position(|(k, _)| *k == i);
            let mut on = at.is_some();
            if sc::checkbox(ui, "", &mut on).on_hover_text(t!("review.toggle")).changed() {
                match at {
                    Some(pos) => {
                        kept.remove(pos);
                    }
                    None => {
                        kept.push((i, entry.quantity));
                        kept.sort_by_key(|(k, _)| *k);
                    }
                }
            }
            ui.label(listing::file_name(&entry.target.path));
            if print && let Some(pos) = kept.iter().position(|(k, _)| *k == i) {
                ui.add(egui::DragValue::new(&mut kept[pos].1).range(1..=999).prefix("×"));
            }
        });
    }
}

/// Files a plugin made: where they go, what they are, and which lists they join.
fn bundle_row(ui: &mut Ui, item: &mut ReviewItem) {
    const SHOWN: usize = 12;
    let Ok(bundle) = serde_json::from_value::<ResolvedBundle>(item.proposal.payload.clone()) else { return };
    ui.horizontal(|ui| {
        sc::checkbox(ui, "", &mut item.picked.whole);
        ui.label(settings_kit::strong(&bundle.title));
        if !bundle.plugin.is_empty() {
            ui.weak(format!("· {}", bundle.plugin));
        }
    });
    // Where it lands now: its place may have been taken while it waited.
    let bundle = bundle.at_free_place();
    // A folder below its library, as the user knows it; the library itself by its name.
    let within = |dir: &std::path::Path| match dir.strip_prefix(&bundle.root) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.display().to_string(),
        _ => meshgrove_core::listing::file_name(&bundle.root),
    };
    let loose = bundle.is_loose();
    // Loose files beside what they were made from: beside one file, or each beside its own.
    let sources: std::collections::BTreeSet<_> =
        bundle.files.iter().filter_map(|f| Some((f.beside.as_deref()?, f.at.as_ref()?.parent()?))).collect();
    match (loose, sources.first()) {
        (true, Some((name, dir))) if sources.len() == 1 => {
            let (count, path) = (bundle.files.len(), within(dir));
            ui.label(match count {
                1 => t!("review.bundle_beside_one", name = name, path = path),
                _ => t!("review.bundle_beside", count = count, name = name, path = path),
            });
        }
        (true, _) => _ = ui.label(t!("review.bundle_beside_each", count = bundle.files.len())),
        (false, _) => {
            let (count, path) = (bundle.files.len(), within(&bundle.to));
            ui.label(match count {
                1 => t!("review.bundle_files_one", path = path),
                _ => t!("review.bundle_files", count = count, path = path),
            });
        }
    }
    ui.weak(t!("review.bundle_size", size = i18n::file_size(bundle.bytes)));
    for f in bundle.files.iter().take(SHOWN) {
        // A loose file by the name it gets; with several sources, also which one it is beside.
        let mut line = match (&f.at, &f.beside) {
            (Some(at), Some(beside)) if sources.len() > 1 => {
                let beside = t!("review.beside", name = beside, path = at.parent().map(within).unwrap_or_default());
                format!("• {}  ({beside})", meshgrove_core::listing::file_name(at))
            }
            (Some(at), _) => format!("• {}", meshgrove_core::listing::file_name(at)),
            (None, _) => format!("• {}", f.rel),
        };
        if !f.tags.is_empty() {
            line += &format!("  🏷 {}", f.tags.join(", "));
        }
        ui.label(RichText::new(line).small());
    }
    if bundle.files.len() > SHOWN {
        ui.weak(t!("review.more", count = bundle.files.len() - SHOWN));
    }
    for list in &bundle.lists {
        let kind = if list.kind == ListKind::Print { t!("review.print_list") } else { t!("review.collection") };
        let items: Vec<String> = list
            .items
            .iter()
            .take(SHOWN)
            .map(|(rel, n)| {
                let name = rel.rsplit('/').next().unwrap_or(rel);
                if *n > 1 { format!("{name} ×{n}") } else { name.to_owned() }
            })
            .collect();
        ui.label(format!("{kind} „{}“: {}", list.name, items.join(", ")));
    }
}

pub fn show_history(ui: &mut Ui, changes: &[ChangeInfo]) -> Option<HistoryAction> {
    settings_kit::pane(ui, |ui| history(ui, changes))
}

fn history(ui: &mut Ui, changes: &[ChangeInfo]) -> Option<HistoryAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.label(settings_kit::strong(t!("history.title")));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if settings_kit::action(ui, t!("history.review"), true).clicked() {
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
                    if settings_kit::action(ui, t!("history.redo"), true).on_hover_text(t!("history.redo_hint")).clicked() {
                        action = Some(HistoryAction::Redo(c.id));
                    }
                } else if settings_kit::action(ui, t!("history.undo"), true).on_hover_text(t!("history.undo_hint")).clicked()
                {
                    action = Some(HistoryAction::Undo(c.id));
                }
                ui.end_row();
            }
        });
    });
    action
}

#[cfg(test)]
mod tag_tests {
    use super::*;

    /// A tag suggestion for one file, drawn in a headless egui; `run` is one frame.
    struct Row {
        ctx: egui::Context,
        item: ReviewItem,
        time: f64,
        shapes: Vec<egui::epaint::ClippedShape>,
    }

    impl Row {
        fn new() -> Row {
            let payload = serde_json::json!({"add": [{"name": "garden", "status": "suggested"}]});
            let proposal = Proposal {
                id: 7,
                batch_id: 1,
                kind: ProposalKind::Tags,
                target: None,
                payload,
                reason: None,
                confidence: None,
                state: "pending".into(),
            };
            let item = ReviewItem {
                picked: Picked::all(&proposal),
                proposal,
                entry: None,
                moved_to: None,
                stale: false,
                copies: 1,
                note_before: None,
                taken: false,
                draft: ItemDraft::default(),
            };
            let mut row = Row { ctx: egui::Context::default(), item, time: 0.0, shapes: Vec::new() };
            row.run(Vec::new());
            row.run(Vec::new());
            row
        }

        fn run(&mut self, events: Vec<egui::Event>) {
            self.time += 0.05;
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(500.0, 400.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let item = &mut self.item;
            let tags = [("holder".to_owned(), 3), ("hook".to_owned(), 1), ("garden".to_owned(), 9)];
            let mut output = self.ctx.run_ui(raw, |ui| {
                settings_kit::pane(ui, |ui| tags_row(ui, item, &TagStyle::default(), &tags));
            });
            output.textures_delta.clear();
            self.shapes = output.shapes;
        }

        fn find(&self, text: &str) -> Option<Rect> {
            fn walk(shape: &Shape, text: &str, out: &mut Option<Rect>) {
                match shape {
                    Shape::Text(t) if t.galley.text() == text => *out = Some(Rect::from_min_size(t.pos, t.galley.size())),
                    Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, text, out)),
                    _ => {}
                }
            }
            let mut out = None;
            self.shapes.iter().for_each(|s| walk(&s.shape, text, &mut out));
            out
        }

        fn click(&mut self, at: egui::Pos2) {
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            self.run(vec![egui::Event::PointerMoved(at), button(true)]);
            self.run(vec![button(false)]);
            self.run(Vec::new());
        }

        /// Types into the field (a click on its hint gives it the keyboard first), and lets
        /// the list settle.
        fn text(&mut self, text: &str) {
            self.run(vec![egui::Event::Text(text.into())]);
            self.run(Vec::new());
            self.run(Vec::new());
        }

        fn key(&mut self, key: egui::Key) {
            let event = |pressed| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            };
            self.run(vec![event(true)]);
            self.run(vec![event(false)]);
        }
    }

    /// Typing a tag of one's own opens a list of the library's tags that match; a click takes
    /// one, the field is empty again and keeps the keyboard.
    #[test]
    fn a_typed_tag_offers_the_librarys_tags_and_a_click_takes_one() {
        let mut row = Row::new();
        let hint = row.find(&t!("review.add_tag_hint")).expect("the field's hint");
        row.click(hint.center());
        row.text("ho");
        let offered = row.find("holder").expect("the list is open");
        assert!(row.find("hook").is_some() && row.find("garden").is_none(), "what matches, without what is on already");
        row.click(offered.center());
        assert_eq!(row.item.picked.add, ["garden", "holder"]);
        assert!(row.item.draft.tag.is_empty());
        row.text("x");
        assert_eq!(row.item.draft.tag, "x", "the field kept the keyboard");
    }

    /// Down and Enter take the tag the list offers; Enter alone takes what is typed.
    #[test]
    fn the_arrow_keys_pick_a_tag_and_enter_takes_it() {
        let mut row = Row::new();
        let hint = row.find(&t!("review.add_tag_hint")).expect("the field's hint");
        row.click(hint.center());
        row.text("ho");
        row.key(egui::Key::ArrowDown);
        row.key(egui::Key::Enter);
        assert_eq!(row.item.picked.add, ["garden", "holder"]);
        row.text("own");
        row.key(egui::Key::Enter);
        assert_eq!(row.item.picked.add, ["garden", "holder", "own"]);
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::*;
    use egui::pos2;

    fn card(top: f32, height: f32) -> Rect {
        Rect::from_min_size(pos2(0.0, top), vec2(400.0, height))
    }

    #[test]
    fn the_next_batch_comes_to_where_the_clicked_one_is() {
        // Cards at 100, 300 and 450 on screen, the view scrolled by 40.
        let rects = [card(100.0, 194.0), card(300.0, 144.0), card(450.0, 80.0)];
        assert_eq!(jump_to_next(&rects, 0, 40.0), Some(240.0));
        assert_eq!(jump_to_next(&rects, 1, 40.0), Some(190.0));
        assert_eq!(jump_to_next(&rects, 2, 40.0), None, "nothing after the last one");
    }

    #[test]
    fn a_card_that_goes_above_the_view_takes_its_height_out_of_the_offset() {
        let viewport = Rect::from_min_size(pos2(0.0, 100.0), vec2(400.0, 600.0));
        assert_eq!(compensation(card(-50.0, 120.0), viewport), 120.0 + GAP);
        assert_eq!(compensation(card(100.0, 120.0), viewport), 120.0 + GAP, "starting at the edge");
        assert_eq!(compensation(card(150.0, 120.0), viewport), 0.0, "in view: what is below moves up");
        assert_eq!(compensation(card(900.0, 120.0), viewport), 0.0, "below the view");
    }
}
