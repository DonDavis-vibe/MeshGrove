//! The library as plugins may read it (`library` in the WIT world): search, facts and triangles
//! of files by their ids, never paths or bytes. What a plugin sees is what the user granted
//! (`library_read`): what AI of the caller's access class may see (a panel or job reads as the
//! user, locally, also while AI is switched off; a tool call as the AI that called it), or every
//! library. Beside the grant there are the files the user opened with a panel ([`Opened`]): the
//! panel and its jobs read exactly those, grant or not. The host has checked which of the two
//! applies before any of these is called.
//!
//! `describe` and `triangles` take a reference: a library id, a path in the plugin's own folders
//! (`/tmp/…`, `/data/…`; the host reads those itself, [`crate::guest_files`]), or either with
//! a part of a 3MF named after it ([`part_token`]).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use meshgrove_core::ai::types::ModelFilter;
use meshgrove_core::ai::{Context, Scope, read};
use meshgrove_core::config::{AccessClass, Config, LibraryRead};
use meshgrove_core::db::Library;
use meshgrove_core::{Kind, Mesh, archive, listing, loaders};
use meshgrove_plugin_api::MeshData;
use serde_json::Value;

/// Who an instance of a plugin works for, as far as reading the library goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    /// The user, in a panel or one of its jobs. With the grant "what local AI sees" it reads
    /// the libraries open to local AI, also while AI is switched off: no AI reads along.
    User,
    /// An AI of this access class, through a tool call.
    Ai(AccessClass),
}

impl Caller {
    /// The access class the caller reads as.
    pub fn class(self) -> AccessClass {
        match self {
            Caller::User => AccessClass::Local,
            Caller::Ai(class) => class,
        }
    }
}

/// What a plugin sees of the libraries in `config` with grant `read`, asked by `caller`; `None`
/// where that is nothing (no grant, or an AI while AI is switched off).
pub fn scope(config: &Config, read: LibraryRead, caller: Caller) -> Option<Scope> {
    match (read, caller) {
        (LibraryRead::None, _) => None,
        (LibraryRead::All, _) => Some(Scope::whole(config)),
        (LibraryRead::AiScope, Caller::User) => Some(Scope::for_plugin(config, AccessClass::Local)),
        (LibraryRead::AiScope, Caller::Ai(class)) => config.ai.enabled.then(|| Scope::new(config, class)),
    }
}

/// The library files the user opened with a panel (a file type that receives ids), newest
/// last: the panel and its jobs may read exactly these, whatever the plugin is granted. Shared
/// between the window, the panel's instance and its jobs; the window keeps it across restarts.
#[derive(Debug, Clone, Default)]
pub struct Opened(Arc<Mutex<Vec<String>>>);

impl Opened {
    /// How many files a panel keeps: the oldest go first.
    pub const MAX: usize = 512;

    /// An id as it is compared: without blanks around it and without the `meshgrove:` of a
    /// copied reference.
    fn key(id: &str) -> &str {
        id.trim().trim_start_matches("meshgrove:")
    }

    pub fn add(&self, ids: &[String]) {
        let mut list = self.0.lock().unwrap_or_else(|e| e.into_inner());
        for id in ids.iter().map(|id| Opened::key(id)).filter(|id| !id.is_empty()) {
            list.retain(|known| known != id);
            list.push(id.to_owned());
        }
        let over = list.len().saturating_sub(Opened::MAX);
        list.drain(..over);
    }

    pub fn contains(&self, id: &str) -> bool {
        let id = Opened::key(id);
        self.0.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|known| known == id)
    }

    pub fn ids(&self) -> Vec<String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// What `file` holds (a JSON list of ids); nothing when it is missing or broken.
    pub fn load(file: &Path) -> Opened {
        let opened = Opened::default();
        if let Some(ids) = std::fs::read(file).ok().and_then(|b| serde_json::from_slice::<Vec<String>>(&b).ok()) {
            opened.add(&ids);
        }
        opened
    }

    pub fn save(&self, file: &Path) -> std::io::Result<()> {
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(file, serde_json::to_vec(&self.ids()).unwrap_or_default())
    }
}

