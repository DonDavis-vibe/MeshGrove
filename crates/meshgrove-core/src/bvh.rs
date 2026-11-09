//! A bounding volume hierarchy over a triangle soup, for picking on large meshes: the triangle
//! a ray meets first, and the triangles near a point. Built once (binned surface area
//! heuristic) off the UI thread; a query then takes microseconds where trying every triangle
//! of a two-million-triangle preview takes tens of milliseconds.
//!
//! The tree does not hold the triangles: queries get the same soup it was built from.

use glam::Vec3;

use crate::mesh::{closest_on_triangle, ray_triangle};

/// Bins the centroids of a node are sorted into to find a good split.
const BINS: usize = 12;
/// A node with this many triangles or fewer is a leaf.
const LEAF: usize = 4;

#[derive(Debug, Clone, Copy)]
struct Node {
    min: Vec3,
    max: Vec3,
    /// Leaf: the first of `count` entries of `order`. Inner node: the left child (the right
    /// one follows it).
    start: u32,
    count: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Bvh {
    nodes: Vec<Node>,
    /// Triangle indices, grouped by leaf.
    order: Vec<u32>,
}

#[derive(Clone, Copy)]
struct Bounds {
    min: Vec3,
    max: Vec3,
}

impl Bounds {
    const EMPTY: Bounds = Bounds { min: Vec3::splat(f32::INFINITY), max: Vec3::splat(f32::NEG_INFINITY) };

    fn grow(&mut self, other: &Bounds) {
        self.min = self.min.min(other.min);
        self.max = self.max.max(other.max);
    }

    fn grow_point(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    fn area(&self) -> f32 {
        let d = (self.max - self.min).max(Vec3::ZERO);
        2.0 * (d.x * d.y + d.y * d.z + d.z * d.x)
    }
}

impl Bvh {
    /// The tree of `soup` (`[a, b, c, a, b, c, …]`). Triangles with a coordinate that is not
    /// finite are left out.
    pub fn build(soup: &[[f32; 3]]) -> Bvh {
        let tris = soup.as_chunks::<3>().0;
        let mut boxes = Vec::with_capacity(tris.len());
        let mut centres = Vec::with_capacity(tris.len());
        let mut order = Vec::with_capacity(tris.len());
        for (i, t) in tris.iter().enumerate() {
            let (a, b, c) = (Vec3::from(t[0]), Vec3::from(t[1]), Vec3::from(t[2]));
            let bounds = Bounds { min: a.min(b).min(c), max: a.max(b).max(c) };
            if a.is_finite() && b.is_finite() && c.is_finite() {
                order.push(i as u32);
            }
            centres.push((bounds.min + bounds.max) * 0.5);
            boxes.push(bounds);
        }
        let mut bvh = Bvh { nodes: Vec::with_capacity(2 * order.len() / LEAF + 1), order };
        if bvh.order.is_empty() {
            return bvh;
        }
        let count = bvh.order.len() as u32;
        bvh.nodes.push(Node { min: Vec3::ZERO, max: Vec3::ZERO, start: 0, count });
        let mut stack = vec![0usize];
        while let Some(index) = stack.pop() {
            let (start, count) = (bvh.nodes[index].start as usize, bvh.nodes[index].count as usize);
            let entries = &mut bvh.order[start..start + count];
            let (mut bounds, mut spread) = (Bounds::EMPTY, Bounds::EMPTY);
            for &t in entries.iter() {
                bounds.grow(&boxes[t as usize]);
                spread.grow_point(centres[t as usize]);
            }
            (bvh.nodes[index].min, bvh.nodes[index].max) = (bounds.min, bounds.max);
            if count <= LEAF {
                continue;
            }
            let Some(mid) = split(entries, &boxes, &centres, &spread) else { continue };
            let left = bvh.nodes.len();
            bvh.nodes.push(Node { min: Vec3::ZERO, max: Vec3::ZERO, start: start as u32, count: mid as u32 });
            let right =
                Node { min: Vec3::ZERO, max: Vec3::ZERO, start: (start + mid) as u32, count: (count - mid) as u32 };
            bvh.nodes.push(right);
            bvh.nodes[index].start = left as u32;
            bvh.nodes[index].count = 0;
            stack.extend([left, left + 1]);
        }
        bvh
    }

