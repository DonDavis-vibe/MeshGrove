use glam::Vec3;

/// A triangle mesh in model units (millimetres for 3D-printing files), Z-up.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    /// Triangle indices into `positions`. `None` means `positions` is a
    /// triangle soup with three consecutive vertices per triangle (what STL gives us).
    pub indices: Option<Vec<u32>>,
    /// One sRGB colour per triangle (filament or material colours of 3MF projects);
    /// `None` for plain meshes, which the renderer draws in its model colour.
    pub colors: Option<Vec<[u8; 4]>>,
    /// One sRGB colour per entry of `positions` (vertex colours, textures of OBJ/PLY/glTF),
    /// blended across each triangle. Per-triangle `colors` win when both are present.
    pub vertex_colors: Option<Vec<[u8; 4]>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    /// Radius of the sphere around the box, used to fit the camera.
    pub fn radius(&self) -> f32 {
        self.size().length() * 0.5
    }
}

impl Mesh {
    /// Multiplies every coordinate by `factor` (a change of unit).
    pub fn scale(&mut self, factor: f32) {
        for p in &mut self.positions {
            *p = p.map(|v| v * factor);
        }
    }

    pub fn triangle_count(&self) -> usize {
        match &self.indices {
            Some(indices) => indices.len() / 3,
            None => self.positions.len() / 3,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.triangle_count() == 0
    }

    pub fn triangle(&self, i: usize) -> [Vec3; 3] {
        let p = |k: usize| Vec3::from(self.positions[k]);
        match &self.indices {
            Some(idx) => [p(idx[3 * i] as usize), p(idx[3 * i + 1] as usize), p(idx[3 * i + 2] as usize)],
            None => [p(3 * i), p(3 * i + 1), p(3 * i + 2)],
        }
    }

    pub fn triangles(&self) -> impl Iterator<Item = [Vec3; 3]> + '_ {
        (0..self.triangle_count()).map(|i| self.triangle(i))
    }

    /// Bounding box over all finite vertices; `None` for empty or all-garbage meshes.
    pub fn bounds(&self) -> Option<Aabb> {
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for p in &self.positions {
            let v = Vec3::from(*p);
            if v.is_finite() {
                min = min.min(v);
                max = max.max(v);
            }
        }
        (min.x <= max.x).then_some(Aabb { min, max })
    }

    /// Flattens to a triangle soup, the layout the renderer uploads (flat shading needs no sharing).
    pub fn to_soup(&self) -> Vec<[f32; 3]> {
        match &self.indices {
            None => self.positions.clone(),
            Some(idx) => idx.iter().map(|&i| self.positions[i as usize]).collect(),
        }
    }

    /// Per-vertex colours matching [`Mesh::to_soup`]: each triangle's colour three times, or
    /// the vertex colours of its corners.
    pub fn soup_colors(&self) -> Option<Vec<[u8; 4]>> {
        if let Some(colors) = &self.colors {
            return Some(colors.iter().take(self.triangle_count()).flat_map(|&c| [c; 3]).collect());
        }
        let colors = self.vertex_colors.as_ref().filter(|c| c.len() == self.positions.len())?;
        Some(match &self.indices {
            None => colors.clone(),
            Some(idx) => idx.iter().map(|&i| colors[i as usize]).collect(),
        })
    }

    /// Whether the mesh brings colours of its own.
    pub fn is_colored(&self) -> bool {
        self.colors.is_some() || self.vertex_colors.is_some()
    }

    /// Axis-aligned cube from (0,0,0) to (size,size,size), 12 triangles, outward winding.
    pub fn cube(size: f32) -> Mesh {
        let s = size;
        let positions = vec![
            [0.0, 0.0, 0.0],
            [s, 0.0, 0.0],
            [s, s, 0.0],
            [0.0, s, 0.0],
            [0.0, 0.0, s],
            [s, 0.0, s],
            [s, s, s],
            [0.0, s, s],
        ];
        #[rustfmt::skip]
        let indices = vec![
            0, 2, 1,  0, 3, 2, // bottom (-Z)
            4, 5, 6,  4, 6, 7, // top (+Z)
            0, 1, 5,  0, 5, 4, // front (-Y)
            2, 3, 7,  2, 7, 6, // back (+Y)
            1, 2, 6,  1, 6, 5, // right (+X)
            3, 0, 4,  3, 4, 7, // left (-X)
        ];
        Mesh { positions, indices: Some(indices), colors: None, vertex_colors: None }
    }
}

/// Distance along the ray (`dir` normalised) to the nearest triangle of a triangle soup
/// (`[a, b, c, a, b, c, …]`), or `None` if it misses (Möller–Trumbore, both faces).
pub fn ray_hit(soup: &[[f32; 3]], origin: Vec3, dir: Vec3) -> Option<f32> {
    let mut best = f32::INFINITY;
    for tri in soup.as_chunks::<3>().0 {
        let a = Vec3::from(tri[0]);
        let (e1, e2) = (Vec3::from(tri[1]) - a, Vec3::from(tri[2]) - a);
        let p = dir.cross(e2);
        let det = e1.dot(p);
        if det == 0.0 {
            continue;
        }
        let inv = 1.0 / det;
        let s = origin - a;
        let u = s.dot(p) * inv;
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let q = s.cross(e1);
        let v = dir.dot(q) * inv;
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        let t = e2.dot(q) * inv;
        if t > 0.0 && t < best {
            best = t;
        }
    }
    best.is_finite().then_some(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rays_hit_the_nearest_face() {
        let soup = Mesh::cube(10.0).to_soup(); // 0..10 on every axis
        let hit = ray_hit(&soup, Vec3::new(5.0, -20.0, 5.0), Vec3::Y).unwrap();
        assert!((hit - 20.0).abs() < 1e-4, "front face at y = 0: {hit}");
        let inside = ray_hit(&soup, Vec3::new(5.0, 5.0, 5.0), -Vec3::Z).unwrap();
        assert!((inside - 5.0).abs() < 1e-4);
        assert_eq!(ray_hit(&soup, Vec3::new(50.0, -20.0, 5.0), Vec3::Y), None);
        assert_eq!(ray_hit(&soup, Vec3::new(5.0, -20.0, 5.0), -Vec3::Y), None, "pointing away");
    }

    #[test]
    fn cube_bounds_and_soup() {
        let cube = Mesh::cube(10.0);
        assert_eq!(cube.triangle_count(), 12);
        let b = cube.bounds().unwrap();
        assert_eq!(b.size(), Vec3::splat(10.0));
        assert_eq!(b.center(), Vec3::splat(5.0));
        let soup = Mesh { positions: cube.to_soup(), indices: None, colors: None, vertex_colors: None };
        assert_eq!(soup.triangle_count(), 12);
        assert_eq!(soup.triangle(3), cube.triangle(3));
    }

    #[test]
    fn bounds_ignore_non_finite_vertices() {
        let mesh = Mesh {
            positions: vec![[0.0, 0.0, 0.0], [f32::NAN, 1.0, 1.0], [2.0, 3.0, 4.0]],
            indices: None,
            colors: None,
            vertex_colors: None,
        };
        let b = mesh.bounds().unwrap();
        assert_eq!(b.max, Vec3::new(2.0, 3.0, 4.0));
        assert!(Mesh::default().bounds().is_none());
    }
}

#[cfg(test)]
mod scale_tests {
    use super::*;

    #[test]
    fn scaling_changes_the_size() {
        let mut m = Mesh::cube(2.0);
        let before = m.bounds().unwrap().size();
        m.scale(25.4);
        assert!((m.bounds().unwrap().size() - before * 25.4).length() < 1e-3);
    }
}
