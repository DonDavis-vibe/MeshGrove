//! A render target that egui can show as an image: the mesh is drawn with our own MSAA and
//! depth buffers (independent of egui's pass) into a texture registered with egui.

use meshgrove_render::{GpuMesh, RenderTarget, Renderer, ViewParams};

#[derive(Default)]
pub struct OffscreenView {
    target: Option<(RenderTarget, egui::TextureId)>,
}

impl OffscreenView {
    /// Makes sure the texture is `width`×`height` physical pixels; returns true if it was (re)created
    /// and therefore holds no picture yet.
    pub fn ensure_size(&mut self, rs: &egui_wgpu::RenderState, renderer: &Renderer, width: u32, height: u32) -> bool {
        if self.target.as_ref().is_some_and(|(t, _)| t.width == width && t.height == height) {
            return false;
        }
        let target = renderer.create_target(&rs.device, width, height, wgpu::TextureUsages::TEXTURE_BINDING);
        let mut egui_renderer = rs.renderer.write();
        let id = match &self.target {
            Some((_, id)) => {
                egui_renderer.update_egui_texture_from_wgpu_texture(
                    &rs.device,
                    &target.color_view,
                    wgpu::FilterMode::Linear,
                    *id,
                );
                *id
            }
            None => egui_renderer.register_native_texture(&rs.device, &target.color_view, wgpu::FilterMode::Linear),
        };
        self.target = Some((target, id));
        true
    }

    pub fn render(&self, rs: &egui_wgpu::RenderState, renderer: &Renderer, mesh: &GpuMesh, view: &ViewParams) {
        let Some((target, _)) = &self.target else { return };
        let mut encoder =
            rs.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("meshgrove offscreen") });
        renderer.draw(&rs.queue, &mut encoder, target, mesh, view);
        rs.queue.submit([encoder.finish()]);
    }

    pub fn paint(&self, painter: &egui::Painter, rect: egui::Rect) {
        if let Some((_, id)) = &self.target {
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(*id, rect, uv, egui::Color32::WHITE);
        }
    }
}

/// Physical pixel size of a rect in points.
pub fn pixel_size(ctx: &egui::Context, rect: egui::Rect) -> (u32, u32) {
    let ppp = ctx.pixels_per_point();
    ((rect.width() * ppp).round().max(1.0) as u32, (rect.height() * ppp).round().max(1.0) as u32)
}