/// How an instance of a plugin reads the library: for whom, with which grant, and which files
/// the user opened with it. The host asks it before every read, and again for the files a
/// bundle is put beside.
#[derive(Clone)]
pub struct Reading {
    /// The plugin's id, for what it is told when it may not read.
    pub plugin: String,
    pub library: Option<Arc<dyn LibraryAccess>>,
    /// The user's grant to read the library, where the manifest asks for it too.
    pub grant: Option<LibraryRead>,
    pub caller: Caller,
    pub opened: Opened,
}

/// How a plugin got at a library file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Through {
    /// The user lets the plugin read the library.
    Grant,
    /// The user opened this very file with the panel.
    Opening,
}

impl Reading {
    /// Nobody reading anything (a plugin being loaded).
    pub fn nothing(plugin: &str) -> Reading {
        Reading {
            plugin: plugin.to_owned(),
            library: None,
            grant: None,
            caller: Caller::User,
            opened: Opened::default(),
        }
    }

    pub fn not_allowed(&self) -> String {
        format!("the user has not allowed {} to read the library (Meshgrove → Settings → Plugins)", self.plugin)
    }

    fn library(&self) -> Result<&dyn LibraryAccess, String> {
        self.library.as_deref().ok_or_else(|| "no library is open here".to_owned())
    }

    /// Runs `f` on the library as far as the user lets this plugin read it (a search).
    pub fn all<T>(
        &self,
        f: impl FnOnce(&dyn LibraryAccess, LibraryRead, Caller) -> Result<T, String>,
    ) -> Result<T, String> {
        let read = self.grant.ok_or_else(|| self.not_allowed())?;
        f(self.library()?, read, self.caller)
    }

    /// Runs `f` on library file `id`: as far as the user lets this plugin read the library, and
    /// where that does not reach the file, because the user opened it with this panel. Such a
    /// file reads as the user sees it, whatever AI may see.
    pub fn file<T>(
        &self,
        id: &str,
        f: impl Fn(&dyn LibraryAccess, LibraryRead, Caller) -> Result<T, String>,
    ) -> Result<(T, Through), String> {
        let opened = self.opened.contains(id);
        if self.grant.is_none() && !opened {
            return Err(self.not_allowed());
        }
        let library = self.library()?;
        let refused = match self.grant {
            Some(read) => match f(library, read, self.caller) {
                Ok(value) => return Ok((value, Through::Grant)),
                Err(e) => e,
            },
            None => String::new(),
        };
        if !opened {
            return Err(refused);
        }
        f(library, LibraryRead::All, Caller::User).map(|value| (value, Through::Opening))
    }

    /// Where library file `id` is on this computer, if the plugin may read it. For the host
    /// only: a plugin never hears of paths.
    pub fn locate(&self, id: &str) -> Result<Located, String> {
        self.file(id, |library, read, caller| library.locate(id, read, caller)).map(|(located, _)| located)
    }
}

/// A library file on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    /// The library it is in.
    pub root: PathBuf,
    /// The file itself (also one inside an archive).
    pub path: PathBuf,
}

pub trait LibraryAccess: Send + Sync {
    /// Matches for a search as typed in Meshgrove, as JSON (ids, names, sizes).
    fn search(&self, query: &str, limit: u32, read: LibraryRead, caller: Caller) -> Result<Value, String>;
    /// Facts of one file by its id, as JSON; of a 3MF with its `parts` ([`parts_json`]).
    fn describe(&self, id: &str, read: LibraryRead, caller: Caller) -> Result<Value, String>;
    /// The model's triangles, or those of its part `part` (a 3MF's, in the part's own
    /// coordinates), simplified to at most `max_triangles`.
    fn triangles(
        &self,
        id: &str,
        part: Option<usize>,
        max_triangles: u32,
        read: LibraryRead,
        caller: Caller,
    ) -> Result<MeshData, String>;
    /// Where the file behind an id is, for the host itself (to put a bundle's files beside it,
    /// to copy a 3MF): never passed on to a plugin.
    fn locate(&self, _id: &str, _read: LibraryRead, _caller: Caller) -> Result<Located, String> {
        Err("this library does not say where its files are".into())
    }
}

