//! "Duplikate" view: groups of exact copies and of models with the same geometry, with a
//! suggestion which copy to keep (or the user's choice) and a way to move the others to the
//! trash: group by group, or every group ticked as fine at once.

use std::ops::Range;

use egui::{Color32, RichText, Sense, Ui, vec2};
use meshgrove_core::listing::{self, Entry};

use super::gallery::{Gallery, Thumb};
use crate::i18n;

pub struct GroupInfo {
    /// The group's files in the view's entry list.
    pub range: Range<usize>,
    pub exact: bool,
    /// Entry index of the copy we suggest keeping.
    pub keep: usize,
    pub wasted: u64,
    /// Why the group's extra copies cannot be trashed (read-only drive, archive member).
    pub blocked: Option<String>,
    /// The user is happy with the group's choice: it is cleaned up with the others.
    pub picked: bool,
}

pub enum DupAction {
    Select(usize),
    Open(usize),
    /// Trash every file of the group except the one kept.
    TrashOthers(usize),
    /// Keep this entry of its group instead.
    Keep(usize),
    /// Tick or untick a group (its choice is fine).
    Pick(usize, bool),
    /// Tick (true) or untick every group that can be cleaned up.
    PickAll(bool),
    /// Trash the extra copies of every ticked group.
    TrashPicked,
}

/// The copy worth keeping: not inside an archive, then the oldest, then the shortest path.
pub fn suggest_keep(files: &[Entry]) -> usize {
    (0..files.len())
        .min_by_key(|&i| {
            let f = &files[i];
            (meshgrove_core::archive::split(&f.path).is_some(), f.modified, f.path.as_os_str().len())
        })
        .unwrap_or(0)
}

pub fn show(ui: &mut Ui, entries: &[Entry], groups: &[GroupInfo], gallery: &Gallery) -> Option<DupAction> {
    let mut action = None;
    if groups.is_empty() {
        ui.centered_and_justified(|ui| ui.weak(t!("dupes.none")));
        return None;
    }
    // The summary and the cleanup of all ticked groups stay on top while the groups scroll.
    let wasted: u64 = groups.iter().map(|g| g.wasted).sum();
    ui.label(RichText::new(t!("dupes.summary", groups = groups.len(), size = i18n::file_size(wasted))).strong());
    ui.weak(t!("dupes.explain"));
    ui.add_space(4.0);
    let open: Vec<&GroupInfo> = groups.iter().filter(|g| g.blocked.is_none()).collect();
    let picked: Vec<&GroupInfo> = open.iter().copied().filter(|g| g.picked).collect();
    ui.horizontal(|ui| {
        let mut all = !open.is_empty() && picked.len() == open.len();
        let some = !picked.is_empty() && !all;
        let check = egui::Checkbox::new(&mut all, t!("dupes.pick_all")).indeterminate(some);
        if ui.add_enabled(!open.is_empty(), check).changed() {
            action = Some(DupAction::PickAll(all));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let size = i18n::file_size(picked.iter().map(|g| g.wasted).sum());
            let label = match picked.len() {
                0 => t!("dupes.trash_picked_none"),
                n => t!("dupes.trash_picked", groups = n, size = size),
            };
            let button = ui
                .add_enabled(!picked.is_empty(), egui::Button::new(RichText::new(label).strong()))
                .on_disabled_hover_text(t!("dupes.trash_picked_hint"));
            if button.clicked() {
                action = Some(DupAction::TrashPicked);
            }
        });
    });
    ui.add_space(6.0);
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        for (gi, group) in groups.iter().enumerate() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let mut on = group.picked;
                    let check = egui::Checkbox::without_text(&mut on);
                    let response = ui
                        .add_enabled(group.blocked.is_none(), check)
                        .on_hover_text(t!("dupes.pick_group"))
                        .on_disabled_hover_text(group.blocked.clone().unwrap_or_default());
                    if response.changed() {
                        action = Some(DupAction::Pick(gi, on));
                    }
                    let title = if group.exact { t!("dupes.exact") } else { t!("dupes.geometry") };
                    ui.label(RichText::new(title).strong());
                    ui.weak(t!("dupes.group_info", count = group.range.len(), size = i18n::file_size(group.wasted)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let button = ui
                            .add_enabled(group.blocked.is_none(), egui::Button::new(t!("dupes.trash_others")))
                            .on_disabled_hover_text(group.blocked.clone().unwrap_or_default());
                        if button.clicked() {
                            action = Some(DupAction::TrashOthers(gi));
                        }
                        if let Some(reason) = &group.blocked {
                            ui.label("🔒").on_hover_text(reason);
                        }
                    });
                });
                ui.horizontal_wrapped(|ui| {
                    for i in group.range.clone() {
                        let keep = format!("★ {}", t!("dupes.keep"));
                        let badge = (i == group.keep).then_some((keep.as_str(), KEEP_GREEN));
                        let (response, click) = tile(
                            ui,
                            &entries[i],
                            gallery.thumb(i),
                            (badge, false),
                            gallery.selected == Some(i),
                            TILE_WIDTH,
                            true,
                        );
                        // The others offer themselves for keeping where the pointer is.
                        let mut on_badge = false;
                        if i != group.keep && response.hovered() {
                            let img = image_rect(response.rect);
                            let label = format!("☆ {}", t!("dupes.keep"));
                            let fill = ui.visuals().widgets.inactive.bg_fill;
                            let fg = ui.visuals().strong_text_color();
                            let rect = super::icons::corner_badge(ui.painter(), img, &label, fill, fg);
                            on_badge = response.hover_pos().is_some_and(|p| rect.expand(2.0).contains(p));
                            if on_badge {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                        }
                        match click {
                            Some(TileClick::Select) if on_badge => action = Some(DupAction::Keep(i)),
                            Some(TileClick::Select) => action = Some(DupAction::Select(i)),
                            Some(TileClick::Open) => action = Some(DupAction::Open(i)),
                            None => {}
                        }
                    }
                });
            });
            ui.add_space(6.0);
        }
    });
    action
}

