//! Printability check of a mesh: open and non-manifold edges, degenerate triangles, separate
//! parts, volume; plus the hints that need no geometry (suspicious units, fits no printer).

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::ModelUnits;
use crate::formats::Kind;
use crate::mesh::Mesh;
use crate::printer::{self, Fit, Printer};

/// Bump when the check changes, so stored results are computed again.
pub const CHECKS_VERSION: u8 = 1;
/// Meshes up to this size are checked while their thumbnail is made; larger ones later, in the background.
pub const INLINE_LIMIT: usize = 500_000;
/// Above this the edges are not counted (memory); the mesh is only flagged as very large.
pub const MAX_TOPOLOGY: usize = 5_000_000;
/// From here on a mesh counts as having very many triangles (slow to slice, a hint for scans).
pub const MANY_TRIANGLES: usize = 2_000_000;
/// Points closer than this (mm) are one point: exporters split vertices at texture seams.
const WELD_MM: f32 = 1e-4;
/// Largest side below which a model is suspiciously small (drawn in inches or metres?).
const TINY_MM: f32 = 5.0;
/// Largest side above which a model is suspiciously big.
const HUGE_MM: f32 = 2000.0;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MeshChecks {
    pub version: u8,
    pub triangles: u64,
    /// Edges with one triangle: holes in the surface.
    pub open_edges: u64,
    /// Edges shared by more than two triangles.
    pub non_manifold_edges: u64,
    /// Triangles without area.
    pub degenerate: u64,
    /// Separate connected parts.
    pub shells: u32,
    /// Enclosed volume, when the surface is closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_mm3: Option<f64>,
    /// Why the edges were not counted (too large), or why the file could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    /// The file could not be read at all.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unreadable: bool,
}

/// Bits of [`MeshChecks::flags`], stored with the checks so searches need not parse them.
pub mod flag {
    pub const OPEN: u32 = 1;
    pub const NON_MANIFOLD: u32 = 2;
    pub const DEGENERATE: u32 = 4;
    pub const MANY_TRIANGLES: u32 = 8;
    pub const UNREADABLE: u32 = 16;
}

impl MeshChecks {
    pub fn unreadable(message: &str) -> MeshChecks {
        MeshChecks {
            version: CHECKS_VERSION,
            unreadable: true,
            skipped: Some(message.to_owned()),
            ..MeshChecks::default()
        }
    }

    /// The problems found, as [`flag`] bits. A few degenerate triangles are normal (slicers drop
    /// them); only more than one in a hundred counts.
    pub fn flags(&self) -> u32 {
        let mut flags = 0;
        if self.open_edges > 0 {
            flags |= flag::OPEN;
        }
        if self.non_manifold_edges > 0 {
            flags |= flag::NON_MANIFOLD;
        }
        if self.degenerate * 100 > self.triangles {
            flags |= flag::DEGENERATE;
        }
        if self.triangles as usize > MANY_TRIANGLES {
            flags |= flag::MANY_TRIANGLES;
        }
        if self.unreadable {
            flags |= flag::UNREADABLE;
        }
        flags
    }

    /// Closed surface without other problems.
    pub fn is_sound(&self) -> bool {
        self.flags() & !flag::MANY_TRIANGLES == 0 && self.skipped.is_none()
    }
}

