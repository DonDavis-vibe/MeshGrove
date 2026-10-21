//! The tile grid. Only rows on screen are laid out (`show_rows`), and the tiles on screen are
//! reported to the thumbnail service so they are generated first.

use std::collections::{HashMap, HashSet};

use egui::text::{LayoutJob, TextWrapping};
use egui::{Align, Color32, FontId, Key, Rect, ScrollArea, Sense, TextureOptions, Ui, pos2, vec2};
use stlb_core::Kind;
use stlb_core::cache::{ThumbCache, ThumbMeta};
use stlb_core::listing::Entry;

use super::icons;
use super::turntable::HoveredTile;
use crate::thumbs::{self, Job, ThumbResult, ThumbService};

const SPACING: f32 = 8.0;
const PAD: f32 = 6.0;
const LABEL_FONT: f32 = 12.5;
/// Texture uploads per frame; more would stutter when a whole folder comes out of the cache at once.
const UPLOADS_PER_FRAME: usize = 24;

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
    /// Double-click / Enter on an entry (index into the folder's entries).
    Open(usize),
}

pub struct GalleryOutput {
    pub action: Option<Action>,
    /// Model tile under the pointer, for the hover turntable.
    pub hovered: Option<HoveredTile>,
    /// The visible part of the grid (for clipping overlays).
    pub viewport: Rect,
}

#[derive(Default)]
pub struct Gallery {
    keys: Vec<Option<String>>,
    thumbs: HashMap<String, Thumb>,
    shown: Vec<usize>,
    pub selected: Option<usize>,
    visible: HashSet<String>,
    scroll_to_selected: bool,
    last_offset: f32,
    last_viewport_h: f32,
}

impl Gallery {
    /// New folder contents: forget old thumbnails and queue the new ones.
    pub fn set_entries(&mut self, entries: &[Entry], service: &ThumbService) {
        self.keys = entries
            .iter()
            .map(|e| thumbs::has_thumbnail(e.kind).then(|| ThumbCache::key(&e.path, e.size, e.modified, None)))
            .collect();
        self.thumbs = self.keys.iter().flatten().map(|k| (k.clone(), Thumb::Pending)).collect();
        let jobs = entries
            .iter()
            .zip(&self.keys)
            .filter_map(|(e, k)| Some(Job { key: k.clone()?, path: e.path.clone(), kind: e.kind }))
            .collect();
        service.set_jobs(jobs);
        self.selected = None;
        self.visible.clear();
        self.last_offset = 0.0;
    }

    pub fn apply_filter(&mut self, entries: &[Entry], filter: &str, only_models: bool) {
        self.shown = filtered(entries, filter, only_models);
        if self.selected.is_some_and(|s| !self.shown.contains(&s)) {
            self.selected = None;
        }
    }

    pub fn shown_count(&self) -> usize {
        self.shown.len()
    }

    pub fn thumb(&self, entry: usize) -> Option<&Thumb> {
        self.keys.get(entry)?.as_ref().and_then(|k| self.thumbs.get(k))
    }

    /// Moves finished thumbnails from the service into textures.
    pub fn receive(&mut self, ctx: &egui::Context, service: &ThumbService) {
        for _ in 0..UPLOADS_PER_FRAME {
            let Some(result) = service.try_recv() else { return };
            self.accept(ctx, result);
        }
        ctx.request_repaint(); // budget used up: more results may be waiting
    }

    fn accept(&mut self, ctx: &egui::Context, result: ThumbResult) {
        match result {
            ThumbResult::Ready { key, image, meta } => {
                if let Some(slot) = self.thumbs.get_mut(&key) {
                    *slot = Thumb::Ready(ctx.load_texture(key.as_str(), image, TEXTURE_OPTIONS), meta);
                }
            }
            ThumbResult::Failed { key, error } => {
                if let Some(slot) = self.thumbs.get_mut(&key) {
                    *slot = Thumb::Failed(error);
                }
            }
        }
    }

