//! The user's printers, read from the slicers, and whether a model fits on them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Printable volume in millimetres.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bed {
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// One of the user's printers: its build volume and what G-code calls it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Printer {
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nozzle: Option<f32>,
    /// Materials it prints, as the user wrote them (PLA, PETG …).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub materials: Vec<String>,
    /// Further names G-code may give it: the slicer profiles it was set up as.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gcode_names: Vec<String>,
}

impl Printer {
    pub fn bed(&self) -> Bed {
        Bed { name: self.name.clone(), x: self.x, y: self.y, z: self.z }
    }

    /// Whether G-code sliced for `printer` (the name the slicer wrote) was made for this printer.
    pub fn matches_gcode(&self, printer: &str) -> bool {
        same_name(printer, &self.name) || self.gcode_names.iter().any(|n| same_name(printer, n))
    }
}

impl From<Bed> for Printer {
    fn from(bed: Bed) -> Printer {
        Printer { name: bed.name, x: bed.x, y: bed.y, z: bed.z, nozzle: None, materials: Vec::new(), gcode_names: Vec::new() }
    }
}

/// A printer found in a slicer's profiles.
#[derive(Debug, Clone, PartialEq)]
pub struct DetectedPrinter {
    pub printer: Printer,
    /// The slicer it was found in, e.g. "OrcaSlicer".
    pub slicer: &'static str,
    /// The printer currently selected in that slicer.
    pub selected: bool,
}

/// Orca-family slicers keep the same profile format: (config folder, settings file, name).
const SLICERS: &[(&str, &str, &str)] = &[
    ("AnycubicSlicerNext", "AnycubicSlicerNext.conf", "Anycubic Slicer Next"),
    ("OrcaSlicer", "OrcaSlicer.conf", "OrcaSlicer"),
    ("BambuStudio", "BambuStudio.conf", "Bambu Studio"),
];

impl Bed {
    /// The printer selected in Anycubic Slicer Next, OrcaSlicer or Bambu Studio.
    pub fn detect() -> Option<Bed> {
        let config = dirs::config_dir()?;
        SLICERS.iter().find_map(|(dir, conf, _)| from_slicer_config(&config.join(dir), conf))
    }
}

/// Every printer set up in Anycubic Slicer Next, OrcaSlicer, Bambu Studio and PrusaSlicer: the
/// selected ones, the user's own profiles and the printer models picked in the setup wizard.
/// Selected printers come first; a printer several slicers know is listed once.
pub fn detect_all() -> Vec<DetectedPrinter> {
    dirs::config_dir().map(|config| detect_in(&config)).unwrap_or_default()
}

fn detect_in(config: &Path) -> Vec<DetectedPrinter> {
    let mut found: Vec<DetectedPrinter> = Vec::new();
    for (dir, conf, slicer) in SLICERS {
        found.extend(orca_printers(&config.join(dir), conf, slicer));
    }
    found.extend(prusa_printers(&config.join("PrusaSlicer")));
    found.sort_by_key(|d| !d.selected);
    let mut out: Vec<DetectedPrinter> = Vec::new();
    for d in found {
        match out.iter_mut().find(|o| same_name(&o.printer.name, &d.printer.name)) {
            Some(known) => {
                for name in d.printer.gcode_names {
                    if !known.printer.gcode_names.contains(&name) {
                        known.printer.gcode_names.push(name);
                    }
                }
            }
            None => out.push(d),
        }
    }
    out
}

/// The machine profiles of an Orca-family slicer by name, and whether each is the user's own.
fn machine_profiles(dir: &Path) -> HashMap<String, (PathBuf, bool)> {
    let mut profiles = HashMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "json") && p.parent().is_some_and(|d| d.ends_with("machine")) {
                let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let own = p.strip_prefix(dir).is_ok_and(|rel| rel.starts_with("user"));
                profiles.entry(stem).or_insert((p, own));
            }
        }
    }
    profiles
}

