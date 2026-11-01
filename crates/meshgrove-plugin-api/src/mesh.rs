//! Triangles as they cross between plugin and host (`mesh-data` in the WIT world).

use serde::{Deserialize, Serialize};

/// Triangles in millimetres: three coordinates per vertex, three indices per triangle.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MeshData {
    pub positions: Vec<f32>,
    pub indices: Vec<u32>,
}

impl MeshData {
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn vertices(&self) -> usize {
        self.positions.len() / 3
    }

    fn vertex(&self, i: u32) -> [f32; 3] {
        let i = i as usize * 3;
        [self.positions[i], self.positions[i + 1], self.positions[i + 2]]
    }

    /// Whether the data is whole: full vertices and triangles, indices in range, finite numbers.
    pub fn check(&self) -> Result<(), String> {
        if !self.positions.len().is_multiple_of(3) || !self.indices.len().is_multiple_of(3) {
            return Err("positions and indices must come in threes".into());
        }
        if self.positions.iter().any(|p| !p.is_finite()) {
            return Err("a coordinate is not a finite number".into());
        }
        let n = self.vertices();
        if self.indices.iter().any(|&i| i as usize >= n) {
            return Err("an index points past the vertices".into());
        }
        Ok(())
    }

    /// The smallest and largest corner, `None` without vertices.
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let (vertices, _) = self.positions.as_chunks::<3>();
        let (mut lo, mut hi) = (*vertices.first()?, vertices[0]);
        for p in &vertices[1..] {
            for a in 0..3 {
                lo[a] = lo[a].min(p[a]);
                hi[a] = hi[a].max(p[a]);
            }
        }
        Some((lo, hi))
    }

    /// A box from the origin to `size`, its faces facing outwards.
    pub fn cuboid(size: [f32; 3]) -> MeshData {
        let [x, y, z] = size;
        #[rustfmt::skip]
        let positions = vec![
            0.0, 0.0, 0.0,  x, 0.0, 0.0,  x, y, 0.0,  0.0, y, 0.0,
            0.0, 0.0, z,    x, 0.0, z,    x, y, z,    0.0, y, z,
        ];
        #[rustfmt::skip]
        let indices = vec![
            0, 2, 1,  0, 3, 2,  // bottom
            4, 5, 6,  4, 6, 7,  // top
            0, 1, 5,  0, 5, 4,  // front
            1, 2, 6,  1, 6, 5,  // right
            2, 3, 7,  2, 7, 6,  // back
            3, 0, 4,  3, 4, 7,  // left
        ];
        MeshData { positions, indices }
    }

    /// Binary STL, `name` in its header.
    pub fn to_stl(&self, name: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(84 + self.triangles() * 50);
        let mut header = [0u8; 80];
        let name = name.as_bytes();
        header[..name.len().min(80)].copy_from_slice(&name[..name.len().min(80)]);
        out.extend_from_slice(&header);
        out.extend_from_slice(&(self.triangles() as u32).to_le_bytes());
        for t in self.indices.as_chunks::<3>().0 {
            let [a, b, c] = [self.vertex(t[0]), self.vertex(t[1]), self.vertex(t[2])];
            let (u, v) = (sub(b, a), sub(c, a));
            let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            let n = if len > 0.0 { n.map(|c| c / len) } else { [0.0; 3] };
            for value in n.iter().chain(&a).chain(&b).chain(&c) {
                out.extend_from_slice(&value.to_le_bytes());
            }
            out.extend_from_slice(&[0, 0]);
        }
        out
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cuboid_is_whole_and_writes_as_stl() {
        let m = MeshData::cuboid([20.0, 30.0, 40.0]);
        assert_eq!((m.vertices(), m.triangles()), (8, 12));
        assert_eq!(m.check(), Ok(()));
        assert_eq!(m.bounds(), Some(([0.0; 3], [20.0, 30.0, 40.0])));
        let stl = m.to_stl("box");
        assert_eq!(stl.len(), 84 + 12 * 50);
        assert_eq!(u32::from_le_bytes(stl[80..84].try_into().unwrap()), 12);
        // The first triangle lies on the bottom: its normal points down.
        assert_eq!(f32::from_le_bytes(stl[92..96].try_into().unwrap()), -1.0);
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<MeshData>(&json).unwrap(), m);
        assert!(MeshData { indices: vec![0, 1, 9], ..m.clone() }.check().is_err());
        assert!(MeshData { positions: vec![f32::NAN; 3], indices: vec![] }.check().is_err());
    }
}
