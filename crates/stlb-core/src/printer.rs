//! The user's printer bed, read from the slicer, and whether a model fits on it.

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

/// Orca-family slicers keep the same profile format: (config folder, settings file).
const SLICERS: &[(&str, &str)] = &[
    ("AnycubicSlicerNext", "AnycubicSlicerNext.conf"),
    ("OrcaSlicer", "OrcaSlicer.conf"),
    ("BambuStudio", "BambuStudio.conf"),
];

impl Bed {
    /// The printer selected in Anycubic Slicer Next, OrcaSlicer or Bambu Studio.
    pub fn detect() -> Option<Bed> {
        let config = dirs::config_dir()?;
        SLICERS.iter().find_map(|(dir, conf)| from_slicer_config(&config.join(dir), conf))
    }
}

/// Reads `presets.machine` from the slicer's settings and resolves that machine profile,
/// following `inherits` until the bed size is known.
fn from_slicer_config(dir: &Path, conf: &str) -> Option<Bed> {
    let settings: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join(conf)).ok()?).ok()?;
    let selected = settings.pointer("/presets/machine")?.as_str()?.to_owned();
    let mut profiles: HashMap<String, PathBuf> = HashMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "json") && p.parent().is_some_and(|d| d.ends_with("machine")) {
                let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                profiles.entry(stem).or_insert(p);
            }
        }
    }
    let (mut area, mut height, mut model) = (None, None, None);
    let mut name = selected.clone();
    for _ in 0..16 {
        let profile: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(profiles.get(&name)?).ok()?).ok()?;
        area = area.or_else(|| profile.get("printable_area").and_then(parse_area));
        height = height.or_else(|| profile.get("printable_height").and_then(number));
        model = model.or_else(|| {
            profile.get("printer_model").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(str::to_owned)
        });
        if area.is_some() && height.is_some() {
            break;
        }
        match profile.get("inherits").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            Some(parent) => name = parent.to_owned(),
            None => break,
        }
    }
    let (x, y) = area?;
    Some(Bed { name: model.unwrap_or(selected), x, y, z: height? })
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