/// Resolves machine profile `name`, following `inherits` until the bed size is known.
fn resolve(profiles: &HashMap<String, (PathBuf, bool)>, name: &str) -> Option<Printer> {
    let (mut area, mut height, mut model, mut nozzle) = (None, None, None, None);
    let mut current = name.to_owned();
    for _ in 0..16 {
        let profile: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&profiles.get(&current)?.0).ok()?).ok()?;
        area = area.or_else(|| profile.get("printable_area").and_then(parse_area));
        height = height.or_else(|| profile.get("printable_height").and_then(number));
        model = model.or_else(|| {
            profile.get("printer_model").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(str::to_owned)
        });
        nozzle = nozzle.or_else(|| profile.get("nozzle_diameter").and_then(first_number));
        if area.is_some() && height.is_some() && model.is_some() && nozzle.is_some() {
            break;
        }
        match profile.get("inherits").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            Some(parent) => current = parent.to_owned(),
            None => break,
        }
    }
    let (x, y) = area?;
    let name_ = model.unwrap_or_else(|| name.to_owned());
    let gcode_names = if name_ == name { Vec::new() } else { vec![name.to_owned()] };
    Some(Printer { name: name_, x, y, z: height?, nozzle, materials: Vec::new(), gcode_names })
}

/// Reads `presets.machine` from the slicer's settings and resolves that machine profile.
fn from_slicer_config(dir: &Path, conf: &str) -> Option<Bed> {
    let settings: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join(conf)).ok()?).ok()?;
    let selected = settings.pointer("/presets/machine")?.as_str()?.to_owned();
    resolve(&machine_profiles(dir), &selected).map(|p| p.bed())
}

/// The selected printer, the user's own machine profiles and the models picked in the wizard
/// (`models: [{model, nozzle_diameter}]`, profiles named "<model> <nozzle> nozzle").
fn orca_printers(dir: &Path, conf: &str, slicer: &'static str) -> Vec<DetectedPrinter> {
    let Some(settings) = std::fs::read_to_string(dir.join(conf))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
    else {
        return Vec::new();
    };
    let profiles = machine_profiles(dir);
    let selected = settings.pointer("/presets/machine").and_then(|v| v.as_str()).map(str::to_owned);
    let mut names: Vec<String> = selected.iter().cloned().collect();
    let mut own: Vec<&String> = profiles.iter().filter(|(_, (_, own))| *own).map(|(n, _)| n).collect();
    own.sort();
    names.extend(own.into_iter().cloned());
    for m in settings.get("models").and_then(|v| v.as_array()).into_iter().flatten() {
        let (Some(model), Some(nozzle)) = (m.get("model").and_then(|v| v.as_str()), m.get("nozzle_diameter")) else {
            continue;
        };
        let nozzle = nozzle.as_str().map(str::to_owned).unwrap_or_else(|| nozzle.to_string());
        names.extend(nozzle.split(';').map(|n| format!("{model} {} nozzle", n.trim())));
    }
    let mut out = Vec::new();
    for name in names {
        if let Some(printer) = resolve(&profiles, &name) {
            out.push(DetectedPrinter { printer, slicer, selected: selected.as_deref() == Some(name.as_str()) });
        }
    }
    out
}

/// PrusaSlicer's own printer profiles (`printer/*.ini`) and the selected one (`PrusaSlicer.ini`).
fn prusa_printers(dir: &Path) -> Vec<DetectedPrinter> {
    let selected = std::fs::read_to_string(dir.join("PrusaSlicer.ini")).ok().and_then(|text| {
        let mut in_presets = false;
        text.lines().find_map(|line| {
            let line = line.trim();
            if line.starts_with('[') {
                in_presets = line == "[presets]";
                return None;
            }
            let (key, value) = line.split_once('=')?;
            (in_presets && key.trim() == "printer").then(|| value.trim().to_owned())
        })
    });
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir.join("printer"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ini"))
        .collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|path| {
            let name = path.file_stem()?.to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&path).ok()?;
            let value = |key: &str| {
                text.lines().find_map(|l| {
                    let (k, v) = l.split_once('=')?;
                    (k.trim() == key).then(|| v.trim().to_owned())
                })
            };
            let shape: Vec<serde_json::Value> =
                value("bed_shape")?.split(',').map(|p| serde_json::Value::String(p.trim().to_owned())).collect();
            let (x, y) = parse_area(&serde_json::Value::Array(shape))?;
            let z = value("max_print_height")?.parse().ok()?;
            let nozzle = value("nozzle_diameter").and_then(|n| n.split(',').next()?.trim().parse().ok());
            let selected = selected.as_deref() == Some(name.as_str());
            let printer = Printer { name, x, y, z, nozzle, materials: Vec::new(), gcode_names: Vec::new() };
            Some(DetectedPrinter { printer, slicer: "PrusaSlicer", selected })
        })
        .collect()
}

