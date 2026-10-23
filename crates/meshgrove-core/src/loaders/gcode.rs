//! G-code from slicers (Orca, Anycubic, Bambu, Prusa, Cura): the facts in its comments, the
//! preview pictures slicers embed, and the extruded paths, drawn when there is no picture.
//! Everything runs on the CPU, so the Dolphin thumbnailer can use it too.

use base64::Engine;
use glam::Vec3;
use image::{Rgba, RgbaImage};

/// What the slicer wrote into the file's comments.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GcodeFacts {
    /// Program and version, e.g. "OrcaSlicer 2.3.1".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slicer: Option<String>,
    /// The printer the file was sliced for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub printer: Option<String>,
    /// Print time as the slicer wrote it, e.g. "1h 6m 46s".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub print_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filament_g: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filament_m: Option<f32>,
    /// Filament materials, one per extruder used.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub materials: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer_height: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nozzle: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layers: Option<u32>,
    /// Filament cost in the currency of the slicer's filament profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f32>,
}

/// A parsed G-code file.
#[derive(Debug, Default)]
pub struct Gcode {
    pub facts: GcodeFacts,
    /// Filament colours (sRGB) the slicer named.
    pub colors: Vec<[u8; 3]>,
    /// Extruded segments in print order.
    pub paths: Vec<[Vec3; 2]>,
    /// The largest preview picture embedded in the file.
    pub preview: Option<RgbaImage>,
}

