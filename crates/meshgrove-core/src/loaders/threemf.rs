//! 3MF reader: geometry of all build items (with component trees, transforms, units and the
//! Production extension's `p:path` sub-models used by Bambu Studio / Orca / Anycubic / Elegoo),
//! their colours (filaments assigned to objects and parts, multi-material painting, 3MF
//! material colours), plus the preview images slicers embed.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{Cursor, Read};

use glam::{Affine3A, Vec3};
use image::RgbaImage;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use super::texture::NEUTRAL;
use super::{LoadError, paint};
use crate::mesh::Mesh;

const START_PART: &str = "3D/3dmodel.model";
const REL_MODEL: &str = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";
const REL_THUMBNAIL: &str = "http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail";
/// Component trees deeper than this are treated as broken (or cyclic) files.
const MAX_DEPTH: usize = 16;

#[derive(Debug, Clone, Default)]
pub struct ThreeMf {
    /// All build items in millimetres.
    pub mesh: Mesh,
    pub title: Option<String>,
    pub designer: Option<String>,
    /// Slicer or CAD program that wrote the file, e.g. "BambuStudio-2.2.0".
    pub application: Option<String>,
    /// Slicer build plates that hold objects (Bambu/Orca family), in order.
    pub plates: Vec<usize>,
    /// The plate `mesh` shows; `None` means all build items.
    pub plate: Option<usize>,
    /// Filaments of a slicer project that `mesh` uses, by slot.
    pub filaments: Vec<Filament>,
    /// Triangles as stored in the file; `mesh` has more when painted triangles were split.
    pub triangles: usize,
}

/// A filament of a slicer project.
#[derive(Debug, Clone, PartialEq)]
pub struct Filament {
    /// 1-based filament slot (extruder number).
    pub slot: u32,
    /// sRGB.
    pub color: [u8; 3],
    /// "PLA", "PETG", … when the project says.
    pub material: Option<String>,
}

/// What a 3MF project says about itself (the `<metadata>` of its model, read without the mesh).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Meta {
    pub title: Option<String>,
    pub designer: Option<String>,
    pub application: Option<String>,
    pub license: Option<String>,
    pub description: Option<String>,
}

/// Reads [`Meta`]: only the start of the model part, up to its resources.
pub fn metadata(bytes: &[u8]) -> Result<Meta, LoadError> {
    let mut package = Package::open(bytes)?;
    let start = package.start_part();
    let data = package.read(&start)?;
    let mut reader = Reader::from_reader(data.as_slice());
    let mut meta = Meta::default();
    let mut name: Option<String> = None;
    let mut text = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.local_name().as_ref() == "metadata" => {
                name = attr(&e, "name");
                text.clear();
            }
            Ok(Event::Text(t)) if name.is_some() => text.push_str(&t.xml10_content()),
            Ok(Event::GeneralRef(r)) if name.is_some() => {
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    text.push(c);
                } else {
                    text.push_str(match r.xml10_content().as_ref() {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        _ => "",
                    });
                }
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == "metadata" => {
                let value = Some(text.trim().to_owned()).filter(|v| !v.is_empty());
                match name.take().as_deref().map(|n| n.rsplit(':').next().unwrap_or(n).to_lowercase()).as_deref() {
                    Some("title") => meta.title = value,
                    Some("designer") => meta.designer = value,
                    Some("application") => meta.application = value,
                    Some("license" | "licenseterms") => meta.license = meta.license.take().or(value),
                    Some("description") => meta.description = value,
                    _ => {}
                }
            }
            Ok(Event::Start(e)) if e.local_name().as_ref() == "resources" => break,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    Ok(meta)
}

/// Parses the model. Multi-plate slicer projects show their first plate, matching the
/// plate picture used as thumbnail; see [`parse_plate`] for the others. Painted triangles keep
/// their part's colour, so the triangle count matches the file (facts, duplicate detection).
pub fn parse(bytes: &[u8]) -> Result<ThreeMf, LoadError> {
    load(bytes, None, false)
}

/// What the 3D viewer shows: build plate `plate` (everything if it is unknown), with painted
/// triangles split into their coloured areas.
pub fn parse_plate(bytes: &[u8], plate: Option<usize>) -> Result<ThreeMf, LoadError> {
    load(bytes, plate, true)
}

fn load(bytes: &[u8], plate: Option<usize>, split_paint: bool) -> Result<ThreeMf, LoadError> {
    let mut package = Package::open(bytes)?;
    let style = Style { filaments: package.filaments(), objects: package.assignments(), split_paint };
    let plate_objects = package.plate_objects();
    let plates: Vec<usize> = plate_objects.keys().copied().collect();
    let shown = plate.filter(|p| plate_objects.contains_key(p)).or_else(|| plates.first().copied());
    let wanted = shown.and_then(|p| plate_objects.get(&p));
    let start = package.start_part();
    let root = package.model(&start)?;
    let scale = root.unit_scale;
    let meta = |k: &str| root.metadata.get(k).filter(|v| !v.trim().is_empty()).cloned();
    let (title, designer, application) = (meta("Title"), meta("Designer"), meta("Application"));
    let build = root.build.clone();

    let mut out =
        Mesh { positions: Vec::new(), indices: Some(Vec::new()), colors: Some(Vec::new()), vertex_colors: None };
    let on_plate: Vec<&BuildItem> = build.iter().filter(|i| wanted.is_none_or(|ids| ids.contains(&i.object))).collect();
    // Objects parked outside every plate would leave nothing to show: then show them all.
    let (items, shown) = if on_plate.is_empty() { (build.iter().collect(), None) } else { (on_plate, shown) };
    let mut tally = Tally::default();
    for item in items {
        let path = item.path.clone().unwrap_or_else(|| start.clone());
        let assign = style.objects.get(&item.object);
        let ctx = Ctx { assign, filament: assign.and_then(|a| a.filament) };
        package.emit(&path, item.object, item.transform, 0, &mut out, &style, ctx, &mut tally)?;
    }
    if out.is_empty() {
        return Err(LoadError::Empty);
    }
    if !tally.colored {
        out.colors = None;
    }
    let filaments = tally
        .used
        .iter()
        .filter_map(|&slot| {
            let (color, material) = style.filaments.get(slot as usize - 1)?;
            Some(Filament { slot, color: [color[0], color[1], color[2]], material: material.clone() })
        })
        .collect();
    if scale != 1.0 {
        for p in &mut out.positions {
            *p = (Vec3::from(*p) * scale).to_array();
        }
    }
    Ok(ThreeMf { mesh: out, title, designer, application, plates, plate: shown, filaments, triangles: tally.triangles })
}

