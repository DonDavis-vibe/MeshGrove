//! Hover turntable: resting the pointer on a model tile spins the real model in place of the
//! still thumbnail, around a fixed axis so it never zooms or wobbles. Dragging on the tile
//! turns it by hand instead, and the mouse wheel zooms while a button is held.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Color32, Rect, Ui};
use meshgrove_core::Kind;
use meshgrove_render::{Framing, Renderer, ViewParams};

use super::offscreen::{OffscreenView, pixel_size};
use crate::meshes::{MeshState, MeshStore};

/// Pointer must rest this long before loading, so sweeping across the grid loads nothing.
const HOVER_DELAY: Duration = Duration::from_millis(300);
const DEGREES_PER_SECOND: f32 = 50.0;

/// The model tile under the pointer (or being dragged), as reported by the gallery.
pub struct HoveredTile {
    pub path: PathBuf,
    pub kind: Kind,
    /// Thumbnail area of the tile, in screen points.
    pub image_rect: Rect,
    /// Tile background, painted over the still thumbnail.
    pub background: Color32,
    /// Mouse drag on the tile this frame: turns the model by hand.
    pub drag: egui::Vec2,
    /// A mouse button is held on the tile: the wheel zooms the preview instead of scrolling.
    pub dragging: bool,
}

struct Spin {
    path: PathBuf,
    since: Instant,
    azimuth: f32,
    elevation: f32,
    /// The user turned it by hand: stop spinning on our own.
    manual: bool,
    zoom: f32,
    last_frame: Instant,
}

#[derive(Default)]
pub struct Turntable {
    offscreen: OffscreenView,
    spin: Option<Spin>,
}

impl Turntable {
    /// Call after the gallery has been drawn; `clip` is the gallery's visible area.
    pub fn show(
        &mut self,
        ui: &Ui,
        hovered: Option<HoveredTile>,
        clip: Rect,
        store: &mut MeshStore,
        rs: &egui_wgpu::RenderState,
        renderer: &Renderer,
    ) {
        let wheel = ui.input(|i| i.smooth_scroll_delta.y);
        // Plain scrolling moves the grid, so the preview stops; with a button held it zooms instead.
        let Some(tile) = hovered.filter(|t| t.dragging || wheel == 0.0) else {
            self.spin = None;
            return;
        };
        let now = Instant::now();
        let start = ViewParams::default();
        let spin = match &mut self.spin {
            Some(spin) if spin.path == tile.path => spin,
            slot => slot.insert(Spin {
                path: tile.path.clone(),
                since: now,
                azimuth: start.azimuth_deg,
                elevation: start.elevation_deg,
                manual: false,
                zoom: 1.0,
                last_frame: now,
            }),
        };
        if tile.dragging && wheel != 0.0 {
            spin.manual = true;
            spin.zoom = (spin.zoom * (wheel * 0.003).exp()).clamp(0.5, 8.0);
        }
        if tile.drag != egui::Vec2::ZERO {
            spin.manual = true;
            spin.azimuth -= tile.drag.x * 0.6;
            spin.elevation = (spin.elevation + tile.drag.y * 0.6).clamp(-89.0, 89.0);
        }
        let dt = (now - spin.last_frame).as_secs_f32().min(0.1);
        spin.last_frame = now;
        // Dragging starts right away; merely resting the pointer waits a moment.
        let waited = spin.since.elapsed();
        if !spin.manual && waited < HOVER_DELAY {
            ui.ctx().request_repaint_after(HOVER_DELAY - waited);
            return;
        }

        let painter = ui.painter().with_clip_rect(clip);
        match store.request(&tile.path, tile.kind, None) {
            MeshState::Loading => {
                let spinner =
                    Rect::from_min_size(tile.image_rect.right_top() + egui::vec2(-22.0, 4.0), egui::vec2(18.0, 18.0));
                egui::Spinner::new().size(16.0).paint_at(ui, spinner);
            }
            MeshState::Failed(_) => {}
            MeshState::Ready(loaded) => {
                let mesh = loaded.mesh.clone();
                if !spin.manual {
                    spin.azimuth += dt * DEGREES_PER_SECOND;
                }
                let (w, h) = pixel_size(ui.ctx(), tile.image_rect);
                self.offscreen.ensure_size(rs, renderer, w, h);
                let view = ViewParams {
                    azimuth_deg: spin.azimuth,
                    elevation_deg: spin.elevation,
                    zoom: spin.zoom,
                    framing: Framing::Orbit,
                    ..ViewParams::default()
                };
                self.offscreen.render(rs, renderer, &mesh, &view);
                painter.rect_filled(tile.image_rect, 4.0, tile.background);
                self.offscreen.paint(&painter, tile.image_rect);
                ui.ctx().request_repaint();
            }
        }
    }
}