/// Checks the surface of `mesh` (positions in mm).
pub fn check(mesh: &Mesh) -> MeshChecks {
    let triangles = mesh.triangle_count();
    let mut out = MeshChecks { version: CHECKS_VERSION, triangles: triangles as u64, ..MeshChecks::default() };
    if triangles > MAX_TOPOLOGY {
        out.skipped = Some(format!("more than {MAX_TOPOLOGY} triangles"));
        return out;
    }
    // Weld: one id per point on the grid.
    let mut ids: HashMap<[i64; 3], u32> = HashMap::with_capacity(triangles * 3 / 2 + 1);
    let key = |p: [f32; 3]| p.map(|v| (v / WELD_MM).round() as i64);
    let mut corner_ids = Vec::with_capacity(triangles);
    let mut volume = 0.0f64;
    for t in mesh.triangles() {
        let c = t.map(|v| {
            let next = ids.len() as u32;
            *ids.entry(key(v.to_array())).or_insert(next)
        });
        let area2 = (t[1] - t[0]).cross(t[2] - t[0]).length_squared();
        if c[0] == c[1] || c[1] == c[2] || c[0] == c[2] || area2 <= f32::EPSILON * f32::EPSILON {
            out.degenerate += 1;
        }
        if c[0] != c[1] && c[1] != c[2] && c[0] != c[2] {
            corner_ids.push(c);
        }
        volume += t[0].as_dvec3().dot(t[1].as_dvec3().cross(t[2].as_dvec3())) / 6.0;
    }
    let mut edges: Vec<u64> = Vec::with_capacity(corner_ids.len() * 3);
    for [a, b, c] in &corner_ids {
        for (u, v) in [(*a, *b), (*b, *c), (*c, *a)] {
            edges.push(((u.min(v) as u64) << 32) | u.max(v) as u64);
        }
    }
    edges.sort_unstable();
    for run in edges.chunk_by(|a, b| a == b) {
        match run.len() {
            1 => out.open_edges += 1,
            2 => {}
            _ => out.non_manifold_edges += 1,
        }
    }
    // Parts: points joined by triangles.
    let mut parent: Vec<u32> = (0..ids.len() as u32).collect();
    fn root(parent: &mut [u32], mut i: u32) -> u32 {
        while parent[i as usize] != i {
            parent[i as usize] = parent[parent[i as usize] as usize];
            i = parent[i as usize];
        }
        i
    }
    for [a, b, c] in &corner_ids {
        for (u, v) in [(*a, *b), (*a, *c)] {
            let (ru, rv) = (root(&mut parent, u), root(&mut parent, v));
            if ru != rv {
                parent[ru as usize] = rv;
            }
        }
    }
    let mut used = vec![false; ids.len()];
    for c in &corner_ids {
        for &v in c {
            used[v as usize] = true;
        }
    }
    out.shells = (0..ids.len() as u32).filter(|&v| used[v as usize] && root(&mut parent, v) == v).count() as u32;
    if out.open_edges == 0 && out.non_manifold_edges == 0 && !corner_ids.is_empty() {
        // Rounded to 0.01 mm³: exact enough, and the same after a trip through JSON.
        out.volume_mm3 = Some((volume.abs() * 100.0).round() / 100.0);
    }
    out
}

/// Loads and checks a model file the way its thumbnail reads it; `None` for kinds not checked
/// (STEP tessellations are approximations, G-code and pictures have no surface).
pub fn check_file(path: &Path, kind: Kind, bytes: &[u8]) -> Option<MeshChecks> {
    use crate::loaders;
    let mesh = match kind {
        Kind::Stl => loaders::stl::parse(bytes).map(|m| loaders::in_file_units(kind, m)),
        Kind::Obj | Kind::Ply | Kind::Gltf => {
            let sibling = |name: &str| crate::archive::read_sibling(path, name);
            loaders::load_mesh_with(kind, bytes, &sibling)
        }
        Kind::ThreeMf => loaders::threemf::parse(bytes).map(|m| m.mesh),
        _ => return None,
    };
    Some(match mesh {
        Ok(mesh) => check(&mesh),
        Err(e) => MeshChecks::unreadable(&e.to_string()),
    })
}

/// Kinds [`check_file`] checks.
pub fn checked(kind: Kind) -> bool {
    matches!(kind, Kind::Stl | Kind::Obj | Kind::Ply | Kind::Gltf | Kind::ThreeMf)
}

/// Something about a model that may make printing it fail.
#[derive(Debug, Clone, PartialEq)]
pub enum Problem {
    /// Edges with one triangle.
    Open(u64),
    NonManifold(u64),
    Degenerate(u64),
    ManyTriangles(u64),
    /// Largest side `max_mm`; drawn in inches, it would be `as_inch_mm`.
    Tiny {
        max_mm: f32,
        as_inch_mm: f32,
    },
    Huge {
        max_mm: f32,
    },
    /// Too big for every printer.
    FitsNone,
    Unreadable(String),
}

