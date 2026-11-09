//! A copy of a 3MF in which some objects have a new mesh and everything else is as it was: the
//! other objects, where each stands, the plates, the slicer's settings and profiles. For
//! plugins that work on one object of a project (a texture on the lid, not on the box).
//!
//! The package is copied file by file, compressed as it is. Only the model parts that hold a
//! swapped object are written anew, and there only the `<mesh>` of that object. What a slicer
//! keeps per triangle of it (painted colours, supports, seams) goes with the old triangles.
//! The slicer's counts of them are set right, and the pictures of the plates are emptied: they
//! show the model as it was.

use std::collections::{HashMap, HashSet};
use std::io::{BufWriter, Read, Seek, Write};

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer};

use super::{Found, LoadError, Package, REL_THUMBNAIL, attr, normalize};
use crate::mesh::Mesh;

/// A new mesh for one part.
pub struct Replacement<'a> {
    /// The part's index in [`super::parse_parts`].
    pub part: usize,
    /// In millimetres and in the part's own coordinates, as [`super::Part::mesh`] is.
    pub mesh: &'a Mesh,
}

/// Writes the 3MF in `bytes` to `out` with the meshes of the parts named in `replacements`
/// swapped. Corners at the same place become one vertex, so a mesh that came as loose triangles
/// (an STL) is closed again; triangles that collapse are left out.
///
/// An object PrusaSlicer cuts into several volumes (parts or modifiers within one mesh) cannot
/// be replaced: the ranges would not fit the new triangles.
pub fn replace_parts<W: Write + Seek>(bytes: &[u8], replacements: &[Replacement], out: W) -> Result<(), LoadError> {
    let mut package = Package::open(bytes)?;
    let found = package.parts()?;
    let mut swaps: Vec<Swap> = Vec::new();
    for r in replacements {
        let part = found
            .get(r.part)
            .ok_or_else(|| bad(format!("the file has {} parts, there is no part {}", found.len(), r.part)))?;
        if swaps.iter().any(|s| std::ptr::eq(s.part, part)) {
            return Err(bad(format!("part {} is named twice", r.part)));
        }
        if part.volumes > 1 {
            return Err(bad(format!(
                "part {} is cut into several volumes within one mesh (PrusaSlicer parts or modifiers) \
                 and cannot be replaced by itself",
                r.part
            )));
        }
        swaps.push(Swap { part, mesh: NewMesh::of(r.mesh, part.unit_scale)? });
    }
    // The model parts to write anew, by their names in the archive: object id → its new mesh.
    let mut models: HashMap<String, HashMap<u32, &NewMesh>> = HashMap::new();
    for swap in &swaps {
        let name = package.real_name(&swap.part.part.model).cloned().unwrap_or_default();
        models.entry(name).or_default().insert(swap.part.part.object_id, &swap.mesh);
    }
    let previews: HashSet<String> =
        package.relationship_targets(REL_THUMBNAIL).iter().map(|target| normalize(target)).collect();

    let mut writer = zip::ZipWriter::new(out);
    for index in 0..package.zip.len() {
        let file = package.zip.by_index_raw(index).map_err(zip_error)?;
        let name = file.name().map_err(zip_error)?.into_owned();
        let key = normalize(&name);
        let swapped = models.get(&name);
        let config = match key.as_str() {
            "metadata/model_settings.config" => Some(Config::Bambu),
            "metadata/slic3r_pe_model.config" => Some(Config::Prusa),
            _ => None,
        };
        let picture = if is_preview(&key, &previews) { blank(&key) } else { None };
        if swapped.is_none() && config.is_none() && picture.is_none() {
            writer.raw_copy_file(file).map_err(zip_error)?;
            continue;
        }
        let mut options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .large_file(swapped.is_some_and(|meshes| meshes.values().any(|m| m.is_huge())));
        if let Some(time) = file.last_modified() {
            options = options.last_modified_time(time);
        }
        drop(file);
        if let Some(picture) = picture {
            writer.start_file(name, options).map_err(zip_error)?;
            writer.write_all(&picture)?;
            continue;
        }
        let mut data = Vec::new();
        package.zip.by_index(index).map_err(zip_error)?.read_to_end(&mut data)?;
        writer.start_file(name, options).map_err(zip_error)?;
        let mut to = BufWriter::with_capacity(1 << 16, &mut writer);
        match (swapped, config) {
            (Some(meshes), _) => write_model(&data, meshes, &mut to)?,
            (None, Some(Config::Bambu)) => write_bambu_config(&data, &swaps, &mut to)?,
            (None, _) => write_prusa_config(&data, &swaps, &mut to)?,
        }
        to.flush()?;
    }
    writer.finish().map_err(zip_error)?;
    Ok(())
}

