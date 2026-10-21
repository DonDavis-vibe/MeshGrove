//! Hover turntable: resting the pointer on a model tile spins the real model in place of the
//! still thumbnail. It starts at the thumbnail's camera angle, so the switch is seamless.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Color32, Rect, Ui};
use stlb_core::Kind;
use stlb_render::{Renderer, ViewParams};

use super::offscreen::{OffscreenView, pixel_size};
use crate::meshes::{MeshState, MeshStore};

/// Pointer must rest this long before loading, so sweeping across the grid loads nothing.
const HOVER_DELAY: Duration = Duration::from_millis(300);
const DEGREES_PER_SECOND: f32 = 50.0;

/// The model tile under the pointer, as reported by the gallery.
pub struct HoveredTile {
    pub path: PathBuf,
    pub kind: Kind,
    /// Thumbnail area of the tile, in screen points.
    pub image_rect: Rect,
    /// Tile background, painted over the still thumbnail.
    pub background: Color32,
}

#[derive(Default)]
pub struct Turntable {
    offscreen: OffscreenView,
    current: Option<(PathBuf, Instant)>,
    spinning_since: Option<Instant>,
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
        let scrolling = ui.input(|i| i.smooth_scroll_delta != egui::Vec2::ZERO || i.pointer.any_down());
        let Some(tile) = hovered.filter(|_| !scrolling) else {
            self.current = None;
            self.spinning_since = None;
            return;
        };
        let since = match &self.current {
            Some((path, since)) if *path == tile.path => *since,
            _ => {
                self.current = Some((tile.path.clone(), Instant::now()));
                self.spinning_since = None;
                Instant::now()
            }
        };
        let waited = since.elapsed();
        if waited < HOVER_DELAY {
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
                let started = *self.spinning_since.get_or_insert_with(Instant::now);
                let (w, h) = pixel_size(ui.ctx(), tile.image_rect);
                self.offscreen.ensure_size(rs, renderer, w, h);
                let view = ViewParams {
                    azimuth_deg: ViewParams::default().azimuth_deg
                        + started.elapsed().as_secs_f32() * DEGREES_PER_SECOND,
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