impl Problem {
    /// A short code for machine readers (`meshgrove-mcp`).
    pub fn code(&self) -> &'static str {
        match self {
            Problem::Open(_) => "open_edges",
            Problem::NonManifold(_) => "non_manifold",
            Problem::Degenerate(_) => "degenerate",
            Problem::ManyTriangles(_) => "many_triangles",
            Problem::Tiny { .. } => "tiny",
            Problem::Huge { .. } => "huge",
            Problem::FitsNone => "fits_no_printer",
            Problem::Unreadable(_) => "unreadable",
        }
    }
}

/// Everything that may make printing a model of `kind` and `size_mm` fail. Units and the bed
/// fit are judged here, live, so changing the printers or the unit setting needs no new check.
pub fn problems(
    kind: Kind,
    size_mm: Option<[f32; 3]>,
    checks: Option<&MeshChecks>,
    printers: &[Printer],
    units: ModelUnits,
) -> Vec<Problem> {
    let mut out = Vec::new();
    if let Some(c) = checks {
        let flags = c.flags();
        if flags & flag::UNREADABLE != 0 {
            out.push(Problem::Unreadable(c.skipped.clone().unwrap_or_default()));
            return out;
        }
        if flags & flag::OPEN != 0 {
            out.push(Problem::Open(c.open_edges));
        }
        if flags & flag::NON_MANIFOLD != 0 {
            out.push(Problem::NonManifold(c.non_manifold_edges));
        }
        if flags & flag::DEGENERATE != 0 {
            out.push(Problem::Degenerate(c.degenerate));
        }
        if flags & flag::MANY_TRIANGLES != 0 {
            out.push(Problem::ManyTriangles(c.triangles));
        }
    }
    if kind.is_model()
        && let Some(size) = size_mm
    {
        let max = size.iter().fold(0.0f32, |a, &b| a.max(b));
        // Unit-less formats only: 3MF, STEP and glTF state their unit.
        let unitless = matches!(kind, Kind::Stl | Kind::Obj | Kind::Ply);
        if unitless && max > 0.0 && max < TINY_MM && units == ModelUnits::Millimetres {
            out.push(Problem::Tiny { max_mm: max, as_inch_mm: max * crate::config::MM_PER_INCH });
        } else if max > HUGE_MM {
            out.push(Problem::Huge { max_mm: max });
        }
        if printer::best_fit(size, printers).is_some_and(|(fit, _)| matches!(fit, Fit::TooBig(_))) {
            out.push(Problem::FitsNone);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printer::Bed;

    fn soup(triangles: &[[[f32; 3]; 3]]) -> Mesh {
        Mesh {
            positions: triangles.iter().flatten().copied().collect(),
            indices: None,
            colors: None,
            vertex_colors: None,
        }
    }

    fn cube_soup(size: f32, offset: f32) -> Vec<[[f32; 3]; 3]> {
        let cube = Mesh::cube(size);
        cube.triangles().map(|t| t.map(|v| (v + glam::Vec3::splat(offset)).to_array())).collect()
    }

    #[test]
    fn a_cube_is_closed_and_has_a_volume() {
        let c = check(&Mesh::cube(20.0));
        assert_eq!((c.triangles, c.open_edges, c.non_manifold_edges, c.degenerate, c.shells), (12, 0, 0, 0, 1));
        assert_eq!(c.volume_mm3, Some(8000.0));
        assert!(c.is_sound() && c.flags() == 0);
    }

    #[test]
    fn holes_parts_and_shared_edges_are_found() {
        let mut open = cube_soup(10.0, 0.0);
        open.pop();
        let c = check(&soup(&open));
        assert_eq!(c.open_edges, 3);
        assert_eq!(c.volume_mm3, None, "an open surface has no volume");
        assert_eq!(c.flags(), flag::OPEN);

        let mut two = cube_soup(10.0, 0.0);
        two.extend(cube_soup(10.0, 50.0));
        let c = check(&soup(&two));
        assert_eq!((c.shells, c.open_edges), (2, 0));

        let mut doubled = cube_soup(10.0, 0.0);
        doubled.push(doubled[0]);
        let c = check(&soup(&doubled));
        assert_eq!(c.non_manifold_edges, 3);

        let mut flat = cube_soup(10.0, 0.0);
        flat.push([[0.0, 0.0, 0.0], [5.0, 0.0, 0.0], [10.0, 0.0, 0.0]]);
        let c = check(&soup(&flat));
        assert_eq!(c.degenerate, 1);
        assert_eq!(c.flags() & flag::DEGENERATE, flag::DEGENERATE, "1 of 13 is more than 1 %");
    }

    #[test]
    fn split_seams_are_welded() {
        // Every corner a point of its own, a hair apart, as exporters write texture seams.
        let mut nudged = cube_soup(10.0, 0.0);
        for (i, t) in nudged.iter_mut().enumerate() {
            t[0][0] += if i % 2 == 0 { 0.00001 } else { 0.0 };
        }
        let mesh = Mesh { indices: Some((0..nudged.len() as u32 * 3).collect()), ..soup(&nudged) };
        let c = check(&mesh);
        assert_eq!((c.open_edges, c.non_manifold_edges, c.shells), (0, 0, 1));
    }

    #[test]
    fn units_and_printers_are_judged_live() {
        let s1 = Printer::from(Bed { name: "S1".into(), x: 250.0, y: 250.0, z: 250.0 });
        let mm = ModelUnits::Millimetres;
        assert_eq!(
            problems(Kind::Stl, Some([3.2, 1.0, 1.0]), None, &[], mm),
            [Problem::Tiny { max_mm: 3.2, as_inch_mm: 3.2 * 25.4 }]
        );
        assert!(problems(Kind::ThreeMf, Some([3.2, 1.0, 1.0]), None, &[], mm).is_empty(), "3MF has a unit");
        assert!(problems(Kind::Stl, Some([3.2, 1.0, 1.0]), None, &[], ModelUnits::Inches).is_empty());
        assert_eq!(problems(Kind::Stl, Some([2500.0, 1.0, 1.0]), None, &[], mm)[0], Problem::Huge { max_mm: 2500.0 });
        assert_eq!(
            problems(Kind::Stl, Some([300.0, 300.0, 10.0]), None, std::slice::from_ref(&s1), mm),
            [Problem::FitsNone]
        );
        assert!(problems(Kind::Stl, Some([100.0; 3]), Some(&check(&Mesh::cube(100.0))), &[s1], mm).is_empty());
        let broken = MeshChecks::unreadable("not an STL");
        assert_eq!(problems(Kind::Stl, None, Some(&broken), &[], mm), [Problem::Unreadable("not an STL".into())]);
        assert_eq!(broken.flags(), flag::UNREADABLE);
    }

    #[test]
    fn files_are_loaded_like_their_thumbnails() {
        let bytes = crate::loaders::stl::write_binary(&Mesh::cube(20.0), "x");
        let c = check_file(Path::new("/x/a.stl"), Kind::Stl, &bytes).unwrap();
        assert!(c.is_sound());
        assert!(check_file(Path::new("/x/a.stl"), Kind::Stl, b"\x00\x01").unwrap().unreadable);
        assert_eq!(check_file(Path::new("/x/a.gcode"), Kind::Gcode, b""), None);
    }

    #[test]
    fn huge_meshes_are_not_counted() {
        let c = MeshChecks { triangles: (MANY_TRIANGLES + 1) as u64, version: CHECKS_VERSION, ..Default::default() };
        assert_eq!(c.flags(), flag::MANY_TRIANGLES);
        assert!(c.is_sound(), "many triangles alone print fine");
    }
}