/// Most triangles `library.triangles` hands over; larger models are simplified to it.
pub const MAX_TRIANGLES: u32 = 8_000_000;

/// What stands between a reference and the number of the part that is meant. A NUL cannot be
/// in a file name or an id, so no reference is mistaken for a token.
const PART_MARK: &str = "\0part=";

/// The reference of part `part` of the 3MF `reference` (an id or a path): what `describe` gives
/// as a part's `token`, and `triangles` and a bundle's `derive_3mf` take.
pub fn part_token(reference: &str, part: usize) -> String {
    format!("{reference}{PART_MARK}{part}")
}

/// A reference and the part it names, if it is a [`part_token`].
pub fn split_part(reference: &str) -> Result<(&str, Option<usize>), String> {
    match reference.split_once(PART_MARK) {
        None if reference.contains('\0') => Err("not a reference: it holds a NUL".into()),
        None => Ok((reference, None)),
        Some((base, part)) => {
            let part = part.parse().map_err(|_| "not a part token (they come from describe)".to_owned())?;
            Ok((base, Some(part)))
        }
    }
}

/// Part `part` of the 3MF in `bytes`, in its own coordinates and millimetres.
pub fn part_mesh(bytes: &[u8], part: usize) -> Result<Mesh, String> {
    let mut parts = loaders::threemf::parse_parts(bytes).map_err(|e| e.to_string())?;
    if part >= parts.len() {
        return Err(format!("the file has {} parts, there is no part {part}", parts.len()));
    }
    Ok(parts.swap_remove(part).mesh)
}

/// The parts of the 3MF in `bytes` as `describe` lists them, each with the `token` that names
/// it (after `reference`), its name, its triangles, the corners of its box in its own
/// coordinates (`min`, `max`, mm), how often it is placed (`instances`) and where first
/// (`transform`: twelve numbers as in 3MF, the three axes and then the shift, so a point `p` of
/// the part stands at `x·p.x + y·p.y + z·p.z + shift`). Empty when the file cannot be read.
pub fn parts_json(bytes: &[u8], reference: &str) -> Vec<Value> {
    let Ok(parts) = loaders::threemf::parse_parts(bytes) else { return Vec::new() };
    parts
        .iter()
        .enumerate()
        .map(|(i, part)| {
            let m = part.transform.matrix3;
            let axes = [m.x_axis, m.y_axis, m.z_axis, part.transform.translation];
            let transform: Vec<f32> = axes.iter().flat_map(|a| a.to_array()).collect();
            let (min, max) = part.mesh.bounds().map_or(([0.0; 3], [0.0; 3]), |b| (b.min.to_array(), b.max.to_array()));
            serde_json::json!({
                "token": part_token(reference, i),
                "name": part.name,
                "triangles": part.mesh.triangle_count(),
                "min": min,
                "max": max,
                "transform": transform,
                "instances": part.instances,
            })
        })
        .collect()
}

