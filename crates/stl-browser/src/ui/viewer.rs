//! Interactive 3D view of the selected model (orbit, pan, zoom), drawn by the shared renderer.

use std::sync::Arc;

use egui::{PointerButton, Rect, Sense, Ui};
use stlb_render::{Framing, GpuMesh, Renderer, ViewParams};

use super::offscreen::{OffscreenView, pixel_size};

pub struct ModelViewer {
    mesh: Option<Arc<GpuMesh>>,
    offscreen: OffscreenView,
    view: ViewParams,
    dirty: bool,
}

impl ModelViewer {
    pub fn new() -> ModelViewer {
        ModelViewer { mesh: None, offscreen: OffscreenView::default(), view: orbit_view(), dirty: true }
    }

    /// Shows `mesh`; the camera is reset only when it is a different model.
    pub fn set_mesh(&mut self, mesh: Option<Arc<GpuMesh>>) {
        let same = match (&self.mesh, &mesh) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.mesh = mesh;
            self.reset_view();
        }
    }

    pub fn reset_view(&mut self) {
        self.view = orbit_view();
        self.dirty = true;
    }

    pub fn ui(&mut self, ui: &mut Ui, rs: &egui_wgpu::RenderState, renderer: &Renderer) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        self.handle_input(ui, rect, &response);
        let Some(mesh) = &self.mesh else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                t!("viewer.no_model"),
                egui::FontId::proportional(16.0),
                ui.visuals().weak_text_color(),
            );
            return;
        };
        let (w, h) = pixel_size(ui.ctx(), rect);
        if w < 8 || h < 8 {
            return;
        }
        if self.offscreen.ensure_size(rs, renderer, w, h) {
            self.dirty = true;
        }
        // Only re-render when something changed; the texture keeps the last frame.
        if std::mem::take(&mut self.dirty) {
            self.offscreen.render(rs, renderer, mesh, &self.view);
        }
        self.offscreen.paint(ui.painter(), rect);
    }

    fn handle_input(&mut self, ui: &Ui, rect: Rect, response: &egui::Response) {
        if response.dragged_by(PointerButton::Primary) {
            let d = response.drag_delta();
            self.view.azimuth_deg -= d.x * 0.4;
            self.view.elevation_deg = (self.view.elevation_deg + d.y * 0.4).clamp(-89.0, 89.0);
            self.dirty = true;
        }
        if response.dragged_by(PointerButton::Secondary) || response.dragged_by(PointerButton::Middle) {
            let d = response.drag_delta();
            // Roughly one bounding radius per half view height at zoom 1.
            let scale = 2.2 / rect.height().max(1.0) / self.view.zoom;
            self.view.pan += glam::Vec2::new(-d.x, d.y) * scale;
            self.dirty = true;
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.view.zoom = (self.view.zoom * (scroll * 0.003).exp()).clamp(0.2, 50.0);
                self.dirty = true;
            }
        }
        if response.double_clicked() {
            self.reset_view();
        }
    }
}

/// Turning the model must not zoom or shift it: fixed pivot and distance.
fn orbit_view() -> ViewParams {
    ViewParams { framing: Framing::Orbit, ..ViewParams::default() }
}
