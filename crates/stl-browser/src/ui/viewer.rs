//! Interactive 3D view. The model is drawn by the shared `stlb_render::Renderer` into an
//! offscreen texture (own MSAA + depth, independent of egui's pass) and shown as an egui image.

use egui::{Color32, PointerButton, Rect, Sense, Ui, pos2};
use stlb_render::{GpuMesh, RenderTarget, Renderer, ViewParams};

pub struct ModelViewer {
    mesh: Option<GpuMesh>,
    target: Option<(RenderTarget, egui::TextureId)>,
    view: ViewParams,
    dirty: bool,
}

impl ModelViewer {
    pub fn new() -> ModelViewer {
        ModelViewer {
            mesh: None,
            target: None,
            view: ViewParams::default(),
            dirty: true,
        }
    }

    pub fn set_mesh(&mut self, mesh: Option<GpuMesh>) {
        self.mesh = mesh;
        self.reset_view();
    }

    pub fn reset_view(&mut self) {
        self.view = ViewParams::default();
        self.dirty = true;
    }

    pub fn ui(&mut self, ui: &mut Ui, render_state: &egui_wgpu::RenderState, renderer: &Renderer) {
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
        let ppp = ui.ctx().pixels_per_point();
        let (w, h) = (
            (rect.width() * ppp).round() as u32,
            (rect.height() * ppp).round() as u32,
        );
        if w < 8 || h < 8 {
            return;
        }

        if self.target.as_ref().is_none_or(|(t, _)| t.width != w || t.height != h) {
            let target = renderer.create_target(&render_state.device, w, h, wgpu::TextureUsages::TEXTURE_BINDING);
            let mut egui_renderer = render_state.renderer.write();
            let id = match &self.target {
                Some((_, id)) => {
                    egui_renderer.update_egui_texture_from_wgpu_texture(
                        &render_state.device,
                        &target.color_view,
                        wgpu::FilterMode::Linear,
                        *id,
                    );
                    *id
                }
                None => egui_renderer.register_native_texture(
                    &render_state.device,
                    &target.color_view,
                    wgpu::FilterMode::Linear,
                ),
            };
            self.target = Some((target, id));
            self.dirty = true;
        }
        let (target, texture_id) = self.target.as_ref().expect("created above");

        // Only re-render when something changed; the texture keeps the last frame.
        if self.dirty {
            let mut encoder = render_state
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("stlb viewer"),
                });
            renderer.draw(&render_state.queue, &mut encoder, target, mesh, &self.view);
            render_state.queue.submit([encoder.finish()]);
            self.dirty = false;
        }
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        ui.painter().image(*texture_id, rect, uv, Color32::WHITE);
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
