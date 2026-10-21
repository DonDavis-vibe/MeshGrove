//! The tile grid. Only rows on screen are laid out (`show_rows`), and the tiles on screen are
//! reported to the thumbnail service so they are generated first. Folder tiles can show a
//! mosaic of the models inside them.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use egui::text::{LayoutJob, TextWrapping};
use egui::{Align, Color32, FontId, Key, Rect, ScrollArea, Sense, TextureOptions, Ui, pos2, vec2};
use stlb_core::Kind;
use stlb_core::cache::{ThumbCache, ThumbMeta};
use stlb_core::config::SortKey;
use stlb_core::listing::{self, Entry};

use super::icons;
use super::turntable::HoveredTile;
use crate::thumbs::{self, Job, ThumbResult, ThumbService};

const SPACING: f32 = 8.0;
const PAD: f32 = 6.0;
const LABEL_FONT: f32 = 12.5;

const TEXTURE_OPTIONS: TextureOptions = TextureOptions {
    magnification: egui::TextureFilter::Linear,
    minification: egui::TextureFilter::Linear,
    wrap_mode: egui::TextureWrapMode::ClampToEdge,
    mipmap_mode: Some(egui::TextureFilter::Linear),
};

pub enum Thumb {
    Pending,
    Ready(egui::TextureHandle, ThumbMeta),
    Failed(String),
}

pub enum Action {
    /// Double-click / Enter on an entry (index into the shown entries).
    Open(usize),
    /// Go to the folder that contains a search or "include subfolders" result.
    ShowInFolder(usize),
}

pub struct GalleryOutput {
    pub action: Option<Action>,
    /// Model tile under the pointer, for the hover turntable.
    pub hovered: Option<HoveredTile>,
    /// The visible part of the grid (for clipping overlays).
    pub viewport: Rect,
}

/// How the shown entries are filtered and ordered.
#[derive(Clone, PartialEq)]
pub struct Arrange {
    pub filter: String,
    pub only_models: bool,
    pub sort: SortKey,
    pub descending: bool,
}

#[derive(Default)]
pub struct Gallery {
    keys: Vec<Option<String>>,
    /// Folder entry index → thumbnail keys of the files previewed in its mosaic.
    mosaics: HashMap<usize, Vec<String>>,
    thumbs: HashMap<String, Thumb>,
    shown: Vec<usize>,
    pub selected: Option<usize>,
    visible: HashSet<String>,
    scroll_to_selected: bool,
    last_offset: f32,
    last_viewport_h: f32,
    /// Show the containing folder under each name (search results, flattened views).
    pub show_folders: bool,
    /// A tile was being dragged last frame: the wheel then zooms its preview, not the grid.
    tile_dragged: bool,
}

impl Gallery {
    /// New contents. Thumbnails already loaded for files that are still shown are kept,
    /// so refreshing a folder does not flicker; everything else is queued.
    pub fn set_entries(&mut self, entries: &[Entry], mosaics: HashMap<usize, Vec<Entry>>, service: &ThumbService) {
        let key = |e: &Entry| ThumbCache::key(&e.path, e.size, e.modified, None);
        self.keys = entries.iter().map(|e| thumbs::has_thumbnail(e.kind).then(|| key(e))).collect();
        let mut jobs: Vec<Job> = entries
            .iter()
            .zip(&self.keys)
            .filter_map(|(e, k)| Some(Job { key: k.clone()?, path: e.path.clone(), kind: e.kind, background: false }))
            .collect();
        self.mosaics.clear();
        for (index, files) in mosaics {
            let keys = files.iter().map(key).collect();
            jobs.extend(files.into_iter().map(|f| Job { key: key(&f), path: f.path, kind: f.kind, background: false }));
            self.mosaics.insert(index, keys);
        }
        let mut old = std::mem::take(&mut self.thumbs);
        for job in &jobs {
            let thumb = old.remove(&job.key).unwrap_or(Thumb::Pending);
            self.thumbs.insert(job.key.clone(), thumb);
        }
        // Only what is not on screen yet needs the pipeline (cache hits are cheap, but not free).
        jobs.retain(|j| matches!(self.thumbs.get(&j.key), Some(Thumb::Pending)));
        service.set_jobs(jobs);
        self.visible.clear();
    }