    /// The triangles in the tree.
    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// The triangle of `soup` the ray (`dir` normalised) meets first, from either side, and how
    /// far along: what [`crate::mesh::ray_hit_triangle`] finds.
    pub fn ray(&self, soup: &[[f32; 3]], origin: Vec3, dir: Vec3) -> Option<(f32, u32)> {
        let tris = soup.as_chunks::<3>().0;
        let inv = dir.recip();
        let mut best: Option<(f32, u32)> = None;
        let mut stack: Vec<u32> = Vec::with_capacity(64);
        if !self.nodes.is_empty() {
            stack.push(0);
        }
        while let Some(index) = stack.pop() {
            let node = &self.nodes[index as usize];
            let limit = best.map_or(f32::INFINITY, |(t, _)| t);
            if slab(node, origin, inv).is_none_or(|t| t > limit) {
                continue;
            }
            if node.count > 0 {
                for &t in &self.order[node.start as usize..(node.start + node.count) as usize] {
                    let Some(tri) = tris.get(t as usize) else { continue };
                    if let Some(at) = ray_triangle(tri, origin, dir)
                        && best.is_none_or(|(b, i)| at < b || (at == b && t < i))
                    {
                        best = Some((at, t));
                    }
                }
                continue;
            }
            // The nearer child last, so it is looked at first.
            let (l, r) = (node.start, node.start + 1);
            let near = |i: u32| slab(&self.nodes[i as usize], origin, inv).unwrap_or(f32::INFINITY);
            if near(l) < near(r) {
                stack.extend([r, l]);
            } else {
                stack.extend([l, r]);
            }
        }
        best
    }

