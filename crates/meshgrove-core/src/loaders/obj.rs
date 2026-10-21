//! Wavefront OBJ, with the colours of its `.mtl` materials (diffuse colour or texture) and
//! vertex colours (`v x y z r g b`, as MeshLab and scanners write them). OBJ has no unit;
//! like the slicers we read it as millimetres, Z up.

use std::collections::HashMap;

use super::texture::{self, NEUTRAL, Texture};
use super::{LoadError, Sibling};
use crate::mesh::Mesh;

pub fn parse(bytes: &[u8], sibling: Sibling) -> Result<Mesh, LoadError> {
    // Exporters write comments and names in all sorts of encodings; only the numbers matter.
    let text = String::from_utf8_lossy(bytes);
    let options = tobj::LoadOptions { triangulate: true, single_index: true, ignore_points: true, ignore_lines: true };
    let (models, materials) = tobj::load_obj_buf(&mut text.as_bytes(), &options, |path| {
        let bytes = sibling(&path.to_string_lossy()).ok_or(tobj::LoadError::OpenFileFailed)?;
        tobj::load_mtl_buf(&mut String::from_utf8_lossy(&bytes).as_bytes())
    })
    .map_err(|e| LoadError::Malformed(format!("OBJ: {e}")))?;
    // A missing or broken .mtl only costs the colours.
    let materials = materials.unwrap_or_default();
    let mut textures: HashMap<String, Option<Texture>> = HashMap::new();

    let mut out = Mesh { indices: Some(Vec::new()), vertex_colors: Some(Vec::new()), ..Mesh::default() };
    for model in &models {
        let m = &model.mesh;
        let base = out.positions.len();
        out.positions.extend(m.positions.as_chunks::<3>().0.iter().copied());
        let count = out.positions.len() - base;
        let indices = out.indices.get_or_insert_with(Vec::new);
        for t in m.indices.as_chunks::<3>().0 {
            if t.iter().all(|&v| (v as usize) < count) {
                indices.extend(t.iter().map(|&v| base as u32 + v));
            }
        }

        let material = m.material_id.and_then(|i| materials.get(i));
        let texture = material
            .and_then(|mat| mat.diffuse_texture.as_deref())
            .and_then(|name| textures.entry(name.to_owned()).or_insert_with(|| load_texture(name, sibling)).as_ref());
        // Some writers use 0..255 instead of 0..1 for vertex colours.
        let scale = if m.vertex_color.iter().any(|&c| c > 1.0) { 1.0 / 255.0 } else { 1.0 };
        let vertex = |i: usize| {
            let c = m.vertex_color.get(3 * i..3 * i + 3)?;
            Some([c[0] * scale, c[1] * scale, c[2] * scale].map(texture::srgb_to_linear))
        };
        let diffuse = material.and_then(|mat| mat.diffuse).map(|d| d.map(texture::srgb_to_linear));
        let colors = out.vertex_colors.get_or_insert_with(Vec::new);
        for i in 0..count {
            // A texture replaces the diffuse colour (exporters write a default grey next to it).
            let surface = match (texture, m.texcoords.get(2 * i..2 * i + 2)) {
                (Some(tex), Some(uv)) => Some(tex.sample([uv[0], 1.0 - uv[1]])),
                _ => diffuse.map(|[r, g, b]| [r, g, b, 1.0]),
            };
            let color = match (surface, vertex(i)) {
                (Some(s), Some([r, g, b])) => texture::to_srgb8(texture::multiply(s, [r, g, b, 1.0])),
                (Some(s), None) => texture::to_srgb8(s),
                (None, Some([r, g, b])) => texture::to_srgb8([r, g, b, 1.0]),
                (None, None) => NEUTRAL,
            };
            colors.push(color);
        }
    }
    if out.is_empty() {
        return Err(LoadError::Empty);
    }
    if !out.vertex_colors.as_deref().is_some_and(texture::meaningful) {
        out.vertex_colors = None;
    }
    Ok(out)
}

/// `map_Kd` may carry options before the file name (`-s 1 1 1 wood.png`); try the whole value,
/// then its last word.
fn load_texture(name: &str, sibling: Sibling) -> Option<Texture> {
    let name = name.trim().replace('\\', "/");
    let last = name.rsplit(' ').next().unwrap_or(&name).to_owned();
    [name, last].iter().find_map(|n| sibling(n)).and_then(|bytes| Texture::decode(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUADS: &str = "\
# two quads with different materials
mtllib parts.mtl
v 0 0 0
v 10 0 0
v 10 10 0
v 0 10 0
v 0 0 5
v 10 0 5
vt 0.25 0.5
vt 0.75 0.5
usemtl red
f 1 2 3 4
usemtl textured
f 1/1 2/2 6/2 5/1
";
    const MTL: &str = "\
newmtl red
Kd 1 0 0
newmtl textured
Kd 0.8 0.8 0.8
map_Kd skin.png
";

    fn png() -> Vec<u8> {
        let mut png = Vec::new();
        image::RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 { image::Rgba([0, 0, 255, 255]) } else { image::Rgba([0, 255, 0, 255]) }
        })
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
        png
    }

    #[test]
    fn materials_and_textures_from_files_next_to_it() {
        let png = png();
        let files = |name: &str| match name {
            "parts.mtl" => Some(MTL.as_bytes().to_vec()),
            "skin.png" => Some(png.clone()),
            _ => None,
        };
        let mesh = parse(QUADS.as_bytes(), &files).unwrap();
        assert_eq!(mesh.triangle_count(), 4);
        let soup = mesh.soup_colors().unwrap();
        assert_eq!(soup[0], [255, 0, 0, 255], "Kd of the first quad");
        assert!(soup[6..].contains(&[0, 0, 255, 255]) && soup[6..].contains(&[0, 255, 0, 255]), "texture: {soup:?}");
        assert_eq!(mesh.bounds().unwrap().size(), glam::Vec3::new(10.0, 10.0, 5.0));
    }

    #[test]
    fn without_mtl_the_model_stays_plain() {
        let mesh = parse(QUADS.as_bytes(), &|_| None).unwrap();
        assert_eq!(mesh.triangle_count(), 4);
        assert!(mesh.vertex_colors.is_none());
    }

    #[test]
    fn vertex_colours_in_either_range() {
        for (a, b) in [("1 0 0", "0 0 1"), ("255 0 0", "0 0 255")] {
            let obj = format!("v 0 0 0 {a}\nv 1 0 0 {a}\nv 0 1 0 {b}\nf 1 2 3\n");
            let mesh = parse(obj.as_bytes(), &|_| None).unwrap();
            assert_eq!(mesh.vertex_colors.unwrap(), [[255, 0, 0, 255], [255, 0, 0, 255], [0, 0, 255, 255]]);
        }
    }

    #[test]
    fn broken_input() {
        assert!(matches!(parse(b"# nothing here\n", &|_| None), Err(LoadError::Empty)));
        // Latin-1 in a comment is fine.
        let mesh = parse(b"# W\xfcrfel\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n", &|_| None).unwrap();
        assert_eq!(mesh.triangle_count(), 1);
    }
}