fn bad(why: String) -> LoadError {
    LoadError::Malformed(why)
}

fn zip_error(e: zip::result::ZipError) -> LoadError {
    LoadError::Malformed(e.to_string())
}

fn xml_error(e: quick_xml::Error) -> LoadError {
    LoadError::Malformed(format!("XML: {e}"))
}

/// A slicer's own list of the objects, beside the model.
#[derive(Clone, Copy)]
enum Config {
    Bambu,
    Prusa,
}

struct Swap<'a> {
    part: &'a Found,
    mesh: NewMesh,
}

/// A mesh as the file takes it: in the file's unit, every place one vertex.
struct NewMesh {
    vertices: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
}

impl NewMesh {
    /// `mesh` (millimetres) in the unit that is `unit_scale` millimetres long.
    fn of(mesh: &Mesh, unit_scale: f32) -> Result<NewMesh, LoadError> {
        let mut ids: HashMap<[u32; 3], u32> = HashMap::new();
        let mut new = NewMesh { vertices: Vec::new(), triangles: Vec::new() };
        for triangle in mesh.triangles() {
            if triangle.iter().any(|p| !p.is_finite()) {
                return Err(bad("a coordinate of the new mesh is not a finite number".into()));
            }
            let corners = triangle.map(|p| {
                // -0.0 and 0.0 are one place.
                let p = (p + 0.0).to_array();
                *ids.entry(p.map(f32::to_bits)).or_insert_with(|| {
                    new.vertices.push(p.map(|c| c / unit_scale));
                    (new.vertices.len() - 1) as u32
                })
            });
            if corners[0] != corners[1] && corners[1] != corners[2] && corners[0] != corners[2] {
                new.triangles.push(corners);
            }
        }
        if new.triangles.is_empty() {
            return Err(LoadError::Empty);
        }
        Ok(new)
    }

    /// Whether its XML may pass 4 GiB, which a ZIP entry has to be told before it is written.
    fn is_huge(&self) -> bool {
        (self.vertices.len() as u64 + self.triangles.len() as u64) * 64 > u64::from(u32::MAX) / 2
    }

    /// The inside of a `<mesh>`; `prefix` is the namespace prefix of that tag ("" or "m:").
    fn write(&self, prefix: &str, to: &mut impl Write) -> std::io::Result<()> {
        writeln!(to, "\n<{prefix}vertices>")?;
        for [x, y, z] in &self.vertices {
            writeln!(to, "<{prefix}vertex x=\"{x}\" y=\"{y}\" z=\"{z}\"/>")?;
        }
        writeln!(to, "</{prefix}vertices>\n<{prefix}triangles>")?;
        for [a, b, c] in &self.triangles {
            writeln!(to, "<{prefix}triangle v1=\"{a}\" v2=\"{b}\" v3=\"{c}\"/>")?;
        }
        writeln!(to, "</{prefix}triangles>")
    }
}

