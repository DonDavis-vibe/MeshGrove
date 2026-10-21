//! Stanford PLY (ASCII and binary, either byte order): vertices, polygon faces and the colours
//! scanners and MeshLab store per vertex or per face. Like OBJ, PLY has no unit; we read it as
//! millimetres, Z up.

use super::LoadError;
use super::texture;
use crate::mesh::Mesh;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Scalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Scalar {
    fn parse(name: &str) -> Option<Scalar> {
        Some(match name {
            "char" | "int8" => Scalar::I8,
            "uchar" | "uint8" => Scalar::U8,
            "short" | "int16" => Scalar::I16,
            "ushort" | "uint16" => Scalar::U16,
            "int" | "int32" => Scalar::I32,
            "uint" | "uint32" => Scalar::U32,
            "float" | "float32" => Scalar::F32,
            "double" | "float64" => Scalar::F64,
            _ => return None,
        })
    }

    fn size(self) -> usize {
        match self {
            Scalar::I8 | Scalar::U8 => 1,
            Scalar::I16 | Scalar::U16 => 2,
            Scalar::I32 | Scalar::U32 | Scalar::F32 => 4,
            Scalar::F64 => 8,
        }
    }

    /// What full intensity is for a colour stored in this type.
    fn color_max(self) -> f64 {
        match self {
            Scalar::F32 | Scalar::F64 => 1.0,
            Scalar::U16 | Scalar::I16 => 65535.0,
            _ => 255.0,
        }
    }
}

struct Property {
    name: String,
    /// `Some(count type)` for list properties.
    list: Option<Scalar>,
    ty: Scalar,
}

struct Element {
    name: String,
    count: usize,
    properties: Vec<Property>,
}

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Ascii,
    Little,
    Big,
}

pub fn parse(bytes: &[u8]) -> Result<Mesh, LoadError> {
    let bad = |m: &str| LoadError::Malformed(format!("PLY: {m}"));
    let (format, elements, body) = header(bytes).ok_or_else(|| bad("invalid header"))?;
    let mut values = Values::new(format, body);

    let mut out = Mesh { indices: Some(Vec::new()), ..Mesh::default() };
    let mut vertex_colors: Vec<[u8; 4]> = Vec::new();
    let mut face_colors: Vec<[u8; 4]> = Vec::new();
    for element in &elements {
        let is_vertex = element.name == "vertex";
        let is_face = element.name == "face";
        let find = |names: &[&str]| element.properties.iter().position(|p| names.contains(&p.name.as_str()));
        let xyz = [find(&["x"]), find(&["y"]), find(&["z"])];
        let rgb = [
            find(&["red", "r", "diffuse_red"]),
            find(&["green", "g", "diffuse_green"]),
            find(&["blue", "b", "diffuse_blue"]),
        ];
        let corners = find(&["vertex_indices", "vertex_index"]);
        let has_rgb = rgb.iter().all(Option::is_some);
        // Every instance is read, also of elements we ignore, to get past them.
        let reserve = element.count.min(body.len() / 4);
        if is_vertex {
            out.positions.reserve(reserve);
        }
        let mut scalars = vec![0.0f64; element.properties.len()];
        let mut list: Vec<f64> = Vec::new();
        for _ in 0..element.count {
            for (i, p) in element.properties.iter().enumerate() {
                match p.list {
                    None => scalars[i] = values.next(p.ty).ok_or_else(|| bad("file ends early"))?,
                    Some(count_ty) => {
                        let n = values.next(count_ty).ok_or_else(|| bad("file ends early"))? as usize;
                        let keep = is_face && Some(i) == corners;
                        if keep {
                            list.clear();
                        }
                        for _ in 0..n {
                            let v = values.next(p.ty).ok_or_else(|| bad("file ends early"))?;
                            if keep {
                                list.push(v);
                            }
                        }
                    }
                }
            }
            // Only called when all three channels exist.
            let color = |scalars: &[f64]| {
                let channel = |k: Option<usize>| {
                    let k = k.unwrap_or_default();
                    let c = (scalars[k] / element.properties[k].ty.color_max()).clamp(0.0, 1.0);
                    (c * 255.0).round() as u8
                };
                let [r, g, b] = rgb.map(channel);
                [r, g, b, 255]
            };
            if is_vertex {
                let [x, y, z] = xyz.map(|k| k.map_or(0.0, |k| scalars[k] as f32));
                out.positions.push([x, y, z]);
                if has_rgb {
                    vertex_colors.push(color(&scalars));
                }
            } else if is_face && list.len() >= 3 {
                let fan = list.iter().map(|&v| v as u32).collect::<Vec<_>>();
                let indices = out.indices.get_or_insert_with(Vec::new);
                for k in 1..fan.len() - 1 {
                    indices.extend([fan[0], fan[k], fan[k + 1]]);
                    if has_rgb {
                        face_colors.push(color(&scalars));
                    }
                }
            }
        }
    }

    // Drop faces that point past the vertices.
    let n = out.positions.len() as u32;
    let indices = out.indices.take().unwrap_or_default();
    let mut kept = Vec::with_capacity(indices.len());
    let mut kept_colors = Vec::new();
    for (t, tri) in indices.as_chunks::<3>().0.iter().enumerate() {
        if tri.iter().all(|&v| v < n) {
            kept.extend_from_slice(tri);
            if let Some(c) = face_colors.get(t) {
                kept_colors.push(*c);
            }
        }
    }
    out.indices = Some(kept);
    if out.is_empty() {
        return Err(LoadError::Empty); // a point cloud
    }
    if !face_colors.is_empty() && kept_colors.len() == out.triangle_count() && texture::meaningful(&kept_colors) {
        out.colors = Some(kept_colors);
    } else if vertex_colors.len() == out.positions.len() && texture::meaningful(&vertex_colors) {
        out.vertex_colors = Some(vertex_colors);
    }
    Ok(out)
}