/// The printer called `name` among `printers`, compared like G-code names (exact names win).
pub fn find<'a>(name: &str, printers: &'a [Printer]) -> Option<&'a Printer> {
    printers
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(name.trim()))
        .or_else(|| printers.iter().find(|p| same_name(name, &p.name)))
}

/// How a model of `size` fits on each printer.
pub fn fits_all(size: [f32; 3], printers: &[Printer]) -> Vec<(&Printer, Fit)> {
    printers.iter().map(|p| (p, fit(size, &p.bed()))).collect()
}

/// The best a model of `size` does on any of `printers`: fits straight on one, else fits turned
/// on one, else too big (measured against the first printer, the main one).
pub fn best_fit(size: [f32; 3], printers: &[Printer]) -> Option<(Fit, &Printer)> {
    let all = fits_all(size, printers);
    all.iter()
        .find(|(_, f)| *f == Fit::Fits)
        .or_else(|| all.iter().find(|(_, f)| matches!(f, Fit::Rotated(_))))
        .or_else(|| all.first())
        .map(|&(p, f)| (f, p))
}

/// The printer a G-code file was sliced for (`printer` is the name the slicer wrote).
pub fn printer_for_gcode<'a>(printer: &str, printers: &'a [Printer]) -> Option<&'a Printer> {
    printers.iter().find(|p| p.matches_gcode(printer))
}

/// `["0x0", "250x0", "250x250", "0x250"]` → bounding size (250, 250).
fn parse_area(v: &serde_json::Value) -> Option<(f32, f32)> {
    let points: Vec<(f32, f32)> = v
        .as_array()?
        .iter()
        .filter_map(|p| {
            let (x, y) = p.as_str()?.split_once('x')?;
            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
        })
        .collect();
    let (min_x, max_x) = points.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.0), hi.max(p.0)));
    let (min_y, max_y) = points.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.1), hi.max(p.1)));
    (points.len() >= 3).then_some((max_x - min_x, max_y - min_y))
}

fn number(v: &serde_json::Value) -> Option<f32> {
    v.as_f64().map(|n| n as f32).or_else(|| v.as_str()?.trim().parse().ok())
}