/// The model part `data` with the meshes of the objects in `meshes` (by their ids) swapped.
fn write_model(data: &[u8], meshes: &HashMap<u32, &NewMesh>, to: &mut impl Write) -> Result<(), LoadError> {
    let mut reader = Reader::from_reader(data);
    let mut writer = Writer::new(to);
    let mut object: Option<u32> = None;
    let mut swapped = 0;
    loop {
        match reader.read_event().map_err(xml_error)? {
            Event::Eof => break,
            Event::Start(e) if e.local_name().as_ref() == "object" => {
                object = attr(&e, "id").and_then(|id| id.trim().parse().ok());
                writer.write_event(Event::Start(e))?;
            }
            Event::End(e) if e.local_name().as_ref() == "object" => {
                object = None;
                writer.write_event(Event::End(e))?;
            }
            Event::Start(e)
                if e.local_name().as_ref() == "mesh" && object.is_some_and(|id| meshes.contains_key(&id)) =>
            {
                let mesh = meshes[&object.unwrap_or_default()];
                let name = e.name().as_ref().to_owned();
                let prefix = name.strip_suffix("mesh").unwrap_or_default().to_owned();
                writer.write_event(Event::Start(e))?;
                mesh.write(&prefix, writer.get_mut())?;
                // The old mesh, with all a slicer kept per triangle, is left out.
                let mut depth = 1;
                while depth > 0 {
                    match reader.read_event().map_err(xml_error)? {
                        Event::Start(_) => depth += 1,
                        Event::End(end) => {
                            depth -= 1;
                            if depth == 0 {
                                writer.write_event(Event::End(end))?;
                            }
                        }
                        Event::Eof => return Err(bad("a mesh does not end".into())),
                        _ => {}
                    }
                }
                swapped += 1;
            }
            event => writer.write_event(event)?,
        }
    }
    if swapped != meshes.len() {
        return Err(bad("an object to replace has no mesh in its model part".into()));
    }
    Ok(())
}

/// `tag` with attribute `key` set to `value` (added if it has none).
fn with_attribute(tag: &BytesStart, key: &str, value: &str) -> BytesStart<'static> {
    let mut new = BytesStart::new(tag.name().as_ref().to_owned());
    let mut set = false;
    for a in tag.attributes().filter_map(Result::ok) {
        if a.key.as_ref() == key {
            new.push_attribute((key, value));
            set = true;
        } else {
            new.push_attribute(a);
        }
    }
    if !set {
        new.push_attribute((key, value));
    }
    new
}

/// Writes `event` as it is, or with the tag `change` makes of its own.
fn write_changed(
    writer: &mut Writer<&mut impl Write>,
    event: Event,
    change: impl FnOnce(&BytesStart) -> Option<BytesStart<'static>>,
) -> std::io::Result<()> {
    match event {
        Event::Start(e) => match change(&e) {
            Some(new) => writer.write_event(Event::Start(new)),
            None => writer.write_event(Event::Start(e)),
        },
        Event::Empty(e) => match change(&e) {
            Some(new) => writer.write_event(Event::Empty(new)),
            None => writer.write_event(Event::Empty(e)),
        },
        other => writer.write_event(other),
    }
}

fn id_of(tag: &BytesStart) -> Option<u32> {
    attr(tag, "id").and_then(|id| id.trim().parse().ok())
}

/// Bambu/Orca/Anycubic `model_settings.config` with the triangle counts of the swapped parts
/// (`<mesh_stat face_count>` of the part) and of their objects (`<metadata face_count>`).
fn write_bambu_config(data: &[u8], swaps: &[Swap], to: &mut impl Write) -> Result<(), LoadError> {
    let count_of = |object: u32, part: u32| {
        swaps
            .iter()
            .find(|s| s.part.build_object == object && s.part.part.object_id == part)
            .map(|s| s.mesh.triangles.len())
    };
    // How many triangles an object has more than before.
    let grown = |object: u32| -> i64 {
        let of = swaps.iter().filter(|s| s.part.build_object == object);
        of.map(|s| s.mesh.triangles.len() as i64 - s.part.file_triangles as i64).sum()
    };
    let mut reader = Reader::from_reader(data);
    let mut writer = Writer::new(to);
    let (mut object, mut part): (Option<u32>, Option<u32>) = (None, None);
    loop {
        let event = reader.read_event().map_err(xml_error)?;
        match &event {
            Event::Eof => break,
            Event::Start(e) if e.local_name().as_ref() == "object" => object = id_of(e),
            Event::Start(e) if e.local_name().as_ref() == "part" => part = id_of(e),
            Event::End(e) if e.local_name().as_ref() == "part" => part = None,
            Event::End(e) if e.local_name().as_ref() == "object" => (object, part) = (None, None),
            _ => {}
        }
        write_changed(&mut writer, event, |tag| {
            let old: i64 = attr(tag, "face_count")?.trim().parse().ok()?;
            let new = match (tag.local_name().as_ref(), object, part) {
                ("mesh_stat", Some(object), Some(part)) => count_of(object, part)? as i64,
                ("metadata", Some(object), None) => old + Some(grown(object)).filter(|g| *g != 0)?,
                _ => return None,
            };
            Some(with_attribute(tag, "face_count", &new.max(0).to_string()))
        })?;
    }
    Ok(())
}