/// Format, elements and the bytes after `end_header`.
fn header(bytes: &[u8]) -> Option<(Format, Vec<Element>, &[u8])> {
    if !bytes.starts_with(b"ply") {
        return None;
    }
    let end = bytes.windows(10).position(|w| w == b"end_header")?;
    let mut body = &bytes[end + 10..];
    // The header ends with a line break: \n, or \r\n from Windows tools.
    if body.first() == Some(&b'\r') {
        body = &body[1..];
    }
    body = body.strip_prefix(b"\n").unwrap_or(body);
    let text = String::from_utf8_lossy(&bytes[..end]);
    let (mut format, mut elements) = (None, Vec::<Element>::new());
    for line in text.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["format", f, ..] => {
                format = Some(match *f {
                    "ascii" => Format::Ascii,
                    "binary_little_endian" => Format::Little,
                    "binary_big_endian" => Format::Big,
                    _ => return None,
                })
            }
            ["element", name, count] => {
                elements.push(Element { name: name.to_string(), count: count.parse().ok()?, properties: Vec::new() })
            }
            ["property", "list", count, ty, name] => elements.last_mut()?.properties.push(Property {
                name: name.to_string(),
                list: Some(Scalar::parse(count)?),
                ty: Scalar::parse(ty)?,
            }),
            ["property", ty, name] => elements.last_mut()?.properties.push(Property {
                name: name.to_string(),
                list: None,
                ty: Scalar::parse(ty)?,
            }),
            _ => {} // ply, comment, obj_info
        }
    }
    Some((format?, elements, body))
}