/// `["0.4"]`, `"0.4"` or `0.4` → 0.4.
fn first_number(v: &serde_json::Value) -> Option<f32> {
    match v.as_array() {
        Some(items) => items.first().and_then(number),
        None => number(v),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Fit {
    /// Fits as it is.
    Fits,
    /// Fits after turning it on the bed by this many degrees.
    Rotated(f32),
    /// Too big even when turned; how far it sticks out (mm) in X, Y and Z as it is.
    TooBig([f32; 3]),
}

/// Whether a model of `size` (mm, as oriented in the file) fits on `bed`, turning it around
/// the vertical axis if needed (to 90°, or diagonally).
pub fn fit(size: [f32; 3], bed: &Bed) -> Fit {
    const EPS: f32 = 0.01;
    let [w, h, z] = size;
    let over = [(w - bed.x).max(0.0), (h - bed.y).max(0.0), (z - bed.z).max(0.0)];
    if z > bed.z + EPS {
        return Fit::TooBig(over);
    }
    if w <= bed.x + EPS && h <= bed.y + EPS {
        return Fit::Fits;
    }
    if h <= bed.x + EPS && w <= bed.y + EPS {
        return Fit::Rotated(90.0);
    }
    // Diagonal placement: a w×h rectangle turned by θ needs (w cosθ + h sinθ) × (w sinθ + h cosθ).
    for step in 1..180 {
        let theta = (step as f32 * 0.5).to_radians();
        let (s, c) = theta.sin_cos();
        if w * c + h * s <= bed.x + EPS && w * s + h * c <= bed.y + EPS {
            return Fit::Rotated(step as f32 * 0.5);
        }
    }
    Fit::TooBig(over)
}

/// Whether G-code sliced for `printer` (the name the slicer wrote) was made for `bed`'s printer.
pub fn same_printer(printer: &str, bed: &Bed) -> bool {
    same_name(printer, &bed.name)
}

/// Printer names compared loosely: profile names add nozzle sizes and the like, so either name
/// may contain the other.
fn same_name(a: &str, b: &str) -> bool {
    let simple = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
    let (a, b) = (simple(a), simple(b));
    !a.is_empty() && !b.is_empty() && (a.contains(&b) || b.contains(&a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printer_names_are_compared_loosely() {
        let bed = Bed { name: "Creality Ender-3 V3 SE".into(), x: 220.0, y: 220.0, z: 250.0 };
        assert!(same_printer("Creality Ender-3 V3 SE", &bed));
        assert!(same_printer("Creality Ender-3 V3 SE 0.4 nozzle", &bed));
        assert!(same_printer("creality ender3 v3 se", &bed));
        assert!(!same_printer("Anycubic Kobra S1", &bed));
        assert!(!same_printer("", &bed));
    }

    fn s1() -> Bed {
        Bed { name: "Anycubic Kobra S1".into(), x: 250.0, y: 250.0, z: 250.0 }
    }

    #[test]
    fn fits_rotated_diagonal_and_too_big() {
        assert_eq!(fit([200.0, 100.0, 50.0], &s1()), Fit::Fits);
        let narrow = Bed { name: "x".into(), x: 300.0, y: 200.0, z: 250.0 };
        assert_eq!(fit([150.0, 280.0, 10.0], &narrow), Fit::Rotated(90.0));
        // A 320 mm rod does not fit straight on 250×250 but does along the diagonal (353 mm).
        assert!(matches!(fit([320.0, 10.0, 10.0], &s1()), Fit::Rotated(a) if a > 30.0 && a < 60.0));
        assert_eq!(fit([300.0, 300.0, 10.0], &s1()), Fit::TooBig([50.0, 50.0, 0.0]));
        assert_eq!(fit([100.0, 100.0, 260.0], &s1()), Fit::TooBig([0.0, 0.0, 10.0]));
    }

    #[test]
    fn reads_bed_through_profile_inheritance() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("user/1/machine")).unwrap();
        std::fs::create_dir_all(root.join("system/Anycubic/machine")).unwrap();
        std::fs::write(root.join("Slicer.conf"), r#"{"presets": {"machine": "Kobra S1 hard"}}"#).unwrap();
        std::fs::write(
            root.join("user/1/machine/Kobra S1 hard.json"),
            r#"{"name": "Kobra S1 hard", "inherits": "Kobra S1 0.4"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("system/Anycubic/machine/Kobra S1 0.4.json"),
            r#"{"printable_area": ["0x0", "250x0", "250x250", "0x250"], "printable_height": "250", "printer_model": "Anycubic Kobra S1"}"#,
        )
        .unwrap();
        assert_eq!(
            from_slicer_config(root, "Slicer.conf"),
            Some(Bed { name: "Anycubic Kobra S1".into(), x: 250.0, y: 250.0, z: 250.0 })
        );
        assert_eq!(from_slicer_config(root, "missing.conf"), None);
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn finds_every_printer_the_slicers_know_once() {
        let dir = tempfile::tempdir().unwrap();
        let anycubic = dir.path().join("AnycubicSlicerNext");
        write(
            &anycubic.join("AnycubicSlicerNext.conf"),
            r#"{"presets": {"machine": "Kobra S1 hard"},
                "models": [{"model": "Anycubic Kobra S1", "nozzle_diameter": "0.4", "vendor": "Anycubic"},
                           {"model": "Anycubic Kobra 3", "nozzle_diameter": "0.4", "vendor": "Anycubic"}]}"#,
        );
        write(&anycubic.join("user/1/machine/Kobra S1 hard.json"), r#"{"inherits": "Anycubic Kobra S1 0.4 nozzle"}"#);
        write(
            &anycubic.join("system/Anycubic/machine/Anycubic Kobra S1 0.4 nozzle.json"),
            r#"{"printable_area": ["0x0", "250x0", "250x250", "0x250"], "printable_height": "250",
                "printer_model": "Anycubic Kobra S1", "nozzle_diameter": ["0.4"]}"#,
        );
        write(
            &anycubic.join("system/Anycubic/machine/Anycubic Kobra 3 0.4 nozzle.json"),
            r#"{"printable_area": ["0x0", "255x0", "255x255", "0x255"], "printable_height": 260,
                "printer_model": "Anycubic Kobra 3", "nozzle_diameter": ["0.4"]}"#,
        );
        // Not set up by the user: system profiles alone are not the user's printers.
        write(
            &anycubic.join("system/Anycubic/machine/Anycubic Photon.json"),
            r#"{"printable_area": ["0x0", "1x0", "1x1"], "printable_height": 1, "printer_model": "Photon"}"#,
        );
        let orca = dir.path().join("OrcaSlicer");
        write(&orca.join("OrcaSlicer.conf"), r#"{"presets": {"machine": "Anycubic Kobra S1 0.4 nozzle"}}"#);
        write(
            &orca.join("system/Anycubic/machine/Anycubic Kobra S1 0.4 nozzle.json"),
            r#"{"printable_area": ["0x0", "250x0", "250x250", "0x250"], "printable_height": "250",
                "printer_model": "Anycubic Kobra S1"}"#,
        );
        let prusa = dir.path().join("PrusaSlicer");
        write(&prusa.join("PrusaSlicer.ini"), "[presets]\nprint = 0.2mm\nprinter = Mein MK4\n");
        write(
            &prusa.join("printer/Mein MK4.ini"),
            "bed_shape = 0x0,250x0,250x210,0x210\nmax_print_height = 220\nnozzle_diameter = 0.6,0.6\n",
        );

        let found = detect_in(dir.path());
        let names: Vec<(&str, &str, bool)> =
            found.iter().map(|d| (d.printer.name.as_str(), d.slicer, d.selected)).collect();
        assert_eq!(
            names,
            [
                ("Anycubic Kobra S1", "Anycubic Slicer Next", true),
                ("Mein MK4", "PrusaSlicer", true),
                ("Anycubic Kobra 3", "Anycubic Slicer Next", false),
            ]
        );
        let s1 = &found[0].printer;
        assert_eq!((s1.x, s1.y, s1.z, s1.nozzle), (250.0, 250.0, 250.0, Some(0.4)));
        assert!(s1.gcode_names.contains(&"Kobra S1 hard".to_owned()), "{:?}", s1.gcode_names);
        let mk4 = &found[1].printer;
        assert_eq!((mk4.x, mk4.y, mk4.z, mk4.nozzle), (250.0, 210.0, 220.0, Some(0.6)));
    }

    #[test]
    fn printers_are_found_by_name_and_by_gcode() {
        let s1 = Printer::from(s1());
        let mini = Printer {
            name: "Bambu Lab A1 mini".into(),
            x: 180.0,
            y: 180.0,
            z: 180.0,
            nozzle: None,
            materials: Vec::new(),
            gcode_names: vec!["Werkstatt-Mini".into()],
        };
        let printers = [s1, mini];
        assert_eq!(find("bambu lab a1 mini", &printers).map(|p| p.x), Some(180.0));
        assert_eq!(find("Kobra S1", &printers).map(|p| p.x), Some(250.0), "loose match");
        assert!(find("Prusa MK4", &printers).is_none());
        assert_eq!(printer_for_gcode("Werkstatt-Mini", &printers).map(|p| p.x), Some(180.0));
        assert_eq!(printer_for_gcode("Anycubic Kobra S1 0.4 nozzle", &printers).map(|p| p.x), Some(250.0));
        let fits: Vec<bool> =
            fits_all([200.0, 200.0, 10.0], &printers).iter().map(|(_, f)| !matches!(f, Fit::TooBig(_))).collect();
        assert_eq!(fits, [true, false]);
        let names = |size| best_fit(size, &printers).map(|(f, p)| (f, p.name.as_str()));
        assert_eq!(names([170.0, 170.0, 10.0]), Some((Fit::Fits, "Anycubic Kobra S1")));
        assert!(matches!(names([320.0, 10.0, 10.0]), Some((Fit::Rotated(_), "Anycubic Kobra S1"))));
        assert_eq!(names([300.0, 300.0, 10.0]), Some((Fit::TooBig([50.0, 50.0, 0.0]), "Anycubic Kobra S1")));
        assert_eq!(best_fit([1.0; 3], &[]), None);
    }
}