    /// Forgets the selection and scroll position (a different folder or result set).
    pub fn reset_view(&mut self) {
        self.selected = None;
        self.last_offset = 0.0;
        self.scroll_to_selected = false;
    }

    pub fn apply_filter(&mut self, entries: &[Entry], arrange: &Arrange) {
        self.shown = arranged(entries, arrange);
        if self.selected.is_some_and(|s| !self.shown.contains(&s)) {
            self.selected = None;
        }
    }

    pub fn shown_count(&self) -> usize {
        self.shown.len()
    }

    pub fn select(&mut self, entry: Option<usize>) {
        self.selected = entry;
        self.scroll_to_selected = entry.is_some();
    }

    pub fn thumb(&self, entry: usize) -> Option<&Thumb> {
        self.keys.get(entry)?.as_ref().and_then(|k| self.thumbs.get(k))
    }

    /// Takes one finished thumbnail from the service (foreground results carry a picture).
    pub fn accept(&mut self, ctx: &egui::Context, result: &ThumbResult) {
        match result {
            ThumbResult::Ready { job, image: Some(image), meta } => {
                if let Some(slot) = self.thumbs.get_mut(&job.key) {
                    *slot =
                        Thumb::Ready(ctx.load_texture(job.key.as_str(), image.clone(), TEXTURE_OPTIONS), meta.clone());
                }
            }
            ThumbResult::Ready { image: None, .. } => {}
            ThumbResult::Failed { job, error } => {
                if let Some(slot) = self.thumbs.get_mut(&job.key) {
                    *slot = Thumb::Failed(error.clone());
                }
            }
        }
    }