    /// The triangles of `soup` with a point within `radius` of `centre`, in ascending order.
    pub fn sphere(&self, soup: &[[f32; 3]], centre: Vec3, radius: f32) -> Vec<u32> {
        let tris = soup.as_chunks::<3>().0;
        let mut found = Vec::new();
        let mut stack: Vec<u32> = Vec::with_capacity(64);
        if !self.nodes.is_empty() {
            stack.push(0);
        }
        let r2 = radius * radius;
        while let Some(index) = stack.pop() {
            let node = &self.nodes[index as usize];
            if centre.clamp(node.min, node.max).distance_squared(centre) > r2 {
                continue;
            }
            if node.count == 0 {
                stack.extend([node.start, node.start + 1]);
                continue;
            }
            for &t in &self.order[node.start as usize..(node.start + node.count) as usize] {
                if let Some(tri) = tris.get(t as usize)
                    && closest_on_triangle(tri, centre).distance_squared(centre) <= r2
                {
                    found.push(t);
                }
            }
        }
        found.sort_unstable();
        found
    }
}

/// Where the ray enters the node's box (0 when it starts inside); `None` when it misses.
fn slab(node: &Node, origin: Vec3, inv: Vec3) -> Option<f32> {
    let (mut near, mut far) = (0.0f32, f32::INFINITY);
    for axis in 0..3 {
        // A ray parallel to this axis's planes: inside the slab it does not limit, outside it
        // misses.
        if !inv[axis].is_finite() {
            if origin[axis] < node.min[axis] || origin[axis] > node.max[axis] {
                return None;
            }
            continue;
        }
        let (a, b) = ((node.min[axis] - origin[axis]) * inv[axis], (node.max[axis] - origin[axis]) * inv[axis]);
        let (lo, hi) = (a.min(b), a.max(b));
        near = near.max(lo);
        far = far.min(hi);
    }
    (far >= near).then_some(near)
}

/// Sorts `entries` so the first `n` go left and the rest right, by the cheapest of `BINS - 1`
/// planes along each axis; `n`, or `None` when no split is cheaper than a leaf.
fn split(entries: &mut [u32], boxes: &[Bounds], centres: &[Vec3], spread: &Bounds) -> Option<usize> {
    let mut best: Option<(f32, usize, f32)> = None; // (cost, axis, plane)
    for axis in 0..3 {
        let (lo, hi) = (spread.min[axis], spread.max[axis]);
        if hi - lo < 1e-12 {
            continue;
        }
        let scale = BINS as f32 / (hi - lo);
        let bin_of = |c: f32| (((c - lo) * scale) as usize).min(BINS - 1);
        let mut bins = [(Bounds::EMPTY, 0usize); BINS];
        for &t in entries.iter() {
            let bin = &mut bins[bin_of(centres[t as usize][axis])];
            bin.0.grow(&boxes[t as usize]);
            bin.1 += 1;
        }
        // Left sweep, then right sweep: the cost of each plane between bins.
        let mut left = [(0.0f32, 0usize); BINS - 1];
        let (mut acc, mut n) = (Bounds::EMPTY, 0);
        for i in 0..BINS - 1 {
            acc.grow(&bins[i].0);
            n += bins[i].1;
            left[i] = (acc.area(), n);
        }
        let (mut acc, mut n) = (Bounds::EMPTY, 0);
        for i in (1..BINS).rev() {
            acc.grow(&bins[i].0);
            n += bins[i].1;
            let (area_l, n_l) = left[i - 1];
            if n_l == 0 || n == 0 {
                continue;
            }
            let cost = area_l * n_l as f32 + acc.area() * n as f32;
            if best.is_none_or(|(c, ..)| cost < c) {
                best = Some((cost, axis, lo + i as f32 / scale));
            }
        }
    }
    let parent = {
        let mut b = Bounds::EMPTY;
        entries.iter().for_each(|&t| b.grow(&boxes[t as usize]));
        b.area() * entries.len() as f32
    };
    let (cost, axis, plane) = best?;
    if cost >= parent && entries.len() <= 4 * LEAF {
        return None;
    }
    // Partition in place.
    let (mut i, mut j) = (0, entries.len());
    while i < j {
        if centres[entries[i] as usize][axis] < plane {
            i += 1;
        } else {
            j -= 1;
            entries.swap(i, j);
        }
    }
    (i > 0 && i < entries.len()).then_some(i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{Mesh, ray_hit_triangle};

    /// A small generator, so the tests need no crate for random numbers.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }

        fn point(&mut self, scale: f32) -> Vec3 {
            Vec3::new(self.next() - 0.5, self.next() - 0.5, self.next() - 0.5) * scale
        }
    }

    /// A sphere of `n × n` quads (as a soup) and a cloud of loose triangles around it.
    fn scene(n: usize, loose: usize, rng: &mut Lcg) -> Vec<[f32; 3]> {
        let p = |i: usize, j: usize| {
            let (th, ph) = (i as f32 / n as f32 * std::f32::consts::PI, j as f32 / n as f32 * std::f32::consts::TAU);
            [20.0 * th.sin() * ph.cos(), 20.0 * th.sin() * ph.sin(), 20.0 * th.cos()]
        };
        let mut soup = Vec::new();
        for i in 0..n {
            for j in 0..n {
                let (a, b, c, d) = (p(i, j), p(i + 1, j), p(i + 1, j + 1), p(i, j + 1));
                soup.extend([a, b, c, a, c, d]);
            }
        }
        for _ in 0..loose {
            let at = rng.point(80.0);
            soup.extend([at, at + rng.point(6.0), at + rng.point(6.0)].map(|v| v.to_array()));
        }
        soup
    }

    #[test]
    fn rays_find_what_trying_every_triangle_finds() {
        let mut rng = Lcg(7);
        let soup = scene(40, 600, &mut rng);
        let bvh = Bvh::build(&soup);
        assert_eq!(bvh.len(), soup.len() / 3);
        let mut hits = 0;
        for _ in 0..2000 {
            let origin = rng.point(200.0);
            // Half the rays aimed near the middle, half anywhere.
            let target = if rng.next() < 0.5 { rng.point(30.0) } else { rng.point(200.0) };
            let dir = (target - origin).normalize();
            let brute = ray_hit_triangle(&soup, origin, dir);
            let fast = bvh.ray(&soup, origin, dir);
            match (brute, fast) {
                (None, None) => {}
                (Some((tb, ib)), Some((tf, i_f))) => {
                    hits += 1;
                    assert!((tb - tf).abs() <= 1e-4 * tb.max(1.0), "{tb} vs {tf}");
                    // Two triangles at the same distance (a shared edge) are both right.
                    if ib != i_f {
                        assert!((tb - tf).abs() < 1e-5, "{ib} vs {i_f}");
                    }
                }
                other => panic!("{origin} {dir}: {other:?}"),
            }
        }
        assert!(hits > 500, "the rays mostly hit something: {hits}");
        // Along the axes, from inside, and parallel to faces.
        let cube = Mesh::cube(10.0).to_soup();
        let tree = Bvh::build(&cube);
        for (origin, dir) in [
            (Vec3::new(5.0, -20.0, 5.0), Vec3::Y),
            (Vec3::new(5.0, 5.0, 5.0), -Vec3::Z),
            (Vec3::new(50.0, -20.0, 5.0), Vec3::Y),
            (Vec3::new(0.0, -20.0, 0.0), Vec3::Y),
            (Vec3::new(5.0, -20.0, 5.0), -Vec3::Y),
        ] {
            assert_eq!(tree.ray(&cube, origin, dir).map(|h| h.0), ray_hit_triangle(&cube, origin, dir).map(|h| h.0));
        }
    }

    #[test]
    fn spheres_find_what_trying_every_triangle_finds() {
        let mut rng = Lcg(11);
        let soup = scene(30, 300, &mut rng);
        let bvh = Bvh::build(&soup);
        for _ in 0..300 {
            let (centre, radius) = (rng.point(60.0), rng.next() * 12.0);
            let brute: Vec<u32> = soup
                .as_chunks::<3>()
                .0
                .iter()
                .enumerate()
                .filter(|(_, t)| closest_on_triangle(t, centre).distance(centre) <= radius)
                .map(|(i, _)| i as u32)
                .collect();
            assert_eq!(bvh.sphere(&soup, centre, radius), brute, "{centre} {radius}");
        }
        // The nearest point is right on each region of a triangle.
        let tri = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 4.0, 0.0]];
        for (p, nearest) in [
            (Vec3::new(1.0, 1.0, 3.0), Vec3::new(1.0, 1.0, 0.0)),
            (Vec3::new(-2.0, -2.0, 0.0), Vec3::ZERO),
            (Vec3::new(6.0, -1.0, 0.0), Vec3::new(4.0, 0.0, 0.0)),
            (Vec3::new(2.0, -3.0, 1.0), Vec3::new(2.0, 0.0, 0.0)),
            (Vec3::new(3.0, 3.0, 0.0), Vec3::new(2.0, 2.0, 0.0)),
        ] {
            assert!((closest_on_triangle(&tri, p) - nearest).length() < 1e-5, "{p}");
        }
    }

    #[test]
    fn odd_meshes_build_and_answer() {
        let empty = Bvh::build(&[]);
        assert!(empty.is_empty() && empty.ray(&[], Vec3::ZERO, Vec3::X).is_none());
        assert!(empty.sphere(&[], Vec3::ZERO, 10.0).is_empty());
        // The same triangle a thousand times (no plane splits it) and one that is broken.
        let mut soup: Vec<[f32; 3]> = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]].repeat(1000);
        soup.extend([[f32::NAN, 0.0, 0.0], [1.0, 1.0, 1.0], [2.0, 2.0, 2.0]]);
        let bvh = Bvh::build(&soup);
        assert_eq!(bvh.len(), 1000, "the broken one is left out");
        let hit = bvh.ray(&soup, Vec3::new(0.2, 0.2, 5.0), -Vec3::Z).unwrap();
        assert_eq!(hit, (5.0, 0), "the first of equals");
        assert_eq!(bvh.sphere(&soup, Vec3::new(0.2, 0.2, 0.5), 0.6).len(), 1000);
    }
}