/// The package's main preview (slicer plate image or CAD thumbnail), shrunk to fit `max`,
/// or `None` if there is none or it is blank (headless slicing leaves empty plate images).
pub fn embedded_thumbnail(bytes: &[u8], max: u32) -> Option<RgbaImage> {
    let mut package = Package::open(bytes).ok()?;
    let mut candidates: Vec<String> = package.relationship_targets(REL_THUMBNAIL);
    candidates.extend(
        ["Metadata/thumbnail.png", "Metadata/plate_1.png", "Metadata/thumbnail.jpg", "Thumbnails/thumbnail.png"]
            .map(String::from),
    );
    candidates.into_iter().find_map(|name| {
        let data = package.read(&name).ok()?;
        let image = image::load_from_memory(&data).ok()?.to_rgba8();
        (!is_blank(&image)).then(|| fit(image, max))
    })
}

/// Plate preview images (`Metadata/plate_N.png`) in plate order, blank ones skipped.
pub fn plate_images(bytes: &[u8]) -> Vec<(usize, RgbaImage)> {
    let Ok(mut package) = Package::open(bytes) else { return Vec::new() };
    let mut plates: Vec<(usize, String)> = package
        .names
        .keys()
        .filter_map(|n| {
            let num = n.strip_prefix("metadata/plate_")?.strip_suffix(".png")?;
            Some((num.parse().ok()?, package.names[n].clone()))
        })
        .collect();
    plates.sort();
    plates
        .into_iter()
        .filter_map(|(num, name)| {
            let image = image::load_from_memory(&package.read(&name).ok()?).ok()?.to_rgba8();
            (!is_blank(&image)).then_some((num, image))
        })
        .collect()
}

pub(crate) fn fit(image: RgbaImage, max: u32) -> RgbaImage {
    if image.width() <= max && image.height() <= max {
        return image;
    }
    let scale = max as f32 / image.width().max(image.height()) as f32;
    let (w, h) = (
        ((image.width() as f32 * scale).round() as u32).max(1),
        ((image.height() as f32 * scale).round() as u32).max(1),
    );
    image::imageops::thumbnail(&image, w, h)
}

/// Fully transparent, or (nearly) one flat colour over all visible pixels.
pub fn is_blank(image: &RgbaImage) -> bool {
    let (mut lo, mut hi, mut visible) = (u8::MAX, u8::MIN, 0usize);
    for p in image.pixels() {
        if p[3] < 16 {
            continue;
        }
        visible += 1;
        let luma = ((p[0] as u32 * 54 + p[1] as u32 * 183 + p[2] as u32 * 19) >> 8) as u8;
        lo = lo.min(luma);
        hi = hi.max(luma);
    }
    visible < 16 || hi.saturating_sub(lo) < 8
}

struct Package {
    zip: zip::ZipArchive<Cursor<Vec<u8>>>,
    /// Lower-case name without leading slash → real name in the archive.
    names: HashMap<String, String>,
    models: HashMap<String, ModelFile>,
}