    pub fn show(&mut self, ui: &mut Ui, entries: &[Entry], tile: f32, service: &ThumbService) -> GalleryOutput {
        let rows_of_text = if self.show_folders { 3.0 } else { 2.0 };
        let label_h = rows_of_text * ui.ctx().fonts_mut(|f| f.row_height(&FontId::proportional(LABEL_FONT))) + 4.0;
        let avail = ui.available_width();
        let cols = (((avail + SPACING) / (tile + SPACING)).floor() as usize).max(1);
        // Stretch tiles a little so every row ends flush with the right edge.
        let tile_w = ((avail + SPACING) / cols as f32 - SPACING).max(tile.min(avail));
        let tile_h = tile_w + label_h + PAD;
        let rows = self.shown.len().div_ceil(cols);

        let mut action = self.keyboard(ui, cols);

        // (Mouse drags do not scroll by default, so they stay free for turning previews.)
        let wheel_scrolls = !self.tile_dragged;
        let mut area = ScrollArea::vertical()
            .auto_shrink(false)
            .scroll_source(egui::scroll_area::ScrollSource { mouse_wheel: wheel_scrolls, ..Default::default() });
        if std::mem::take(&mut self.scroll_to_selected)
            && let Some(pos) = self.selected.and_then(|s| self.shown.iter().position(|&i| i == s))
        {
            let row_top = (pos / cols) as f32 * (tile_h + SPACING);
            let row_bottom = row_top + tile_h;
            if row_top < self.last_offset {
                area = area.vertical_scroll_offset(row_top);
            } else if row_bottom > self.last_offset + self.last_viewport_h {
                area = area.vertical_scroll_offset(row_bottom - self.last_viewport_h);
            }
        }

        ui.spacing_mut().item_spacing = vec2(SPACING, SPACING);
        let mut visible = HashSet::new();
        let mut hovered = None;
        let mut tile_dragged = false;
        let output = area.show_rows(ui, tile_h, rows, |ui, row_range| {
            for row in row_range {
                let row_entries: Vec<usize> = self.shown.iter().skip(row * cols).take(cols).copied().collect();
                ui.horizontal(|ui| {
                    for entry in row_entries {
                        visible.extend(self.keys.get(entry).cloned().flatten());
                        visible.extend(self.mosaics.get(&entry).into_iter().flatten().cloned());
                        let response = self.tile(ui, &entries[entry], entry, tile_w, tile_h);
                        if response.clicked() {
                            self.selected = Some(entry);
                        }
                        if response.double_clicked() {
                            action = Some(Action::Open(entry));
                        }
                        let show_folders = self.show_folders;
                        response.context_menu(|ui| {
                            if ui.button(t!("menu.open")).clicked() {
                                action = Some(Action::Open(entry));
                                ui.close();
                            }
                            if show_folders && ui.button(t!("menu.show_in_folder")).clicked() {
                                action = Some(Action::ShowInFolder(entry));
                                ui.close();
                            }
                        });
                        let e = &entries[entry];
                        // Held button counts too: press-and-scroll without moving should zoom as well.
                        let held = response.dragged() || response.is_pointer_button_down_on();
                        tile_dragged |= held;
                        if (response.hovered() || held) && crate::meshes::viewable(e.kind) {
                            let selected = self.selected == Some(entry);
                            let drag = if response.dragged_by(egui::PointerButton::Primary) {
                                response.drag_delta()
                            } else {
                                egui::Vec2::ZERO
                            };
                            hovered = Some(HoveredTile {
                                path: e.path.clone(),
                                kind: e.kind,
                                image_rect: image_rect(response.rect),
                                background: tile_background(ui.visuals(), selected, true),
                                drag,
                                dragging: held,
                            });
                        }
                    }
                });
            }
        });
        self.tile_dragged = tile_dragged;
        self.last_offset = output.state.clamped_offset().y;
        self.last_viewport_h = output.inner_rect.height();

        if visible != self.visible {
            service.set_visible(|k| visible.contains(k));
            self.visible = visible;
        }
        GalleryOutput { action, hovered, viewport: output.inner_rect }
    }

    fn keyboard(&mut self, ui: &Ui, cols: usize) -> Option<Action> {
        if ui.ctx().egui_wants_keyboard_input() || self.shown.is_empty() {
            return None;
        }
        let pos = self.selected.and_then(|s| self.shown.iter().position(|&i| i == s));
        let last = self.shown.len() - 1;
        let target = ui.input(|i| {
            let p = pos?;
            if i.key_pressed(Key::ArrowRight) {
                Some((p + 1).min(last))
            } else if i.key_pressed(Key::ArrowLeft) {
                Some(p.saturating_sub(1))
            } else if i.key_pressed(Key::ArrowDown) {
                Some((p + cols).min(last))
            } else if i.key_pressed(Key::ArrowUp) {
                Some(p.saturating_sub(cols))
            } else if i.key_pressed(Key::Home) {
                Some(0)
            } else if i.key_pressed(Key::End) {
                Some(last)
            } else {
                None
            }
        });
        let any_arrow = ui.input(|i| {
            [Key::ArrowRight, Key::ArrowLeft, Key::ArrowDown, Key::ArrowUp, Key::Home, Key::End]
                .iter()
                .any(|k| i.key_pressed(*k))
        });
        if let Some(t) = target.or_else(|| (any_arrow && pos.is_none()).then_some(0)) {
            self.selected = Some(self.shown[t]);
            self.scroll_to_selected = true;
        }
        if ui.input(|i| i.key_pressed(Key::Enter)) {
            return self.selected.map(Action::Open);
        }
        None
    }