impl Gcode {
    /// Size of everything extruded on the layers (the start purge line left out), in mm.
    pub fn size_mm(&self) -> Option<[f32; 3]> {
        let (min, max) = bounds(&self.paths)?;
        Some((max - min).to_array())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PictureFormat {
    Png,
    Jpg,
    Qoi,
}

/// Reads facts, colours, the extruded paths and the largest embedded picture.
pub fn parse(bytes: &[u8]) -> Gcode {
    let mut out = Gcode::default();
    let mut machine = Machine::default();
    // Embedded pictures: the one being read, and the largest finished one.
    let mut reading: Option<(PictureFormat, u32, String)> = None;
    let mut best: Option<(u32, PictureFormat, String)> = None;
    let mut cura_seconds = None;
    let mut printer_profile = None;
    // What is extruded before the first layer starts is the start G-code's purge line at the
    // bed's edge, not part of the print; kept only if the slicer marks no layers at all.
    let mut start_paths = Vec::new();
    let mut in_layers = false;
    for raw in bytes.split(|&b| b == b'\n') {
        let line = trim_ascii(raw);
        if line.is_empty() {
            continue;
        }
        if line[0] == b';' {
            let text = String::from_utf8_lossy(&line[1..]);
            let text = text.trim();
            if let Some((format, area, data)) = &mut reading {
                if text.starts_with("thumbnail") && text.ends_with(" end") {
                    if best.as_ref().is_none_or(|(a, ..)| *area > *a) {
                        best = Some((*area, *format, std::mem::take(data)));
                    }
                    reading = None;
                } else {
                    data.push_str(text);
                }
                continue;
            }
            if let Some(start) = picture_start(text) {
                reading = Some((start.0, start.1, String::new()));
                continue;
            }
            in_layers |= text == "LAYER_CHANGE" || text == "CHANGE_LAYER" || text.starts_with("LAYER:");
            comment(text, &mut out, &mut cura_seconds, &mut printer_profile);
        } else {
            machine.command(line, if in_layers { &mut out.paths } else { &mut start_paths });
        }
    }
    if !in_layers {
        out.paths = start_paths;
    }
    if out.facts.print_time.is_none() {
        out.facts.print_time = cura_seconds.map(duration);
    }
    if out.facts.printer.is_none() {
        out.facts.printer = printer_profile;
    }
    out.preview = best.and_then(|(_, format, data)| decode_picture(format, &data));
    out
}

/// "thumbnail begin 300x300 12345", "thumbnail_JPG begin …", "thumbnail_QOI begin …":
/// the format and the picture's area.
fn picture_start(text: &str) -> Option<(PictureFormat, u32)> {
    let (head, rest) = text.split_once(" begin ")?;
    let format = match head {
        "thumbnail" | "thumbnail_PNG" => PictureFormat::Png,
        "thumbnail_JPG" => PictureFormat::Jpg,
        "thumbnail_QOI" => PictureFormat::Qoi,
        _ => return None,
    };
    let (w, h) = rest.split_whitespace().next()?.split_once('x')?;
    Some((format, w.parse::<u32>().ok()? * h.parse::<u32>().ok()?))
}

fn decode_picture(format: PictureFormat, data: &str) -> Option<RgbaImage> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(data.as_bytes()).ok()?;
    let format = match format {
        PictureFormat::Png => image::ImageFormat::Png,
        PictureFormat::Jpg => image::ImageFormat::Jpeg,
        PictureFormat::Qoi => image::ImageFormat::Qoi,
    };
    Some(image::load_from_memory_with_format(&bytes, format).ok()?.to_rgba8())
}

/// One comment line: the facts slicers write as "key = value" or "key: value".
fn comment(text: &str, out: &mut Gcode, cura_seconds: &mut Option<u64>, printer_profile: &mut Option<String>) {
    let facts = &mut out.facts;
    let lower = text.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("generated by ").or_else(|| lower.strip_prefix("generated with ")) {
        // "generated by OrcaSlicer 2.3.1 on 2026-02-28 at …", "Generated with Cura_SteamEngine 5.4.0"
        let start = text.len() - rest.len();
        let name = text[start..].split(" on ").next().unwrap_or_default().trim();
        if facts.slicer.is_none() && !name.is_empty() {
            facts.slicer = Some(name.to_owned());
        }
        return;
    }
    // Bambu: "model printing time: 1h 2m; total estimated time: 1h 10m"
    if let Some(pos) = lower.find("total estimated time:") {
        let value = text[pos + "total estimated time:".len()..].split(';').next().unwrap_or_default().trim();
        facts.print_time = Some(value.to_owned());
        return;
    }
    let Some((key, value)) = text.split_once(" = ").or_else(|| text.split_once(':')) else { return };
    let (key, value) = (key.trim().to_ascii_lowercase(), value.trim());
    let number = |v: &str| v.split([',', ';']).filter_map(|n| n.trim().parse::<f32>().ok()).sum::<f32>();
    match key.as_str() {
        "estimated printing time (normal mode)" | "estimated printing time" => {
            facts.print_time = Some(value.to_owned());
        }
        "time" => *cura_seconds = value.parse().ok(),
        "total filament used [g]" => facts.filament_g = Some(number(value)),
        "filament used [g]" if facts.filament_g.is_none() => facts.filament_g = Some(number(value)),
        "filament used [mm]" => facts.filament_m = Some(number(value) / 1000.0),
        // Cura: "Filament used: 1.23456m"
        "filament used" => facts.filament_m = Some(number(value.trim_end_matches('m'))),
        "filament_type" => {
            facts.materials = value.split(';').map(|m| m.trim().to_owned()).filter(|m| !m.is_empty()).collect();
        }
        "filament_colour" | "filament_color" => {
            out.colors = value.split([';', ',']).filter_map(hex_color).collect();
        }
        "layer_height" | "layer height" => facts.layer_height = value.parse().ok(),
        "nozzle_diameter" => facts.nozzle = value.split([',', ';']).next().and_then(|n| n.trim().parse().ok()),
        "printer_model" if !value.is_empty() => facts.printer = Some(value.to_owned()),
        "printer_settings_id" if !value.is_empty() => *printer_profile = Some(value.trim_matches('"').to_owned()),
        "total layer number" | "total layers count" | "layer_count" => facts.layers = value.parse().ok(),
        // Orca writes the cost per filament, then the total; Prusa only the former.
        "total filament cost" => facts.cost = Some(number(value)),
        "filament cost" if facts.cost.is_none() => facts.cost = Some(number(value)),
        _ => {}
    }
}

fn hex_color(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    (h.len() >= 6).then_some(())?;
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// "1h 2m 3s" from seconds.
fn duration(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 { format!("{h}h {m}m {s}s") } else { format!("{m}m {s}s") }
}

/// Just enough of a printer to follow the nozzle: positions, absolute/relative modes, arcs.
#[derive(Default)]
struct Machine {
    pos: Vec3,
    e: f32,
    relative: bool,
    relative_e: bool,
}

impl Machine {
    fn command(&mut self, line: &[u8], paths: &mut Vec<[Vec3; 2]>) {
        let code = line.split(|&b| b == b';').next().unwrap_or_default();
        let mut words = code.split(|b| b.is_ascii_whitespace()).filter(|w| !w.is_empty());
        let Some(cmd) = words.next() else { return };
        let mut arg = [None::<f32>; 6]; // X Y Z E I J
        for w in words {
            let slot = match w[0].to_ascii_uppercase() {
                b'X' => 0,
                b'Y' => 1,
                b'Z' => 2,
                b'E' => 3,
                b'I' => 4,
                b'J' => 5,
                _ => continue,
            };
            arg[slot] = std::str::from_utf8(&w[1..]).ok().and_then(|n| n.parse().ok());
        }
        let cmd = cmd.to_ascii_uppercase();
        match cmd.as_slice() {
            b"G90" => {
                self.relative = false;
                self.relative_e = false;
            }
            b"G91" => {
                self.relative = true;
                self.relative_e = true;
            }
            b"M82" => self.relative_e = false,
            b"M83" => self.relative_e = true,
            b"G92" => {
                let [x, y, z, e, ..] = arg;
                self.pos = Vec3::new(x.unwrap_or(self.pos.x), y.unwrap_or(self.pos.y), z.unwrap_or(self.pos.z));
                self.e = e.unwrap_or(self.e);
            }
            b"G0" | b"G00" | b"G1" | b"G01" | b"G2" | b"G02" | b"G3" | b"G03" => {
                let target = self.target(arg);
                let extruded = match arg[3] {
                    Some(e) if self.relative_e => e > 0.0,
                    Some(e) => {
                        let more = e > self.e;
                        self.e = e;
                        more
                    }
                    None => false,
                };
                let flat = (target - self.pos).truncate().length_squared() > 1e-8;
                let arc = matches!(cmd.as_slice(), b"G2" | b"G02" | b"G3" | b"G03");
                if extruded && flat {
                    if arc && (arg[4].is_some() || arg[5].is_some()) {
                        let clockwise = matches!(cmd.as_slice(), b"G2" | b"G02");
                        arc_segments(self.pos, target, arg[4].unwrap_or(0.0), arg[5].unwrap_or(0.0), clockwise, paths);
                    } else {
                        paths.push([self.pos, target]);
                    }
                }
                self.pos = target;
            }
            _ => {}
        }
    }

    fn target(&self, arg: [Option<f32>; 6]) -> Vec3 {
        let pick = |v: Option<f32>, current: f32| match v {
            Some(v) if self.relative => current + v,
            Some(v) => v,
            None => current,
        };
        Vec3::new(pick(arg[0], self.pos.x), pick(arg[1], self.pos.y), pick(arg[2], self.pos.z))
    }
}

/// An arc (G2 clockwise, G3 counter-clockwise) around `start + (i, j)` as short straight pieces.
fn arc_segments(start: Vec3, end: Vec3, i: f32, j: f32, clockwise: bool, paths: &mut Vec<[Vec3; 2]>) {
    let center = glam::Vec2::new(start.x + i, start.y + j);
    let (a, b) = (start.truncate() - center, end.truncate() - center);
    let radius = a.length();
    let mut sweep = b.y.atan2(b.x) - a.y.atan2(a.x);
    if clockwise && sweep >= 0.0 {
        sweep -= std::f32::consts::TAU;
    } else if !clockwise && sweep <= 0.0 {
        sweep += std::f32::consts::TAU;
    }
    let steps = ((sweep.abs() * radius / 0.5).ceil() as usize).clamp(1, 64);
    let mut previous = start;
    for k in 1..=steps {
        let t = k as f32 / steps as f32;
        let angle = a.y.atan2(a.x) + sweep * t;
        let point = Vec3::new(
            center.x + radius * angle.cos(),
            center.y + radius * angle.sin(),
            start.z + (end.z - start.z) * t,
        );
        paths.push([previous, point]);
        previous = point;
    }
}

fn bounds(paths: &[[Vec3; 2]]) -> Option<(Vec3, Vec3)> {
    let mut points = paths.iter().flatten();
    let first = *points.next()?;
    Some(points.fold((first, first), |(lo, hi), p| (lo.min(*p), hi.max(*p))))
}

/// The extruded paths seen from the front-left and above, shaded by height (lower is darker),
/// on a transparent `size` × `size` picture.
pub fn render_paths(paths: &[[Vec3; 2]], color: [u8; 3], size: u32) -> Option<RgbaImage> {
    const SUPER: u32 = 2;
    let (lo, hi) = bounds(paths)?;
    let (az, el) = (45f32.to_radians(), 35f32.to_radians());
    let project = |p: Vec3| {
        let p = p - (lo + hi) / 2.0;
        let x = p.x * az.cos() - p.y * az.sin();
        let depth = p.x * az.sin() + p.y * az.cos();
        let y = p.z * el.cos() + depth * el.sin();
        glam::Vec2::new(x, -y)
    };
    let corners = [
        Vec3::new(lo.x, lo.y, lo.z),
        Vec3::new(hi.x, lo.y, lo.z),
        Vec3::new(lo.x, hi.y, lo.z),
        Vec3::new(hi.x, hi.y, lo.z),
        Vec3::new(lo.x, lo.y, hi.z),
        Vec3::new(hi.x, lo.y, hi.z),
        Vec3::new(lo.x, hi.y, hi.z),
        Vec3::new(hi.x, hi.y, hi.z),
    ];
    let (pmin, pmax) = corners
        .iter()
        .map(|&c| project(c))
        .fold((glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN)), |(a, b), p| (a.min(p), b.max(p)));
    let big = size * SUPER;
    let span = (pmax - pmin).max_element().max(1e-3);
    let scale = big as f32 * 0.9 / span;
    let offset = glam::Vec2::splat(big as f32 / 2.0) - (pmin + pmax) / 2.0 * scale;
    let height = (hi.z - lo.z).max(1e-3);
    let mut image = RgbaImage::new(big, big);
    for &[a, b] in paths {
        let shade = 0.42 + 0.58 * (((a.z + b.z) / 2.0 - lo.z) / height);
        // Lines across the view a little lighter than lines along it, for some texture.
        let direction = (b - a).truncate().normalize_or_zero();
        let shade = shade * (0.88 + 0.12 * (direction.x * az.cos() - direction.y * az.sin()).abs());
        let px = Rgba([
            (f32::from(color[0]) * shade) as u8,
            (f32::from(color[1]) * shade) as u8,
            (f32::from(color[2]) * shade) as u8,
            255,
        ]);
        draw_line(&mut image, project(a) * scale + offset, project(b) * scale + offset, px);
    }
    Some(image::imageops::resize(&image, size, size, image::imageops::FilterType::Triangle))
}

/// A line two pixels wide.
fn draw_line(image: &mut RgbaImage, a: glam::Vec2, b: glam::Vec2, color: Rgba<u8>) {
    let steps = (b - a).abs().max_element().ceil().max(1.0) as usize;
    let (w, h) = (image.width() as i64, image.height() as i64);
    for k in 0..=steps {
        let p = a + (b - a) * (k as f32 / steps as f32);
        let (x, y) = (p.x as i64, p.y as i64);
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let (x, y) = (x + dx, y + dy);
            if (0..w).contains(&x) && (0..h).contains(&y) {
                image.put_pixel(x as u32, y as u32, color);
            }
        }
    }
}

