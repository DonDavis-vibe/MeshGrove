//! Interactive 3D view of the selected model (orbit, pan, zoom), drawn by the shared renderer,
//! with standard views, picture export and point-to-point measuring.

use std::path::PathBuf;
use std::sync::Arc;

use crossbeam_channel::{Receiver, bounded};
use egui::{Color32, PointerButton, Pos2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use glam::{Vec2, Vec3};
use meshgrove_core::Kind;
use meshgrove_render::{Framing, GpuMesh, Renderer, ViewParams, project_point, view_ray};

use super::offscreen::{OffscreenView, pixel_size};
use crate::i18n;

/// Longer side of an exported picture, in pixels.
const PICTURE_SIZE: f32 = 2048.0;

/// Where the shown mesh comes from (to load its triangles for measuring).
#[derive(Clone, PartialEq)]
pub struct MeshSource {
    pub path: PathBuf,
    pub kind: Kind,
    pub plate: Option<usize>,
}

/// Point-to-point measuring on the model's surface.
#[derive(Default)]
struct Measure {
    triangles: Option<Arc<Vec<[f32; 3]>>>,
    loading: Option<Receiver<Result<Vec<[f32; 3]>, String>>>,
    error: Option<String>,
    /// Picked points in mesh coordinates (millimetres): none, the first, or both.
    points: Vec<Vec3>,
}

pub struct ModelViewer {
    mesh: Option<Arc<GpuMesh>>,
    source: Option<MeshSource>,
    offscreen: OffscreenView,
    view: ViewParams,
    dirty: bool,
    measure: Option<Measure>,
    saving: Option<Receiver<Result<PathBuf, String>>>,
    /// Test hook: measuring points to place once the triangles are loaded, as fractions of the
    /// view (0..1 from the top left).
    scripted_picks: Vec<egui::Vec2>,
    /// Test hook: a standard view (`top`, `front`, …) to show once the model is there.
    scripted_view: Option<String>,
}

impl ModelViewer {
    pub fn new() -> ModelViewer {
        ModelViewer {
            mesh: None,
            source: None,
            offscreen: OffscreenView::default(),
            view: orbit_view(),
            dirty: true,
            measure: None,
            saving: None,
            scripted_picks: Vec::new(),
            scripted_view: None,
        }
    }

    /// Test hook: shows a standard view (`top`, `front`, …) as soon as the model is there.
    pub fn script_view(&mut self, name: &str) {
        self.scripted_view = Some(format!("viewer.view_{name}"));
    }

    /// Test hook: starts measuring and places these points (fractions of the view) as soon as
    /// the model's triangles are there.
    pub fn script_measure(&mut self, picks: Vec<egui::Vec2>) {
        self.measure = Some(Measure::default());
        self.scripted_picks = picks;
    }

    /// Shows `mesh`; the camera is reset only when it is a different model.
    pub fn set_mesh(&mut self, mesh: Option<(Arc<GpuMesh>, MeshSource)>) {
        let (mesh, source) = match mesh {
            Some((m, s)) => (Some(m), Some(s)),
            None => (None, None),
        };
        let same = match (&self.mesh, &mesh) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.mesh = mesh;
            self.source = source;
            self.reset_view();
            if self.measure.is_some() {
                self.measure = Some(Measure::default()); // other model: pick again
            }
        }
    }

    pub fn reset_view(&mut self) {
        self.view = orbit_view();
        self.dirty = true;
    }

    /// Draws the view and its tool buttons; returns a message for the status bar, if any.
    pub fn ui(&mut self, ui: &mut Ui, rs: &egui_wgpu::RenderState, renderer: &Renderer) -> Option<String> {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        self.handle_input(ui, rect, &response);
        let mut message = self.poll_saving();
        self.poll_triangles();
        let Some(mesh) = self.mesh.clone() else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                t!("viewer.no_model"),
                egui::FontId::proportional(16.0),
                ui.visuals().weak_text_color(),
            );
            return message;
        };
        if let Some(key) = self.scripted_view.take()
            && let Some(&(_, az, el)) = PRESETS.iter().find(|(k, ..)| *k == key)
        {
            self.view = ViewParams { azimuth_deg: az, elevation_deg: el, ..orbit_view() };
            self.dirty = true;
        }
        let (w, h) = pixel_size(ui.ctx(), rect);
        if w < 8 || h < 8 {
            return message;
        }
        if self.offscreen.ensure_size(rs, renderer, w, h) {
            self.dirty = true;
        }
        // Only re-render when something changed; the texture keeps the last frame.
        if std::mem::take(&mut self.dirty) {
            self.offscreen.render(rs, renderer, &mesh, &self.view);
        }
        self.offscreen.paint(ui.painter(), rect);

        if response.clicked() && self.measure.is_some() {
            self.pick(&mesh, rect, response.interact_pointer_pos());
        }
        if !self.scripted_picks.is_empty() && self.measure.as_ref().is_some_and(|m| m.triangles.is_some()) {
            for at in std::mem::take(&mut self.scripted_picks) {
                self.pick(&mesh, rect, Some(rect.min + at * rect.size()));
            }
        }
        self.paint_measure(ui, &mesh, rect);
        if let Some(m) = self.toolbar(ui, rect, rs, renderer, &mesh) {
            message = Some(m);
        }
        message
    }

    fn handle_input(&mut self, ui: &Ui, rect: Rect, response: &egui::Response) {
        if response.dragged_by(PointerButton::Primary) {
            let d = response.drag_delta();
            self.view.azimuth_deg -= d.x * 0.4;
            self.view.elevation_deg = (self.view.elevation_deg + d.y * 0.4).clamp(-89.5, 89.5);
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

    /// Small buttons over the top-left corner: standard views, measuring, picture export.
    fn toolbar(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        rs: &egui_wgpu::RenderState,
        renderer: &Renderer,
        mesh: &GpuMesh,
    ) -> Option<String> {
        let mut message = None;
        let bar = Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(rect.width() - 12.0, 24.0));
        let mut child =
            ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::left_to_right(egui::Align::Center)));
        let ui = &mut child;
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.menu_button(RichText::new(format!("🎥 {} ▾", t!("viewer.views"))).small(), |ui| {
            for &(key, az, el) in PRESETS {
                if ui.button(t!(key)).clicked() {
                    self.view = ViewParams { azimuth_deg: az, elevation_deg: el, ..orbit_view() };
                    self.dirty = true;
                    ui.close();
                }
            }
        });
        let measuring = self.measure.is_some();
        if ui
            .selectable_label(measuring, RichText::new(format!("📏 {}", t!("viewer.measure"))).small())
            .on_hover_text(t!("viewer.measure_hint"))
            .clicked()
        {
            self.measure = if measuring { None } else { Some(Measure::default()) };
        }
        let saving = self.saving.is_some();
        let picture = ui
            .add_enabled(!saving, egui::Button::new(RichText::new(format!("📷 {}", t!("viewer.picture"))).small()))
            .on_hover_text(t!("viewer.picture_hint"));
        if picture.clicked() {
            message = self.save_picture(rs, renderer, mesh, rect.aspect_ratio()).err();
        }
        message
    }

    /// Renders the current view large and writes it as PNG on a thread.
    fn save_picture(
        &mut self,
        rs: &egui_wgpu::RenderState,
        renderer: &Renderer,
        mesh: &GpuMesh,
        aspect: f32,
    ) -> Result<(), String> {
        let (w, h) =
            if aspect >= 1.0 { (PICTURE_SIZE, PICTURE_SIZE / aspect) } else { (PICTURE_SIZE * aspect, PICTURE_SIZE) };
        let image = renderer
            .render_image(&rs.device, &rs.queue, mesh, &self.view, w.round() as u32, h.round() as u32)
            .map_err(|e| t!("toast.picture_failed", error = e.to_string()).into_owned())?;
        let name = self
            .source
            .as_ref()
            .and_then(|s| s.path.file_stem())
            .map_or_else(|| "model".to_owned(), |s| s.to_string_lossy().into_owned());
        let dir = dirs::picture_dir().or_else(dirs::home_dir).unwrap_or_default().join("Meshgrove");
        let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
        let path = dir.join(format!("{name}_{stamp}.png"));
        let (tx, rx) = bounded(1);
        std::thread::spawn(move || {
            let result = std::fs::create_dir_all(&dir)
                .map_err(|e| e.to_string())
                .and_then(|()| image.save(&path).map_err(|e| e.to_string()))
                .map(|()| path);
            let _ = tx.send(result);
        });
        self.saving = Some(rx);
        Ok(())
    }

    fn poll_saving(&mut self) -> Option<String> {
        let result = self.saving.as_ref()?.try_recv().ok()?;
        self.saving = None;
        Some(match result {
            Ok(path) => t!("toast.picture_saved", path = path.display().to_string()).into_owned(),
            Err(e) => t!("toast.picture_failed", error = e).into_owned(),
        })
    }

    /// Starts loading the triangles when measuring needs them; collects them when ready.
    fn poll_triangles(&mut self) {
        let Some(measure) = &mut self.measure else { return };
        if let Some(rx) = &measure.loading {
            if let Ok(result) = rx.try_recv() {
                measure.loading = None;
                match result {
                    Ok(soup) => measure.triangles = Some(Arc::new(soup)),
                    Err(e) => measure.error = Some(e),
                }
            }
            return;
        }
        if measure.triangles.is_some() || measure.error.is_some() {
            return;
        }
        let Some(source) = self.source.clone() else { return };
        let (tx, rx) = bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send(crate::meshes::load_triangles(&source.path, source.kind, source.plate));
        });
        measure.loading = Some(rx);
    }

    /// Puts a measuring point where the click hits the model.
    fn pick(&mut self, mesh: &GpuMesh, rect: Rect, pointer: Option<Pos2>) {
        let (Some(measure), Some(pos)) = (&mut self.measure, pointer) else { return };
        let Some(soup) = &measure.triangles else { return };
        let ndc = Vec2::new(
            (pos.x - rect.center().x) / (rect.width() * 0.5),
            (rect.center().y - pos.y) / (rect.height() * 0.5),
        );
        let (origin, dir) = view_ray(&mesh.bounds, &self.view, rect.aspect_ratio(), ndc);
        let Some(t) = meshgrove_core::mesh::ray_hit(soup, origin, dir) else { return };
        if measure.points.len() >= 2 {
            measure.points.clear();
        }
        measure.points.push(origin + dir * t);
    }

    fn paint_measure(&self, ui: &Ui, mesh: &GpuMesh, rect: Rect) {
        let Some(measure) = &self.measure else { return };
        let painter = ui.painter_at(rect);
        let accent = Color32::from_rgb(255, 196, 0);
        let to_screen = |p: Vec3| {
            project_point(&mesh.bounds, &self.view, rect.aspect_ratio(), p)
                .map(|n| pos2(rect.center().x + n.x * rect.width() * 0.5, rect.center().y - n.y * rect.height() * 0.5))
        };
        let screen: Vec<Pos2> = measure.points.iter().filter_map(|&p| to_screen(p)).collect();
        if let [a, b] = screen[..] {
            painter.line_segment([a, b], Stroke::new(2.0, accent));
        }
        for p in &screen {
            painter.circle(*p, 4.5, accent, Stroke::new(1.5, Color32::BLACK));
        }
        let text = match (&measure.error, measure.loading.is_some(), &measure.points[..]) {
            (Some(e), _, _) => e.clone(),
            (None, true, _) => t!("viewer.measure_loading").into_owned(),
            (None, false, [a, b]) => {
                let d = *b - *a;
                format!(
                    "{} mm   (ΔX {} · ΔY {} · ΔZ {})",
                    i18n::decimal(d.length()),
                    i18n::decimal(d.x.abs()),
                    i18n::decimal(d.y.abs()),
                    i18n::decimal(d.z.abs())
                )
            }
            (None, false, [_]) => t!("viewer.measure_second").into_owned(),
            (None, false, _) => t!("viewer.measure_first").into_owned(),
        };
        let galley = painter.layout_no_wrap(text, egui::FontId::proportional(13.0), Color32::WHITE);
        let pos = pos2(rect.center().x - galley.size().x / 2.0, rect.bottom() - galley.size().y - 10.0);
        painter.rect_filled(Rect::from_min_size(pos, galley.size()).expand(5.0), 5.0, Color32::from_black_alpha(170));
        painter.galley(pos, galley, Color32::WHITE);
    }
}

/// Standard views: (label key, azimuth, elevation).
const PRESETS: &[(&str, f32, f32)] = &[
    ("viewer.view_iso", 30.0, 25.0),
    ("viewer.view_front", 0.0, 0.0),
    ("viewer.view_back", 180.0, 0.0),
    ("viewer.view_left", -90.0, 0.0),
    ("viewer.view_right", 90.0, 0.0),
    ("viewer.view_top", 0.0, 89.5),
    ("viewer.view_bottom", 0.0, -89.5),
];

/// Turning the model must not zoom or shift it: fixed pivot and distance.
fn orbit_view() -> ViewParams {
    ViewParams { framing: Framing::Orbit, ..ViewParams::default() }
}
