//! "Vorschläge prüfen": what an AI suggested, batch by batch, to accept in parts, in full or
//! not at all, and changed on the way: tags added or left out, a note rewritten, a new place
//! typed, a list renamed and trimmed; and "Verlauf": the changes accepted, to take back or do
//! again.

use std::path::PathBuf;
use std::time::SystemTime;

use egui::{Color32, RichText, Ui};
use meshgrove_core::ai::proposals::{
    DeletePayload, ListPayload, MovePayload, NotePayload, Picked, Proposal, ProposalKind, TagsPayload,
};
use meshgrove_core::ai::vocab::TagStatus;
use meshgrove_core::bundle::ResolvedBundle;
use meshgrove_core::changes::ChangeInfo;
use meshgrove_core::listing::{self, Entry};
use meshgrove_core::lists::ListKind;

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
    /// Made in the chat (its own switch decides whether it applies at once), or in a plugin's
    /// panel for the user to look at: never accepted by itself.
    pub chat: bool,
    pub items: Vec<ReviewItem>,
}

impl ReviewBatch {
    fn moves(&self) -> bool {
        self.items.iter().any(|i| i.proposal.kind == ProposalKind::Move)
    }

    fn deletes(&self) -> bool {
        self.items.iter().any(|i| i.proposal.kind == ProposalKind::Delete)
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
                            ui.add(egui::Label::new(RichText::new(format!("✨ {}", batch.title)).strong()).truncate());
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
                if batch.deletes() {
                    ui.weak(t!("review.deletes_hint"));
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
                ProposalKind::Bundle => bundle_row(ui, item),
                ProposalKind::Delete => delete_row(ui, item),
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
                    ProposalKind::List | ProposalKind::Move | ProposalKind::Bundle | ProposalKind::Delete
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
        // Tags the user added themselves, each with its ✖.
        let own: Vec<String> =
            item.picked.add.iter().filter(|t| !payload.add.iter().any(|a| &a.name == *t)).cloned().collect();
        for tag in own {
            if chip(ui, style, &tag, &format!("+{tag} ✖"), true).on_hover_text(t!("review.own_tag_remove")).clicked()
            {
                toggle(&mut item.picked.add, &tag);
            }
        }
        let field = ui.add(
            egui::TextEdit::singleline(&mut item.draft.tag)
                .hint_text(t!("review.add_tag_hint"))
                .desired_width(120.0)
                .id_salt(("own_tag", item.proposal.id)),
        );
        let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if entered {
            let tag = std::mem::take(&mut item.draft.tag);
            let tag = tag.trim();
            if !tag.is_empty() && !item.picked.add.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
                item.picked.add.push(tag.to_owned());
                // A removal of the same tag makes no sense any more.
                item.picked.remove.retain(|t| !t.eq_ignore_ascii_case(tag));
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
        ui.checkbox(&mut item.picked.whole, "");
        ui.label(RichText::new(target_name(item)).strong());
    });
    let Ok(note) = serde_json::from_value::<NotePayload>(item.proposal.payload.clone()) else { return };
    let has_old = item.note_before.as_deref().is_some_and(|b| !b.is_empty());
    let append = item.picked.append.get_or_insert(note.append);
    if let Some(before) = item.note_before.as_deref().filter(|b| !b.is_empty()) {
        let text = RichText::new(t!("review.note_before", text = before)).weak();
        ui.label(if *append { text } else { text.strikethrough() });
    }
    let text = item.picked.note.get_or_insert_with(|| note.text.clone());
    ui.add(
        egui::TextEdit::multiline(text)
            .desired_rows(2)
            .desired_width(f32::INFINITY)
            .id_salt(("note_text", item.proposal.id)),
    )
    .on_hover_text(t!("review.note_edit_hint"));
    if has_old {
        ui.checkbox(append, t!("review.note_append_check"));
    }
    if text.trim().is_empty() {
        ui.colored_label(ui.visuals().warn_fg_color, t!("review.note_empty"));
    }
}

/// Old and new place, relative to the library; folders the move makes.
fn move_row(ui: &mut Ui, item: &mut ReviewItem) {
    let Ok(m) = serde_json::from_value::<MovePayload>(item.proposal.payload.clone()) else { return };
    let Some(from) = item.proposal.target.as_ref().map(|t| t.path.clone()) else { return };
    // The new place is told from the library it goes to; a move into another library names it.
    let cross = !from.starts_with(&m.root);
    let rel = |p: &std::path::Path| match p.strip_prefix(&m.root) {
        Ok(rel) if cross => format!("{}/{}", listing::file_name(&m.root), rel.display()),
        Ok(rel) => rel.display().to_string(),
        Err(_) => p.display().to_string(),
    };
    let rename = !cross && from.parent() == m.to.parent();
    ui.horizontal(|ui| {
        ui.checkbox(&mut item.picked.whole, "");
        let icon = if m.folder { "📁" } else { "📄" };
        let what = if rename { t!("review.rename") } else { t!("review.move") };
        ui.label(RichText::new(format!("{icon} {what}")).strong());
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
        ui.label(RichText::new("→").strong());
        let field =
            ui.add(egui::TextEdit::singleline(typed).desired_width(320.0).id_salt(("move_to_text", item.proposal.id)));
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
            egui::ComboBox::from_id_salt(("move_to", item.proposal.id)).selected_text(folder_label(&chosen)).show_ui(
                ui,
                |ui| {
                    for option in std::iter::once(&m.to).chain(&m.alternatives) {
                        if ui.selectable_label(*option == chosen, folder_label(option)).clicked() {
                            item.picked.to = (*option != m.to).then(|| option.clone());
                            item.draft.path = Some(shown(option));
                            item.draft.path_error = None;
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
        ui.checkbox(&mut item.picked.whole, "");
        let icon = if d.folder { "📁" } else { "📄" };
        ui.label(RichText::new(format!("🗑 {icon} {}", t!("review.delete"))).strong());
    });
    ui.label(RichText::new(rel).strikethrough());
    ui.weak(t!("review.delete_hint"));
}

/// A list as suggested: its name and kind to change, each item to leave out, the copies to set.
fn list_row(ui: &mut Ui, item: &mut ReviewItem) {
    let Ok(list) = serde_json::from_value::<ListPayload>(item.proposal.payload.clone()) else { return };
    let id = item.proposal.id;
    let kind = item.picked.list_kind.get_or_insert_with(|| list.kind.clone());
    let name = item.picked.list_name.get_or_insert_with(|| list.name.clone());
    ui.horizontal(|ui| {
        ui.checkbox(&mut item.picked.whole, "");
        if list.query.is_none() {
            let mut print = kind == "print";
            let options = [(true, t!("review.print_list").into_owned()), (false, t!("review.collection").into_owned())];
            if super::settings_kit::segmented(ui, &mut print, &options) {
                *kind = if print { "print".into() } else { "collection".into() };
            }
        } else {
            ui.label(RichText::new(t!("review.collection")).strong());
        }
        ui.add(egui::TextEdit::singleline(name).desired_width(220.0).id_salt(("list_name", id)))
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
            if ui.checkbox(&mut on, "").on_hover_text(t!("review.toggle")).changed() {
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
        ui.checkbox(&mut item.picked.whole, "");
        ui.label(RichText::new(&bundle.title).strong());
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