impl Package {
    fn open(bytes: &[u8]) -> Result<Package, LoadError> {
        let zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec()))
            .map_err(|e| LoadError::Malformed(format!("not a 3MF/ZIP package: {e}")))?;
        let names = zip.file_names().filter_map(Result::ok).map(|n| (n.to_lowercase(), n.into_owned())).collect();
        Ok(Package { zip, names, models: HashMap::new() })
    }

    /// Reads a part by OPC name ("/3D/x.model", case-insensitive, percent-encoding tolerated).
    fn read(&mut self, name: &str) -> Result<Vec<u8>, LoadError> {
        let key = normalize(name);
        let real = self
            .names
            .get(&key)
            .or_else(|| self.names.get(&percent_decode(&key)))
            .cloned()
            .ok_or_else(|| LoadError::Malformed(format!("missing part {name}")))?;
        let mut file = self.zip.by_name(&real).map_err(|e| LoadError::Malformed(e.to_string()))?;
        let mut data = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut data)?;
        Ok(data)
    }

    fn relationship_targets(&mut self, rel_type: &str) -> Vec<String> {
        let Ok(rels) = self.read("_rels/.rels") else { return Vec::new() };
        let mut reader = Reader::from_reader(rels.as_slice());
        let mut targets = Vec::new();
        loop {
            match reader.read_event() {
                Ok(Event::Empty(e) | Event::Start(e)) if e.local_name().as_ref() == "Relationship" => {
                    if attr(&e, "Type").as_deref() == Some(rel_type)
                        && let Some(target) = attr(&e, "Target")
                    {
                        targets.push(target);
                    }
                }
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }
        targets
    }

    /// Plate number → build object ids, from Bambu/Orca's `Metadata/model_settings.config`.
    fn plate_objects(&mut self) -> std::collections::BTreeMap<usize, Vec<u32>> {
        let mut plates = std::collections::BTreeMap::new();
        let Ok(config) = self.read("Metadata/model_settings.config") else { return plates };
        let mut reader = Reader::from_reader(config.as_slice());
        let (mut in_plate, mut in_instance) = (false, false);
        let (mut plate_id, mut objects) = (None::<usize>, Vec::new());
        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) if e.local_name().as_ref() == "plate" => {
                    (in_plate, plate_id) = (true, None);
                    objects.clear();
                }
                Ok(Event::Start(e)) if e.local_name().as_ref() == "model_instance" => in_instance = true,
                Ok(Event::Empty(e) | Event::Start(e)) if in_plate && e.local_name().as_ref() == "metadata" => {
                    let value = attr(&e, "value").and_then(|v| v.trim().parse::<usize>().ok());
                    match attr(&e, "key").as_deref() {
                        Some("plater_id") if !in_instance => plate_id = value,
                        Some("object_id") if in_instance => objects.extend(value.map(|v| v as u32)),
                        _ => {}
                    }
                }
                Ok(Event::End(e)) if e.local_name().as_ref() == "model_instance" => in_instance = false,
                Ok(Event::End(e)) if e.local_name().as_ref() == "plate" => {
                    in_plate = false;
                    if let Some(id) = plate_id
                        && !objects.is_empty()
                    {
                        plates.insert(id, std::mem::take(&mut objects));
                    }
                }
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }
        plates
    }

    fn start_part(&mut self) -> String {
        self.relationship_targets(REL_MODEL)
            .into_iter()
            .next()
            .map(|t| normalize(&t))
            .unwrap_or_else(|| START_PART.to_lowercase())
    }

    fn model(&mut self, path: &str) -> Result<&ModelFile, LoadError> {
        let key = normalize(path);
        if !self.models.contains_key(&key) {
            let data = self.read(&key)?;
            let model = ModelFile::parse(&data)?;
            self.models.insert(key.clone(), model);
        }
        Ok(&self.models[&key])
    }

    /// Filament colours and materials by slot (index 0 = slot 1): Bambu/Orca/Anycubic's
    /// `project_settings.config`, else PrusaSlicer's `Slic3r_PE.config`.
    fn filaments(&mut self) -> Vec<([u8; 4], Option<String>)> {
        if let Ok(data) = self.read("Metadata/project_settings.config")
            && let Ok(json) = serde_json::from_slice::<serde_json::Value>(&data)
        {
            let list = |key: &str| -> Vec<String> {
                json.get(key)
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().map(|v| v.as_str().unwrap_or_default().to_owned()).collect())
                    .unwrap_or_default()
            };
            let types = list("filament_type");
            return list("filament_colour")
                .iter()
                .enumerate()
                .map(|(i, c)| (hex_color(c).unwrap_or(NEUTRAL), types.get(i).filter(|t| !t.is_empty()).cloned()))
                .collect();
        }
        if let Ok(data) = self.read("Metadata/Slic3r_PE.config") {
            let text = String::from_utf8_lossy(&data);
            // Lines like `; extruder_colour = "#FF8000";"#DB5182"` or `; filament_type = PLA;PETG`.
            let list = |key: &str| -> Vec<String> {
                text.lines()
                    .find_map(|l| {
                        let (k, v) = l.trim_start_matches(';').split_once('=')?;
                        (k.trim() == key).then(|| v.split(';').map(|s| s.trim().trim_matches('"').to_owned()).collect())
                    })
                    .unwrap_or_default()
            };
            let (extruder, filament, types) = (list("extruder_colour"), list("filament_colour"), list("filament_type"));
            let count = extruder.len().max(filament.len());
            return (0..count)
                .map(|i| {
                    let color = [extruder.get(i), filament.get(i)].into_iter().flatten().find_map(|c| hex_color(c));
                    (color.unwrap_or(NEUTRAL), types.get(i).filter(|t| !t.is_empty()).cloned())
                })
                .collect();
        }
        Vec::new()
    }

    /// Which filament each build object and its parts print with, and which parts are not
    /// printed geometry (modifiers, negative volumes, support blockers/enforcers).
    fn assignments(&mut self) -> HashMap<u32, Assign> {
        let mut objects = HashMap::new();
        if let Ok(config) = self.read("Metadata/model_settings.config") {
            read_bambu_assignments(&config, &mut objects);
        } else if let Ok(config) = self.read("Metadata/Slic3r_PE_model.config") {
            read_prusa_assignments(&config, &mut objects);
        }
        objects
    }

    /// Appends object `id` of model part `path`, transformed, to `out` (recursing into components).
    #[allow(clippy::too_many_arguments)]
    fn emit(
        &mut self,
        path: &str,
        id: u32,
        transform: Affine3A,
        depth: usize,
        out: &mut Mesh,
        style: &Style,
        ctx: Ctx,
        tally: &mut Tally,
    ) -> Result<(), LoadError> {
        if depth > MAX_DEPTH {
            return Err(LoadError::Malformed("component tree too deep (cyclic?)".into()));
        }
        let object = self
            .model(path)?
            .objects
            .get(&id)
            .cloned()
            .ok_or_else(|| LoadError::Malformed(format!("object {id} missing in {path}")))?;
        if !object.printable {
            return Ok(());
        }
        if let Some(mesh) = &*object.mesh {
            emit_mesh(mesh, transform, style, ctx, out, tally);
        }
        for c in &object.components {
            if ctx.assign.is_some_and(|a| a.hidden_parts.contains(&c.object)) {
                continue;
            }
            let sub_path = c.path.clone().unwrap_or_else(|| path.to_owned());
            let part = ctx.assign.and_then(|a| a.parts.get(&c.object)).copied();
            let child = Ctx { filament: part.or(ctx.filament), ..ctx };
            self.emit(&sub_path, c.object, transform * c.transform, depth + 1, out, style, child, tally)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
struct ObjectMesh {
    vertices: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    /// 3MF material colour per triangle; empty while no triangle has one.
    colors: Vec<Option<[u8; 4]>>,
    /// Painted triangles in ascending order: (triangle index, subdivision code).
    paint: Vec<(u32, String)>,
}

/// Colour sources of a whole package.
struct Style {
    filaments: Vec<([u8; 4], Option<String>)>,
    objects: HashMap<u32, Assign>,
    /// Split painted triangles into their coloured areas (viewer) or keep them whole (facts).
    split_paint: bool,
}

impl Style {
    /// The colour of filament `slot` (1-based; 0 or none means the first), noting it as used.
    fn filament(&self, slot: Option<u32>, tally: &mut Tally) -> Option<[u8; 4]> {
        if self.filaments.is_empty() {
            return None;
        }
        let slot = slot.filter(|&s| s > 0).unwrap_or(1);
        let (color, _) = self.filaments.get(slot as usize - 1)?;
        tally.used.insert(slot);
        Some(*color)
    }
}

/// Filament assignment of one build object (by its id in the start part).
#[derive(Debug, Default)]
struct Assign {
    filament: Option<u32>,
    /// Component object id → filament (Bambu/Orca parts).
    parts: HashMap<u32, u32>,
    /// Components that are modifiers or negative volumes, not printed geometry.
    hidden_parts: HashSet<u32>,
    /// PrusaSlicer volumes as triangle ranges of the object's mesh: (first, last, filament).
    volumes: Vec<(u32, u32, Option<u32>)>,
    /// PrusaSlicer triangle ranges that are not printed geometry.
    hidden_volumes: Vec<(u32, u32)>,
}

#[derive(Clone, Copy)]
struct Ctx<'a> {
    assign: Option<&'a Assign>,
    /// The filament of the object or part being emitted.
    filament: Option<u32>,
}

#[derive(Default)]
struct Tally {
    used: BTreeSet<u32>,
    /// Some triangle got a colour of its own.
    colored: bool,
    /// Triangles of the file that were emitted (each painted one counts once).
    triangles: usize,
}

/// Appends `mesh` (an object's own triangles) to `out`, coloured.
fn emit_mesh(mesh: &ObjectMesh, transform: Affine3A, style: &Style, ctx: Ctx, out: &mut Mesh, tally: &mut Tally) {
    let base = out.positions.len() as u32;
    let place = |v: Vec3| transform.transform_point3(v).to_array();
    out.positions.extend(mesh.vertices.iter().map(|v| place(Vec3::from(*v))));
    let mut painted = mesh.paint.iter().peekable();
    for (i, t) in mesh.triangles.iter().enumerate() {
        let index = i as u32;
        let code = loop {
            match painted.peek() {
                Some((p, _)) if *p < index => _ = painted.next(),
                Some((p, code)) if *p == index => break Some(code.as_str()),
                _ => break None,
            }
        };
        if !t.iter().all(|&v| (v as usize) < mesh.vertices.len()) {
            continue;
        }
        if let Some(a) = ctx.assign
            && a.hidden_volumes.iter().any(|&(first, last)| (first..=last).contains(&index))
        {
            continue;
        }
        let volume = ctx
            .assign
            .and_then(|a| a.volumes.iter().find(|(first, last, _)| (*first..=*last).contains(&index)))
            .and_then(|(_, _, f)| *f);
        tally.triangles += 1;
        let own = style.filament(volume.or(ctx.filament), tally).or(mesh.colors.get(i).copied().flatten());
        if let (Some(code), false) = (code, style.filaments.is_empty())
            && let Some(leaves) = paint::decode(code, t.map(|v| Vec3::from(mesh.vertices[v as usize])))
        {
            let colors: Vec<Option<[u8; 4]>> = leaves
                .iter()
                .map(|(_, state)| if *state == 0 { own } else { style.filament(Some(u32::from(*state)), tally) })
                .collect();
            if style.split_paint {
                for ((corners, _), color) in leaves.iter().zip(colors) {
                    let first = out.positions.len() as u32;
                    out.positions.extend(corners.iter().map(|c| place(*c)));
                    push_triangle(out, [first, first + 1, first + 2], color.or(own), tally);
                }
                continue;
            }
        }
        push_triangle(out, t.map(|v| base + v), own, tally);
    }
}

fn push_triangle(out: &mut Mesh, indices: [u32; 3], color: Option<[u8; 4]>, tally: &mut Tally) {
    out.indices.get_or_insert_with(Vec::new).extend(indices);
    tally.colored |= color.is_some();
    if let Some(colors) = &mut out.colors {
        colors.push(color.unwrap_or(NEUTRAL));
    }
}

/// Bambu/Orca/Anycubic `Metadata/model_settings.config`: `<object id>` with an `extruder`
/// metadata and `<part id subtype>` children with their own.
fn read_bambu_assignments(config: &[u8], objects: &mut HashMap<u32, Assign>) {
    let mut reader = Reader::from_reader(config);
    let (mut object, mut part): (Option<u32>, Option<u32>) = (None, None);
    let id = |e: &BytesStart| attr(e, "id").and_then(|v| v.trim().parse::<u32>().ok());
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.local_name().as_ref() == "object" => object = id(&e),
            Ok(event @ (Event::Start(_) | Event::Empty(_))) => {
                let (Event::Start(e) | Event::Empty(e)) = &event else { unreachable!() };
                match e.local_name().as_ref() {
                    "part" => {
                        let subtype = attr(e, "subtype").unwrap_or_default();
                        if let (Some(o), Some(p)) = (object, id(e))
                            && !subtype.is_empty()
                            && subtype != "normal_part"
                        {
                            objects.entry(o).or_default().hidden_parts.insert(p);
                        }
                        if matches!(event, Event::Start(_)) {
                            part = id(e);
                        }
                    }
                    "metadata" => {
                        let Some(o) = object else { continue };
                        if attr(e, "key").as_deref() != Some("extruder") {
                            continue;
                        }
                        let value = attr(e, "value").and_then(|v| v.trim().parse::<u32>().ok()).filter(|&v| v > 0);
                        let entry = objects.entry(o).or_default();
                        match part {
                            Some(p) => {
                                if let Some(v) = value {
                                    entry.parts.insert(p, v);
                                }
                            }
                            None => entry.filament = value,
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == "part" => part = None,
            Ok(Event::End(e)) if e.local_name().as_ref() == "object" => (object, part) = (None, None),
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
}

/// PrusaSlicer `Metadata/Slic3r_PE_model.config`: `<object id>` with `<volume firstid lastid>`
/// triangle ranges, each with `extruder` and `volume_type` metadata.
fn read_prusa_assignments(config: &[u8], objects: &mut HashMap<u32, Assign>) {
    let mut reader = Reader::from_reader(config);
    let mut object: Option<u32> = None;
    let mut volume: Option<(u32, u32, Option<u32>, bool)> = None;
    let num = |e: &BytesStart, k: &str| attr(e, k).and_then(|v| v.trim().parse::<u32>().ok());
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.local_name().as_ref() == "object" => object = num(&e, "id"),
            Ok(Event::Start(e)) if e.local_name().as_ref() == "volume" => {
                volume = Some((num(&e, "firstid").unwrap_or(0), num(&e, "lastid").unwrap_or(u32::MAX), None, false));
            }
            Ok(Event::Empty(e) | Event::Start(e)) if e.local_name().as_ref() == "metadata" => {
                let Some(o) = object else { continue };
                let (key, value) = (attr(&e, "key").unwrap_or_default(), attr(&e, "value").unwrap_or_default());
                match (&mut volume, key.as_str()) {
                    (Some(v), "extruder") => v.2 = value.trim().parse().ok().filter(|&x: &u32| x > 0),
                    (Some(v), "volume_type") => v.3 = value != "ModelPart",
                    (None, "extruder") => {
                        objects.entry(o).or_default().filament = value.trim().parse().ok().filter(|&x: &u32| x > 0)
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == "volume" => {
                if let (Some(o), Some((first, last, filament, hidden))) = (object, volume.take()) {
                    let entry = objects.entry(o).or_default();
                    if hidden {
                        entry.hidden_volumes.push((first, last));
                    } else {
                        entry.volumes.push((first, last, filament));
                    }
                }
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == "object" => object = None,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
}

/// `#RRGGBB` or `#RRGGBBAA`.
fn hex_color(text: &str) -> Option<[u8; 4]> {
    let hex = text.trim().strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    match hex.len() {
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 255]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct Component {
    path: Option<String>,
    object: u32,
    transform: Affine3A,
}

#[derive(Debug, Clone)]
struct Object {
    /// Support structures and "other" helper objects are not part of the printed model.
    printable: bool,
    /// Shared so cloning an object while walking the component tree stays cheap.
    mesh: std::sync::Arc<Option<ObjectMesh>>,
    components: Vec<Component>,
}

#[derive(Debug, Clone)]
struct BuildItem {
    path: Option<String>,
    object: u32,
    transform: Affine3A,
}

#[derive(Debug, Default)]
struct ModelFile {
    unit_scale: f32,
    metadata: HashMap<String, String>,
    objects: HashMap<u32, Object>,
    build: Vec<BuildItem>,
    /// `basematerials` / `colorgroup` resources: id → colours by index.
    materials: HashMap<u32, Vec<[u8; 4]>>,
}

impl ModelFile {
    fn parse(data: &[u8]) -> Result<ModelFile, LoadError> {
        let mut reader = Reader::from_reader(data);
        let mut model = ModelFile { unit_scale: 1.0, ..Default::default() };
        // (id, object, mesh, the object's default colour from its pid/pindex)
        let mut current: Option<(u32, Object, ObjectMesh, Option<[u8; 4]>)> = None;
        let mut group: Option<(u32, Vec<[u8; 4]>)> = None;
        let mut metadata_name: Option<String> = None;
        let mut metadata_text = String::new();
        let bad = |e: quick_xml::Error| LoadError::Malformed(format!("XML: {e}"));

        loop {
            let event = reader.read_event().map_err(bad)?;
            match &event {
                Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                    "model" => model.unit_scale = attr(e, "unit").map_or(1.0, |u| unit_to_mm(&u)),
                    "vertex" => {
                        if let Some((_, _, mesh, _)) = &mut current {
                            mesh.vertices.push(vertex(e));
                        }
                    }
                    "triangle" => {
                        if let Some((_, _, mesh, default)) = &mut current {
                            let t = triangle(e);
                            let index = mesh.triangles.len();
                            mesh.triangles.push(t.vertices);
                            let color = match t.pid {
                                Some(pid) => {
                                    model.materials.get(&pid).and_then(|g| g.get(t.p1.unwrap_or(0) as usize)).copied()
                                }
                                None => *default,
                            };
                            if color.is_some() || !mesh.colors.is_empty() {
                                mesh.colors.resize(index, None);
                                mesh.colors.push(color);
                            }
                            if let Some(code) = t.paint {
                                mesh.paint.push((index as u32, code));
                            }
                        }
                    }
                    "object" => {
                        let id = attr(e, "id").and_then(|v| v.parse().ok()).unwrap_or(0);
                        let printable = !matches!(attr(e, "type").as_deref(), Some("support" | "other"));
                        let default = attr(e, "pid").and_then(|v| v.parse::<u32>().ok()).and_then(|pid| {
                            let index = attr(e, "pindex").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
                            model.materials.get(&pid)?.get(index).copied()
                        });
                        current = Some((
                            id,
                            Object { printable, mesh: Default::default(), components: Vec::new() },
                            ObjectMesh::default(),
                            default,
                        ));
                    }
                    "basematerials" | "colorgroup" => {
                        group = attr(e, "id").and_then(|v| v.parse().ok()).map(|id| (id, Vec::new()));
                    }
                    "base" => {
                        if let Some((_, colors)) = &mut group {
                            colors.push(attr(e, "displaycolor").and_then(|c| hex_color(&c)).unwrap_or(NEUTRAL));
                        }
                    }
                    "color" => {
                        if let Some((_, colors)) = &mut group {
                            colors.push(attr(e, "color").and_then(|c| hex_color(&c)).unwrap_or(NEUTRAL));
                        }
                    }
                    "component" => {
                        if let Some((_, object, _, _)) = &mut current {
                            object.components.push(Component {
                                path: attr(e, "path"),
                                object: attr(e, "objectid").and_then(|v| v.parse().ok()).unwrap_or(0),
                                transform: transform(e),
                            });
                        }
                    }
                    "item" => model.build.push(BuildItem {
                        path: attr(e, "path"),
                        object: attr(e, "objectid").and_then(|v| v.parse().ok()).unwrap_or(0),
                        transform: transform(e),
                    }),
                    "metadata" if current.is_none() => {
                        metadata_name = attr(e, "name");
                        metadata_text.clear();
                    }
                    _ => {}
                },
                Event::Text(t) if metadata_name.is_some() => metadata_text.push_str(&t.xml10_content()),
                Event::GeneralRef(r) if metadata_name.is_some() => {
                    if let Ok(Some(c)) = r.resolve_char_ref() {
                        metadata_text.push(c);
                    } else {
                        metadata_text.push_str(match r.xml10_content().as_ref() {
                            "amp" => "&",
                            "lt" => "<",
                            "gt" => ">",
                            "quot" => "\"",
                            "apos" => "'",
                            _ => "",
                        });
                    }
                }
                Event::End(e) => match e.local_name().as_ref() {
                    "basematerials" | "colorgroup" => {
                        if let Some((id, colors)) = group.take() {
                            model.materials.insert(id, colors);
                        }
                    }
                    "object" => {
                        if let Some((id, mut object, mesh, _)) = current.take() {
                            if !mesh.vertices.is_empty() {
                                object.mesh = std::sync::Arc::new(Some(mesh));
                            }
                            model.objects.insert(id, object);
                        }
                    }
                    "metadata" => {
                        if let Some(name) = metadata_name.take() {
                            // Prefixed names like "BambuStudio:3mfVersion" stay as they are.
                            model.metadata.insert(name, metadata_text.trim().to_owned());
                        }
                    }
                    _ => {}
                },
                Event::Eof => break,
                _ => {}
            }
        }
        Ok(model)
    }
}

/// Attribute by local name (namespace prefix ignored, so `p:path` matches `path`).
fn attr(e: &BytesStart, local: &str) -> Option<String> {
    e.attributes()
        .filter_map(Result::ok)
        .find(|a| a.key.local_name().as_ref() == local)
        .and_then(|a| a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok().map(|v| v.into_owned()))
}

/// `<vertex x= y= z=>` in one pass over the attributes (this is the hot loop for big files).
fn vertex(e: &BytesStart) -> [f32; 3] {
    let mut v = [f32::NAN; 3];
    for a in e.attributes().filter_map(Result::ok) {
        let slot = match a.key.local_name().as_ref() {
            "x" => 0,
            "y" => 1,
            "z" => 2,
            _ => continue,
        };
        v[slot] = fast_float2::parse(a.value.as_ref()).unwrap_or(f32::NAN);
    }
    v
}

struct Triangle {
    vertices: [u32; 3],
    /// Material resource and index (3MF materials extension).
    pid: Option<u32>,
    p1: Option<u32>,
    /// Multi-material painting (`paint_color` / `slic3rpe:mmu_segmentation`).
    paint: Option<String>,
}

/// `<triangle v1= v2= v3= …>` in one pass; invalid indices become `u32::MAX` and the triangle
/// is dropped later.
fn triangle(e: &BytesStart) -> Triangle {
    let mut t = Triangle { vertices: [u32::MAX; 3], pid: None, p1: None, paint: None };
    let number = |v: &str| v.trim().parse::<u32>().ok();
    for a in e.attributes().filter_map(Result::ok) {
        match a.key.local_name().as_ref() {
            "v1" => t.vertices[0] = number(&a.value).unwrap_or(u32::MAX),
            "v2" => t.vertices[1] = number(&a.value).unwrap_or(u32::MAX),
            "v3" => t.vertices[2] = number(&a.value).unwrap_or(u32::MAX),
            "pid" => t.pid = number(&a.value),
            "p1" => t.p1 = number(&a.value),
            "paint_color" | "mmu_segmentation" if !a.value.is_empty() => t.paint = Some(a.value.to_string()),
            _ => {}
        }
    }
    t
}

/// 3MF matrices are 4×3, row-vector convention: `m00 m01 m02 m10 … m32` with translation last.
fn transform(e: &BytesStart) -> Affine3A {
    let Some(text) = attr(e, "transform") else { return Affine3A::IDENTITY };
    let m: Vec<f32> = text.split_ascii_whitespace().filter_map(|s| s.parse().ok()).collect();
    if m.len() != 12 {
        return Affine3A::IDENTITY;
    }
    Affine3A::from_cols(
        Vec3::new(m[0], m[1], m[2]).into(),
        Vec3::new(m[3], m[4], m[5]).into(),
        Vec3::new(m[6], m[7], m[8]).into(),
        Vec3::new(m[9], m[10], m[11]).into(),
    )
}

fn unit_to_mm(unit: &str) -> f32 {
    match unit {
        "micron" => 0.001,
        "centimeter" => 10.0,
        "inch" => 25.4,
        "foot" => 304.8,
        "meter" => 1000.0,
        _ => 1.0, // millimeter (the default)
    }
}

fn normalize(name: &str) -> String {
    name.trim_start_matches('/').to_lowercase()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::tests_support::*;

    #[test]
    fn single_object_with_build_transform_and_metadata() {
        let pkg = package(&[(
            "3D/3dmodel.model",
            model(
                "millimeter",
                "<metadata name=\"Title\">Tom &amp; Jerry</metadata><metadata name=\"Application\">FreeCAD</metadata>",
                &cube_object(1, "type=\"model\""),
                "<item objectid=\"1\" transform=\"10 0 0 0 10 0 0 0 10 100 0 5\"/>",
            ),
        )]);
        let m = parse(&pkg).unwrap();
        assert_eq!(m.mesh.triangle_count(), 12);
        let b = m.mesh.bounds().unwrap();
        assert_eq!((b.min, b.max), (Vec3::new(100.0, 0.0, 5.0), Vec3::new(110.0, 10.0, 15.0)));
        assert_eq!(m.title.as_deref(), Some("Tom & Jerry"));
        assert_eq!(m.application.as_deref(), Some("FreeCAD"));
        assert_eq!(m.designer, None);
    }

    #[test]
    fn metadata_is_read_without_the_mesh() {
        let pkg = package(&[(
            "3D/3dmodel.model",
            model(
                "millimeter",
                "<metadata name=\"Title\">Rankhilfe &amp; Topf</metadata><metadata name=\"Designer\">DonDavis</metadata>\
                 <metadata name=\"License\">CC BY 4.0</metadata><metadata name=\"Description\">Für &lt;Kletterrosen&gt;</metadata>",
                &cube_object(1, "type=\"model\""),
                "<item objectid=\"1\"/>",
            ),
        )]);
        let m = metadata(&pkg).unwrap();
        assert_eq!(m.title.as_deref(), Some("Rankhilfe & Topf"));
        assert_eq!(m.designer.as_deref(), Some("DonDavis"));
        assert_eq!(m.license.as_deref(), Some("CC BY 4.0"));
        assert_eq!(m.description.as_deref(), Some("Für <Kletterrosen>"));
        assert_eq!(m.application, None);
        assert!(metadata(b"no zip").is_err());
    }

    #[test]
    fn production_extension_components_in_other_parts() {
        let root = model(
            "millimeter",
            "",
            "<object id=\"10\" type=\"model\"><components>\
               <component p:path=\"/3D/Objects/part.model\" objectid=\"1\" transform=\"2 0 0 0 2 0 0 0 2 0 0 0\"/>\
               <component p:path=\"/3D/Objects/part.model\" objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 5 0 0\"/>\
             </components></object>",
            "<item objectid=\"10\" transform=\"1 0 0 0 1 0 0 0 1 0 0 1\"/>",
        );
        let part = model("millimeter", "", &cube_object(1, ""), "");
        let pkg = package(&[("3D/3dmodel.model", root), ("3D/Objects/part.model", part)]);
        let m = parse(&pkg).unwrap();
        assert_eq!(m.mesh.triangle_count(), 24);
        let b = m.mesh.bounds().unwrap();
        assert_eq!((b.min, b.max), (Vec3::new(0.0, 0.0, 1.0), Vec3::new(6.0, 2.0, 3.0)));
    }

    #[test]
    fn units_are_converted_and_supports_skipped() {
        let res = format!("{}{}", cube_object(1, ""), cube_object(2, "type=\"support\""));
        let pkg =
            package(&[("3D/3dmodel.model", model("inch", "", &res, "<item objectid=\"1\"/><item objectid=\"2\"/>"))]);
        let m = parse(&pkg).unwrap();
        assert_eq!(m.mesh.triangle_count(), 12, "support object must not be part of the model");
        assert_eq!(m.mesh.bounds().unwrap().size(), Vec3::splat(25.4));
    }

    #[test]
    fn broken_references_are_errors() {
        let missing =
            package(&[("3D/3dmodel.model", model("millimeter", "", &cube_object(1, ""), "<item objectid=\"7\"/>"))]);
        assert!(parse(&missing).is_err());
        let cyclic = package(&[(
            "3D/3dmodel.model",
            model(
                "millimeter",
                "",
                "<object id=\"1\"><components><component objectid=\"1\"/></components></object>",
                "<item objectid=\"1\"/>",
            ),
        )]);
        assert!(parse(&cyclic).is_err());
        assert!(parse(b"not a zip").is_err());
    }

    #[test]
    fn embedded_previews_skip_blank_plates() {
        let blank = RgbaImage::from_pixel(64, 64, image::Rgba([200, 200, 200, 255]));
        let mut picture = RgbaImage::new(512, 256);
        for (x, _, p) in picture.enumerate_pixels_mut() {
            *p = image::Rgba([(x / 2) as u8, 100, 50, 255]);
        }
        let rels = format!(
            "<Relationships><Relationship Target=\"/Metadata/plate_1.png\" Id=\"r1\" Type=\"{REL_THUMBNAIL}\"/></Relationships>"
        );
        let pkg = package(&[
            ("_rels/.rels", rels.into_bytes()),
            ("Metadata/plate_1.png", png(&blank)),
            ("Metadata/plate_2.png", png(&picture)),
            ("Metadata/thumbnail.png", png(&picture)),
        ]);
        let thumb = embedded_thumbnail(&pkg, 128).expect("falls back past the blank plate");
        assert_eq!((thumb.width(), thumb.height()), (128, 64));
        let plates = plate_images(&pkg);
        assert_eq!(plates.iter().map(|(n, _)| *n).collect::<Vec<_>>(), [2]);
        assert!(embedded_thumbnail(&package(&[("Metadata/plate_1.png", png(&blank))]), 128).is_none());
    }
}

#[cfg(test)]
mod plate_tests {
    use super::tests_support::*;
    use super::*;

    #[test]
    fn multi_plate_projects_show_one_plate_at_a_time() {
        let resources = format!("{}{}", cube_object(1, ""), cube_object(2, ""));
        let root = model(
            "millimeter",
            "",
            &resources,
            "<item objectid=\"1\"/><item objectid=\"2\" transform=\"2 0 0 0 2 0 0 0 2 300 0 0\"/>",
        );
        let config = "<config>\
            <plate><metadata key=\"plater_id\" value=\"1\"/>\
              <model_instance><metadata key=\"object_id\" value=\"1\"/><metadata key=\"instance_id\" value=\"0\"/></model_instance></plate>\
            <plate><metadata key=\"plater_id\" value=\"2\"/>\
              <model_instance><metadata key=\"object_id\" value=\"2\"/></model_instance></plate>\
            <plate><metadata key=\"plater_id\" value=\"3\"/></plate>\
          </config>";
        let pkg =
            package(&[("3D/3dmodel.model", root), ("Metadata/model_settings.config", config.as_bytes().to_vec())]);

        let first = parse(&pkg).unwrap();
        assert_eq!(first.plates, [1, 2], "empty plate 3 is not listed");
        assert_eq!(first.plate, Some(1));
        assert_eq!(first.mesh.bounds().unwrap().size(), Vec3::ONE);

        let second = parse_plate(&pkg, Some(2)).unwrap();
        assert_eq!(second.plate, Some(2));
        assert_eq!(second.mesh.bounds().unwrap().size(), Vec3::splat(2.0));

        let unknown = parse_plate(&pkg, Some(9)).unwrap();
        assert_eq!(unknown.plate, Some(1));
        assert!(first.mesh.colors.is_none(), "no colour information, no colours");
    }

    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    fn project_settings() -> Vec<u8> {
        br##"{"filament_colour": ["#FF0000", "#00FF00", "#0000FF"], "filament_type": ["PLA", "PLA", "PETG"]}"##.to_vec()
    }

    #[test]
    fn bambu_parts_print_with_their_filaments() {
        // Object 3 = parts 1 (object's filament), 2 (filament 3) and 4 (a modifier, not printed).
        let resources = format!(
            "{}{}{}<object id=\"3\"><components><component objectid=\"1\"/>\
             <component objectid=\"2\" transform=\"1 0 0 0 1 0 0 0 1 5 0 0\"/><component objectid=\"4\"/></components></object>",
            cube_object(1, ""),
            cube_object(2, ""),
            cube_object(4, "")
        );
        let root = model("millimeter", "", &resources, "<item objectid=\"3\"/>");
        let config = "<config><object id=\"3\"><metadata key=\"extruder\" value=\"1\"/>\
            <part id=\"1\" subtype=\"normal_part\"><metadata key=\"name\" value=\"a\"/></part>\
            <part id=\"2\" subtype=\"normal_part\"><metadata key=\"extruder\" value=\"3\"/></part>\
            <part id=\"4\" subtype=\"modifier_part\"><metadata key=\"extruder\" value=\"2\"/></part>\
            </object></config>";
        let pkg = package(&[
            ("3D/3dmodel.model", root),
            ("Metadata/model_settings.config", config.as_bytes().to_vec()),
            ("Metadata/project_settings.config", project_settings()),
        ]);
        let m = parse(&pkg).unwrap();
        assert_eq!(m.mesh.triangle_count(), 24, "the modifier is not geometry");
        let colors = m.mesh.colors.unwrap();
        assert!(colors[..12].iter().all(|c| *c == RED) && colors[12..].iter().all(|c| *c == BLUE));
        assert_eq!(
            m.filaments,
            [
                Filament { slot: 1, color: [255, 0, 0], material: Some("PLA".into()) },
                Filament { slot: 3, color: [0, 0, 255], material: Some("PETG".into()) }
            ]
        );
    }

    #[test]
    fn painted_triangles_are_split_for_the_viewer_only() {
        // The first triangle is split three ways; its four parts are painted 1, 2, 1, 2.
        let cube = cube_object(1, "");
        let painted = cube.replacen("/>", " paint_color=\"84843\"/>", cube.matches("<vertex").count() + 1);
        let painted = painted.replacen(" paint_color=\"84843\"/>", "/>", cube.matches("<vertex").count());
        let root = model("millimeter", "", &painted, "<item objectid=\"1\"/>");
        let pkg = package(&[("3D/3dmodel.model", root), ("Metadata/project_settings.config", project_settings())]);

        let facts = parse(&pkg).unwrap();
        assert_eq!(facts.mesh.triangle_count(), 12, "facts keep the file's triangles");
        assert_eq!(facts.filaments.iter().map(|f| f.slot).collect::<Vec<_>>(), [1, 2], "the paint uses filament 2");

        let shown = parse_plate(&pkg, None).unwrap();
        assert_eq!(shown.mesh.triangle_count(), 15);
        assert_eq!(shown.triangles, 12, "the file's count for the details panel");
        let colors = shown.mesh.colors.clone().unwrap();
        assert_eq!(colors.iter().filter(|c| **c == GREEN).count(), 2);
        let area = |m: &Mesh| m.triangles().map(|t| (t[1] - t[0]).cross(t[2] - t[0]).length() * 0.5).sum::<f32>();
        assert!((area(&shown.mesh) - area(&facts.mesh)).abs() < 1e-4, "splitting keeps the surface");
    }

    #[test]
    fn material_colours_of_plain_3mf_files() {
        let resources = format!(
            "<m:colorgroup id=\"5\"><m:color color=\"#FF0000\"/><m:color color=\"#0000FFFF\"/></m:colorgroup>{}",
            cube_object(1, "pid=\"5\" pindex=\"0\"").replacen("/>", " pid=\"5\" p1=\"1\"/>", 9).replacen(
                " pid=\"5\" p1=\"1\"/>",
                "/>",
                8
            )
        );
        let root = model("millimeter", "", &resources, "<item objectid=\"1\"/>");
        let m = parse(&package(&[("3D/3dmodel.model", root)])).unwrap();
        let colors = m.mesh.colors.unwrap();
        assert_eq!(colors[0], BLUE, "the triangle's own material");
        assert!(colors[1..].iter().all(|c| *c == RED), "the object's default material");
        assert!(m.filaments.is_empty());
    }

    #[test]
    fn prusa_volumes_and_extruder_colours() {
        let root = model("millimeter", "", &cube_object(1, ""), "<item objectid=\"1\"/>");
        let model_config = "<config><object id=\"1\"><metadata type=\"object\" key=\"extruder\" value=\"1\"/>\
            <volume firstid=\"0\" lastid=\"5\"><metadata type=\"volume\" key=\"extruder\" value=\"2\"/>\
            <metadata type=\"volume\" key=\"volume_type\" value=\"ModelPart\"/></volume>\
            <volume firstid=\"6\" lastid=\"11\"><metadata type=\"volume\" key=\"volume_type\" value=\"ParameterModifier\"/></volume>\
            </object></config>";
        let print_config = "; generated by PrusaSlicer\n; extruder_colour = \"\";\"#00FF00\"\n; filament_colour = #FF0000;#0000FF\n; filament_type = PLA;PETG\n";
        let pkg = package(&[
            ("3D/3dmodel.model", root),
            ("Metadata/Slic3r_PE_model.config", model_config.as_bytes().to_vec()),
            ("Metadata/Slic3r_PE.config", print_config.as_bytes().to_vec()),
        ]);
        let m = parse(&pkg).unwrap();
        assert_eq!(m.mesh.triangle_count(), 6, "the modifier volume is not geometry");
        assert!(m.mesh.colors.unwrap().iter().all(|c| *c == GREEN), "extruder colour wins over filament colour");
        assert_eq!(m.filaments, [Filament { slot: 2, color: [0, 255, 0], material: Some("PETG".into()) }]);
    }
}

#[cfg(test)]
mod tests_support {
    use super::*;
    use std::io::Write;

    const CORE_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";
    const PROD_NS: &str = "http://schemas.microsoft.com/3dmanufacturing/production/2015/06";

    pub fn package(parts: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in parts {
            w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    /// Unit cube object (8 vertices, 12 triangles) as 3MF XML.
    pub fn cube_object(id: u32, extra: &str) -> String {
        let cube = Mesh::cube(1.0);
        let verts: String =
            cube.positions.iter().map(|[x, y, z]| format!("<vertex x=\"{x}\" y=\"{y}\" z=\"{z}\"/>")).collect();
        let tris: String = cube
            .indices
            .unwrap()
            .chunks(3)
            .map(|t| format!("<triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>", t[0], t[1], t[2]))
            .collect();
        format!(
            "<object id=\"{id}\" {extra}><mesh><vertices>{verts}</vertices><triangles>{tris}</triangles></mesh></object>"
        )
    }

    pub fn model(unit: &str, metadata: &str, resources: &str, build: &str) -> Vec<u8> {
        format!(
            "<?xml version=\"1.0\"?><model unit=\"{unit}\" xmlns=\"{CORE_NS}\" xmlns:p=\"{PROD_NS}\">{metadata}\
             <resources>{resources}</resources><build>{build}</build></model>"
        )
        .into_bytes()
    }

    pub fn png(img: &RgbaImage) -> Vec<u8> {
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }
}