/// PrusaSlicer's `Slic3r_PE_model.config` with the one volume of each swapped object reaching
/// over all its new triangles.
fn write_prusa_config(data: &[u8], swaps: &[Swap], to: &mut impl Write) -> Result<(), LoadError> {
    let count_of = |object: u32| {
        let own = |s: &&Swap| s.part.build_object == object && s.part.part.object_id == object;
        swaps.iter().find(own).map(|s| s.mesh.triangles.len())
    };
    let mut reader = Reader::from_reader(data);
    let mut writer = Writer::new(to);
    let mut object: Option<u32> = None;
    loop {
        let event = reader.read_event().map_err(xml_error)?;
        match &event {
            Event::Eof => break,
            Event::Start(e) if e.local_name().as_ref() == "object" => object = id_of(e),
            Event::End(e) if e.local_name().as_ref() == "object" => object = None,
            _ => {}
        }
        write_changed(&mut writer, event, |tag| {
            if tag.local_name().as_ref() != "volume" {
                return None;
            }
            let last = count_of(object?)?.saturating_sub(1).to_string();
            Some(with_attribute(&with_attribute(tag, "firstid", "0"), "lastid", &last))
        })?;
    }
    Ok(())
}

/// Whether the file `key` (lower case) is a picture of the model: the package's thumbnail, or a
/// slicer's picture of a plate.
fn is_preview(key: &str, thumbnails: &HashSet<String>) -> bool {
    let plate = |prefix: &str| key.strip_prefix(prefix).is_some_and(|rest| rest.ends_with(".png"));
    thumbnails.contains(key)
        || matches!(key, "metadata/thumbnail.png" | "metadata/thumbnail.jpg" | "thumbnails/thumbnail.png")
        || plate("metadata/plate_")
        || plate("metadata/top_")
}

