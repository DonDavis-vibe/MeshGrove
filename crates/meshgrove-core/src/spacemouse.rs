//! 3D mouse (3Dconnexion SpaceMouse and look-alikes, also DIY builds): the HID report format,
//! telling which HID devices are 3D mice, turning the six raw axes into camera movements, and
//! the calibration that finds out which raw axis is which. No device access here, so all of it
//! is testable; the app reads the devices.

use serde::{Deserialize, Serialize};

/// The six movements of the knob as the user sees them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// Push the knob right (+) or left (−).
    PushLr,
    /// Push it forward, away from you (+) or back (−).
    PushFb,
    /// Lift it up (+) or press it down (−).
    Lift,
    /// Tilt it forward (+) or back (−).
    TiltFb,
    /// Tilt it right (+) or left (−).
    TiltLr,
    /// Twist it clockwise seen from above (+) or counter-clockwise (−).
    Twist,
}

impl Axis {
    pub const ALL: [Axis; 6] = [Axis::PushLr, Axis::PushFb, Axis::Lift, Axis::TiltFb, Axis::TiltLr, Axis::Twist];

    pub fn index(self) -> usize {
        Axis::ALL.iter().position(|a| *a == self).unwrap_or(0)
    }
}

/// Which raw axis (0..6: X, Y, Z, rotation X, Y, Z of the HID report) a movement is, and its
/// sign: the value is multiplied by `sign` to make the positive direction of [`Axis`] positive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxisCal {
    pub raw: u8,
    pub sign: i8,
}

/// What a camera movement of the viewer is driven by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AxisMapping {
    /// Turn the model around the vertical axis.
    pub orbit_h: Option<Axis>,
    /// Look from higher or lower.
    pub orbit_v: Option<Axis>,
    pub pan_x: Option<Axis>,
    pub pan_y: Option<Axis>,
    pub zoom: Option<Axis>,
}

impl AxisMapping {
    /// Twisting the knob turns the model, tilting it forward and back looks from above or below.
    pub fn twist_orbits() -> AxisMapping {
        AxisMapping {
            orbit_h: Some(Axis::Twist),
            orbit_v: Some(Axis::TiltFb),
            pan_x: Some(Axis::PushLr),
            pan_y: Some(Axis::Lift),
            zoom: Some(Axis::PushFb),
        }
    }

    /// Tilting the knob right and left turns the model (twisting does nothing).
    pub fn tilt_orbits() -> AxisMapping {
        AxisMapping { orbit_h: Some(Axis::TiltLr), ..AxisMapping::twist_orbits() }
    }
}

impl Default for AxisMapping {
    fn default() -> Self {
        AxisMapping::twist_orbits()
    }
}

/// How the viewer's camera moves, each in −1…1 of full speed.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CameraMotion {
    pub orbit_h: f32,
    pub orbit_v: f32,
    pub pan_x: f32,
    pub pan_y: f32,
    pub zoom: f32,
}

impl CameraMotion {
    pub fn is_still(&self) -> bool {
        *self == CameraMotion::default()
    }
}

/// The settings of the 3D mouse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpaceMouseConfig {
    /// Navigate with the 3D mouse.
    pub enabled: bool,
    /// Overall speed.
    pub speed: f32,
    /// Speed per movement, in the order of [`Axis::ALL`].
    pub sensitivity: [f32; 6],
    pub invert: [bool; 6],
    /// Part of the travel that does nothing (0…0.5).
    pub deadzone: f32,
    /// Only the strongest movement counts at a time.
    pub dominant: bool,
    pub mapping: AxisMapping,
    /// From the calibration; `None`: the usual layout of the HID report.
    pub calibration: Option<[AxisCal; 6]>,
}

impl Default for SpaceMouseConfig {
    fn default() -> Self {
        SpaceMouseConfig {
            enabled: true,
            speed: 1.0,
            sensitivity: [1.0; 6],
            invert: [false; 6],
            deadzone: 0.05,
            dominant: false,
            mapping: AxisMapping::default(),
            calibration: None,
        }
    }
}

/// Speed range of the sliders.
pub const SPEED_RANGE: std::ops::RangeInclusive<f32> = 0.1..=3.0;
/// Range of the deadzone.
pub const DEADZONE_RANGE: std::ops::RangeInclusive<f32> = 0.0..=0.4;

