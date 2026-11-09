//! Interactive 3D view of the selected model (orbit, pan, zoom), drawn by the shared renderer,
//! with standard views, picture export and point-to-point measuring; for a plugin's mesh view
//! also painting on the model ([`PaintMode`], gestures in [`super::paint`]).

use std::path::PathBuf;
use std::sync::Arc;

use crossbeam_channel::{Receiver, bounded};
use egui::{Color32, PointerButton, Pos2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use glam::{Vec2, Vec3};
use meshgrove_core::bvh::Bvh;
use meshgrove_core::config::PreviewQuality;
use meshgrove_core::spacemouse::CameraMotion;
use meshgrove_core::{Aabb, Kind};
use meshgrove_render::{Framing, GpuMesh, Renderer, ViewParams, project_point, view_ray};

use super::offscreen::{OffscreenView, pixel_size};
use super::paint::{self, Action, Gestures, Outbox, Tool};
use crate::i18n;

/// Longer side of an exported picture, in pixels.
const PICTURE_SIZE: f32 = 2048.0;

/// Where the shown mesh comes from (to load its triangles for measuring).
#[derive(Clone, PartialEq)]
pub enum MeshSource {
    File {
        path: PathBuf,
        kind: Kind,
        plate: Option<usize>,
    },
    /// Triangles a plugin made, centred like the uploaded mesh; `name` names exported pictures.
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    Memory {
        name: String,
        soup: Arc<Vec<[f32; 3]>>,
    },
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

/// Painting on the model: the left button belongs to the tool.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaintMode {
    pub tool: Tool,
    /// Radius of the brush in mm.
    pub radius_mm: f32,
    /// What painted triangles are tinted with until the plugin's overlay arrives (sRGB; none:
    /// no tint).
    pub tint: Option<[u8; 4]>,
    pub erase_tint: Option<[u8; 4]>,
}

/// A point on the model, in the mesh's recentred coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceHit {
    /// The triangle's index in the soup (its order in the plugin's mesh).
    pub triangle: u32,
    pub point: Vec3,
    /// The triangle's normal, turned towards the camera.
    pub normal: Vec3,
}

/// Meshes with up to this many triangles are picked on without a tree while it is being built.
const PICK_WITHOUT_TREE: usize = 100_000;

/// The stroke under way.
struct OpenStroke {
    erase: bool,
    /// Where the last dab of the brush went.
    last_dab: Option<Vec3>,
    /// Where the pointer last gave a hit (drag tool).
    last_screen: Option<Pos2>,
    /// Triangles this stroke has passed (triangles tool).
    seen: std::collections::HashSet<u32>,
}

/// Painting: the mode, the gesture under way and what waits to go to the plugin.
struct Painting {
    mode: PaintMode,
    gestures: Gestures,
    stroke: Option<OpenStroke>,
    outbox: Outbox<SurfaceHit>,
    /// Under the pointer, for the brush circle.
    hover: Option<SurfaceHit>,
    /// Switched off with something still on its way: gone once that went out.
    leaving: bool,
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
    /// Render this many times the pixels the view shows (smooths edges).
    supersample: f32,
    studio: bool,
    /// The buttons over the view (standard views, measuring, picture).
    tools: bool,
    /// The standard view the 3D mouse's second button showed last.
    preset: usize,
    /// Free orbit: also turn about the view axis and over the poles.
    free_orbit: bool,
    /// A printer bed (x, y in mm) drawn under the model, centred.
    bed: Option<[f32; 2]>,
    /// The space the model is meant to fit, in the mesh's recentred coordinates: drawn as a
    /// coloured frame and kept in the picture.
    frame: Option<Aabb>,
    /// Whether the frame is shown; the toolbar's switch, kept while the plugin sends new meshes.
    frame_shown: bool,
    painting: Option<Painting>,
    /// The tree to pick on in the soup of a `MeshSource::Memory`.
    picking: Option<Arc<Bvh>>,
}