    fn tile(&self, ui: &mut Ui, entry: &Entry, index: usize, w: f32, h: f32) -> egui::Response {
        // Dragging on a model tile turns the hover preview by hand.
        let (rect, response) = ui.allocate_exact_size(vec2(w, h), Sense::click_and_drag());
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let visuals = ui.visuals();
        let selected = self.selected == Some(index);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 6.0, tile_background(visuals, selected, response.hovered()));

        let img = image_rect(rect);
        let ink = visuals.text_color();
        let paper = visuals.widgets.inactive.bg_fill;
        match (entry.kind, self.thumb(index)) {
            (Kind::Folder, _) => {
                let previews: Vec<(egui::TextureId, egui::Vec2)> = self
                    .mosaics
                    .get(&index)
                    .into_iter()
                    .flatten()
                    .filter_map(|k| match self.thumbs.get(k) {
                        Some(Thumb::Ready(t, _)) => Some((t.id(), t.size_vec2())),
                        _ => None,
                    })
                    .collect();
                if previews.is_empty() {
                    icons::folder(&painter, img);
                } else {
                    icons::folder_mosaic(&painter, img, &previews, visuals.extreme_bg_color);
                }
            }
            (_, Some(Thumb::Ready(texture, _))) => {
                let fit = fit_inside(texture.size_vec2(), img);
                painter.image(texture.id(), fit, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            (_, Some(Thumb::Pending)) => {
                icons::file(&painter, img, "", paper.linear_multiply(0.5), ink.linear_multiply(0.3));
                egui::Spinner::new()
                    .size((w * 0.16).clamp(12.0, 28.0))
                    .paint_at(ui, Rect::from_center_size(img.center(), vec2(32.0, 32.0)));
            }
            (_, Some(Thumb::Failed(_))) => {
                icons::file(&painter, img, "⚠", paper, visuals.warn_fg_color);
            }
            (_, None) => icons::file(&painter, img, &icons::extension_label(&entry.name), paper, ink),
        }
        if entry.kind.is_model() {
            let label = icons::extension_label(&entry.name);
            icons::badge(&painter, img, &label, Color32::from_black_alpha(150), Color32::from_gray(230));
        }

        let wrap = |rows| TextWrapping {
            max_width: w - 4.0,
            max_rows: rows,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        let mut job = LayoutJob::simple(entry.name.clone(), FontId::proportional(LABEL_FONT), ink, w - 4.0);
        job.wrap = wrap(2);
        job.halign = Align::Center;
        let galley = painter.layout_job(job);
        let name_bottom = img.bottom() + 4.0 + galley.size().y;
        painter.galley(pos2(rect.center().x, img.bottom() + 4.0), galley, ink);
        let folder = entry.path.parent().map(listing::file_name).unwrap_or_default();
        if self.show_folders {
            let weak = visuals.weak_text_color();
            let mut job =
                LayoutJob::simple(format!("📁 {folder}"), FontId::proportional(LABEL_FONT - 1.5), weak, w - 4.0);
            job.wrap = wrap(1);
            job.halign = Align::Center;
            painter.galley(pos2(rect.center().x, name_bottom + 1.0), painter.layout_job(job), weak);
        }

        let mut tooltip = entry.name.clone();
        if let Some(parent) = entry.path.parent() {
            tooltip += &format!("\n{}", parent.display());
        }
        if let Some(Thumb::Failed(error)) = self.thumb(index) {
            tooltip += &format!("\n⚠ {error}");
        }
        response.on_hover_text_at_pointer(tooltip)
    }
}

fn tile_background(visuals: &egui::Visuals, selected: bool, hovered: bool) -> Color32 {
    if selected {
        visuals.selection.bg_fill.linear_multiply(0.55)
    } else if hovered {
        visuals.widgets.hovered.weak_bg_fill
    } else {
        Color32::TRANSPARENT
    }
}

/// The square thumbnail area at the top of a tile.
fn image_rect(tile: Rect) -> Rect {
    let side = tile.width() - 2.0 * PAD;
    Rect::from_min_size(tile.min + vec2(PAD, PAD), vec2(side, side))
}

/// Indices of the entries to show, in display order: matching the filter text (case-insensitive)
/// and the model-only switch, folders first, then sorted. Folders stay visible with "only models"
/// so you can still navigate.
fn arranged(entries: &[Entry], a: &Arrange) -> Vec<usize> {
    let needle = a.filter.trim().to_lowercase();
    let mut shown: Vec<usize> = (0..entries.len())
        .filter(|&i| {
            let e = &entries[i];
            (!a.only_models || e.is_dir() || e.kind.is_model())
                && (needle.is_empty() || e.name.to_lowercase().contains(&needle))
        })
        .collect();
    shown.sort_by(|&x, &y| {
        let (ex, ey) = (&entries[x], &entries[y]);
        let by_name = || listing::natural_cmp(&ex.name, &ey.name);
        let order = match a.sort {
            SortKey::Name => by_name(),
            SortKey::Modified => ex.modified.cmp(&ey.modified).then_with(by_name),
            SortKey::Size => ex.size.cmp(&ey.size).then_with(by_name),
            SortKey::Type => icons::extension_label(&ex.name).cmp(&icons::extension_label(&ey.name)).then_with(by_name),
        };
        let order = if a.descending { order.reverse() } else { order };
        // Folders stay on top whatever the direction.
        ey.is_dir().cmp(&ex.is_dir()).then(order).then(Ordering::Equal)
    });
    shown
}

/// Largest rect with the image's aspect ratio that fits centred in `area`.
fn fit_inside(size: egui::Vec2, area: Rect) -> Rect {
    let scale = (area.width() / size.x).min(area.height() / size.y);
    Rect::from_center_size(area.center(), size * scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn entry(name: &str, kind: Kind, size: u64, age: u64) -> Entry {
        Entry {
            name: name.into(),
            path: name.into(),
            kind,
            size,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1000 - age)),
        }
    }

    fn arrange(filter: &str, only_models: bool, sort: SortKey, descending: bool) -> Arrange {
        Arrange { filter: filter.into(), only_models, sort, descending }
    }

    fn entries() -> [Entry; 5] {
        [
            entry("dragon_photo.jpg", Kind::Image, 50, 1),
            entry("Dragon.stl", Kind::Stl, 900, 30),
            entry("bases", Kind::Folder, 0, 99),
            entry("plate.3mf", Kind::ThreeMf, 10, 5),
            entry("arms", Kind::Folder, 0, 1),
        ]
    }

    #[test]
    fn filter_by_text_and_model_switch() {
        let e = entries();
        assert_eq!(arranged(&e, &arrange("", false, SortKey::Name, false)), [4, 2, 1, 0, 3]);
        assert_eq!(arranged(&e, &arrange("DRAGON", false, SortKey::Name, false)), [1, 0]);
        assert_eq!(arranged(&e, &arrange("", true, SortKey::Name, false)), [4, 2, 1, 3]);
        assert_eq!(arranged(&e, &arrange(" dragon ", true, SortKey::Name, false)), [1]);
    }

    #[test]
    fn sorting_keeps_folders_first() {
        let e = entries();
        assert_eq!(arranged(&e, &arrange("", false, SortKey::Size, false)), [4, 2, 3, 0, 1]);
        assert_eq!(arranged(&e, &arrange("", false, SortKey::Size, true)), [2, 4, 1, 0, 3]);
        assert_eq!(arranged(&e, &arrange("", false, SortKey::Modified, true)), [4, 2, 0, 3, 1], "newest first");
        assert_eq!(arranged(&e, &arrange("", false, SortKey::Type, false)), [4, 2, 3, 0, 1]);
    }

    #[test]
    fn fit_keeps_aspect_ratio() {
        let r = fit_inside(egui::vec2(400.0, 200.0), Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(100.0, 100.0)));
        assert_eq!(r, Rect::from_min_max(pos2(0.0, 25.0), pos2(100.0, 75.0)));
    }
}