/// Facts of a model that is in no library (a file in a plugin's folder): its name, kind, size
/// in bytes and, of the mesh, its size in mm and its triangles.
pub fn mesh_facts(name: &str, file_size: usize, mesh: &Mesh) -> Value {
    let mut facts = serde_json::json!({
        "name": name,
        "kind": Kind::from_file_name(name).as_str(),
        "file_size": file_size,
        "triangles": mesh.triangle_count(),
    });
    if let Some(bounds) = mesh.bounds() {
        facts["size_mm"] = serde_json::json!(bounds.size().to_array());
    }
    facts
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
        caller: Caller,
        f: impl FnOnce(&Context) -> Result<T, String>,
    ) -> Result<T, String> {
        let config = self.config()?;
        let mut lib = Library::open_reader(&self.db).ok_or("the library is not indexed yet")?;
        lib.set_printers(&config.printers_in_order()).map_err(|e| e.to_string())?;
        let scope = match scope(&config, read, caller) {
            Some(scope) => scope,
            None if read == LibraryRead::None => return Err("not allowed to read the library".into()),
            None => return Err(meshgrove_core::ai::ToolError::Disabled.to_string()),
        };
        let cx = Context {
            lib: &lib,
            scope,
            config: &config,
            thumbs: None,
            printers: config.printers_in_order(),
            now: std::time::SystemTime::now(),
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
    fn search(&self, query: &str, limit: u32, read: LibraryRead, caller: Caller) -> Result<Value, String> {
        self.with(read, caller, |cx| {
            let filter =
                ModelFilter { q: Some(query.to_owned()), limit: Some(limit as usize), ..ModelFilter::default() };
            let page = read::search_models(cx, &filter).map_err(|e| e.to_string())?;
            Ok(without_paths(serde_json::to_value(page).map_err(|e| e.to_string())?))
        })
    }

    fn describe(&self, id: &str, read: LibraryRead, caller: Caller) -> Result<Value, String> {
        let (mut facts, path) = self.with(read, caller, |cx| {
            let (detail, _) = read::get_model(cx, id, false).map_err(|e| e.to_string())?;
            let path = PathBuf::from(&detail.path);
            Ok((without_paths(serde_json::to_value(detail).map_err(|e| e.to_string())?), path))
        })?;
        // A 3MF also says what it is put together from.
        if Kind::from_file_name(&listing::file_name(&path)) == Kind::ThreeMf
            && let Ok(bytes) = archive::read(&path)
        {
            let parts = parts_json(&bytes, id.trim());
            if !parts.is_empty() {
                facts["parts"] = Value::Array(parts);
            }
        }
        Ok(facts)
    }

    fn triangles(
        &self,
        id: &str,
        part: Option<usize>,
        max_triangles: u32,
        read: LibraryRead,
        caller: Caller,
    ) -> Result<MeshData, String> {
        let (path, units) =
            self.with(read, caller, |cx| Ok((cx.resolve(id).map_err(|e| e.to_string())?, cx.config.model_units)))?;
        meshgrove_core::config::set_model_units(units);
        let mesh = match part {
            Some(part) => part_mesh(&archive::read(&path).map_err(|e| e.to_string())?, part)?,
            None => load_file(&path)?,
        };
        let data = simplify(&mesh, max_triangles as usize);
        if data.triangles() == 0 {
            return Err(format!("{id} has no triangles"));
        }
        Ok(data)
    }

    fn locate(&self, id: &str, read: LibraryRead, caller: Caller) -> Result<Located, String> {
        self.with(read, caller, |cx| {
            let path = cx.resolve(id).map_err(|e| e.to_string())?;
            let root = cx.scope.root_of(&path).ok_or_else(|| format!("unknown id: {id}"))?.path.clone();
            Ok(Located { root, path })
        })
    }
}

/// The model in a file (or inside an archive), in millimetres. It needs no library.
pub fn load_file(path: &Path) -> Result<Mesh, String> {
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
    fn opened_files_are_kept_in_order_and_survive_a_restart() {
        let opened = Opened::default();
        opened.add(&["Eigene/a.stl".into(), " meshgrove:Eigene/b.3mf ".into(), String::new()]);
        assert!(opened.contains("Eigene/a.stl") && opened.contains("meshgrove:Eigene/a.stl"));
        assert!(opened.contains("Eigene/b.3mf") && !opened.contains("Eigene/c.stl") && !opened.contains(""));
        assert!(!opened.contains("Eigene/a.stl/../b.stl") && !opened.contains("Eigene"), "the ids as they are");
        opened.add(&["Eigene/a.stl".into()]);
        assert_eq!(opened.ids(), ["Eigene/b.3mf", "Eigene/a.stl"], "opened again: the newest");
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("state").join("box.opened.json");
        opened.save(&file).unwrap();
        assert_eq!(Opened::load(&file).ids(), opened.ids());
        assert!(Opened::load(&dir.path().join("missing.json")).ids().is_empty());
        std::fs::write(&file, "not json").unwrap();
        assert!(Opened::load(&file).ids().is_empty());
        // A panel keeps the newest; a clone is the same list.
        let shared = opened.clone();
        let many: Vec<String> = (0..Opened::MAX + 3).map(|i| format!("Eigene/{i}.stl")).collect();
        shared.add(&many);
        assert_eq!(opened.ids().len(), Opened::MAX);
        assert!(!opened.contains("Eigene/2.stl") && opened.contains("Eigene/3.stl"));
    }

    #[test]
    fn a_panel_reads_with_the_grant_while_ai_is_off() {
        use meshgrove_core::config::{AiAccess, LibraryAi};
        let mut config =
            Config { library_roots: vec!["/lib/eigene".into(), "/lib/kunden".into()], ..Config::default() };
        config.ai.libraries = vec![
            LibraryAi { root: "/lib/eigene".into(), access: AiAccess::Cloud, images: false },
            LibraryAi { root: "/lib/kunden".into(), access: AiAccess::LocalOnly, images: false },
        ];
        let seen = |config: &Config, read, caller| {
            scope(config, read, caller).map(|s| s.roots().iter().map(|r| r.alias.clone()).collect::<Vec<_>>())
        };
        let (local, cloud) = (Caller::Ai(AccessClass::Local), Caller::Ai(AccessClass::Cloud));
        assert!(!config.ai.enabled);
        assert_eq!(seen(&config, LibraryRead::AiScope, Caller::User), Some(vec!["eigene".into(), "kunden".into()]));
        assert_eq!(seen(&config, LibraryRead::AiScope, local), None, "no AI reads while AI is off");
        assert_eq!(seen(&config, LibraryRead::All, cloud).map(|r| r.len()), Some(2));
        assert_eq!(seen(&config, LibraryRead::None, Caller::User), None);
        config.ai.enabled = true;
        assert_eq!(seen(&config, LibraryRead::AiScope, cloud), Some(vec!["eigene".into()]));
        assert_eq!(seen(&config, LibraryRead::AiScope, local).map(|r| r.len()), Some(2));
    }

    #[test]
    fn part_tokens_name_a_part_of_a_reference() {
        let token = part_token("Eigene/dose.3mf", 3);
        assert_eq!(split_part(&token), Ok(("Eigene/dose.3mf", Some(3))));
        assert_eq!(split_part("Eigene/dose.3mf"), Ok(("Eigene/dose.3mf", None)));
        assert_eq!(split_part(&part_token("/data/picked/dose.3mf", 0)), Ok(("/data/picked/dose.3mf", Some(0))));
        // A file may be called anything, a token it is not.
        assert_eq!(split_part("Eigene/part=2.3mf"), Ok(("Eigene/part=2.3mf", None)));
        for bad in ["Eigene/dose.3mf\0part=x", "Eigene/dose.3mf\0part=-1", "Eigene/dose.3mf\0", "a\0b"] {
            assert!(split_part(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn answers_carry_no_paths() {
        let v = serde_json::json!({"id": "lib/a.stl", "path": "/home/u/lib/a.stl", "copies": [{"id": "x", "full_path": "/p"}]});
        assert_eq!(without_paths(v), serde_json::json!({"id": "lib/a.stl", "copies": [{"id": "x"}]}));
    }
}
