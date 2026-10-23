//! The 3D mouse in the settings: a small 3D picture of a SpaceMouse that moves with the real
//! knob, the sliders per movement, the mapping of movements to the camera and the calibration.

use std::sync::Arc;

use egui::{Color32, Painter, Pos2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use glam::{Mat4, Vec2, Vec3};
use meshgrove_core::Mesh;
use meshgrove_core::config::Config;
use meshgrove_core::spacemouse::{
    Axis, AxisMapping, CALIBRATION_STEPS, CalibrationNote, Calibrator, CameraMotion, DEADZONE_RANGE, SPEED_RANGE,
    SpaceMouseConfig,
};
use meshgrove_render::{Framing, GpuMesh, Renderer, ViewParams};

use super::offscreen::{OffscreenView, pixel_size};
use super::settings::SettingsChange;
use crate::spacemouse::{Snapshot, Status};

// ---- the picture ----

/// Largest travel of the drawn knob (in units of its base's radius) and tilt and twist (degrees),
/// exaggerated so a small push is easy to see.
const TRAVEL: Vec3 = Vec3::new(0.35, 0.35, 0.3);
const TILT_DEGREES: f32 = 22.0;
const TWIST_DEGREES: f32 = 30.0;
const SEGMENTS: usize = 40;

/// One face to paint: corners in world space, its outward normal and a base colour.
struct Face {
    corners: [Vec3; 4],
    normal: Vec3,
    color: Color32,
}

/// A solid of revolution about the vertical axis. `profile` is (radius, height) from the
/// bottom's middle, out and up round the outside and back in to the top's middle, so the
/// outward normal of a piece is (dz, −dr) turned to the angle; `colors` has one colour per piece.
fn lathe(profile: &[(f32, f32)], colors: &[Color32], faces: &mut Vec<Face>) {
    for (piece, pair) in profile.windows(2).enumerate() {
        let ((r0, z0), (r1, z1)) = (pair[0], pair[1]);
        let (dr, dz) = (r1 - r0, z1 - z0);
        let len = dr.hypot(dz).max(1e-6);
        let (nr, nz) = (dz / len, -dr / len);
        let at = |r: f32, z: f32, i: usize| {
            let a = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            Vec3::new(r * a.cos(), r * a.sin(), z)
        };
        for i in 0..SEGMENTS {
            let middle = (i as f32 + 0.5) / SEGMENTS as f32 * std::f32::consts::TAU;
            faces.push(Face {
                corners: [at(r0, z0, i), at(r0, z0, i + 1), at(r1, z1, i + 1), at(r1, z1, i)],
                normal: Vec3::new(nr * middle.cos(), nr * middle.sin(), nz),
                color: colors[piece.min(colors.len() - 1)],
            });
        }
    }
}

/// The picture's faces for the given movements (the order of [`Axis::ALL`], −1…1): a round
/// base and a knob with a domed cap.
fn mouse_faces(motion: [f32; 6], active: [bool; 6]) -> Vec<Face> {
    let grey = |v: u8| Color32::from_rgb(v, v, v + 4);
    let mut faces = Vec::new();
    lathe(
        &[(0.0, -0.3), (1.0, -0.3), (1.12, -0.26), (1.18, -0.18), (1.16, -0.08), (1.06, -0.02), (0.9, 0.0), (0.0, 0.0)],
        &[grey(40), grey(50), grey(58), grey(62), grey(66), grey(70), grey(74)],
        &mut faces,
    );
    let cap = if active.iter().any(|a| *a) { Color32::from_rgb(110, 170, 215) } else { grey(150) };
    let mut knob = Vec::new();
    lathe(
        &[
            (0.0, 0.0),
            (0.4, 0.0),
            (0.42, 0.1),
            (0.42, 0.5),
            (0.55, 0.52),
            (0.78, 0.56),
            (0.9, 0.66),
            (0.9, 0.78),
            (0.8, 0.88),
            (0.6, 0.93),
            (0.0, 0.95),
        ],
        &[grey(70), grey(90), grey(100), grey(110), grey(120), grey(128), grey(134), cap, cap, cap],
        &mut knob,
    );
    let m = |a: Axis| motion[a.index()];
    let pivot = Mat4::from_translation(Vec3::new(m(Axis::PushLr), m(Axis::PushFb), m(Axis::Lift)) * TRAVEL)
        * Mat4::from_rotation_z(-m(Axis::Twist) * TWIST_DEGREES.to_radians())
        * Mat4::from_rotation_y(m(Axis::TiltLr) * TILT_DEGREES.to_radians())
        * Mat4::from_rotation_x(-m(Axis::TiltFb) * TILT_DEGREES.to_radians());
    for face in &mut knob {
        face.corners = face.corners.map(|c| pivot.transform_point3(c));
        face.normal = pivot.transform_vector3(face.normal);
    }
    faces.extend(knob);
    faces
}

/// The camera: in front of and above the mouse.
const EYE: Vec3 = Vec3::new(0.0, -5.2, 3.3);

fn camera(rect: Rect) -> Mat4 {
    let aspect = rect.width() / rect.height().max(1.0);
    glam::camera::rh::proj::directx::perspective(30f32.to_radians(), aspect, 0.1, 50.0)
        * glam::camera::rh::view::look_at_mat4(EYE, Vec3::new(0.0, 0.0, 0.2), Vec3::Z)
}

fn project(view_proj: &Mat4, rect: Rect, p: Vec3) -> Pos2 {
    let clip = *view_proj * p.extend(1.0);
    let ndc = clip.truncate() / clip.w;
    pos2(rect.center().x + ndc.x * rect.width() * 0.5, rect.center().y - ndc.y * rect.height() * 0.5)
}

/// The path of an arrow showing a movement's positive direction, over the knob.
fn arrow_path(axis: Axis) -> Vec<Vec3> {
    let top = Vec3::new(0.0, 0.0, 0.95);
    match axis {
        Axis::PushLr => vec![top + Vec3::new(-0.2, 0.0, 0.25), top + Vec3::new(1.5, 0.0, 0.25)],
        Axis::PushFb => vec![top + Vec3::new(0.0, -0.2, 0.25), top + Vec3::new(0.0, 1.5, 0.25)],
        Axis::Lift => vec![top, top + Vec3::new(0.0, 0.0, 1.3)],
        Axis::TiltFb => vec![top + Vec3::new(0.0, -0.3, 0.3), top + Vec3::new(0.0, 1.3, 0.1)],
        Axis::TiltLr => vec![top + Vec3::new(-0.3, 0.0, 0.3), top + Vec3::new(1.3, 0.0, 0.1)],
        // Clockwise seen from above, a quarter turn.
        Axis::Twist => (0..=12)
            .map(|i| {
                let a = std::f32::consts::FRAC_PI_2 - i as f32 / 12.0 * std::f32::consts::PI;
                top + Vec3::new(1.15 * a.cos(), 1.15 * a.sin(), 0.3)
            })
            .collect(),
    }
}

fn paint_arrow(painter: &Painter, view_proj: &Mat4, rect: Rect, path: &[Vec3], color: Color32) {
    let points: Vec<Pos2> = path.iter().map(|p| project(view_proj, rect, *p)).collect();
    painter.add(egui::Shape::line(points.clone(), Stroke::new(3.0, color)));
    if let [.., a, b] = points.as_slice() {
        let dir = (*b - *a).normalized();
        let side = vec2(-dir.y, dir.x);
        let tip = *b + dir * 5.0;
        painter.add(egui::Shape::convex_polygon(
            vec![tip, *b - dir * 7.0 + side * 6.0, *b - dir * 7.0 - side * 6.0],
            color,
            Stroke::NONE,
        ));
    }
}

/// Paints the faces that look at the eye, far to near, shaded by a light from the front left
/// above. Each gets an outline in its own colour, so no seams show between neighbours.
fn paint_faces(painter: &Painter, rect: Rect, view_proj: &Mat4, mut faces: Vec<Face>) {
    let centre = |f: &Face| f.corners.iter().copied().sum::<Vec3>() / 4.0;
    faces.retain(|f| f.normal.dot(EYE - centre(f)) > 0.0);
    faces.sort_by(|a, b| (centre(b) - EYE).length().total_cmp(&(centre(a) - EYE).length()));
    let light = Vec3::new(-0.4, -0.5, 0.8).normalize();
    for face in &faces {
        let shade = 0.5 + 0.5 * face.normal.normalize_or_zero().dot(light).max(0.0);
        let c = face.color;
        let color = Color32::from_rgb(
            (f32::from(c.r()) * shade) as u8,
            (f32::from(c.g()) * shade) as u8,
            (f32::from(c.b()) * shade) as u8,
        );
        let points = face.corners.iter().map(|p| project(view_proj, rect, *p)).collect();
        painter.add(egui::Shape::convex_polygon(points, color, Stroke::new(1.0, color)));
    }
}

/// Draws the 3D mouse, its knob moved by `motion` (the order of [`Axis::ALL`], −1…1); `arrow`
/// shows a movement's direction (the calibration asks for it).
pub fn draw_mouse(ui: &mut Ui, size: egui::Vec2, motion: [f32; 6], arrow: Option<Axis>) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, ui.visuals().extreme_bg_color);
    let view_proj = camera(rect);
    let active = motion.map(|v| v.abs() > 0.08);
    paint_faces(&painter, rect, &view_proj, mouse_faces(motion, active));
    if let Some(axis) = arrow {
        paint_arrow(&painter, &view_proj, rect, &arrow_path(axis), Color32::from_rgb(255, 196, 60));
    }
}