/// The layout of the HID reports of 3Dconnexion devices: X right, Y towards you, Z down, and the
/// rotations about them.
pub fn default_calibration() -> [AxisCal; 6] {
    let cal = |raw, sign| AxisCal { raw, sign };
    [cal(0, 1), cal(1, -1), cal(2, -1), cal(3, -1), cal(4, -1), cal(5, 1)]
}

impl SpaceMouseConfig {
    /// The movements of the knob (−1…1, in the order of [`Axis::ALL`]) from the raw axes.
    pub fn physical(&self, raw: [f32; 6]) -> [f32; 6] {
        let cal = self.calibration.unwrap_or_else(default_calibration);
        let mut out = [0.0; 6];
        for (o, c) in out.iter_mut().zip(cal) {
            *o = raw.get(usize::from(c.raw)).copied().unwrap_or(0.0) * f32::from(c.sign);
        }
        out
    }

    /// Movements after deadzone, the strongest-only option, inversion and sensitivity.
    pub fn process(&self, raw: [f32; 6]) -> [f32; 6] {
        let mut v = self.physical(raw);
        let dz = self.deadzone.clamp(0.0, 0.9);
        for x in &mut v {
            *x = if x.abs() <= dz { 0.0 } else { x.signum() * (x.abs() - dz) / (1.0 - dz) };
        }
        if self.dominant {
            let top = v.iter().map(|x| x.abs()).fold(0.0, f32::max);
            for x in &mut v {
                if x.abs() < top {
                    *x = 0.0;
                }
            }
        }
        for (i, x) in v.iter_mut().enumerate() {
            let sign = if self.invert[i] { -1.0 } else { 1.0 };
            *x = (*x * sign * self.sensitivity[i] * self.speed).clamp(-3.0, 3.0);
        }
        v
    }

    /// The camera movement for processed axis values.
    pub fn camera(&self, processed: [f32; 6]) -> CameraMotion {
        let pick = |a: Option<Axis>| a.map_or(0.0, |a| processed[a.index()]);
        let m = &self.mapping;
        CameraMotion {
            orbit_h: pick(m.orbit_h),
            orbit_v: pick(m.orbit_v),
            pan_x: pick(m.pan_x),
            pan_y: pick(m.pan_y),
            zoom: pick(m.zoom),
        }
    }
}

// ---- HID ----

/// Full travel of a SpaceNavigator's axis in its reports.
const FULL_TRAVEL: f32 = 350.0;

/// What a report said.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Report {
    /// New values of the first three raw axes (and of the others, when the device sends all six
    /// in one report).
    Translation([f32; 3], Option<[f32; 3]>),
    Rotation([f32; 3]),
    /// The buttons pressed now, one bit each.
    Buttons(u32),
}

/// Reads one report of a 3D mouse (with its report id as first byte, as hidraw delivers it):
/// id 1 translation, id 2 rotation, id 3 buttons. Devices that send all six axes in one report
/// 1 (12 bytes of data) are understood too.
pub fn parse_report(report: &[u8]) -> Option<Report> {
    let axes = |bytes: &[u8]| -> Option<[f32; 3]> {
        let v = |i: usize| {
            let raw = i16::from_le_bytes([*bytes.get(2 * i)?, *bytes.get(2 * i + 1)?]);
            Some((f32::from(raw) / FULL_TRAVEL).clamp(-1.0, 1.0))
        };
        Some([v(0)?, v(1)?, v(2)?])
    };
    let (id, data) = report.split_first()?;
    match id {
        1 => Some(Report::Translation(axes(data)?, data.get(6..).and_then(axes))),
        2 => Some(Report::Rotation(axes(data)?)),
        3 => {
            let mut bits = 0u32;
            for (i, byte) in data.iter().take(4).enumerate() {
                bits |= u32::from(*byte) << (8 * i);
            }
            Some(Report::Buttons(bits))
        }
        _ => None,
    }
}

