//! 3MF reader: geometry of all build items (with component trees, transforms, units and the
//! Production extension's `p:path` sub-models used by Bambu Studio / Orca / Anycubic / Elegoo),
//! plus the preview images slicers embed.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use glam::{Affine3A, Vec3};
use image::RgbaImage;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use super::LoadError;
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
}

/// Parses the model. Multi-plate slicer projects show their first plate, matching the
/// plate picture used as thumbnail; see [`parse_plate`] for the others.
pub fn parse(bytes: &[u8]) -> Result<ThreeMf, LoadError> {
    parse_plate(bytes, None)
}

/// Like [`parse`], but shows build plate `plate` (falls back to everything if it is unknown).
pub fn parse_plate(bytes: &[u8], plate: Option<usize>) -> Result<ThreeMf, LoadError> {
    let mut package = Package::open(bytes)?;
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

    let mut out = Mesh { positions: Vec::new(), indices: Some(Vec::new()) };
    let on_plate: Vec<&BuildItem> = build.iter().filter(|i| wanted.is_none_or(|ids| ids.contains(&i.object))).collect();
    // Objects parked outside every plate would leave nothing to show: then show them all.
    let (items, shown) = if on_plate.is_empty() { (build.iter().collect(), None) } else { (on_plate, shown) };
    for item in items {
        let path = item.path.clone().unwrap_or_else(|| start.clone());
        package.emit(&path, item.object, item.transform, 0, &mut out)?;
    }
    if out.is_empty() {
        return Err(LoadError::Empty);
    }
    if scale != 1.0 {
        for p in &mut out.positions {
            *p = (Vec3::from(*p) * scale).to_array();
        }
    }
    Ok(ThreeMf { mesh: out, title, designer, application, plates, plate: shown })
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

fn fit(image: RgbaImage, max: u32) -> RgbaImage {
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

    /// Appends object `id` of model part `path`, transformed, to `out` (recursing into components).
    fn emit(
        &mut self,
        path: &str,
        id: u32,
        transform: Affine3A,
        depth: usize,
        out: &mut Mesh,
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
            let base = out.positions.len() as u32;
            out.positions.extend(mesh.vertices.iter().map(|v| transform.transform_point3(Vec3::from(*v)).to_array()));
            let indices = out.indices.get_or_insert_with(Vec::new);
            for t in &mesh.triangles {
                if t.iter().all(|&i| (i as usize) < mesh.vertices.len()) {
                    indices.extend(t.iter().map(|i| base + i));
                }
            }
        }
        for c in &object.components {
            let sub_path = c.path.clone().unwrap_or_else(|| path.to_owned());
            self.emit(&sub_path, c.object, transform * c.transform, depth + 1, out)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
struct ObjectMesh {
    vertices: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
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
}

impl ModelFile {
    fn parse(data: &[u8]) -> Result<ModelFile, LoadError> {
        let mut reader = Reader::from_reader(data);
        let mut model = ModelFile { unit_scale: 1.0, ..Default::default() };
        let mut current: Option<(u32, Object, ObjectMesh)> = None;
        let mut metadata_name: Option<String> = None;
        let mut metadata_text = String::new();
        let bad = |e: quick_xml::Error| LoadError::Malformed(format!("XML: {e}"));

        loop {
            let event = reader.read_event().map_err(bad)?;
            match &event {
                Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                    "model" => model.unit_scale = attr(e, "unit").map_or(1.0, |u| unit_to_mm(&u)),
                    "vertex" => {
                        if let Some((_, _, mesh)) = &mut current {
                            mesh.vertices.push(vertex(e));
                        }
                    }
                    "triangle" => {
                        if let Some((_, _, mesh)) = &mut current {
                            mesh.triangles.push(triangle(e));
                        }
                    }
                    "object" => {
                        let id = attr(e, "id").and_then(|v| v.parse().ok()).unwrap_or(0);
                        let printable = !matches!(attr(e, "type").as_deref(), Some("support" | "other"));
                        current = Some((
                            id,
                            Object { printable, mesh: Default::default(), components: Vec::new() },
                            ObjectMesh::default(),
                        ));
                    }
                    "component" => {
                        if let Some((_, object, _)) = &mut current {
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
                    "object" => {
                        if let Some((id, mut object, mesh)) = current.take() {
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

/// `<triangle v1= v2= v3=>`; invalid indices become `u32::MAX` and the triangle is dropped later.
fn triangle(e: &BytesStart) -> [u32; 3] {
    let mut t = [u32::MAX; 3];
    for a in e.attributes().filter_map(Result::ok) {
        let slot = match a.key.local_name().as_ref() {
            "v1" => 0,
            "v2" => 1,
            "v3" => 2,
            _ => continue,
        };
        t[slot] = a.value.as_ref().trim().parse().unwrap_or(u32::MAX);
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