// ---- the cube that shows what the settings do ----

/// The cube turns, looks up or down, slides and zooms by this much at full deflection, and goes
/// back when the knob does: it shows the effect of the settings, it does not move on.
const RESULT_TURN_DEGREES: f32 = 40.0;
const RESULT_LOOK_DEGREES: f32 = 28.0;
const RESULT_ROLL_DEGREES: f32 = 35.0;
const RESULT_SLIDE: f32 = 0.55;
const RESULT_ZOOM: f32 = 0.55;

/// How the viewer would look at the cube for the camera movement `motion` (what the settings
/// make of the knob's current position): a bounded offset from a fixed start.
fn result_view(motion: &CameraMotion, free_orbit: bool) -> ViewParams {
    let m = |v: f32| v.clamp(-1.2, 1.2);
    let mut view = ViewParams {
        azimuth_deg: 35.0,
        elevation_deg: 25.0,
        zoom: (m(motion.zoom) * RESULT_ZOOM).exp(),
        pan: -Vec2::new(m(motion.pan_x), m(motion.pan_y)) * RESULT_SLIDE,
        framing: Framing::Orbit,
        ..ViewParams::default()
    };
    if free_orbit {
        view.free_turn(
            (m(motion.orbit_h) * RESULT_TURN_DEGREES).to_radians(),
            (m(motion.orbit_v) * RESULT_LOOK_DEGREES).to_radians(),
            (m(motion.roll) * RESULT_ROLL_DEGREES).to_radians(),
        );
    } else {
        view.azimuth_deg += m(motion.orbit_h) * RESULT_TURN_DEGREES;
        view.elevation_deg = (view.elevation_deg + m(motion.orbit_v) * RESULT_LOOK_DEGREES).clamp(-80.0, 80.0);
    }
    view
}