    pub fn show(&mut self, ui: &mut Ui, entries: &[Entry], tile: f32, service: &ThumbService) -> GalleryOutput {
        let label_h = 2.0 * ui.ctx().fonts_mut(|f| f.row_height(&FontId::proportional(LABEL_FONT))) + 4.0;
        let avail = ui.available_width();
        let cols = (((avail + SPACING) / (tile + SPACING)).floor() as usize).max(1);
        // Stretch tiles a little so every row ends flush with the right edge.
        let tile_w = ((avail + SPACING) / cols as f32 - SPACING).max(tile.min(avail));
        let tile_h = tile_w + label_h + PAD;
        let rows = self.shown.len().div_ceil(cols);

        let mut action = self.keyboard(ui, cols);

        let mut area = ScrollArea::vertical().auto_shrink(false);
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
        let output = area.show_rows(ui, tile_h, rows, |ui, row_range| {
            for row in row_range {
                let row_entries: Vec<usize> = self.shown.iter().skip(row * cols).take(cols).copied().collect();
                ui.horizontal(|ui| {
                    for entry in row_entries {
                        if let Some(Some(key)) = self.keys.get(entry) {
                            visible.insert(key.clone());
                        }
                        let response = self.tile(ui, &entries[entry], entry, tile_w, tile_h);
                        if response.clicked() {
                            self.selected = Some(entry);
                        }
                        if response.double_clicked() {
                            action = Some(Action::Open(entry));
                        }
                        let e = &entries[entry];
                        if response.hovered() && crate::meshes::viewable(e.kind) {
                            let selected = self.selected == Some(entry);
                            hovered = Some(HoveredTile {
                                path: e.path.clone(),
                                kind: e.kind,
                                image_rect: image_rect(response.rect),
                                background: tile_background(ui.visuals(), selected, true),
                            });
                        }
                    }
                });
            }
        });
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
        let (rect, response) = ui.allocate_exact_size(vec2(w, h), Sense::click());
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
            (Kind::Folder, _) => icons::folder(&painter, img),
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

        let mut job = LayoutJob::simple(entry.name.clone(), FontId::proportional(LABEL_FONT), ink, w - 4.0);
        job.wrap =
            TextWrapping { max_width: w - 4.0, max_rows: 2, break_anywhere: true, overflow_character: Some('…') };
        job.halign = Align::Center;
        let galley = painter.layout_job(job);
        painter.galley(pos2(rect.center().x, img.bottom() + 4.0), galley, ink);

        let tooltip = match self.thumb(index) {
            Some(Thumb::Failed(error)) => format!("{}\n⚠ {error}", entry.name),
            _ => entry.name.clone(),
        };
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

/// Indices of the entries matching the filter text (case-insensitive) and the model-only switch.
/// Folders stay visible with "only models" so you can still navigate.
fn filtered(entries: &[Entry], filter: &str, only_models: bool) -> Vec<usize> {
    let needle = filter.trim().to_lowercase();
    (0..entries.len())
        .filter(|&i| {
            let e = &entries[i];
            (!only_models || e.is_dir() || e.kind.is_model())
                && (needle.is_empty() || e.name.to_lowercase().contains(&needle))
        })
        .collect()
}

/// Largest rect with the image's aspect ratio that fits centred in `area`.
fn fit_inside(size: egui::Vec2, area: Rect) -> Rect {
    let scale = (area.width() / size.x).min(area.height() / size.y);
    Rect::from_center_size(area.center(), size * scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, kind: Kind) -> Entry {
        Entry { name: name.into(), path: name.into(), kind, size: 1, modified: None }
    }

    #[test]
    fn filter_by_text_and_model_switch() {
        let entries = [
            entry("bases", Kind::Folder),
            entry("Dragon.stl", Kind::Stl),
            entry("dragon_photo.jpg", Kind::Image),
            entry("plate.3mf", Kind::ThreeMf),
        ];
        assert_eq!(filtered(&entries, "", false), [0, 1, 2, 3]);
        assert_eq!(filtered(&entries, "DRAGON", false), [1, 2]);
        assert_eq!(filtered(&entries, "", true), [0, 1, 3]);
        assert_eq!(filtered(&entries, " dragon ", true), [1]);
    }

    #[test]
    fn fit_keeps_aspect_ratio() {
        let r = fit_inside(egui::vec2(400.0, 200.0), Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(100.0, 100.0)));
        assert_eq!(r, Rect::from_min_max(pos2(0.0, 25.0), pos2(100.0, 75.0)));
    }
}
