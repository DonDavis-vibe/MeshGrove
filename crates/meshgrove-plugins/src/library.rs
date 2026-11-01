//! The library as plugins may read it (`library` in the WIT world): search, facts and triangles
//! of files by their ids, never paths or bytes. What a plugin sees is what the user granted
//! (`library_read`): what AI of the caller's access class may see (a panel or job reads as the
//! user, locally; a tool call as the AI that called it), or every library. The host has checked
//! that the grant exists before any of these is called.

use std::path::{Path, PathBuf};

use meshgrove_core::ai::types::ModelFilter;
use meshgrove_core::ai::{Context, Scope, read};
use meshgrove_core::config::{AccessClass, Config, LibraryRead};
use meshgrove_core::db::Library;
use meshgrove_core::{Kind, Mesh, archive, listing, loaders};
use meshgrove_plugin_api::MeshData;
use serde_json::Value;

pub trait LibraryAccess: Send + Sync {
    /// Matches for a search as typed in Meshgrove, as JSON (ids, names, sizes).
    fn search(&self, query: &str, limit: u32, read: LibraryRead, class: AccessClass) -> Result<Value, String>;
    /// Facts of one file by its id, as JSON.
    fn describe(&self, id: &str, read: LibraryRead, class: AccessClass) -> Result<Value, String>;
    /// The model's triangles, simplified to at most `max_triangles`.
    fn triangles(
        &self,
        id: &str,
        max_triangles: u32,
        read: LibraryRead,
        class: AccessClass,
    ) -> Result<MeshData, String>;
}

/// The user's library: the index and the settings, read afresh on every call (so what the
/// user changes in Meshgrove counts at once).
pub struct LibraryReader {
    db: PathBuf,
    /// `None`: the user's config.toml.
    config: Option<PathBuf>,
}

impl LibraryReader {
    pub fn new(db: PathBuf, config: Option<PathBuf>) -> LibraryReader {
        LibraryReader { db, config }
    }

    fn config(&self) -> Result<Config, String> {
        match &self.config {
            Some(path) => Config::load_from(path),
            None => Config::load(),
        }
        .map_err(|e| format!("Meshgrove's settings cannot be read: {e}"))
    }

    /// Runs `f` within what the plugin may see.
    fn with<T>(
        &self,
        read: LibraryRead,
        class: AccessClass,
        f: impl FnOnce(&Context) -> Result<T, String>,
    ) -> Result<T, String> {
        let config = self.config()?;
        let mut lib = Library::open_reader(&self.db).ok_or("the library is not indexed yet")?;
        lib.set_printers(&config.printers_in_order()).map_err(|e| e.to_string())?;
        let cx = match read {
            LibraryRead::None => return Err("not allowed to read the library".into()),
            LibraryRead::AiScope => Context::new(&lib, &config, class, None).map_err(|e| e.to_string())?,
            LibraryRead::All => Context {
                lib: &lib,
                scope: Scope::whole(&config),
                config: &config,
                thumbs: None,
                printers: config.printers_in_order(),
                now: std::time::SystemTime::now(),
            },
        };
        f(&cx)
    }
}

