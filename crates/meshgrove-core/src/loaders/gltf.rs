//! glTF 2.0: a single `.glb`, or a `.gltf` with its buffers and textures next to it (or
//! embedded as data URIs). All meshes of the default scene in their node transforms, coloured
//! by base colour, vertex colours and base colour texture. glTF counts in metres with +Y up;
//! the mesh comes out in millimetres with +Z up like every other format here.

use std::borrow::Cow;
use std::collections::HashMap;

use base64::Engine;
use glam::{Mat4, Vec3};
use gltf::mesh::Mode;

use super::texture::{self, Texture};
use super::{LoadError, Sibling};
use crate::mesh::Mesh;

/// Node trees deeper than this do not occur in real files; they guard against cycles.
const MAX_DEPTH: usize = 64;

pub fn parse(bytes: &[u8], sibling: Sibling) -> Result<Mesh, LoadError> {
    let gltf::Gltf { document, blob } =
        gltf::Gltf::from_slice(bytes).map_err(|e| LoadError::Malformed(e.to_string()))?;
    let buffers: Vec<Option<Cow<[u8]>>> = document
        .buffers()
        .map(|buffer| {
            let data = match buffer.source() {
                gltf::buffer::Source::Bin => blob.as_deref().map(Cow::Borrowed),
                gltf::buffer::Source::Uri(uri) => resource(uri, sibling).map(Cow::Owned),
            }?;
            // A short buffer would make the accessor reader slice out of bounds.
            (data.len() >= buffer.length()).then_some(data)
        })
        .collect();
    let mut scene = Scene { buffers: &buffers, sibling, textures: HashMap::new(), out: Mesh::default() };
    scene.out.indices = Some(Vec::new());
    scene.out.vertex_colors = Some(Vec::new());

    // Metres → millimetres, +Y up → +Z up (and glTF's front, +Z, becomes our front, −Y):
    // (x, y, z) → 1000 · (x, −z, y), exactly.
    let root = Mat4::from_cols(
        glam::Vec4::new(1000.0, 0.0, 0.0, 0.0),
        glam::Vec4::new(0.0, 0.0, 1000.0, 0.0),
        glam::Vec4::new(0.0, -1000.0, 0.0, 0.0),
        glam::Vec4::W,
    );
    match document.default_scene().or_else(|| document.scenes().next()) {
        Some(s) => s.nodes().for_each(|node| scene.visit(node, root, 0)),
        None => document.meshes().for_each(|mesh| scene.mesh(mesh, root)),
    }
    let mut out = scene.out;
    if out.is_empty() {
        return Err(LoadError::Empty);
    }
    if !out.vertex_colors.as_deref().is_some_and(texture::meaningful) {
        out.vertex_colors = None;
    }
    Ok(out)
}

struct Scene<'a> {
    buffers: &'a [Option<Cow<'a, [u8]>>],
    sibling: Sibling<'a>,
    /// Decoded base colour textures by image index (`None`: unreadable).
    textures: HashMap<usize, Option<Texture>>,
    out: Mesh,
}

impl Scene<'_> {
    fn visit(&mut self, node: gltf::Node, parent: Mat4, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let transform = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        if let Some(mesh) = node.mesh() {
            self.mesh(mesh, transform);
        }
        for child in node.children() {
            self.visit(child, transform, depth + 1);
        }
    }

    fn mesh(&mut self, mesh: gltf::Mesh, transform: Mat4) {
        for primitive in mesh.primitives() {
            let mode = primitive.mode();
            if !matches!(mode, Mode::Triangles | Mode::TriangleStrip | Mode::TriangleFan) {
                continue; // points and lines are not printable
            }
            let buffers = self.buffers;
            let reader = primitive.reader(|b| buffers.get(b.index())?.as_deref());
            let Some(positions) = reader.read_positions() else { continue };
            let base = self.out.positions.len();
            self.out.positions.extend(positions.map(|p| transform.transform_point3(Vec3::from(p)).to_array()));
            let count = self.out.positions.len() - base;

            let order: Vec<u32> = match reader.read_indices() {
                Some(indices) => indices.into_u32().collect(),
                None => (0..count as u32).collect(),
            };
            let indices = self.out.indices.get_or_insert_with(Vec::new);
            for t in triangles(mode, &order) {
                if t.iter().all(|&v| (v as usize) < count) {
                    indices.extend(t.map(|v| base as u32 + v));
                }
            }

            // Colours, all multiplied in linear light: factor × vertex colour × texture.
            let pbr = primitive.material().pbr_metallic_roughness();
            let factor = pbr.base_color_factor();
            let vertex: Vec<[f32; 4]> = reader.read_colors(0).map(|c| c.into_rgba_f32().collect()).unwrap_or_default();
            let textured = pbr.base_color_texture().and_then(|info| {
                let uv: Vec<[f32; 2]> = reader.read_tex_coords(info.tex_coord())?.into_f32().collect();
                let image = info.texture().source();
                self.texture(&image)?;
                Some((uv, image.index()))
            });
            let colors = self.out.vertex_colors.get_or_insert_with(Vec::new);
            for i in 0..count {
                let mut c = factor;
                if let Some(v) = vertex.get(i) {
                    c = texture::multiply(c, *v);
                }
                if let Some((uv, image)) = &textured
                    && let (Some(uv), Some(Some(tex))) = (uv.get(i), self.textures.get(image))
                {
                    c = texture::multiply(c, tex.sample(*uv));
                }
                colors.push(texture::to_srgb8(c));
            }
        }
    }

    /// Decodes an image once; `None` if it cannot be read.
    fn texture(&mut self, image: &gltf::Image) -> Option<()> {
        let index = image.index();
        if !self.textures.contains_key(&index) {
            let bytes: Option<Cow<[u8]>> = match image.source() {
                gltf::image::Source::View { view, .. } => {
                    let data = self.buffers.get(view.buffer().index())?.as_deref()?;
                    data.get(view.offset()..view.offset() + view.length()).map(Cow::Borrowed)
                }
                gltf::image::Source::Uri { uri, .. } => resource(uri, self.sibling).map(Cow::Owned),
            };
            self.textures.insert(index, bytes.and_then(|b| Texture::decode(&b)));
        }
        self.textures.get(&index)?.as_ref().map(|_| ())
    }
}