/// What the 3D mouse did since the last frame, for the viewer that has the focus.
pub struct SpaceInput {
    pub motion: CameraMotion,
    /// Seconds since the last frame.
    pub dt: f32,
    /// The buttons pressed since the last frame.
    pub presses: Vec<u8>,
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
            supersample: 1.0,
            studio: false,
            preset: 0,
            free_orbit: false,
            tools: true,
            bed: None,
            frame: None,
            frame_shown: true,
            painting: None,
            picking: None,
        }
    }

    /// Draws again (the mesh's colours changed on the GPU).
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    pub fn redraw(&mut self) {
        self.dirty = true;
    }

    /// Paints with `mode`, or (`None`) turns the model with the left button again. A stroke
    /// under way ends.
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    pub fn set_paint(&mut self, mode: Option<PaintMode>) {
        match (&mut self.painting, mode) {
            (Some(p), Some(mode)) => {
                p.mode = mode;
                p.leaving = false;
            }
            (painting @ None, Some(mode)) => {
                self.measure = None;
                *painting = Some(Painting {
                    mode,
                    gestures: Gestures::default(),
                    stroke: None,
                    outbox: Outbox::default(),
                    hover: None,
                    leaving: false,
                });
            }
            (painting, None) => {
                // What is still on its way goes out first.
                if let Some(p) = painting.as_mut() {
                    p.outbox.end();
                    if !p.outbox.is_empty() {
                        (p.gestures, p.stroke, p.hover, p.leaving) = (Gestures::default(), None, None, true);
                        return;
                    }
                }
                *painting = None;
            }
        }
    }

    /// The tree to pick on in the triangles of the shown `MeshSource::Memory` (built off the UI
    /// thread); without one small meshes are picked on triangle by triangle.
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    pub fn set_picking(&mut self, tree: Option<Arc<Bvh>>) {
        self.picking = tree;
    }

    /// Picks and strokes for the plugin, in recentred coordinates; nothing while it is `busy`.
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    pub fn take_paint(&mut self, busy: bool) -> Vec<paint::Out<SurfaceHit>> {
        let Some(p) = &mut self.painting else { return Vec::new() };
        let out = p.outbox.take(busy, std::time::Instant::now());
        if p.leaving && p.outbox.is_empty() {
            self.painting = None;
        }
        out
    }

    /// A printer bed (x, y in mm) under the model, centred on it; `None`: none.
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    pub fn set_bed(&mut self, bed: Option<[f32; 2]>) {
        self.bed = bed.filter(|b| b[0] > 0.0 && b[1] > 0.0);
    }

    /// The space the model is meant to fit (in the mesh's recentred coordinates), drawn as a
    /// coloured frame round it; `None`: none. The camera keeps model and frame in the picture.
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    pub fn set_frame(&mut self, frame: Option<Aabb>) {
        if self.frame != frame {
            self.frame = frame;
            self.dirty = true;
        }
    }

    /// The frame, unless the toolbar's switch hides it.
    fn shown_frame(&self) -> Option<Aabb> {
        self.frame.filter(|_| self.frame_shown)
    }

    /// What the camera keeps in the picture: the model, and the frame round it while it shows.
    fn fitted(&self, mesh: &GpuMesh) -> Aabb {
        match self.shown_frame() {
            Some(f) => Aabb { min: mesh.bounds.min.min(f.min), max: mesh.bounds.max.max(f.max) },
            None => mesh.bounds,
        }
    }

    /// Shows or hides the buttons over the view.
    pub fn set_tools(&mut self, shown: bool) {
        self.tools = shown;
    }

    /// Picture quality of this view; changing it redraws.
    pub fn set_quality(&mut self, quality: PreviewQuality) {
        let (supersample, studio) = match quality {
            PreviewQuality::Standard => (1.0, false),
            PreviewQuality::High => (2.0, false),
            PreviewQuality::Studio => (2.0, true),
        };
        if supersample != self.supersample || studio != self.studio {
            self.supersample = supersample;
            self.studio = studio;
            self.dirty = true;
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

    /// Shows `mesh` in place of the one shown, keeping the camera (a model that changes while it
    /// is being edited, as in a plugin's panel).
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    pub fn update_mesh(&mut self, mesh: Arc<GpuMesh>, source: MeshSource) {
        if self.mesh.is_none() {
            return self.set_mesh(Some((mesh, source)));
        }
        self.mesh = Some(mesh);
        self.source = Some(source);
        self.picking = None;
        self.dirty = true;
        if self.measure.is_some() {
            self.measure = Some(Measure::default());
        }
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
            self.picking = None;
            self.reset_view();
            if self.measure.is_some() {
                self.measure = Some(Measure::default()); // other model: pick again
            }
        }
    }

    /// Moves the camera with the 3D mouse: the first button resets the view, the second shows the
    /// next standard view.
    pub fn apply_space(&mut self, input: &SpaceInput) {
        if self.mesh.is_none() {
            return;
        }
        for button in &input.presses {
            match button {
                0 => self.reset_view(),
                1 => {
                    let &(_, azimuth_deg, elevation_deg) = &PRESETS[self.preset % PRESETS.len()];
                    self.preset += 1;
                    self.view = self.looking_from(azimuth_deg, elevation_deg);
                    self.dirty = true;
                }
                _ => {}
            }
        }
        if !input.motion.is_still() {
            self.view = driven(&self.view, &input.motion, input.dt);
            self.dirty = true;
        }
    }

    /// Free orbit (also turning about the view axis and over the poles) or the upright orbit.
    /// Changing it keeps the direction the camera looks from.
    pub fn set_free_orbit(&mut self, free: bool) {
        if self.free_orbit != free {
            self.free_orbit = free;
            self.view.set_free(free);
            self.dirty = true;
        }
    }

    /// The orbit view from a direction, free or upright as set.
    fn looking_from(&self, azimuth_deg: f32, elevation_deg: f32) -> ViewParams {
        let mut view = ViewParams { azimuth_deg, elevation_deg, ..orbit_view() };
        view.set_free(self.free_orbit);
        view
    }

    pub fn reset_view(&mut self) {
        let start = orbit_view();
        self.view = self.looking_from(start.azimuth_deg, start.elevation_deg);
        self.dirty = true;
    }

    /// Draws the view and its tool buttons; returns a message for the status bar, if any.
    pub fn ui(&mut self, ui: &mut Ui, rs: &egui_wgpu::RenderState, renderer: &Renderer) -> Option<String> {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        if self.painting.as_ref().is_some_and(|p| !p.leaving) {
            self.paint_input(ui, rect, &response, rs);
        } else {
            self.handle_input(ui, rect, &response);
        }
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
            self.view = self.looking_from(az, el);
            self.dirty = true;
        }
        let (w, h) = pixel_size(ui.ctx(), rect);
        if w < 8 || h < 8 {
            return message;
        }
        let max = rs.device.limits().max_texture_dimension_2d;
        let (w, h) = (((w as f32 * self.supersample) as u32).min(max), ((h as f32 * self.supersample) as u32).min(max));
        self.view.studio = self.studio;
        if self.offscreen.ensure_size(rs, renderer, w, h) {
            self.dirty = true;
        }
        // Only re-render when something changed; the texture keeps the last frame.
        if std::mem::take(&mut self.dirty) {
            self.offscreen.render_fitted(rs, renderer, &mesh, &self.fitted(&mesh), &self.view);
        }
        self.paint_bed(ui, &mesh, rect);
        self.paint_frame(ui, &mesh, rect, false);
        self.offscreen.paint(ui.painter(), rect);
        self.paint_frame(ui, &mesh, rect, true);

        if response.clicked() && self.measure.is_some() {
            self.pick(&mesh, rect, response.interact_pointer_pos());
        }
        if !self.scripted_picks.is_empty() && self.measure.as_ref().is_some_and(|m| m.triangles.is_some()) {
            for at in std::mem::take(&mut self.scripted_picks) {
                self.pick(&mesh, rect, Some(rect.min + at * rect.size()));
            }
        }
        self.paint_measure(ui, &mesh, rect);
        self.paint_brush(ui, &mesh, rect);
        if self.tools
            && let Some(m) = self.toolbar(ui, rect, rs, renderer, &mesh)
        {
            message = Some(m);
        }
        message
    }

    fn handle_input(&mut self, ui: &Ui, rect: Rect, response: &egui::Response) {
        if response.dragged_by(PointerButton::Primary) {
            let roll = self.free_orbit && ui.input(|i| i.modifiers.ctrl);
            self.orbit(rect, response.drag_delta(), response.interact_pointer_pos().filter(|_| roll));
        }
        if response.dragged_by(PointerButton::Secondary) || response.dragged_by(PointerButton::Middle) {
            self.pan(rect, response.drag_delta());
        }
        if response.hovered() {
            self.zoom(ui.input(|i| i.smooth_scroll_delta.y));
        }
        if response.double_clicked() {
            self.reset_view();
        }
    }

    /// Turns the model by a drag of `d` points; with `roll_at` (Ctrl in free orbit) the model
    /// follows the pointer there round the centre of the view.
    fn orbit(&mut self, rect: Rect, d: egui::Vec2, roll_at: Option<Pos2>) {
        if d == egui::Vec2::ZERO {
            return;
        }
        if let Some(pos) = roll_at {
            let (now, before) = (pos - rect.center(), pos - d - rect.center());
            let turned = (now.y.atan2(now.x) - before.y.atan2(before.x)).rem_euclid(std::f32::consts::TAU);
            let turned = if turned > std::f32::consts::PI { turned - std::f32::consts::TAU } else { turned };
            self.view.free_turn(0.0, 0.0, turned);
        } else if self.free_orbit {
            self.view.free_turn(-d.x * 0.4f32.to_radians(), -d.y * 0.4f32.to_radians(), 0.0);
        } else {
            self.view.azimuth_deg -= d.x * 0.4;
            self.view.elevation_deg = (self.view.elevation_deg + d.y * 0.4).clamp(-89.5, 89.5);
        }
        self.dirty = true;
    }

    fn pan(&mut self, rect: Rect, d: egui::Vec2) {
        if d == egui::Vec2::ZERO {
            return;
        }
        // Roughly one bounding radius per half view height at zoom 1.
        let scale = 2.2 / rect.height().max(1.0) / self.view.zoom;
        self.view.pan += glam::Vec2::new(-d.x, d.y) * scale;
        self.dirty = true;
    }

    fn zoom(&mut self, scroll: f32) {
        if scroll != 0.0 {
            self.view.zoom = (self.view.zoom * (scroll * 0.003).exp()).clamp(0.2, 50.0);
            self.dirty = true;
        }
    }

    /// The input while painting: the gesture each press starts ([`Gestures`]), and what it does.
    fn paint_input(&mut self, ui: &Ui, rect: Rect, response: &egui::Response, rs: &egui_wgpu::RenderState) {
        let buttons = [
            (PointerButton::Primary, paint::Button::Primary),
            (PointerButton::Secondary, paint::Button::Secondary),
            (PointerButton::Middle, paint::Button::Middle),
        ];
        let inside = response.hovered();
        let frame = ui.input(|i| paint::Frame {
            pos: i.pointer.latest_pos(),
            inside,
            shift: i.modifiers.shift,
            pressed: buttons.iter().filter(|(b, _)| i.pointer.button_pressed(*b)).map(|(_, p)| *p).collect(),
            released: buttons.iter().filter(|(b, _)| i.pointer.button_released(*b)).map(|(_, p)| *p).collect(),
            delta: i.pointer.delta(),
            scroll: i.smooth_scroll_delta.y,
            double_clicked: response.double_clicked(),
        });
        let Some(p) = &mut self.painting else { return };
        let tool = p.mode.tool;
        let actions = p.gestures.frame(&frame, tool);
        if inside {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        for action in actions {
            match action {
                Action::Orbit(d) => self.orbit(rect, d, None),
                Action::Pan(d) => self.pan(rect, d),
                Action::Zoom(scroll) => self.zoom(scroll),
                Action::StrokeStart { erase } => {
                    if let Some(p) = &mut self.painting {
                        p.outbox.start(erase);
                        let seen = Default::default();
                        p.stroke = Some(OpenStroke { erase, last_dab: None, last_screen: None, seen });
                    }
                }
                Action::Sample(at) => self.sample(rect, at, &rs.queue),
                Action::StrokeEnd => {
                    if let Some(p) = &mut self.painting {
                        p.outbox.end();
                        p.stroke = None;
                    }
                }
                Action::Pick { at, shift } => {
                    let hit = self.hit_at(rect, at);
                    if let Some(p) = &mut self.painting {
                        p.outbox.pick(hit, shift);
                    }
                }
            }
        }
        // Under the pointer, for the brush circle (also while turning the model).
        let hover = frame.pos.filter(|_| inside).and_then(|at| self.hit_at(rect, at));
        if let Some(p) = &mut self.painting {
            if p.hover != hover {
                ui.ctx().request_repaint();
            }
            p.hover = hover;
        }
    }

    /// Paints where the pointer is, by the tool: a dab of the brush where it has moved far
    /// enough, a triangle not passed yet, a point of the drag. Tints what it paints.
    fn sample(&mut self, rect: Rect, at: Pos2, queue: &wgpu::Queue) {
        let hit = self.hit_at(rect, at);
        let (Some(p), Some(hit)) = (&mut self.painting, hit) else { return };
        let Some(stroke) = &mut p.stroke else { return };
        let mode = p.mode;
        let take = match mode.tool {
            Tool::Brush => {
                let step = (mode.radius_mm * 0.25).max(0.05);
                let far = stroke.last_dab.is_none_or(|last| last.distance(hit.point) >= step);
                if far {
                    stroke.last_dab = Some(hit.point);
                }
                far
            }
            Tool::Triangles => stroke.seen.insert(hit.triangle),
            Tool::Drag => {
                let far = stroke.last_screen.is_none_or(|last| last.distance(at) >= 3.0);
                if far {
                    stroke.last_screen = Some(at);
                }
                far
            }
            Tool::Click => false,
        };
        if !take {
            return;
        }
        let erase = stroke.erase;
        p.outbox.add(hit);
        let tint = if erase { mode.erase_tint } else { mode.tint };
        let (Some(color), Some(mesh)) = (tint, &self.mesh) else { return };
        let triangles = match (mode.tool, &self.picking, &self.source) {
            (Tool::Brush, Some(tree), Some(MeshSource::Memory { soup, .. })) => {
                tree.sphere(soup, hit.point, mode.radius_mm.max(0.0))
            }
            (Tool::Brush | Tool::Triangles, ..) => vec![hit.triangle],
            _ => Vec::new(),
        };
        if !triangles.is_empty() && mesh.tint_triangles(queue, &triangles, color) {
            self.dirty = true;
        }
    }

    /// Where the view's point `at` meets the model: the tree if there is one, else (small
    /// meshes) every triangle.
    fn hit_at(&self, rect: Rect, at: Pos2) -> Option<SurfaceHit> {
        let (Some(mesh), Some(MeshSource::Memory { soup, .. })) = (&self.mesh, &self.source) else { return None };
        let ndc = Vec2::new(
            (at.x - rect.center().x) / (rect.width() * 0.5),
            (rect.center().y - at.y) / (rect.height() * 0.5),
        );
        let (origin, dir) = view_ray(&self.fitted(mesh), &self.view, rect.aspect_ratio(), ndc);
        let (t, triangle) = match &self.picking {
            Some(tree) => tree.ray(soup, origin, dir)?,
            None if soup.len() / 3 <= PICK_WITHOUT_TREE => meshgrove_core::mesh::ray_hit_triangle(soup, origin, dir)?,
            None => return None,
        };
        let tri = soup.as_chunks::<3>().0.get(triangle as usize)?;
        let normal = meshgrove_core::mesh::triangle_normal(tri);
        let normal = if normal.dot(dir) > 0.0 { -normal } else { normal };
        Some(SurfaceHit { triangle, point: origin + dir * t, normal })
    }

    /// The brush's circle on the surface under the pointer (its radius in mm, so it shrinks
    /// with distance and leans with the surface); a dot for the other tools.
    fn paint_brush(&self, ui: &Ui, mesh: &GpuMesh, rect: Rect) {
        let Some(p) = &self.painting else { return };
        let Some(hit) = p.hover else { return };
        let bounds = self.fitted(mesh);
        let to_screen = |v: Vec3| {
            project_point(&bounds, &self.view, rect.aspect_ratio(), v)
                .map(|n| pos2(rect.center().x + n.x * rect.width() * 0.5, rect.center().y - n.y * rect.height() * 0.5))
        };
        let painter = ui.painter_at(rect);
        let color =
            if p.stroke.as_ref().is_some_and(|s| s.erase) || (p.stroke.is_none() && ui.input(|i| i.modifiers.shift)) {
                Color32::from_rgb(0x3B, 0xA3, 0xFF)
            } else {
                Color32::from_rgb(255, 196, 0)
            };
        if p.mode.tool != Tool::Brush {
            if let Some(at) = to_screen(hit.point) {
                painter.circle(at, 3.0, color, Stroke::new(1.0, Color32::BLACK));
            }
            return;
        }
        let points: Option<Vec<Pos2>> = brush_circle(hit.point, hit.normal, p.mode.radius_mm, 48)
            .into_iter()
            .map(|v| to_screen(hit.point + (v - hit.point) + hit.normal * 0.01))
            .collect();
        if let Some(points) = points {
            painter.add(egui::Shape::closed_line(points.clone(), Stroke::new(2.5, Color32::from_black_alpha(140))));
            painter.add(egui::Shape::closed_line(points, Stroke::new(1.5, color)));
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
                    self.view = self.looking_from(az, el);
                    self.dirty = true;
                    ui.close();
                }
            }
        });
        // While painting the left button is the tool's: no measuring.
        let measuring = self.measure.is_some();
        if self.painting.as_ref().is_none_or(|p| p.leaving)
            && ui
                .selectable_label(measuring, RichText::new(format!("📏 {}", t!("viewer.measure"))).small())
                .on_hover_text(t!("viewer.measure_hint"))
                .clicked()
        {
            self.measure = if measuring { None } else { Some(Measure::default()) };
        }
        if self.frame.is_some()
            && ui
                .selectable_label(self.frame_shown, RichText::new(format!("⬜ {}", t!("viewer.frame"))).small())
                .on_hover_text(t!("viewer.frame_hint"))
                .clicked()
        {
            self.frame_shown = !self.frame_shown;
            self.dirty = true;
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
        let name = match &self.source {
            Some(MeshSource::File { path, .. }) => path.file_stem().map(|s| s.to_string_lossy().into_owned()),
            Some(MeshSource::Memory { name, .. }) => Some(name.clone()),
            None => None,
        }
        .unwrap_or_else(|| "model".to_owned());
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
        let (path, kind, plate) = match self.source.clone() {
            Some(MeshSource::File { path, kind, plate }) => (path, kind, plate),
            Some(MeshSource::Memory { soup, .. }) => {
                measure.triangles = Some(soup);
                return;
            }
            None => return,
        };
        let (tx, rx) = bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send(crate::meshes::load_triangles(&path, kind, plate));
        });
        measure.loading = Some(rx);
    }

    /// Puts a measuring point where the click hits the model.
    fn pick(&mut self, mesh: &GpuMesh, rect: Rect, pointer: Option<Pos2>) {
        let bounds = self.fitted(mesh);
        let (Some(measure), Some(pos)) = (&mut self.measure, pointer) else { return };
        let Some(soup) = &measure.triangles else { return };
        let ndc = Vec2::new(
            (pos.x - rect.center().x) / (rect.width() * 0.5),
            (rect.center().y - pos.y) / (rect.height() * 0.5),
        );
        let (origin, dir) = view_ray(&bounds, &self.view, rect.aspect_ratio(), ndc);
        let Some(t) = meshgrove_core::mesh::ray_hit(soup, origin, dir) else { return };
        if measure.points.len() >= 2 {
            measure.points.clear();
        }
        measure.points.push(origin + dir * t);
    }

    /// The bed under the model (behind the picture of it), with a line every 50 mm.
    fn paint_bed(&self, ui: &Ui, mesh: &GpuMesh, rect: Rect) {
        let Some([bx, by]) = self.bed else { return };
        let z = mesh.bounds.min.z;
        let bounds = self.fitted(mesh);
        let to_screen = |x: f32, y: f32| {
            project_point(&bounds, &self.view, rect.aspect_ratio(), Vec3::new(x, y, z))
                .map(|n| pos2(rect.center().x + n.x * rect.width() * 0.5, rect.center().y - n.y * rect.height() * 0.5))
        };
        let (hx, hy) = (bx / 2.0, by / 2.0);
        let corners: Option<Vec<Pos2>> =
            [(-hx, -hy), (hx, -hy), (hx, hy), (-hx, hy)].into_iter().map(|(x, y)| to_screen(x, y)).collect();
        let Some(corners) = corners else { return };
        let painter = ui.painter_at(rect);
        let line = ui.visuals().widgets.noninteractive.bg_stroke.color;
        painter.add(egui::Shape::convex_polygon(corners, line.gamma_multiply(0.25), Stroke::new(1.0, line)));
        let fine = Stroke::new(0.5, line.gamma_multiply(0.6));
        let mut x = -hx + 50.0;
        while x < hx {
            if let (Some(a), Some(b)) = (to_screen(x, -hy), to_screen(x, hy)) {
                painter.line_segment([a, b], fine);
            }
            x += 50.0;
        }
        let mut y = -hy + 50.0;
        while y < hy {
            if let (Some(a), Some(b)) = (to_screen(-hx, y), to_screen(hx, y)) {
                painter.line_segment([a, b], fine);
            }
            y += 50.0;
        }
    }

    /// The frame round the model. Called twice: before the picture of the model for what lies
    /// behind it (`front` false: the faces and edges turned away from the camera), and after it
    /// for what is in front. Sides the model sticks out of are red, and their edges always drawn
    /// over the model, so a model too large for its space shows from any direction.
    fn paint_frame(&self, ui: &Ui, mesh: &GpuMesh, rect: Rect, front: bool) {
        let Some(frame) = self.shown_frame() else { return };
        let (bounds, aspect) = (self.fitted(mesh), rect.aspect_ratio());
        let to_screen = |p: Vec3| {
            project_point(&bounds, &self.view, aspect, p)
                .map(|n| pos2(rect.center().x + n.x * rect.width() * 0.5, rect.center().y - n.y * rect.height() * 0.5))
        };
        // The direction the camera looks along at a point.
        let towards = |p: Vec3| {
            let ndc = project_point(&bounds, &self.view, aspect, p).unwrap_or(Vec2::ZERO);
            view_ray(&bounds, &self.view, aspect, ndc).1
        };
        let painter = ui.painter_at(rect);
        let (fits, over) = (Color32::from_rgb(0x3B, 0xA3, 0xFF), Color32::from_rgb(0xFF, 0x4D, 0x4D));
        let corners = frame_corners(&frame);
        let quad = |ids: [usize; 4]| ids.into_iter().map(|i| to_screen(corners[i])).collect::<Option<Vec<Pos2>>>();
        let flat = (0..3).find(|&a| frame.size()[a] < 1e-3);
        let mut exceeded = frame_exceeded(&frame, &mesh.bounds);
        let away = match flat {
            // A rectangle: all of it is behind the model when the camera looks at it from the
            // model's side. What stands on it does not stick out of it.
            Some(axis) => {
                (exceeded[2 * axis], exceeded[2 * axis + 1]) = (false, false);
                let normal = Vec3::AXES[axis];
                let side = normal.dot(mesh.bounds.center() - frame.min);
                [side * normal.dot(-towards(frame.center())) >= 0.0; 6]
            }
            None => frame_faces_away(&frame, towards),
        };
        for (face, ids) in FRAME_FACES.iter().enumerate() {
            // Of a rectangle only the one face; of a box the far faces lightly, and red panes
            // where the model comes through.
            let shown = match flat {
                Some(axis) => face == 2 * axis,
                None => away[face] || exceeded[face],
            };
            if shown
                && away[face] != front
                && let Some(points) = quad(*ids)
            {
                let fill = if exceeded[face] { over.gamma_multiply(0.16) } else { fits.gamma_multiply(0.10) };
                painter.add(egui::Shape::convex_polygon(points, fill, Stroke::NONE));
            }
        }
        for (ends, faces) in frame_edges() {
            // Of a rectangle, not the edges across its thickness.
            if flat.is_some_and(|axis| faces.iter().all(|f| f / 2 != axis)) {
                continue;
            }
            let red = faces.iter().any(|&f| exceeded[f]);
            let behind = away[faces[0]] && away[faces[1]] && !red;
            if behind != front
                && let (Some(a), Some(b)) = (to_screen(corners[ends[0]]), to_screen(corners[ends[1]]))
            {
                let stroke = match (red, front) {
                    (true, _) => Stroke::new(2.0, over),
                    (false, true) => Stroke::new(1.5, fits),
                    (false, false) => Stroke::new(1.0, fits.gamma_multiply(0.7)),
                };
                painter.line_segment([a, b], stroke);
            }
        }
    }

    fn paint_measure(&self, ui: &Ui, mesh: &GpuMesh, rect: Rect) {
        let Some(measure) = &self.measure else { return };
        let painter = ui.painter_at(rect);
        let accent = Color32::from_rgb(255, 196, 0);
        let bounds = self.fitted(mesh);
        let to_screen = |p: Vec3| {
            project_point(&bounds, &self.view, rect.aspect_ratio(), p)
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
                    "{}   (ΔX {} · ΔY {} · ΔZ {})",
                    i18n::length(d.length()),
                    i18n::length_value(d.x.abs()),
                    i18n::length_value(d.y.abs()),
                    i18n::length_value(d.z.abs())
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

/// `n` points of the circle of `radius` round `centre` in the plane across `normal`.
fn brush_circle(centre: Vec3, normal: Vec3, radius: f32, n: usize) -> Vec<Vec3> {
    let normal = normal.try_normalize().unwrap_or(Vec3::Z);
    let u = normal.any_orthonormal_vector();
    let v = normal.cross(u);
    (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            centre + (u * a.cos() + v * a.sin()) * radius
        })
        .collect()
}

/// The corners of a frame, numbered by bits: 1 = far in x, 2 = far in y, 4 = far in z.
fn frame_corners(frame: &Aabb) -> [Vec3; 8] {
    std::array::from_fn(|i| {
        let pick = |axis: usize| if i >> axis & 1 == 1 { frame.max[axis] } else { frame.min[axis] };
        Vec3::new(pick(0), pick(1), pick(2))
    })
}

/// The corners of a frame's faces, in order round each: face `2 * axis` is the near one along
/// that axis, `2 * axis + 1` the far one.
const FRAME_FACES: [[usize; 4]; 6] =
    [[0, 2, 6, 4], [1, 3, 7, 5], [0, 1, 5, 4], [2, 3, 7, 6], [0, 1, 3, 2], [4, 5, 7, 6]];

/// The twelve edges of a frame: the two corners each joins and the two faces it lies between.
fn frame_edges() -> impl Iterator<Item = ([usize; 2], [usize; 2])> {
    (0..3).flat_map(|along| {
        let (b, c) = ((along + 1) % 3, (along + 2) % 3);
        (0..4).map(move |k| {
            let (far_b, far_c) = (k & 1, k >> 1);
            let start = far_b << b | far_c << c;
            ([start, start | 1 << along], [2 * b + far_b, 2 * c + far_c])
        })
    })
}

/// Which faces of a frame are turned away from a camera that looks along `towards(point)` at
/// a point: what lies on them is behind whatever stands in the frame.
fn frame_faces_away(frame: &Aabb, towards: impl Fn(Vec3) -> Vec3) -> [bool; 6] {
    let centre = frame.center();
    std::array::from_fn(|face| {
        let (axis, far) = (face / 2, face % 2 == 1);
        let mut on_face = centre;
        on_face[axis] = if far { frame.max[axis] } else { frame.min[axis] };
        let outward = if far { Vec3::AXES[axis] } else { -Vec3::AXES[axis] };
        outward.dot(towards(on_face)) > 0.0
    })
}

/// The faces of a frame the model sticks out of (by more than a twentieth of a millimetre).
fn frame_exceeded(frame: &Aabb, model: &Aabb) -> [bool; 6] {
    std::array::from_fn(|face| {
        let (axis, far) = (face / 2, face % 2 == 1);
        if far { model.max[axis] > frame.max[axis] + 0.05 } else { model.min[axis] < frame.min[axis] - 0.05 }
    })
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
/// Full deflection of the 3D mouse turns the model this fast (degrees per second)...
const ORBIT_DEGREES_PER_SECOND: f32 = 110.0;
/// ...pans this many bounding radii per second at zoom 1...
const PAN_RADII_PER_SECOND: f32 = 0.9;
/// ...and zooms by this factor (as e^x) per second.
const ZOOM_PER_SECOND: f32 = 1.3;

/// The view after the camera moved for `dt` seconds. Twisting the knob clockwise turns the model
/// clockwise seen from above; pushing or lifting moves the model the same way.
fn driven(view: &ViewParams, m: &CameraMotion, dt: f32) -> ViewParams {
    let zoom = view.zoom.max(0.01);
    let mut out = ViewParams {
        pan: view.pan - glam::Vec2::new(m.pan_x, m.pan_y) * (PAN_RADII_PER_SECOND * dt / zoom),
        zoom: (zoom * (m.zoom * ZOOM_PER_SECOND * dt).exp()).clamp(0.2, 50.0),
        ..*view
    };
    let turn = |v: f32| (v * ORBIT_DEGREES_PER_SECOND * dt).to_radians();
    if view.orientation.is_some() {
        // Free orbit: about the camera's own axes, with roll.
        out.free_turn(turn(m.orbit_h), turn(m.orbit_v), turn(m.roll));
    } else {
        out.azimuth_deg += m.orbit_h * ORBIT_DEGREES_PER_SECOND * dt;
        out.elevation_deg = (out.elevation_deg + m.orbit_v * ORBIT_DEGREES_PER_SECOND * dt).clamp(-89.5, 89.5);
    }
    out
}

fn orbit_view() -> ViewParams {
    ViewParams { framing: Framing::Orbit, ..ViewParams::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_has_its_far_side_behind_the_model_and_its_near_side_in_front() {
        let frame = Aabb { min: Vec3::new(-1.0, -2.0, -3.0), max: Vec3::new(1.0, 2.0, 3.0) };
        // Every corner is on three faces, every edge joins two corners that differ along one
        // axis, and lies on the two faces its corners share.
        let corners = frame_corners(&frame);
        let on = |face: usize, corner: usize| FRAME_FACES[face].contains(&corner);
        let edges: Vec<_> = frame_edges().collect();
        assert_eq!(edges.len(), 12);
        for (ends, faces) in &edges {
            let d = corners[ends[1]] - corners[ends[0]];
            assert_eq!([d.x, d.y, d.z].iter().filter(|v| **v != 0.0).count(), 1, "{ends:?}");
            assert!(faces.iter().all(|&f| on(f, ends[0]) && on(f, ends[1])), "{ends:?} on {faces:?}");
        }
        for (face, ids) in FRAME_FACES.iter().enumerate() {
            let (axis, far) = (face / 2, face % 2 == 1);
            let at = if far { frame.max[axis] } else { frame.min[axis] };
            assert!(ids.iter().all(|&i| corners[i][axis] == at), "face {face}");
        }
        // Looking down and along +y from the front left: the bottom, the back (far y) and the
        // right (far x) are turned away, and three edges lie between two of them.
        let towards = |_: Vec3| Vec3::new(0.3, 0.6, -0.7).normalize();
        let away = frame_faces_away(&frame, towards);
        assert_eq!(away, [false, true, false, true, true, false]);
        let behind = edges.iter().filter(|(_, f)| away[f[0]] && away[f[1]]).count();
        assert_eq!(behind, 3);
        // A model wider than the frame on its near x side and taller than it: those two faces.
        let model = Aabb { min: Vec3::new(-1.5, -2.0, -3.0), max: Vec3::new(1.0, 1.0, 3.2) };
        assert_eq!(frame_exceeded(&frame, &model), [true, false, false, false, false, true]);
        assert_eq!(frame_exceeded(&frame, &frame), [false; 6]);
        // From the opposite side it is the other three faces.
        let away = frame_faces_away(&frame, |_| Vec3::new(-0.3, -0.6, 0.7).normalize());
        assert_eq!(away, [true, false, true, false, false, true]);
    }

    #[test]
    fn the_brush_circle_lies_on_the_surface_round_the_point() {
        let centre = Vec3::new(1.0, 2.0, 3.0);
        for normal in [Vec3::Z, Vec3::new(1.0, 1.0, 0.0), -Vec3::X] {
            let circle = brush_circle(centre, normal, 4.0, 32);
            assert_eq!(circle.len(), 32);
            for p in &circle {
                assert!(((*p - centre).length() - 4.0).abs() < 1e-4, "{p}");
                assert!((*p - centre).dot(normal.normalize()).abs() < 1e-4, "across the normal");
            }
        }
        assert_eq!(brush_circle(centre, Vec3::ZERO, 1.0, 8).len(), 8, "a degenerate normal still draws");
    }

    #[test]
    fn the_3d_mouse_moves_the_camera_and_stays_within_limits() {
        let view = orbit_view();
        let still = driven(&view, &CameraMotion::default(), 0.016);
        assert_eq!(still, view);
        let turn = driven(&view, &CameraMotion { orbit_h: 1.0, ..Default::default() }, 0.5);
        assert!((turn.azimuth_deg - view.azimuth_deg - 55.0).abs() < 1e-3, "110 degrees per second at full travel");
        let up = driven(&view, &CameraMotion { orbit_v: 1.0, ..Default::default() }, 10.0);
        assert_eq!(up.elevation_deg, 89.5, "never over the pole");
        let pushed = driven(&view, &CameraMotion { pan_x: 1.0, pan_y: 1.0, ..Default::default() }, 0.1);
        assert!(pushed.pan.x < 0.0 && pushed.pan.y < 0.0, "the model follows the knob");
        let zoomed = driven(&view, &CameraMotion { zoom: 1.0, ..Default::default() }, 1.0);
        assert!(zoomed.zoom > view.zoom);
        assert_eq!(driven(&view, &CameraMotion { zoom: -1.0, ..Default::default() }, 100.0).zoom, 0.2);
        // Free orbit rolls and goes over the pole.
        let mut free = view;
        free.set_free(true);
        let rolled = driven(&free, &CameraMotion { roll: 1.0, ..Default::default() }, 0.5);
        assert!(rolled.axes().2.dot(free.axes().2) < 0.9, "55 degrees of roll");
        assert!((rolled.axes().0 - free.axes().0).length() < 1e-4, "the view direction stays");
        let over = driven(&free, &CameraMotion { orbit_v: 1.0, ..Default::default() }, 1.5);
        assert!(over.axes().2.z < 0.0, "upside down: no 89.5 degree limit");
    }
}