/// An empty picture of the kind `key` is, which readers take for "no preview"; `None` for
/// other kinds of files.
fn blank(key: &str) -> Option<Vec<u8>> {
    let mut out = std::io::Cursor::new(Vec::new());
    if key.ends_with(".png") {
        let empty = image::RgbaImage::from_pixel(16, 16, image::Rgba([255, 255, 255, 0]));
        empty.write_to(&mut out, image::ImageFormat::Png).ok()?;
    } else if key.ends_with(".jpg") || key.ends_with(".jpeg") {
        let empty = image::RgbImage::from_pixel(16, 16, image::Rgb([255, 255, 255]));
        empty.write_to(&mut out, image::ImageFormat::Jpeg).ok()?;
    } else {
        return None;
    }
    Some(out.into_inner())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use glam::Vec3;

    use super::super::tests_support::*;
    use super::super::{embedded_thumbnail, parse, parse_parts};
    use super::*;

    /// A box of `size` as loose triangles, as an STL gives them.
    fn soup(size: f32) -> Mesh {
        let cube = Mesh::cube(size);
        Mesh { positions: cube.to_soup(), indices: None, colors: None, vertex_colors: None }
    }

    fn replaced(pkg: &[u8], replacements: &[Replacement]) -> Result<Vec<u8>, LoadError> {
        let mut out = Cursor::new(Vec::new());
        replace_parts(pkg, replacements, &mut out)?;
        Ok(out.into_inner())
    }

    /// The bytes of file `name` in a package.
    fn file(pkg: &[u8], name: &str) -> Vec<u8> {
        let mut zip = zip::ZipArchive::new(Cursor::new(pkg)).unwrap();
        let mut data = Vec::new();
        zip.by_name(name).unwrap().read_to_end(&mut data).unwrap();
        data
    }

    fn names(pkg: &[u8]) -> Vec<String> {
        let zip = zip::ZipArchive::new(Cursor::new(pkg)).unwrap();
        zip.file_names().map(|n| n.unwrap().into_owned()).collect()
    }

    #[test]
    fn one_object_gets_a_new_mesh_and_the_other_stays() {
        let resources = format!("{}{}", cube_object(1, "name=\"Dose\""), cube_object(2, "name=\"Deckel\""));
        let build = "<item objectid=\"1\"/><item objectid=\"2\" transform=\"1 0 0 0 1 0 0 0 1 50 0 0\"/>";
        let root = model("millimeter", "<metadata name=\"Title\">Dose &amp; Deckel</metadata>", &resources, build);
        let pkg = package(&[
            ("[Content_Types].xml", b"<Types/>".to_vec()),
            ("3D/3dmodel.model", root),
            ("Metadata/notes.txt", b"keep me".to_vec()),
        ]);
        let lid = soup(5.0);
        let new = replaced(&pkg, &[Replacement { part: 1, mesh: &lid }]).unwrap();

        assert_eq!(names(&new), names(&pkg), "the same files in the same order");
        for kept in ["[Content_Types].xml", "Metadata/notes.txt"] {
            assert_eq!(file(&new, kept), file(&pkg, kept), "{kept}");
        }
        let parts = parse_parts(&new).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!((parts[0].name.as_deref(), parts[1].name.as_deref()), (Some("Dose"), Some("Deckel")));
        let before = parse_parts(&pkg).unwrap();
        assert_eq!(parts[0].mesh.positions, before[0].mesh.positions, "the box is as it was");
        assert_eq!(parts[0].mesh.indices, before[0].mesh.indices);
        assert_eq!(parts[1].mesh.bounds().unwrap().size(), Vec3::splat(5.0), "the lid is the new one");
        assert_eq!((parts[1].mesh.triangle_count(), parts[1].mesh.positions.len()), (12, 8), "its corners are one");
        assert_eq!(parts[1].transform, before[1].transform, "where it stood");
        let whole = parse(&new).unwrap();
        assert_eq!(whole.title.as_deref(), Some("Dose & Deckel"));
        assert_eq!(whole.mesh.bounds().unwrap().max, Vec3::new(55.0, 5.0, 5.0));
    }

    /// A slicer project: an object of two parts and a modifier in a sub-model, painted, with
    /// the slicer's settings, plates, profiles and pictures around it.
    fn project() -> Vec<u8> {
        let root = model(
            "millimeter",
            "<metadata name=\"Application\">BambuStudio-02.02.00.85</metadata>",
            "<object id=\"10\" p:UUID=\"0001\" type=\"model\"><components>\
               <component p:path=\"/3D/Objects/object_1.model\" objectid=\"1\" p:UUID=\"0002\"/>\
               <component p:path=\"/3D/Objects/object_1.model\" objectid=\"2\" transform=\"1 0 0 0 1 0 0 0 1 5 0 0\"/>\
               <component p:path=\"/3D/Objects/object_1.model\" objectid=\"3\"/>\
             </components></object>",
            "<item objectid=\"10\" transform=\"1 0 0 0 1 0 0 0 1 128 128 0\" printable=\"1\"/>",
        );
        let painted = |id: u32| cube_object(id, "").replace("\"/></triangles>", "\" paint_color=\"8\"/></triangles>");
        let sub = model("millimeter", "", &format!("{}{}{}", painted(1), painted(2), cube_object(3, "")), "");
        let config = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<config>\n  <object id=\"10\">\n    \
            <metadata key=\"name\" value=\"Dose\"/>\n    <metadata key=\"extruder\" value=\"1\"/>\n    \
            <metadata face_count=\"36\"/>\n    \
            <part id=\"1\" subtype=\"normal_part\"><metadata key=\"name\" value=\"Wand\"/>\
            <mesh_stat face_count=\"12\" edges_fixed=\"0\"/></part>\n    \
            <part id=\"2\" subtype=\"normal_part\"><metadata key=\"name\" value=\"Boden\"/>\
            <metadata key=\"extruder\" value=\"2\"/><mesh_stat face_count=\"12\" edges_fixed=\"0\"/></part>\n    \
            <part id=\"3\" subtype=\"modifier_part\"><metadata key=\"name\" value=\"Infill\"/>\
            <mesh_stat face_count=\"12\" edges_fixed=\"0\"/></part>\n  </object>\n  \
            <plate>\n    <metadata key=\"plater_id\" value=\"1\"/>\n    <metadata key=\"plater_name\" value=\"Erste\"/>\n    \
            <model_instance><metadata key=\"object_id\" value=\"10\"/></model_instance>\n  </plate>\n</config>\n";
        let rels = format!(
            "<Relationships><Relationship Target=\"/3D/3dmodel.model\" Id=\"rel-1\" Type=\"{}\"/>\
             <Relationship Target=\"/Metadata/plate_1.png\" Id=\"rel-2\" Type=\"{REL_THUMBNAIL}\"/></Relationships>",
            super::super::REL_MODEL
        );
        let mut picture = image::RgbaImage::new(64, 64);
        for (x, _, p) in picture.enumerate_pixels_mut() {
            *p = image::Rgba([(x * 4) as u8, 100, 50, 255]);
        }
        package(&[
            ("[Content_Types].xml", b"<Types/>".to_vec()),
            ("_rels/.rels", rels.into_bytes()),
            ("3D/3dmodel.model", root),
            ("3D/_rels/3dmodel.model.rels", b"<Relationships/>".to_vec()),
            ("3D/Objects/object_1.model", sub),
            ("Metadata/model_settings.config", config.as_bytes().to_vec()),
            ("Metadata/project_settings.config", br##"{"filament_colour": ["#FF0000", "#00FF00"]}"##.to_vec()),
            ("Metadata/slice_info.config", b"<config><plate/></config>".to_vec()),
            ("Metadata/plate_1.png", png(&picture)),
            ("Metadata/pick_1.png", png(&picture)),
        ])
    }

    #[test]
    fn a_slicer_project_keeps_its_plates_settings_and_other_parts() {
        let pkg = project();
        let before = parse_parts(&pkg).unwrap();
        assert_eq!(before.iter().map(|p| p.name.as_deref().unwrap()).collect::<Vec<_>>(), ["Wand", "Boden"]);
        // A new floor of 20 triangles: a box with a pyramid of four more on top, as loose triangles.
        let mut floor = soup(2.0);
        let top = [[0.0, 0.0, 2.0], [2.0, 0.0, 2.0], [2.0, 2.0, 2.0], [0.0, 2.0, 2.0]];
        for i in 0..4 {
            floor.positions.extend([top[i], top[(i + 1) % 4], [1.0, 1.0, 3.0]]);
        }
        let new = replaced(&pkg, &[Replacement { part: 1, mesh: &floor }]).unwrap();

        // What the slicer made of the project is all there, to the byte.
        assert_eq!(names(&new), names(&pkg));
        for kept in [
            "[Content_Types].xml",
            "_rels/.rels",
            "3D/3dmodel.model",
            "3D/_rels/3dmodel.model.rels",
            "Metadata/project_settings.config",
            "Metadata/slice_info.config",
            "Metadata/pick_1.png",
        ] {
            assert_eq!(file(&new, kept), file(&pkg, kept), "{kept}");
        }
        // The settings: the plate, the names and the filaments as they were, the counts set right.
        let config = String::from_utf8(file(&new, "Metadata/model_settings.config")).unwrap();
        let old_config = String::from_utf8(file(&pkg, "Metadata/model_settings.config")).unwrap();
        assert_eq!(config.split("<plate>").nth(1), old_config.split("<plate>").nth(1), "the plate");
        assert!(config.contains("<metadata face_count=\"40\"/>"), "36 - 12 + 16: {config}");
        assert!(config.contains("<metadata key=\"name\" value=\"Boden\"/><metadata key=\"extruder\" value=\"2\"/><mesh_stat face_count=\"16\" edges_fixed=\"0\"/>"));
        assert_eq!(config.matches("face_count=\"12\"").count(), 2, "the wall and the modifier");
        assert!(config.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<config>\n  <object id=\"10\">"));

        // The sub-model: the wall still painted, the floor new and unpainted, the modifier there.
        let sub = String::from_utf8(file(&new, "3D/Objects/object_1.model")).unwrap();
        assert_eq!(sub.matches("paint_color").count(), 1, "the wall's paint stays, the floor's went with its mesh");
        assert_eq!(sub.matches("<object id=").count(), 3);
        let parts = parse_parts(&new).unwrap();
        assert_eq!(parts.len(), 2, "the modifier is still no part");
        assert_eq!(parts[0].mesh.positions, before[0].mesh.positions);
        assert_eq!((parts[1].name.as_deref(), parts[1].mesh.triangle_count()), (Some("Boden"), 16));
        assert_eq!(parts[1].mesh.bounds().unwrap().max, Vec3::new(2.0, 2.0, 3.0));
        assert_eq!(parts[1].transform, before[1].transform);
        let whole = parse(&new).unwrap();
        assert_eq!((whole.mesh.triangle_count(), whole.plates.as_slice()), (28, &[1usize][..]));
        assert_eq!(whole.filaments.iter().map(|f| f.slot).collect::<Vec<_>>(), [1, 2], "the parts' filaments");
        // The picture of the plate showed the old model: it is empty now, so viewers draw their own.
        assert!(embedded_thumbnail(&pkg, 64).is_some() && embedded_thumbnail(&new, 64).is_none());

        // Both parts at once, and nothing at all.
        let both =
            replaced(&pkg, &[Replacement { part: 0, mesh: &floor }, Replacement { part: 1, mesh: &floor }]).unwrap();
        let config = String::from_utf8(file(&both, "Metadata/model_settings.config")).unwrap();
        assert!(config.contains("<metadata face_count=\"44\"/>") && config.matches("face_count=\"16\"").count() == 2);
        assert_eq!(parse(&both).unwrap().mesh.triangle_count(), 32);
        let same = replaced(&pkg, &[]).unwrap();
        assert_eq!(file(&same, "3D/Objects/object_1.model"), file(&pkg, "3D/Objects/object_1.model"));
        assert_eq!(file(&same, "Metadata/model_settings.config"), file(&pkg, "Metadata/model_settings.config"));
    }

    #[test]
    fn prusa_projects_keep_one_volume_and_refuse_several() {
        let root = model("millimeter", "", &cube_object(1, ""), "<item objectid=\"1\"/>");
        let config = |volumes: &str| {
            format!(
                "<config><object id=\"1\" instances_count=\"1\"><metadata type=\"object\" key=\"name\" value=\"Würfel\"/>\
                 {volumes}</object></config>"
            )
        };
        let one = "<volume firstid=\"0\" lastid=\"11\"><metadata type=\"volume\" key=\"volume_type\" value=\"ModelPart\"/>\
                   <mesh edges_fixed=\"0\"/></volume>";
        let pkg = |volumes: &str| {
            package(&[
                ("3D/3dmodel.model", root.clone()),
                ("Metadata/Slic3r_PE_model.config", config(volumes).into_bytes()),
                ("Metadata/Slic3r_PE.config", b"; filament_colour = #FF0000\n".to_vec()),
            ])
        };
        let mut mesh = soup(2.0);
        mesh.positions.extend([[0.0, 0.0, 2.0], [2.0, 0.0, 2.0], [1.0, 1.0, 3.0]]);
        let new = replaced(&pkg(one), &[Replacement { part: 0, mesh: &mesh }]).unwrap();
        let written = String::from_utf8(file(&new, "Metadata/Slic3r_PE_model.config")).unwrap();
        assert!(written.contains("<volume firstid=\"0\" lastid=\"12\">"), "13 triangles: {written}");
        assert!(written.contains("value=\"Würfel\"") && written.contains("<mesh edges_fixed=\"0\"/>"));
        assert_eq!(file(&new, "Metadata/Slic3r_PE.config"), b"; filament_colour = #FF0000\n");
        assert_eq!(parse(&new).unwrap().mesh.triangle_count(), 13);

        let two = "<volume firstid=\"0\" lastid=\"5\"><metadata type=\"volume\" key=\"volume_type\" value=\"ModelPart\"/></volume>\
                   <volume firstid=\"6\" lastid=\"11\"><metadata type=\"volume\" key=\"volume_type\" value=\"ParameterModifier\"/></volume>";
        let err = replaced(&pkg(two), &[Replacement { part: 0, mesh: &mesh }]).unwrap_err();
        assert!(err.to_string().contains("several volumes"), "{err}");
    }

    #[test]
    fn the_new_mesh_is_written_in_the_files_unit() {
        let pkg = package(&[(
            "3D/3dmodel.model",
            model("inch", "", &cube_object(1, ""), "<item objectid=\"1\" transform=\"1 0 0 0 1 0 0 0 1 2 0 0\"/>"),
        )]);
        let new = replaced(&pkg, &[Replacement { part: 0, mesh: &soup(50.8) }]).unwrap();
        let written = String::from_utf8(file(&new, "3D/3dmodel.model")).unwrap();
        assert!(written.contains("<vertex x=\"2\" y=\"2\" z=\"2\"/>"), "two inches: {written}");
        let parts = parse_parts(&new).unwrap();
        assert_eq!(parts[0].mesh.bounds().unwrap().size(), Vec3::splat(50.8), "and reads back in millimetres");
        assert_eq!(parse(&new).unwrap().mesh.bounds().unwrap().max, Vec3::new(101.6, 50.8, 50.8));
    }

    #[test]
    fn what_cannot_be_replaced_is_refused() {
        let resources = format!("{}{}", cube_object(1, ""), cube_object(2, ""));
        let pkg = package(&[(
            "3D/3dmodel.model",
            model("millimeter", "", &resources, "<item objectid=\"1\"/><item objectid=\"2\"/>"),
        )]);
        let mesh = soup(1.0);
        let fails = |replacements: &[Replacement]| replaced(&pkg, replacements).unwrap_err().to_string();
        assert!(fails(&[Replacement { part: 2, mesh: &mesh }]).contains("has 2 parts"));
        let twice = [Replacement { part: 0, mesh: &mesh }, Replacement { part: 0, mesh: &mesh }];
        assert!(fails(&twice).contains("named twice"));
        let empty = Mesh::default();
        assert!(matches!(replaced(&pkg, &[Replacement { part: 0, mesh: &empty }]), Err(LoadError::Empty)));
        let flat = Mesh { positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0; 3]], ..Mesh::default() };
        assert!(matches!(replaced(&pkg, &[Replacement { part: 0, mesh: &flat }]), Err(LoadError::Empty)));
        let broken = Mesh { positions: vec![[0.0; 3], [f32::NAN, 0.0, 0.0], [0.0, 1.0, 0.0]], ..Mesh::default() };
        assert!(fails(&[Replacement { part: 0, mesh: &broken }]).contains("finite"));
        assert!(replaced(b"no zip", &[]).is_err());
    }
}