/// Whether a HID report descriptor describes a multi-axis controller (usage page 1 "Generic
/// Desktop", usage 8) as an application collection: what 3Dconnexion devices and the DIY builds
/// that copy them look like.
pub fn is_multi_axis_controller(descriptor: &[u8]) -> bool {
    let (mut page, mut usage) = (0u32, None);
    let mut i = 0;
    while i < descriptor.len() {
        let prefix = descriptor[i];
        if prefix == 0xFE {
            // Long item: its data length is the next byte.
            i += 3 + descriptor.get(i + 1).map_or(0, |n| usize::from(*n));
            continue;
        }
        let size = match prefix & 3 {
            3 => 4,
            n => usize::from(n),
        };
        let mut value = 0u32;
        for (k, byte) in descriptor.iter().skip(i + 1).take(size).enumerate() {
            value |= u32::from(*byte) << (8 * k);
        }
        let (kind, tag) = ((prefix >> 2) & 3, prefix >> 4);
        match (kind, tag) {
            (1, 0) => page = value,        // global: usage page
            (2, 0) => usage = Some(value), // local: usage
            (0, 0xA) if value == 1 => {
                if page == 1 && usage == Some(8) {
                    return true;
                }
                usage = None;
            }
            (0, _) => usage = None, // a main item uses up the local usages
            _ => {}
        }
        i += 1 + size;
    }
    false
}

// ---- calibration ----

/// The movements the calibration asks for, with what to say about each, in the order of
/// [`Axis::ALL`]; the positive direction of each axis.
pub const CALIBRATION_STEPS: usize = 6;

/// What the calibration wants next, after a sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationNote {
    /// Move the knob as asked.
    Waiting,
    /// A movement was found and is being held.
    Holding,
    /// More than one direction at once.
    Mixed,
    /// This raw axis was already taken by an earlier step.
    Taken,
    /// The step was done; let go of the knob.
    Release,
    /// All six steps are done.
    Done,
}

/// Asks the user to move the knob in each of the six positive directions and holds on to which
/// raw axis moved. Feed it a sample per frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Calibrator {
    step: usize,
    held: Option<(usize, i8)>,
    hold: f32,
    need_release: bool,
    found: Vec<AxisCal>,
}

impl Default for Calibrator {
    fn default() -> Self {
        Calibrator::new()
    }
}

/// A movement counts from this deflection...
const PUSHED: f32 = 0.6;
/// ...while every other axis stays below this.
const QUIET: f32 = 0.3;
/// The knob counts as let go below this.
const RELEASED: f32 = 0.15;
/// How long a movement is held before it counts.
const HOLD_SECONDS: f32 = 0.8;

impl Calibrator {
    pub fn new() -> Calibrator {
        Calibrator { step: 0, held: None, hold: 0.0, need_release: false, found: Vec::new() }
    }

    /// The movement asked for now; `None` when done.
    pub fn current(&self) -> Option<Axis> {
        Axis::ALL.get(self.step).copied()
    }

    pub fn step(&self) -> usize {
        self.step
    }

    /// How far the current movement is held, 0…1.
    pub fn progress(&self) -> f32 {
        (self.hold / HOLD_SECONDS).clamp(0.0, 1.0)
    }

    /// The result once all six steps are done.
    pub fn result(&self) -> Option<[AxisCal; 6]> {
        self.found.as_slice().try_into().ok()
    }