/// A regular cube, drawn by the same renderer as the models.
struct ResultCube {
    mesh: Arc<GpuMesh>,
    view: OffscreenView,
}

// ---- texts ----

fn axis_name(axis: Axis) -> String {
    match axis {
        Axis::PushLr => t!("mouse.axis_push_lr"),
        Axis::PushFb => t!("mouse.axis_push_fb"),
        Axis::Lift => t!("mouse.axis_lift"),
        Axis::TiltFb => t!("mouse.axis_tilt_fb"),
        Axis::TiltLr => t!("mouse.axis_tilt_lr"),
        Axis::Twist => t!("mouse.axis_twist"),
    }
    .into_owned()
}

fn prompt(axis: Axis) -> String {
    match axis {
        Axis::PushLr => t!("mouse.cal_push_lr"),
        Axis::PushFb => t!("mouse.cal_push_fb"),
        Axis::Lift => t!("mouse.cal_lift"),
        Axis::TiltFb => t!("mouse.cal_tilt_fb"),
        Axis::TiltLr => t!("mouse.cal_tilt_lr"),
        Axis::Twist => t!("mouse.cal_twist"),
    }
    .into_owned()
}

fn note_text(note: CalibrationNote) -> String {
    match note {
        CalibrationNote::Waiting => t!("mouse.note_waiting"),
        CalibrationNote::Holding => t!("mouse.note_holding"),
        CalibrationNote::Mixed => t!("mouse.note_mixed"),
        CalibrationNote::Taken => t!("mouse.note_taken"),
        CalibrationNote::Release => t!("mouse.note_release"),
        CalibrationNote::Done => t!("mouse.note_done"),
    }
    .into_owned()
}