/// `value` without the paths on this computer.
fn without_paths(mut value: Value) -> Value {
    fn strip(v: &mut Value) {
        match v {
            Value::Object(map) => {
                map.retain(|k, _| k != "path" && !k.ends_with("_path"));
                map.values_mut().for_each(strip);
            }
            Value::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    strip(&mut value);
    value
}

impl LibraryAccess for LibraryReader {
    fn search(&self, query: &str, limit: u32, read: LibraryRead, class: AccessClass) -> Result<Value, String> {
        self.with(read, class, |cx| {
            let filter =
                ModelFilter { q: Some(query.to_owned()), limit: Some(limit as usize), ..ModelFilter::default() };
            let page = read::search_models(cx, &filter).map_err(|e| e.to_string())?;
            Ok(without_paths(serde_json::to_value(page).map_err(|e| e.to_string())?))
        })
    }

    fn describe(&self, id: &str, read: LibraryRead, class: AccessClass) -> Result<Value, String> {
        self.with(read, class, |cx| {
            let (detail, _) = read::get_model(cx, id, false).map_err(|e| e.to_string())?;
            Ok(without_paths(serde_json::to_value(detail).map_err(|e| e.to_string())?))
        })
    }

    fn triangles(
        &self,
        id: &str,
        max_triangles: u32,
        read: LibraryRead,
        class: AccessClass,
    ) -> Result<MeshData, String> {
        let (path, units) =
            self.with(read, class, |cx| Ok((cx.resolve(id).map_err(|e| e.to_string())?, cx.config.model_units)))?;
        meshgrove_core::config::set_model_units(units);
        let data = simplify(&load(&path)?, max_triangles as usize);
        if data.triangles() == 0 {
            return Err(format!("{id} has no triangles"));
        }
        Ok(data)
    }
}

/// The model in a file (or inside an archive), in millimetres.
fn load(path: &Path) -> Result<Mesh, String> {
    let kind = Kind::from_file_name(&listing::file_name(path));
    if kind == Kind::Step {
        return loaders::step::load(path).map_err(|e| e.to_string());
    }
    let bytes = archive::read(path).map_err(|e| e.to_string())?;
    let sibling = |name: &str| archive::read_sibling(path, name);
    loaders::load_mesh_with(kind, &bytes, &sibling).map_err(|e| e.to_string())
}

/// `mesh` with at most `max` triangles: the vertices in each cell of a grid melt into one
/// (vertex clustering), the grid as fine as the budget allows (one or two passes). Small parts may vanish and thin
/// walls close, but outlines and proportions stay — enough for silhouettes and measuring.
pub fn simplify(mesh: &Mesh, max: usize) -> MeshData {
    let tris: Vec<[[f32; 3]; 3]> = mesh.triangles().map(|t| t.map(|v| v.to_array())).collect();
    let max = max.max(1);
    if tris.len() <= max {
        return indexed(&tris);
    }
    let Some(bounds) = mesh.bounds() else { return MeshData::default() };
    let min = bounds.min.to_array();
    // A surface cut into cells of edge `step` gives about two triangles per cell: start from
    // the area, then correct by how far off the count was (it goes with 1/step²).
    let area: f64 = tris
        .iter()
        .map(|[a, b, c]| {
            let (u, v) = (glam::Vec3::from(*b) - glam::Vec3::from(*a), glam::Vec3::from(*c) - glam::Vec3::from(*a));
            f64::from(u.cross(v).length() / 2.0)
        })
        .sum();
    let largest = bounds.size().max_element().max(1e-6);
    let mut step = ((2.0 * area / max as f64).sqrt() as f32).clamp(largest / 4096.0, largest);
    loop {
        let data = clustered(&tris, |p| [0, 1, 2].map(|i| ((p[i] - min[i]) / step).floor() as i32));
        let count = data.triangles();
        if count <= max || step >= largest {
            return data;
        }
        step = (step * ((count as f32 / max as f32).sqrt() * 1.05).max(1.1)).min(largest);
    }
}

/// The triangles with each vertex replaced by the mean of the vertices sharing its key;
/// triangles that collapse, or come twice, are dropped.
fn clustered(tris: &[[[f32; 3]; 3]], key: impl Fn([f32; 3]) -> [i32; 3]) -> MeshData {
    use std::collections::{HashMap, HashSet};
    let mut ids: HashMap<[i32; 3], u32> = HashMap::new();
    let mut sums: Vec<([f64; 3], u32)> = Vec::new();
    let mut indices = Vec::new();
    let mut seen = HashSet::new();
    for t in tris {
        let v = t.map(|p| {
            let id = *ids.entry(key(p)).or_insert_with(|| {
                sums.push(([0.0; 3], 0));
                (sums.len() - 1) as u32
            });
            let (sum, n) = &mut sums[id as usize];
            for i in 0..3 {
                sum[i] += f64::from(p[i]);
            }
            *n += 1;
            id
        });
        if v[0] == v[1] || v[1] == v[2] || v[0] == v[2] {
            continue;
        }
        let mut sorted = v;
        sorted.sort_unstable();
        if seen.insert(sorted) {
            indices.extend(v);
        }
    }
    let positions = sums.iter().flat_map(|(sum, n)| sum.map(|s| (s / f64::from((*n).max(1))) as f32)).collect();
    MeshData { positions, indices }
}

/// The triangles as an indexed mesh, equal vertices shared.
fn indexed(tris: &[[[f32; 3]; 3]]) -> MeshData {
    use std::collections::HashMap;
    let mut ids: HashMap<[u32; 3], u32> = HashMap::new();
    let mut positions = Vec::new();
    let mut indices = Vec::with_capacity(tris.len() * 3);
    for p in tris.iter().flatten() {
        let id = *ids.entry(p.map(f32::to_bits)).or_insert_with(|| {
            positions.extend(p);
            (positions.len() / 3 - 1) as u32
        });
        indices.push(id);
    }
    MeshData { positions, indices }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ball of `n × n` quads, as a triangle soup.
    fn ball(n: usize) -> Mesh {
        let p = |i: usize, j: usize| {
            let (th, ph) = (i as f32 / n as f32 * std::f32::consts::PI, j as f32 / n as f32 * std::f32::consts::TAU);
            [20.0 * th.sin() * ph.cos(), 20.0 * th.sin() * ph.sin(), 20.0 * th.cos()]
        };
        let mut positions = Vec::new();
        for i in 0..n {
            for j in 0..n {
                let (a, b, c, d) = (p(i, j), p(i + 1, j), p(i + 1, j + 1), p(i, j + 1));
                positions.extend([a, b, c, a, c, d]);
            }
        }
        Mesh { positions, ..Mesh::default() }
    }

    #[test]
    fn small_meshes_stay_as_they_are_with_shared_vertices() {
        let data = simplify(&Mesh::cube(10.0), 1000);
        assert_eq!(data.triangles(), 12);
        assert_eq!(data.vertices(), 8, "corners are shared");
        assert_eq!(data.check(), Ok(()));
    }

    #[test]
    fn large_meshes_come_down_to_the_budget_and_keep_their_size() {
        let mesh = ball(300); // 180 000 triangles
        let data = simplify(&mesh, 20_000);
        assert!(data.triangles() <= 20_000 && data.triangles() > 2_000, "{}", data.triangles());
        assert_eq!(data.check(), Ok(()));
        let (min, max) = data.bounds().unwrap();
        assert!((max[0] - min[0] - 40.0).abs() < 3.0, "{min:?} {max:?}");
        assert!(simplify(&mesh, 50).triangles() <= 50);
    }

    #[test]
    fn answers_carry_no_paths() {
        let v = serde_json::json!({"id": "lib/a.stl", "path": "/home/u/lib/a.stl", "copies": [{"id": "x", "full_path": "/p"}]});
        assert_eq!(without_paths(v), serde_json::json!({"id": "lib/a.stl", "copies": [{"id": "x"}]}));
    }
}
