//! The 3D mouse in the settings: a small 3D picture of a SpaceMouse that moves with the real
//! knob, the sliders per movement, the mapping of movements to the camera and the calibration.

use egui::{Color32, Painter, Pos2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use glam::{Mat4, Vec3};
use meshgrove_core::config::Config;
use meshgrove_core::spacemouse::{
    Axis, AxisMapping, CALIBRATION_STEPS, CalibrationNote, Calibrator, CameraMotion, DEADZONE_RANGE, SPEED_RANGE,
    SpaceMouseConfig,
};

use super::settings::SettingsChange;
use crate::spacemouse::{Snapshot, Status};

// ---- the picture ----

/// Largest travel of the drawn knob (in units of its base's radius) and tilt and twist (degrees),
/// exaggerated so a small push is easy to see.
const TRAVEL: Vec3 = Vec3::new(0.35, 0.35, 0.3);
const TILT_DEGREES: f32 = 22.0;
const TWIST_DEGREES: f32 = 30.0;
const SEGMENTS: usize = 28;

/// One face to paint: corners in world space and a base colour.
struct Face {
    corners: Vec<Vec3>,
    color: Color32,
}

/// A vertical cylinder from `z0` to `z1`.
fn cylinder(radius: f32, z0: f32, z1: f32, side: Color32, top: Color32, faces: &mut Vec<Face>) {
    let ring = |z: f32| -> Vec<Vec3> {
        (0..SEGMENTS)
            .map(|i| {
                let a = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
                Vec3::new(radius * a.cos(), radius * a.sin(), z)
            })
            .collect()
    };
    let (low, high) = (ring(z0), ring(z1));
    for i in 0..SEGMENTS {
        let j = (i + 1) % SEGMENTS;
        faces.push(Face { corners: vec![low[i], low[j], high[j], high[i]], color: side });
    }
    faces.push(Face { corners: high, color: top });
}

/// The picture's faces for the given movements (the order of [`Axis::ALL`], −1…1).
fn mouse_faces(motion: [f32; 6], active: [bool; 6]) -> Vec<Face> {
    let grey = |v: u8| Color32::from_rgb(v, v, v + 4);
    let mut faces = Vec::new();
    cylinder(1.15, -0.3, 0.0, grey(52), grey(66), &mut faces);
    let mut knob = Vec::new();
    let cap = if active.iter().any(|a| *a) { Color32::from_rgb(110, 170, 215) } else { grey(150) };
    cylinder(0.42, 0.0, 0.55, grey(95), grey(110), &mut knob);
    cylinder(0.85, 0.55, 0.85, grey(120), cap, &mut knob);
    let m = |a: Axis| motion[a.index()];
    let pivot = Mat4::from_translation(Vec3::new(m(Axis::PushLr), m(Axis::PushFb), m(Axis::Lift)) * TRAVEL)
        * Mat4::from_rotation_z(-m(Axis::Twist) * TWIST_DEGREES.to_radians())
        * Mat4::from_rotation_y(m(Axis::TiltLr) * TILT_DEGREES.to_radians())
        * Mat4::from_rotation_x(-m(Axis::TiltFb) * TILT_DEGREES.to_radians());
    for face in &mut knob {
        for c in &mut face.corners {
            *c = pivot.transform_point3(*c);
        }
    }
    faces.extend(knob);
    faces
}

/// The camera: in front of and above the mouse.
fn camera(rect: Rect) -> Mat4 {
    let aspect = rect.width() / rect.height().max(1.0);
    let eye = Vec3::new(0.0, -5.2, 3.3);
    glam::camera::rh::proj::directx::perspective(30f32.to_radians(), aspect, 0.1, 50.0)
        * glam::camera::rh::view::look_at_mat4(eye, Vec3::new(0.0, 0.0, 0.2), Vec3::Z)
}

fn project(view_proj: &Mat4, rect: Rect, p: Vec3) -> Pos2 {
    let clip = *view_proj * p.extend(1.0);
    let ndc = clip.truncate() / clip.w;
    pos2(rect.center().x + ndc.x * rect.width() * 0.5, rect.center().y - ndc.y * rect.height() * 0.5)
}

/// The path of an arrow showing a movement's positive direction, over the knob.
fn arrow_path(axis: Axis) -> Vec<Vec3> {
    let top = Vec3::new(0.0, 0.0, 0.85);
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

/// Paints faces far to near, shaded by a light from the front left above.
fn paint_faces(painter: &Painter, rect: Rect, view_proj: &Mat4, eye: Vec3, mut faces: Vec<Face>) {
    faces.sort_by(|a, b| {
        let depth = |f: &Face| f.corners.iter().map(|c| (*c - eye).length()).sum::<f32>() / f.corners.len() as f32;
        depth(b).total_cmp(&depth(a))
    });
    let light = Vec3::new(-0.4, -0.5, 0.8).normalize();
    for face in &faces {
        let n = (face.corners[1] - face.corners[0]).cross(face.corners[2] - face.corners[0]).normalize_or_zero();
        let shade = 0.55 + 0.45 * n.dot(light).abs();
        let c = face.color;
        let color = Color32::from_rgb(
            (f32::from(c.r()) * shade) as u8,
            (f32::from(c.g()) * shade) as u8,
            (f32::from(c.b()) * shade) as u8,
        );
        let points = face.corners.iter().map(|p| project(view_proj, rect, *p)).collect();
        painter.add(egui::Shape::convex_polygon(points, color, Stroke::new(0.5, Color32::from_black_alpha(90))));
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
    paint_faces(&painter, rect, &view_proj, Vec3::new(0.0, -5.2, 3.3), mouse_faces(motion, active));
    if let Some(axis) = arrow {
        paint_arrow(&painter, &view_proj, rect, &arrow_path(axis), Color32::from_rgb(255, 196, 60));
    }
}

// ---- the model that shows what the settings do ----

/// The model turns, looks up or down, slides and zooms by this much at full deflection, and
/// goes back when the knob does: it shows the effect of the settings, it does not move on.
const RESULT_TURN_DEGREES: f32 = 40.0;
const RESULT_LOOK_DEGREES: f32 = 28.0;
const RESULT_SLIDE: f32 = 1.1;
const RESULT_ZOOM: f32 = 0.55;

/// An axis-aligned box as faces, each side in its own tone so turning it is easy to see.
fn block(min: Vec3, max: Vec3, tint: [u8; 3], faces: &mut Vec<Face>) {
    let c = |x: bool, y: bool, z: bool| {
        Vec3::new(if x { max.x } else { min.x }, if y { max.y } else { min.y }, if z { max.z } else { min.z })
    };
    let tone = |k: i16| {
        Color32::from_rgb(
            (i16::from(tint[0]) + k).clamp(0, 255) as u8,
            (i16::from(tint[1]) + k).clamp(0, 255) as u8,
            (i16::from(tint[2]) + k).clamp(0, 255) as u8,
        )
    };
    let sides = [
        ([c(false, false, false), c(true, false, false), c(true, false, true), c(false, false, true)], tone(-10)), // front
        ([c(true, true, false), c(false, true, false), c(false, true, true), c(true, true, true)], tone(-30)), // back
        ([c(false, true, false), c(false, false, false), c(false, false, true), c(false, true, true)], tone(-20)), // left
        ([c(true, false, false), c(true, true, false), c(true, true, true), c(true, false, true)], tone(0)), // right
        ([c(false, false, true), c(true, false, true), c(true, true, true), c(false, true, true)], tone(25)), // top
        ([c(false, true, false), c(true, true, false), c(true, false, false), c(false, false, false)], tone(-40)), // bottom
    ];
    for (corners, color) in sides {
        faces.push(Face { corners: corners.to_vec(), color });
    }
}

/// A small part that looks different from every side: a plate with a tower at one end and a
/// peg at the front.
fn part_faces() -> Vec<Face> {
    let mut faces = Vec::new();
    block(Vec3::new(-1.2, -0.7, -0.3), Vec3::new(1.2, 0.7, 0.2), [120, 150, 190], &mut faces);
    block(Vec3::new(-1.2, -0.7, 0.2), Vec3::new(-0.4, 0.7, 1.1), [190, 140, 110], &mut faces);
    block(Vec3::new(0.5, -1.1, -0.1), Vec3::new(0.9, -0.7, 0.2), [130, 190, 130], &mut faces);
    faces
}

/// The eye, the view and the projection for the part, after the camera movement `m` (what the
/// settings make of the knob's current position): a bounded offset from a fixed start.
fn result_camera(rect: Rect, m: &CameraMotion) -> (Vec3, Mat4) {
    let m = CameraMotion {
        orbit_h: m.orbit_h.clamp(-1.2, 1.2),
        orbit_v: m.orbit_v.clamp(-1.2, 1.2),
        pan_x: m.pan_x.clamp(-1.2, 1.2),
        pan_y: m.pan_y.clamp(-1.2, 1.2),
        zoom: m.zoom.clamp(-1.2, 1.2),
    };
    let azimuth = (30.0 + m.orbit_h * RESULT_TURN_DEGREES).to_radians();
    let elevation = (24.0 + m.orbit_v * RESULT_LOOK_DEGREES).clamp(-80.0, 80.0).to_radians();
    let distance = 7.0 / (m.zoom * RESULT_ZOOM).exp();
    // Positive azimuth moves the eye to the right, like the viewer's.
    let direction = Vec3::new(azimuth.sin() * elevation.cos(), -azimuth.cos() * elevation.cos(), elevation.sin());
    let right = Vec3::new(azimuth.cos(), azimuth.sin(), 0.0);
    let up = right.cross(-direction).normalize_or_zero();
    // The part follows the knob: the camera slides the other way.
    let target = Vec3::new(0.0, 0.0, 0.3) - (right * m.pan_x + up * m.pan_y) * RESULT_SLIDE;
    let eye = target + direction * distance;
    let aspect = rect.width() / rect.height().max(1.0);
    let view_proj = glam::camera::rh::proj::directx::perspective(30f32.to_radians(), aspect, 0.1, 80.0)
        * glam::camera::rh::view::look_at_mat4(eye, target, Vec3::Z);
    (eye, view_proj)
}

/// Draws a small part as the viewer would show it for the camera movement `motion`: turned,
/// slid or zoomed a bit while the knob is held that way, and back when it is let go.
pub fn draw_result(ui: &mut Ui, size: egui::Vec2, motion: &CameraMotion) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, ui.visuals().extreme_bg_color);
    let (eye, view_proj) = result_camera(rect, motion);
    paint_faces(&painter, rect, &view_proj, eye, part_faces());
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
}

impl MouseSettings {
    /// Starts the calibration (when the dialog opens it is off).
    pub fn reset(&mut self) {
        self.calibrator = None;
    }

    pub fn section(&mut self, ui: &mut Ui, config: &Config, mouse: &Snapshot, changes: &mut Vec<SettingsChange>) {
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
                draw_result(ui, vec2(250.0, 190.0), &motion);
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
    fn the_result_part_moves_a_bit_and_returns() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(250.0, 190.0));
        let at = |m: CameraMotion| {
            let (eye, view_proj) = result_camera(rect, &m);
            (eye, project(&view_proj, rect, Vec3::ZERO))
        };
        let (eye0, centre0) = at(CameraMotion::default());
        assert_eq!(
            at(CameraMotion::default()),
            (eye0, centre0),
            "no integration: the same knob gives the same picture"
        );
        let (eye_turned, _) = at(CameraMotion { orbit_h: 1.0, ..Default::default() });
        assert!(eye_turned.x > eye0.x, "twisting clockwise moves the eye to the right");
        let (_, centre_pushed) = at(CameraMotion { pan_x: 1.0, ..Default::default() });
        assert!(centre_pushed.x > centre0.x + 5.0, "pushing right moves the part right");
        let (eye_zoomed, _) = at(CameraMotion { zoom: 1.0, ..Default::default() });
        assert!(eye_zoomed.length() < eye0.length(), "zooming in comes closer");
        let (_, centre_up) = at(CameraMotion { pan_y: 1.0, ..Default::default() });
        assert!(centre_up.y < centre0.y - 5.0, "lifting moves the part up (smaller y on screen)");
        let extreme = at(CameraMotion { orbit_h: 50.0, orbit_v: 50.0, pan_x: 50.0, pan_y: 50.0, zoom: 50.0 });
        assert!(extreme.0.is_finite() && extreme.1.x.is_finite(), "a huge deflection stays bounded");
        assert!(!part_faces().is_empty());
    }

    #[test]
    fn every_axis_has_an_arrow_and_text() {
        for axis in Axis::ALL {
            assert!(arrow_path(axis).len() >= 2);
            assert!(!axis_name(axis).is_empty() && !prompt(axis).is_empty());
        }
    }
}
