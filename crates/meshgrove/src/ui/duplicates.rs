//! "Duplikate" view: groups of exact copies and of models with the same geometry, with a
//! suggestion which copy to keep and a way to move the others to the trash.

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
}

pub enum DupAction {
    Select(usize),
    Open(usize),
    /// Trash every file of the group except the suggested one.
    TrashOthers(usize),
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
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        let wasted: u64 = groups.iter().map(|g| g.wasted).sum();
        ui.label(RichText::new(t!("dupes.summary", groups = groups.len(), size = i18n::file_size(wasted))).strong());
        ui.weak(t!("dupes.explain"));
        ui.add_space(8.0);
        for (gi, group) in groups.iter().enumerate() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
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
                        if let (_, Some(a)) =
                            tile(ui, &entries[i], gallery.thumb(i), badge, false, gallery.selected == Some(i))
                        {
                            action = Some(match a {
                                TileClick::Select => DupAction::Select(i),
                                TileClick::Open => DupAction::Open(i),
                            });
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

/// A file of a group: picture, name, folder, size and date; `badge` (text, colour) in the top
/// corner, `warn` a printability warning at the bottom. The response is for context menus.
pub(super) fn tile(
    ui: &mut Ui,
    entry: &Entry,
    thumb: Option<&Thumb>,
    badge: Option<(&str, Color32)>,
    warn: bool,
    selected: bool,
) -> (egui::Response, Option<TileClick>) {
    let width = 170.0;
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
    let img = egui::Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(width - 12.0, width - 12.0));
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
        super::icons::bottom_right_badge(&painter, img, "⚠", visuals.warn_fg_color.linear_multiply(0.9), Color32::BLACK);
    }
    let text = visuals.text_color();
    let weak = visuals.weak_text_color();
    let mut y = img.bottom() + 4.0;
    for (line, color, size) in [
        (listing::file_name(&entry.path), text, 12.5),
        (entry.path.parent().map(|p| p.display().to_string()).unwrap_or_default(), weak, 11.0),
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