    pub fn feed(&mut self, raw: [f32; 6], dt: f32) -> CalibrationNote {
        if self.step >= CALIBRATION_STEPS {
            return CalibrationNote::Done;
        }
        let strongest = (0..6).max_by(|&a, &b| raw[a].abs().total_cmp(&raw[b].abs())).unwrap_or(0);
        let top = raw[strongest].abs();
        if self.need_release {
            if top < RELEASED {
                self.need_release = false;
            } else {
                return CalibrationNote::Release;
            }
        }
        if top < PUSHED {
            self.held = None;
            self.hold = 0.0;
            return CalibrationNote::Waiting;
        }
        if (0..6).any(|a| a != strongest && raw[a].abs() >= QUIET) {
            self.held = None;
            self.hold = 0.0;
            return CalibrationNote::Mixed;
        }
        if self.found.iter().any(|c| usize::from(c.raw) == strongest) {
            self.held = None;
            self.hold = 0.0;
            return CalibrationNote::Taken;
        }
        let candidate = (strongest, if raw[strongest] > 0.0 { 1 } else { -1 });
        if self.held == Some(candidate) {
            self.hold += dt;
        } else {
            self.held = Some(candidate);
            self.hold = 0.0;
        }
        if self.hold < HOLD_SECONDS {
            return CalibrationNote::Holding;
        }
        self.found.push(AxisCal { raw: candidate.0 as u8, sign: candidate.1 });
        self.step += 1;
        self.held = None;
        self.hold = 0.0;
        self.need_release = true;
        if self.step >= CALIBRATION_STEPS { CalibrationNote::Done } else { CalibrationNote::Release }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_report(id: u8, values: &[i16]) -> Vec<u8> {
        let mut r = vec![id];
        for v in values {
            r.extend(v.to_le_bytes());
        }
        r
    }

    #[test]
    fn reports_are_split_or_combined() {
        assert_eq!(
            parse_report(&raw_report(1, &[350, -350, 700])),
            Some(Report::Translation([1.0, -1.0, 1.0], None)),
            "clamped to full travel"
        );
        assert_eq!(parse_report(&raw_report(2, &[175, 0, -175])), Some(Report::Rotation([0.5, 0.0, -0.5])));
        assert_eq!(
            parse_report(&raw_report(1, &[0, 0, 0, 350, 0, 0])),
            Some(Report::Translation([0.0; 3], Some([1.0, 0.0, 0.0]))),
            "all six axes in one report"
        );
        assert_eq!(parse_report(&[3, 0b101, 0]), Some(Report::Buttons(5)));
        assert_eq!(parse_report(&[9, 1, 2]), None);
        assert_eq!(parse_report(&[1, 1]), None, "too short");
        assert_eq!(parse_report(&[]), None);
    }

    #[test]
    fn multi_axis_controllers_are_told_from_other_devices() {
        // Usage page 1, usage 8, collection (application), report id 1, ...
        let space_mouse = [0x05, 0x01, 0x09, 0x08, 0xA1, 0x01, 0x85, 0x01, 0x09, 0x30, 0x81, 0x02, 0xC0];
        assert!(is_multi_axis_controller(&space_mouse));
        // A keyboard: usage page 1, usage 6.
        let keyboard = [0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, 0x01, 0xC0];
        assert!(!is_multi_axis_controller(&keyboard));
        // A mouse after a keyboard in one descriptor, then the 3D mouse.
        let mut both = keyboard.to_vec();
        both.extend(space_mouse);
        assert!(is_multi_axis_controller(&both));
        // Usage 8 without the collection belongs to something else.
        assert!(!is_multi_axis_controller(&[0x05, 0x01, 0x09, 0x08, 0x81, 0x02]));
        assert!(!is_multi_axis_controller(&[]));
        assert!(!is_multi_axis_controller(&[0x05]), "cut short");
    }

    #[test]
    fn the_hid_layout_gives_the_knobs_directions() {
        let config = SpaceMouseConfig::default();
        // The device reports Y towards you, Z down, tilts with the same handedness.
        let p = config.physical([0.5, -0.5, -0.5, -0.5, -0.5, 0.5]);
        assert_eq!(p, [0.5, 0.5, 0.5, 0.5, 0.5, 0.5], "push right/forward, lift, tilt forward/right, twist clockwise");
    }

    #[test]
    fn processing_applies_deadzone_dominance_inversion_and_speed() {
        let mut config = SpaceMouseConfig { deadzone: 0.1, ..SpaceMouseConfig::default() };
        let raw = [0.05, 0.0, 0.0, 0.0, 0.0, 0.0];
        assert_eq!(config.process(raw), [0.0; 6], "inside the deadzone");
        let p = config.process([1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert!((p[0] - 1.0).abs() < 1e-6, "full travel stays full");
        config.speed = 2.0;
        config.invert[0] = true;
        config.sensitivity[0] = 0.5;
        assert!((config.process([1.0, 0.0, 0.0, 0.0, 0.0, 0.0])[0] + 1.0).abs() < 1e-6);
        config.dominant = true;
        let p = config.process([0.9, 0.0, 0.0, 0.0, 0.0, 0.5]);
        assert!(p[0] != 0.0 && p[5] == 0.0, "only the strongest movement");
    }

    #[test]
    fn mapping_presets_choose_what_drives_the_camera() {
        let twist = SpaceMouseConfig::default();
        let tilt = SpaceMouseConfig { mapping: AxisMapping::tilt_orbits(), ..SpaceMouseConfig::default() };
        let mut v = [0.0; 6];
        v[Axis::Twist.index()] = 1.0;
        assert_eq!(twist.camera(v).orbit_h, 1.0);
        assert_eq!(tilt.camera(v).orbit_h, 0.0, "twisting does nothing when tilting orbits");
        let mut v = [0.0; 6];
        v[Axis::TiltLr.index()] = 0.5;
        assert_eq!(tilt.camera(v).orbit_h, 0.5);
        assert_eq!(twist.camera(v), CameraMotion::default());
        assert!(CameraMotion::default().is_still());
        assert_eq!(twist.camera(v).orbit_v, 0.0);
    }

    #[test]
    fn config_round_trips_and_old_files_get_defaults() {
        let mut config = SpaceMouseConfig {
            mapping: AxisMapping::tilt_orbits(),
            calibration: Some(default_calibration()),
            ..SpaceMouseConfig::default()
        };
        config.invert[2] = true;
        let text = toml::to_string_pretty(&config).unwrap();
        assert_eq!(toml::from_str::<SpaceMouseConfig>(&text).unwrap(), config);
        assert_eq!(toml::from_str::<SpaceMouseConfig>("speed = 2.0\n").unwrap().speed, 2.0);
    }

    /// A user who moves the knob as asked, holding each movement long enough.
    fn do_step(c: &mut Calibrator, raw_axis: usize, value: f32) -> CalibrationNote {
        let mut sample = [0.0; 6];
        sample[raw_axis] = value;
        let mut note = CalibrationNote::Waiting;
        for _ in 0..70 {
            note = c.feed(sample, 0.016);
            if matches!(note, CalibrationNote::Release | CalibrationNote::Done) {
                break;
            }
        }
        note
    }

    #[test]
    fn calibration_learns_swapped_and_inverted_axes() {
        let mut c = Calibrator::new();
        // A device whose raw axes come in a different order: push right = raw 2 negative, ...
        let layout = [(2usize, -1.0f32), (0, 1.0), (1, 1.0), (5, 1.0), (4, -1.0), (3, 1.0)];
        for (i, (axis, value)) in layout.iter().enumerate() {
            assert_eq!(c.current(), Some(Axis::ALL[i]));
            let note = do_step(&mut c, *axis, *value);
            assert_eq!(note, if i == 5 { CalibrationNote::Done } else { CalibrationNote::Release });
            // Let go before the next movement.
            c.feed([0.0; 6], 0.016);
        }
        let result = c.result().unwrap();
        assert_eq!(result[0], AxisCal { raw: 2, sign: -1 });
        assert_eq!(result[4], AxisCal { raw: 4, sign: -1 });
        assert_eq!(result[5], AxisCal { raw: 3, sign: 1 });
        let config = SpaceMouseConfig { calibration: Some(result), ..SpaceMouseConfig::default() };
        let mut raw = [0.0; 6];
        raw[2] = -0.7;
        assert_eq!(config.physical(raw)[Axis::PushLr.index()], 0.7);
    }

    #[test]
    fn calibration_rejects_diagonal_pushes_taken_axes_and_short_holds() {
        let mut c = Calibrator::new();
        assert_eq!(c.feed([0.9, 0.5, 0.0, 0.0, 0.0, 0.0], 0.016), CalibrationNote::Mixed);
        assert_eq!(c.feed([0.3, 0.0, 0.0, 0.0, 0.0, 0.0], 0.016), CalibrationNote::Waiting, "too weak");
        // A short push does not count.
        for _ in 0..10 {
            c.feed([0.9, 0.0, 0.0, 0.0, 0.0, 0.0], 0.016);
        }
        assert_eq!(c.step(), 0);
        assert_eq!(c.feed([0.0; 6], 0.016), CalibrationNote::Waiting);
        assert_eq!(do_step(&mut c, 0, 0.9), CalibrationNote::Release);
        // Still pressed: the user has to let go first.
        assert_eq!(c.feed([0.9, 0.0, 0.0, 0.0, 0.0, 0.0], 0.016), CalibrationNote::Release);
        c.feed([0.0; 6], 0.016);
        // The same raw axis again is refused.
        assert_eq!(c.feed([0.9, 0.0, 0.0, 0.0, 0.0, 0.0], 0.016), CalibrationNote::Taken);
        assert_eq!(c.step(), 1);
        assert!(c.result().is_none());
    }
}