/// The triangles of a primitive, as indices into its vertices.
fn triangles(mode: Mode, order: &[u32]) -> Vec<[u32; 3]> {
    match mode {
        Mode::TriangleStrip => (0..order.len().saturating_sub(2))
            .map(|i| {
                if i % 2 == 0 { [order[i], order[i + 1], order[i + 2]] } else { [order[i + 1], order[i], order[i + 2]] }
            })
            .collect(),
        Mode::TriangleFan => (1..order.len().saturating_sub(1)).map(|i| [order[0], order[i], order[i + 1]]).collect(),
        _ => order.as_chunks::<3>().0.to_vec(),
    }
}

/// Bytes behind a URI: an embedded `data:` URI or a file next to the model.
fn resource(uri: &str, sibling: Sibling) -> Option<Vec<u8>> {
    if let Some(data) = uri.strip_prefix("data:") {
        let (header, payload) = data.split_once(',')?;
        return if header.ends_with(";base64") {
            base64::engine::general_purpose::STANDARD.decode(payload.trim()).ok()
        } else {
            Some(percent_encoding::percent_decode_str(payload).collect())
        };
    }
    if uri.contains("://") {
        return None; // never fetch anything from the network
    }
    sibling(&percent_encoding::percent_decode_str(uri).decode_utf8_lossy())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A glTF with one quad (two triangles) in the XY plane, 1 m wide, plus optional extras.
    pub fn quad_gltf(material: &str, extra_attributes: &str, extra_buffers: &[(&str, &[u8])]) -> (String, Vec<u8>) {
        let positions: [[f32; 3]; 4] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
        let indices: [u16; 6] = [0, 1, 2, 0, 2, 3];
        let mut bin: Vec<u8> = bytemuck_cast(&positions);
        bin.extend(indices.iter().flat_map(|i| i.to_le_bytes()));
        let mut views = vec![
            r#"{"buffer":0,"byteOffset":0,"byteLength":48}"#.to_string(),
            r#"{"buffer":0,"byteOffset":48,"byteLength":12}"#.to_string(),
        ];
        let mut accessors = vec![
            r#"{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3","min":[0,0,0],"max":[1,1,0]}"#.to_string(),
            r#"{"bufferView":1,"componentType":5123,"count":6,"type":"SCALAR"}"#.to_string(),
        ];
        for (kind, data) in extra_buffers {
            while !bin.len().is_multiple_of(4) {
                bin.push(0);
            }
            let (component, ty, count) = match *kind {
                "COLOR_0" => (5126, "VEC4", 4),
                "TEXCOORD_0" => (5126, "VEC2", 4),
                _ => (5121, "", 0), // raw bytes (an image)
            };
            views.push(format!(r#"{{"buffer":0,"byteOffset":{},"byteLength":{}}}"#, bin.len(), data.len()));
            if count > 0 {
                accessors.push(format!(
                    r#"{{"bufferView":{},"componentType":{component},"count":{count},"type":"{ty}"}}"#,
                    views.len() - 1
                ));
            }
            bin.extend_from_slice(data);
        }
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],
            "nodes":[{{"mesh":0}}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0{extra_attributes}}},"indices":1{material}}}]}}],
            "buffers":[{{"byteLength":{}}}],
            "bufferViews":[{}],
            "accessors":[{}]}}"#,
            bin.len(),
            views.join(","),
            accessors.join(",")
        );
        (json, bin)
    }

    fn bytemuck_cast(p: &[[f32; 3]]) -> Vec<u8> {
        p.iter().flatten().flat_map(|f| f.to_le_bytes()).collect()
    }

    /// Packs JSON and binary chunk into a `.glb`.
    pub fn glb(json: &str, bin: &[u8]) -> Vec<u8> {
        let mut json = json.as_bytes().to_vec();
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let total = 12 + 8 + json.len() + 8 + bin.len();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"glTF");
        out.extend(2u32.to_le_bytes());
        out.extend((total as u32).to_le_bytes());
        out.extend((json.len() as u32).to_le_bytes());
        out.extend(b"JSON");
        out.extend(&json);
        out.extend((bin.len() as u32).to_le_bytes());
        out.extend(b"BIN\0");
        out.extend(bin);
        out
    }

    fn no_files(_: &str) -> Option<Vec<u8>> {
        None
    }

    #[test]
    fn glb_in_millimetres_and_z_up() {
        let (json, bin) = quad_gltf("", "", &[]);
        let mesh = parse(&glb(&json, &bin), &no_files).unwrap();
        assert_eq!(mesh.triangle_count(), 2);
        let b = mesh.bounds().unwrap();
        // The quad spans X and glTF's Y (up), which becomes our Z.
        assert_eq!(b.size(), Vec3::new(1000.0, 0.0, 1000.0));
        assert!(mesh.vertex_colors.is_none(), "default white material says nothing");
    }

    #[test]
    fn base_colour_and_vertex_colours_multiply() {
        let colors: Vec<u8> =
            [[1.0f32, 1.0, 1.0, 1.0], [1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 1.0, 1.0], [1.0, 1.0, 1.0, 1.0]]
                .iter()
                .flatten()
                .flat_map(|f| f.to_le_bytes())
                .collect();
        let (mut json, bin) = quad_gltf(r#","material":0"#, r#","COLOR_0":2"#, &[("COLOR_0", &colors)]);
        json = json.replacen(
            r#""buffers""#,
            r#""materials":[{"pbrMetallicRoughness":{"baseColorFactor":[1,0,0,1]}}],"buffers""#,
            1,
        );
        let mesh = parse(&glb(&json, &bin), &no_files).unwrap();
        let colors = mesh.vertex_colors.unwrap();
        assert_eq!(colors[0], [255, 0, 0, 255]);
        assert_eq!(colors[2], [0, 0, 0, 255], "red × blue");
    }

    #[test]
    fn embedded_and_external_textures() {
        let mut png = Vec::new();
        image::RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 255, 0, 255]) }
        })
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
        let uv: Vec<u8> = [[0.25f32, 0.5], [0.75, 0.5], [0.75, 0.5], [0.25, 0.5]]
            .iter()
            .flatten()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let material = r#""materials":[{"pbrMetallicRoughness":{"baseColorTexture":{"index":0}}}],"textures":[{"source":0}],"buffers""#;

        // In the GLB's binary chunk.
        let (json, bin) = quad_gltf(r#","material":0"#, r#","TEXCOORD_0":2"#, &[("TEXCOORD_0", &uv), ("image", &png)]);
        let json = json.replacen(r#""buffers""#, material, 1).replacen(
            r#""buffers""#,
            r#""images":[{"bufferView":3,"mimeType":"image/png"}],"buffers""#,
            1,
        );
        let mesh = parse(&glb(&json, &bin), &no_files).unwrap();
        let colors = mesh.vertex_colors.unwrap();
        assert_eq!(colors[0], [255, 0, 0, 255]);
        assert_eq!(colors[1], [0, 255, 0, 255]);

        // A .gltf with buffer and texture as files next to it.
        let (json, bin) = quad_gltf(r#","material":0"#, r#","TEXCOORD_0":2"#, &[("TEXCOORD_0", &uv)]);
        let json = json
            .replacen(r#""buffers":[{"#, r#""buffers":[{"uri":"quad%20data.bin","#, 1)
            .replacen(r#""buffers""#, material, 1)
            .replacen(r#""buffers""#, r#""images":[{"uri":"tex/skin.png"}],"buffers""#, 1);
        let files = |name: &str| match name {
            "quad data.bin" => Some(bin.clone()),
            "tex/skin.png" => Some(png.clone()),
            _ => None,
        };
        let mesh = parse(json.as_bytes(), &files).unwrap();
        assert_eq!(mesh.vertex_colors.unwrap()[1], [0, 255, 0, 255]);
        // Without its buffer there is nothing to show.
        assert!(parse(json.as_bytes(), &no_files).is_err());
    }

    #[test]
    fn data_uris_and_strips() {
        assert_eq!(resource("data:application/octet-stream;base64,AAEC", &no_files), Some(vec![0, 1, 2]));
        assert_eq!(resource("https://example.com/a.bin", &|_| Some(vec![1])), None);
        assert_eq!(triangles(Mode::TriangleStrip, &[0, 1, 2, 3]), [[0, 1, 2], [2, 1, 3]]);
        assert_eq!(triangles(Mode::TriangleFan, &[0, 1, 2, 3]), [[0, 1, 2], [0, 2, 3]]);
        assert!(parse(b"{not json", &no_files).is_err());
    }
}
