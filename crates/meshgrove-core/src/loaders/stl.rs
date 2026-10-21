//! STL reader/writer. Binary vs. ASCII is decided by file size, not by the
//! `solid` prefix: plenty of binary exporters start their 80-byte header with "solid".

use super::LoadError;
use crate::mesh::Mesh;

const HEADER_LEN: usize = 80;
const RECORD_LEN: usize = 50;

pub fn parse(bytes: &[u8]) -> Result<Mesh, LoadError> {
    let mesh = if looks_binary(bytes) { parse_binary(bytes)? } else { parse_ascii(bytes)? };
    if mesh.is_empty() {
        return Err(LoadError::Empty);
    }
    Ok(mesh)
}

fn declared_triangles(bytes: &[u8]) -> Option<usize> {
    let count = bytes.get(HEADER_LEN..HEADER_LEN + 4)?;
    Some(u32::from_le_bytes(count.try_into().unwrap()) as usize)
}

fn looks_binary(bytes: &[u8]) -> bool {
    if let Some(n) = declared_triangles(bytes)
        && n.checked_mul(RECORD_LEN).and_then(|b| b.checked_add(HEADER_LEN + 4)) == Some(bytes.len())
    {
        return true;
    }
    // Size mismatch: trust ASCII only if it really reads like text.
    let head = &bytes[..bytes.len().min(512)];
    let is_text = head.iter().all(|&b| b.is_ascii_graphic() || b.is_ascii_whitespace());
    !(is_text && head.trim_ascii_start().starts_with(b"solid"))
}

fn parse_binary(bytes: &[u8]) -> Result<Mesh, LoadError> {
    let declared = declared_triangles(bytes).ok_or_else(|| LoadError::Malformed("shorter than STL header".into()))?;
    // Some exporters write a wrong count; never read past the data that is actually there.
    let available = (bytes.len() - HEADER_LEN - 4) / RECORD_LEN;
    let n = declared.min(available);
    let mut positions = Vec::with_capacity(n * 3);
    for record in bytes[HEADER_LEN + 4..].as_chunks::<RECORD_LEN>().0.iter().take(n) {
        // Skip the stored normal (bytes 0..12): it is frequently wrong and the shader derives its own.
        for v in 0..3 {
            let base = 12 + v * 12;
            let f = |o: usize| f32::from_le_bytes(record[base + o..base + o + 4].try_into().unwrap());
            positions.push([f(0), f(4), f(8)]);
        }
    }
    Ok(Mesh { positions, indices: None, colors: None, vertex_colors: None })
}

fn parse_ascii(bytes: &[u8]) -> Result<Mesh, LoadError> {
    let mut positions = Vec::new();
    let mut tokens = bytes.split(|b| b.is_ascii_whitespace()).filter(|t| !t.is_empty());
    while let Some(token) = tokens.next() {
        if !token.eq_ignore_ascii_case(b"vertex") {
            continue;
        }
        let mut coord = || -> Result<f32, LoadError> {
            let t = tokens.next().ok_or_else(|| LoadError::Malformed("vertex with fewer than 3 coordinates".into()))?;
            fast_float2::parse(t)
                .map_err(|_| LoadError::Malformed(format!("bad number {:?}", String::from_utf8_lossy(t))))
        };
        positions.push([coord()?, coord()?, coord()?]);
    }
    // Drop a dangling partial facet instead of rejecting the whole file.
    positions.truncate(positions.len() / 3 * 3);
    Ok(Mesh { positions, indices: None, colors: None, vertex_colors: None })
}

/// Binary STL bytes for any mesh (used for the STEP mesh cache and test fixtures).
pub fn write_binary(mesh: &Mesh, header: &str) -> Vec<u8> {
    let n = mesh.triangle_count();
    let mut out = Vec::with_capacity(HEADER_LEN + 4 + n * RECORD_LEN);
    let mut head = [0u8; HEADER_LEN];
    let h = header.as_bytes();
    head[..h.len().min(HEADER_LEN)].copy_from_slice(&h[..h.len().min(HEADER_LEN)]);
    out.extend_from_slice(&head);
    out.extend_from_slice(&(n as u32).to_le_bytes());
    for [a, b, c] in mesh.triangles() {
        let normal = (b - a).cross(c - a).normalize_or_zero();
        for v in [normal, a, b, c] {
            for f in v.to_array() {
                out.extend_from_slice(&f.to_le_bytes());
            }
        }
        out.extend_from_slice(&[0, 0]);
    }
    out
}

/// ASCII STL text for any mesh (test fixtures).
pub fn write_ascii(mesh: &Mesh, name: &str) -> String {
    let mut s = format!("solid {name}\n");
    for [a, b, c] in mesh.triangles() {
        let n = (b - a).cross(c - a).normalize_or_zero();
        s += &format!("  facet normal {} {} {}\n    outer loop\n", n.x, n.y, n.z);
        for v in [a, b, c] {
            s += &format!("      vertex {} {} {}\n", v.x, v.y, v.z);
        }
        s += "    endloop\n  endfacet\n";
    }
    s += &format!("endsolid {name}\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube_soup() -> Mesh {
        let cube = Mesh::cube(20.0);
        Mesh { positions: cube.to_soup(), indices: None, colors: None, vertex_colors: None }
    }

    #[test]
    fn binary_and_ascii_give_the_same_mesh() {
        let binary = parse(&write_binary(&cube_soup(), "test cube")).unwrap();
        let ascii = parse(write_ascii(&cube_soup(), "cube").as_bytes()).unwrap();
        assert_eq!(binary, cube_soup());
        assert_eq!(ascii, cube_soup());
    }

    #[test]
    fn binary_with_solid_header_is_not_mistaken_for_ascii() {
        let bytes = write_binary(&cube_soup(), "solid exported by SomeCAD");
        assert!(bytes.starts_with(b"solid"));
        assert_eq!(parse(&bytes).unwrap().triangle_count(), 12);
    }

    #[test]
    fn binary_with_wrong_triangle_count_reads_available_records() {
        let mut bytes = write_binary(&cube_soup(), "");
        bytes[80..84].copy_from_slice(&1000u32.to_le_bytes());
        assert_eq!(parse(&bytes).unwrap().triangle_count(), 12);
    }

    #[test]
    fn ascii_with_scientific_notation_and_crlf() {
        let text = "solid x\r\nfacet normal 0 0 1\r\nouter loop\r\nvertex 0 0 0\r\nvertex 1.5e1 0 0\r\nvertex 0 -2E0 0\r\nendloop\r\nendfacet\r\nendsolid x\r\n";
        let mesh = parse(text.as_bytes()).unwrap();
        assert_eq!(mesh.positions, vec![[0.0, 0.0, 0.0], [15.0, 0.0, 0.0], [0.0, -2.0, 0.0]]);
    }

    #[test]
    fn empty_and_garbage_are_errors() {
        assert!(matches!(parse(b"solid empty\nendsolid empty\n"), Err(LoadError::Empty)));
        assert!(parse(b"\x00\x01").is_err());
    }
}