const KEEP_GREEN: Color32 = Color32::from_rgb(90, 170, 90);

pub(super) enum TileClick {
    Select,
    Open,
}

/// Width of the tiles in the duplicates and inbox views.
pub(super) const TILE_WIDTH: f32 = 170.0;

/// A file of a group, `width` wide: picture, name, folder, size and date; `badge` (text,
/// colour) in the top corner, `warn` a printability warning at the bottom. The response is for
/// context menus.
/// Where a tile of `rect` draws its picture.
fn image_rect(rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(rect.width() - 12.0, rect.width() - 12.0))
}

pub(super) fn tile(
    ui: &mut Ui,
    entry: &Entry,
    thumb: Option<&Thumb>,
    (badge, warn): (Option<(&str, Color32)>, bool),
    selected: bool,
    width: f32,
    full_path: bool,
) -> (egui::Response, Option<TileClick>) {
    let (rect, response) = ui.allocate_exact_size(vec2(width, width + 64.0), Sense::click());
    let visuals = ui.visuals();
    let bg = if selected {
        visuals.selection.bg_fill.linear_multiply(0.55)
    } else if response.hovered() {
        visuals.widgets.hovered.weak_bg_fill
    } else {
        Color32::TRANSPARENT
    };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, bg);
    let img = image_rect(rect);
    if let Some(Thumb::Ready(texture, _)) = thumb {
        let size = texture.size_vec2();
        let scale = (img.width() / size.x).min(img.height() / size.y);
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(texture.id(), egui::Rect::from_center_size(img.center(), size * scale), uv, Color32::WHITE);
    }
    if let Some((label, color)) = badge {
        super::icons::corner_badge(&painter, img, label, color, Color32::BLACK);
    }
    if warn {
        super::icons::bottom_right_badge(
            &painter,
            img,
            "⚠",
            visuals.warn_fg_color.linear_multiply(0.9),
            Color32::BLACK,
        );
    }
    let text = visuals.text_color();
    let weak = visuals.weak_text_color();
    let mut y = img.bottom() + 4.0;
    for (line, color, size) in [
        (listing::file_name(&entry.path), text, 12.5),
        // Copies are told apart by their whole path; elsewhere the folder name does.
        match entry.path.parent() {
            Some(p) if full_path => (p.display().to_string(), weak, 11.0),
            Some(p) => (format!("📁 {}", listing::file_name(p)), weak, 11.0),
            None => (String::new(), weak, 11.0),
        },
        (
            format!("{} · {}", i18n::file_size(entry.size), entry.modified.map(i18n::datetime).unwrap_or_default()),
            weak,
            11.0,
        ),
    ] {
        let mut job = egui::text::LayoutJob::simple_singleline(line, egui::FontId::proportional(size), color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(width - 10.0);
        let galley = painter.layout_job(job);
        let h = galley.size().y;
        painter.galley(egui::pos2(rect.min.x + 5.0, y), galley, color);
        y += h + 1.0;
    }
    let response = response.on_hover_text_at_pointer(entry.path.display().to_string());
    let click = if response.double_clicked() {
        Some(TileClick::Open)
    } else if response.clicked() {
        Some(TileClick::Select)
    } else {
        None
    };
    (response, click)
}