/// The rule that gives the user access to 3D mice on Linux (it ships with the packages).
const UDEV_RULES: &str = include_str!("../../../../packaging/linux/70-meshgrove-3dmouse.rules");

/// The commands that install the rule by hand (an AppImage or a build from source has none).
fn udev_command() -> String {
    format!(
        "sudo tee /etc/udev/rules.d/70-meshgrove-3dmouse.rules <<'EOF'\n{UDEV_RULES}EOF\nsudo udevadm control --reload && sudo udevadm trigger"
    )
}

// ---- the settings section ----

/// What the settings window keeps for the 3D mouse.
#[derive(Default)]
pub struct MouseSettings {
    calibrator: Option<(Calibrator, CalibrationNote)>,
    cube: Option<ResultCube>,
}

impl MouseSettings {
    /// Starts the calibration (when the dialog opens it is off).
    pub fn reset(&mut self) {
        self.calibrator = None;
    }

    /// A regular cube, turned, slid and zoomed as the viewer would show it for the knob's
    /// current position.
    fn draw_result(
        &mut self,
        ui: &mut Ui,
        size: egui::Vec2,
        (rs, renderer): (&egui_wgpu::RenderState, &Renderer),
        motion: &CameraMotion,
        free_orbit: bool,
    ) {
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 6.0, ui.visuals().extreme_bg_color);
        if self.cube.is_none() {
            self.cube = renderer
                .upload(&rs.device, &Mesh::cube(2.0))
                .map(|mesh| ResultCube { mesh: Arc::new(mesh), view: OffscreenView::default() });
        }
        let Some(cube) = &mut self.cube else { return };
        let (w, h) = pixel_size(ui.ctx(), rect);
        cube.view.ensure_size(rs, renderer, w, h);
        cube.view.render(rs, renderer, &cube.mesh, &result_view(motion, free_orbit));
        cube.view.paint(&painter, rect);
    }

    pub fn section(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        mouse: &Snapshot,
        render: (&egui_wgpu::RenderState, &Renderer),
        changes: &mut Vec<SettingsChange>,
    ) {
        let mut c: SpaceMouseConfig = config.spacemouse.clone();
        ui.label(RichText::new(t!("mouse.title")).strong());
        ui.checkbox(&mut c.enabled, t!("mouse.enabled"));
        match &mouse.status {
            Status::Connected(name) => ui.weak(t!("mouse.connected", name = name)),
            Status::NoDevice => ui.weak(t!("mouse.no_device")),
            Status::NoAccess(name) => ui.colored_label(ui.visuals().warn_fg_color, t!("mouse.no_access", name = name)),
        };
        if matches!(mouse.status, Status::NoAccess(_)) {
            ui.weak(t!("mouse.no_access_hint"));
            let mut command = udev_command();
            ui.add(egui::TextEdit::multiline(&mut command).desired_rows(6).desired_width(f32::INFINITY).code_editor());
        }
        ui.add_space(6.0);

        let physical = config.spacemouse.physical(mouse.raw);
        // The pictures show the settings as they are being changed here.
        let motion = c.camera(c.process(mouse.raw));
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.weak(t!("mouse.picture_knob"));
                draw_mouse(ui, vec2(250.0, 190.0), physical, None);
            });
            ui.vertical(|ui| {
                ui.weak(t!("mouse.picture_result"));
                self.draw_result(ui, vec2(250.0, 190.0), render, &motion, config.free_orbit);
            });
        });
        ui.add_space(4.0);
        egui::Grid::new("mouse_axes").num_columns(4).spacing([8.0, 4.0]).show(ui, |ui| {
            for axis in Axis::ALL {
                let i = axis.index();
                ui.label(axis_name(axis));
                ui.checkbox(&mut c.invert[i], t!("mouse.invert"));
                ui.add(
                    egui::Slider::new(&mut c.sensitivity[i], SPEED_RANGE).custom_formatter(|v, _| format!("{v:.2}×")),
                );
                let level = physical[i].abs().clamp(0.0, 1.0);
                ui.add(egui::ProgressBar::new(level).desired_width(60.0).desired_height(8.0));
                ui.end_row();
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(t!("mouse.speed"));
            ui.add(egui::Slider::new(&mut c.speed, SPEED_RANGE).custom_formatter(|v, _| format!("{v:.2}×")));
        });
        ui.horizontal(|ui| {
            ui.label(t!("mouse.deadzone"));
            ui.add(
                egui::Slider::new(&mut c.deadzone, DEADZONE_RANGE)
                    .custom_formatter(|v, _| format!("{:.0} %", v * 100.0)),
            );
        });
        ui.checkbox(&mut c.dominant, t!("mouse.dominant"));
        ui.add_space(6.0);

        ui.label(RichText::new(t!("mouse.mapping")).strong());
        let (twist, tilt) = (AxisMapping::twist_orbits(), AxisMapping::tilt_orbits());
        ui.horizontal(|ui| {
            if ui.radio(c.mapping == twist, t!("mouse.preset_twist")).clicked() {
                c.mapping = twist;
            }
            if ui.radio(c.mapping == tilt, t!("mouse.preset_tilt")).clicked() {
                c.mapping = tilt;
            }
        });
        ui.collapsing(t!("mouse.custom_mapping"), |ui| {
            egui::Grid::new("mouse_mapping").num_columns(2).show(ui, |ui| {
                let m = &mut c.mapping;
                for (label, slot) in [
                    (t!("mouse.action_orbit_h"), &mut m.orbit_h),
                    (t!("mouse.action_orbit_v"), &mut m.orbit_v),
                    (t!("mouse.action_pan_x"), &mut m.pan_x),
                    (t!("mouse.action_pan_y"), &mut m.pan_y),
                    (t!("mouse.action_zoom"), &mut m.zoom),
                    (t!("mouse.action_roll"), &mut m.roll),
                ] {
                    ui.label(label.as_ref());
                    let shown = slot.map_or_else(|| t!("mouse.nothing").into_owned(), axis_name);
                    egui::ComboBox::from_id_salt(&label).selected_text(shown).show_ui(ui, |ui| {
                        ui.selectable_value(slot, None, t!("mouse.nothing"));
                        for axis in Axis::ALL {
                            ui.selectable_value(slot, Some(axis), axis_name(axis));
                        }
                    });
                    ui.end_row();
                }
            });
        });
        ui.weak(t!("mouse.buttons_hint"));
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            if ui.button(t!("mouse.calibrate")).clicked() {
                self.calibrator = Some((Calibrator::new(), CalibrationNote::Waiting));
            }
            if c.calibration.is_some() {
                ui.label(t!("mouse.calibrated"));
                if ui.small_button(t!("mouse.calibration_reset")).clicked() {
                    c.calibration = None;
                }
            } else {
                ui.weak(t!("mouse.not_calibrated"));
            }
        });
        ui.weak(t!("mouse.calibrate_hint"));
        if c != config.spacemouse {
            changes.push(SettingsChange::SpaceMouse(c));
        }
    }

    /// The calibration window, over the settings: asks for each movement in turn.
    pub fn calibration_modal(
        &mut self,
        ctx: &egui::Context,
        config: &Config,
        mouse: &Snapshot,
        changes: &mut Vec<SettingsChange>,
    ) {
        let Some((calibrator, note)) = &mut self.calibrator else { return };
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        *note = calibrator.feed(mouse.raw, dt);
        if *note == CalibrationNote::Done
            && let Some(result) = calibrator.result()
        {
            let mut c = config.spacemouse.clone();
            c.calibration = Some(result);
            changes.push(SettingsChange::SpaceMouse(c));
            self.calibrator = None;
            return;
        }
        // The knob follows the real one while asking, so the user sees what was understood.
        let target = calibrator.current();
        let step = calibrator.step();
        let progress = calibrator.progress();
        let note = *note;
        let mut cancel = false;
        egui::Modal::new(egui::Id::unique("mouse-calibration")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(t!("mouse.calibrate"));
            ui.add_space(4.0);
            ui.label(t!("mouse.calibrate_step", n = step + 1, total = CALIBRATION_STEPS));
            if let Some(axis) = target {
                ui.label(RichText::new(prompt(axis)).strong().size(16.0));
            }
            ui.add_space(4.0);
            let motion = config.spacemouse.physical(mouse.raw);
            draw_mouse(ui, vec2(ui.available_width().min(380.0), 210.0), motion, target);
            ui.add_space(6.0);
            ui.add(egui::ProgressBar::new(progress).desired_width(f32::INFINITY));
            ui.weak(note_text(note));
            if !matches!(mouse.status, Status::Connected(_)) {
                ui.colored_label(ui.visuals().warn_fg_color, t!("mouse.no_device"));
            }
            ui.add_space(6.0);
            if ui.button(t!("trash.cancel")).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
        });
        if cancel {
            self.calibrator = None;
        }
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picture_moves_with_the_knob() {
        let still = mouse_faces([0.0; 6], [false; 6]);
        let mut motion = [0.0; 6];
        motion[Axis::PushLr.index()] = 1.0;
        let pushed = mouse_faces(motion, [true, false, false, false, false, false]);
        assert_eq!(still.len(), pushed.len());
        // The base stays, the knob's faces move to the right.
        assert_eq!(still[0].corners, pushed[0].corners);
        let (a, b) = (still.last().unwrap().corners[0], pushed.last().unwrap().corners[0]);
        assert!(b.x > a.x + 0.3, "{a:?} -> {b:?}");
    }

    #[test]
    fn the_rule_for_the_device_access_is_part_of_the_hint() {
        let command = udev_command();
        assert!(command.contains("256f") && command.contains("uaccess") && command.contains("udevadm"));
    }

    #[test]
    fn the_result_cube_moves_a_bit_and_returns() {
        let still = result_view(&CameraMotion::default(), false);
        assert_eq!(
            still,
            result_view(&CameraMotion::default(), false),
            "no integration: the same knob, the same picture"
        );
        let turned = result_view(&CameraMotion { orbit_h: 1.0, ..Default::default() }, false);
        assert!(turned.azimuth_deg > still.azimuth_deg && turned.azimuth_deg - still.azimuth_deg <= 50.0);
        let pushed = result_view(&CameraMotion { pan_x: 1.0, pan_y: 1.0, ..Default::default() }, false);
        assert!(pushed.pan.x < 0.0 && pushed.pan.y < 0.0, "the cube follows the knob");
        let zoomed = result_view(&CameraMotion { zoom: 1.0, ..Default::default() }, false);
        assert!(zoomed.zoom > 1.0 && zoomed.zoom < 2.0);
        let huge = result_view(
            &CameraMotion { orbit_h: 50.0, orbit_v: 50.0, pan_x: 50.0, zoom: 50.0, ..Default::default() },
            false,
        );
        assert!(
            huge.elevation_deg <= 80.0 && huge.zoom < 2.0 && huge.pan.length() < 1.0,
            "a huge deflection stays bounded"
        );
        // Free orbit also rolls.
        let free = result_view(&CameraMotion::default(), true);
        let rolled = result_view(&CameraMotion { roll: 1.0, ..Default::default() }, true);
        assert!(rolled.axes().2.dot(free.axes().2) < 0.9);
        assert!(free.orientation.is_some() && rolled.orientation.is_some());
    }

    #[test]
    fn every_axis_has_an_arrow_and_text() {
        for axis in Axis::ALL {
            assert!(arrow_path(axis).len() >= 2);
            assert!(!axis_name(axis).is_empty() && !prompt(axis).is_empty());
        }
    }
}