/// Reads the numbers of the body one after the other.
struct Values<'a> {
    format: Format,
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Values<'a> {
    fn new(format: Format, bytes: &'a [u8]) -> Values<'a> {
        Values { format, bytes, at: 0 }
    }

    fn next(&mut self, ty: Scalar) -> Option<f64> {
        if self.format == Format::Ascii {
            let rest = &self.bytes[self.at..];
            let start = rest.iter().position(|b| !b.is_ascii_whitespace())?;
            let len = rest[start..].iter().position(u8::is_ascii_whitespace).unwrap_or(rest.len() - start);
            self.at += start + len;
            return std::str::from_utf8(&rest[start..start + len]).ok()?.parse().ok();
        }
        let raw = self.bytes.get(self.at..self.at + ty.size())?;
        self.at += ty.size();
        // Little-endian bytes in the low end of `b`.
        let mut b = [0u8; 8];
        b[..raw.len()].copy_from_slice(raw);
        if self.format == Format::Big {
            b[..raw.len()].reverse();
        }
        let [b0, b1, b2, b3, ..] = b;
        Some(match ty {
            Scalar::I8 => f64::from(b0 as i8),
            Scalar::U8 => f64::from(b0),
            Scalar::I16 => f64::from(i16::from_le_bytes([b0, b1])),
            Scalar::U16 => f64::from(u16::from_le_bytes([b0, b1])),
            Scalar::I32 => f64::from(i32::from_le_bytes([b0, b1, b2, b3])),
            Scalar::U32 => f64::from(u32::from_le_bytes([b0, b1, b2, b3])),
            Scalar::F32 => f64::from(f32::from_le_bytes([b0, b1, b2, b3])),
            Scalar::F64 => f64::from_le_bytes(b),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASCII: &str = "ply\r\nformat ascii 1.0\r\ncomment made by hand\r\nelement vertex 4\r\nproperty float x\r\nproperty float y\r\nproperty float z\r\nproperty uchar red\r\nproperty uchar green\r\nproperty uchar blue\r\nelement face 1\r\nproperty list uchar int vertex_indices\r\nend_header\r\n\
0 0 0 255 0 0\r\n10 0 0 255 0 0\r\n10 10 0 0 0 255\r\n0 10 0 0 0 255\r\n4 0 1 2 3\r\n";

    #[test]
    fn ascii_quad_with_vertex_colours() {
        let mesh = parse(ASCII.as_bytes()).unwrap();
        assert_eq!(mesh.triangle_count(), 2);
        assert_eq!(mesh.bounds().unwrap().size(), glam::Vec3::new(10.0, 10.0, 0.0));
        assert_eq!(mesh.vertex_colors.unwrap()[2], [0, 0, 255, 255]);
    }

    fn binary(big: bool) -> Vec<u8> {
        let order = if big { "binary_big_endian" } else { "binary_little_endian" };
        let mut out = format!(
            "ply\nformat {order} 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\n\
             element edge 1\nproperty int vertex1\nproperty int vertex2\n\
             element face 1\nproperty list uchar uint vertex_indices\nproperty float red\nproperty float green\nproperty float blue\nend_header\n"
        )
        .into_bytes();
        let f = |v: f32| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        let u = |v: u32| if big { v.to_be_bytes() } else { v.to_le_bytes() };
        for p in [[0.0, 0.0, 0.0], [5.0, 0.0, 0.0], [0.0, 5.0, 2.0]] {
            p.iter().for_each(|&c| out.extend(f(c)));
        }
        out.extend(u(0));
        out.extend(u(1)); // the edge, skipped
        out.push(3);
        [0, 1, 2].iter().for_each(|&i| out.extend(u(i)));
        [0.0, 1.0, 0.0].iter().for_each(|&c| out.extend(f(c)));
        out
    }

    #[test]
    fn binary_both_byte_orders_with_face_colours() {
        for big in [false, true] {
            let mesh = parse(&binary(big)).unwrap();
            assert_eq!(mesh.triangle_count(), 1, "big endian: {big}");
            assert_eq!(mesh.bounds().unwrap().size(), glam::Vec3::new(5.0, 5.0, 2.0));
            assert_eq!(mesh.colors.unwrap(), [[0, 255, 0, 255]]);
        }
    }

    #[test]
    fn point_clouds_and_garbage() {
        let cloud = "ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nend_header\n1 2 3\n";
        assert!(matches!(parse(cloud.as_bytes()), Err(LoadError::Empty)));
        assert!(parse(b"solid cube").is_err());
        let mut short = binary(false);
        short.truncate(short.len() - 5);
        assert!(parse(&short).is_err());
    }
}