fn trim_ascii(line: &[u8]) -> &[u8] {
    let start = line.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(line.len());
    let end = line.iter().rposition(|b| !b.is_ascii_whitespace()).map_or(start, |e| e + 1);
    &line[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny print in the style of Orca/Anycubic Slicer Next: header facts, a 2-layer square
    /// with relative extrusion, travel moves, and the config block at the end.
    const ORCA: &str = "; HEADER_BLOCK_START
; generated by OrcaSlicer 2.3.1 on 2026-02-28 at 15:44:03
; total layer number: 2
; HEADER_BLOCK_END
G90
M83
G1 X-5 Y-50 Z0.3 F600
G1 X-5 Y150 E8 ; purge line at the bed's edge
;LAYER_CHANGE
G1 Z0.2 F600
G1 X0 Y0 F9000
G1 X20 Y0 E1.2
G1 X20 Y10 E0.6
G1 X0 Y10 E1.2 ; perimeter
G1 X0 Y0 E0.6
G1 Z0.4
G1 X5 Y5
G2 X15 Y5 I5 J0 E1.0
; filament used [mm] = 10567.25
; filament used [g] = 31.52
; total filament used [g] = 31.52
; filament cost = 0.82
; total filament cost = 0.82
; estimated printing time (normal mode) = 1h 6m 46s
; filament_type = PLA
; filament_colour = #FF8000
; layer_height = 0.2
; nozzle_diameter = 0.4,0.4
; printer_model = Creality Ender-3 V3 SE
";

    #[test]
    fn orca_facts_and_paths() {
        let g = parse(ORCA.as_bytes());
        let f = &g.facts;
        assert_eq!(f.slicer.as_deref(), Some("OrcaSlicer 2.3.1"));
        assert_eq!(f.printer.as_deref(), Some("Creality Ender-3 V3 SE"));
        assert_eq!(f.print_time.as_deref(), Some("1h 6m 46s"));
        assert_eq!((f.filament_g, f.filament_m), (Some(31.52), Some(10.56725)));
        assert_eq!((f.layer_height, f.nozzle, f.layers), (Some(0.2), Some(0.4), Some(2)));
        assert_eq!(f.materials, ["PLA"]);
        assert_eq!(f.cost, Some(0.82));
        assert_eq!(g.colors, [[255, 128, 0]]);
        assert!(g.preview.is_none());
        // Four sides of the square plus the half circle; travel moves are not paths.
        assert!(g.paths.len() > 4);
        let [x, y, z] = g.size_mm().unwrap();
        assert_eq!((x, z), (20.0, 0.2));
        assert!((y - 10.0).abs() < 1e-4, "the arc stays inside the square ({y})");
        let picture = render_paths(&g.paths, g.colors[0], 64).unwrap();
        assert_eq!(picture.dimensions(), (64, 64));
        assert!(picture.pixels().any(|p| p[3] > 0), "something is drawn");
        assert_eq!(picture.get_pixel(0, 0)[3], 0, "transparent around it");
    }

    #[test]
    fn absolute_extrusion_and_other_slicers() {
        let prusa = "; generated by PrusaSlicer 2.7.1+linux-x64 on 2024-01-01 at 10:00:00 UTC
M82
G92 E0
G1 X0 Y0 Z0.2
G1 X10 Y0 E1.0
G1 X10 Y10 E1.0
G1 X0 Y10 E2.0
; estimated printing time (normal mode) = 12m 5s
; filament cost = 0.25, 0.5
; filament_cost = 25
; printer_settings_id = Original Prusa MK4 0.4 nozzle
";
        let g = parse(prusa.as_bytes());
        assert_eq!(g.facts.slicer.as_deref(), Some("PrusaSlicer 2.7.1+linux-x64"));
        assert_eq!(g.facts.printer.as_deref(), Some("Original Prusa MK4 0.4 nozzle"), "profile name as fallback");
        assert_eq!(g.paths.len(), 2, "E did not grow on the second move");
        assert_eq!(g.facts.cost, Some(0.75), "costs of all filaments, not the price per kg");

        let cura = ";FLAVOR:Marlin\n;TIME:3723\n;Filament used: 1.5m\n;Layer height: 0.12\n;LAYER_COUNT:150\n;Generated with Cura_SteamEngine 5.4.0\n";
        let g = parse(cura.as_bytes());
        assert_eq!(g.facts.print_time.as_deref(), Some("1h 2m 3s"));
        assert_eq!((g.facts.filament_m, g.facts.layer_height, g.facts.layers), (Some(1.5), Some(0.12), Some(150)));
        assert_eq!(g.facts.slicer.as_deref(), Some("Cura_SteamEngine 5.4.0"));

        let bambu = "; model printing time: 1h 2m; total estimated time: 1h 10m\n; filament_colour = #FFFFFF;#000000\n";
        let g = parse(bambu.as_bytes());
        assert_eq!(g.facts.print_time.as_deref(), Some("1h 10m"));
        assert_eq!(g.colors, [[255, 255, 255], [0, 0, 0]]);
    }

    #[test]
    fn the_largest_embedded_picture_wins() {
        let png = |size: u32| {
            let mut bytes = Vec::new();
            RgbaImage::from_pixel(size, size, Rgba([10, 20, 30, 255]))
                .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
                .unwrap();
            base64::engine::general_purpose::STANDARD.encode(bytes)
        };
        let block = |size: u32| {
            let data = png(size);
            let lines: Vec<String> =
                data.as_bytes().chunks(78).map(|c| format!("; {}", std::str::from_utf8(c).unwrap())).collect();
            format!("; thumbnail begin {size}x{size} {}\n{}\n; thumbnail end\n", data.len(), lines.join("\n"))
        };
        let gcode = format!("{}{}G1 X1 Y1 E1\n", block(16), block(48));
        let g = parse(gcode.as_bytes());
        assert_eq!(g.preview.as_ref().map(|p| p.dimensions()), Some((48, 48)));
        assert_eq!(g.facts, GcodeFacts::default(), "picture data is not mistaken for facts");
    }
}
