//! Models on the GPU for the 3D viewer and the hover turntable. Loads happen on a background
//! thread (newest request first, one file at a time — easy on the HDD and on RAM), results are
//! shared by path and the least recently used ones are dropped once over the VRAM budget.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use crossbeam_channel::{Receiver, unbounded};
use meshgrove_core::Kind;
use meshgrove_core::loaders::{self, threemf};
use meshgrove_render::{GpuMesh, PreparedMesh};

/// Kinds the viewer and turntable can show.
pub fn viewable(kind: Kind) -> bool {
    kind.is_model()
}

const VRAM_BUDGET: usize = 1536 << 20;
/// Older requests beyond this are dropped: the user has moved on.
const MAX_PENDING: usize = 4;

pub struct ModelInfo {
    pub triangles: usize,
    pub size_mm: glam::Vec3,
    pub title: Option<String>,
    pub designer: Option<String>,
    pub application: Option<String>,
    /// 3MF plate previews (plate number, picture).
    pub plates: Vec<(usize, egui::ColorImage)>,
    /// The build plate this mesh shows, for multi-plate slicer projects.
    pub plate: Option<usize>,
    pub plate_count: usize,
    /// Filaments the shown plate prints with (slicer projects).
    pub filaments: Vec<threemf::Filament>,
}

impl ModelInfo {
    /// Facts of a plain mesh file (no 3MF metadata or plates).
    fn plain() -> ModelInfo {
        ModelInfo {
            triangles: 0,
            size_mm: glam::Vec3::ZERO,
            title: None,
            designer: None,
            application: None,
            plates: Vec::new(),
            plate: None,
            plate_count: 0,
            filaments: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct Loaded {
    pub mesh: Arc<GpuMesh>,
    pub info: Arc<ModelInfo>,
}

pub enum MeshState {
    Loading,
    Ready(Loaded),
    Failed(String),
}

struct Slot {
    state: MeshState,
    last_used: u64,
}

/// A model file, optionally narrowed to one build plate.
pub type MeshKey = (PathBuf, Option<usize>);
type Pending = Arc<(Mutex<Vec<(MeshKey, Kind)>>, Condvar)>;

pub struct MeshStore {
    slots: HashMap<MeshKey, Slot>,
    pending: Pending,
    results: Receiver<(MeshKey, Result<Loaded, String>)>,
    frame: u64,
}

/// A model read and made ready for the GPU.
type Read = Result<(PreparedMesh, ModelInfo), String>;

/// A model that is being read before there is a GPU to show it on: the file a start with model
/// files alone shows first, read while the window and the graphics start.
pub struct HeadStart {
    key: MeshKey,
    kind: Kind,
    /// The model, and the ms its reading took.
    read: Receiver<(Read, u128)>,
    began: std::time::Instant,
}

impl HeadStart {
    /// Starts reading the model at `path` (its first plate, as a window shows it first).
    pub fn of(path: &Path) -> HeadStart {
        let kind = Kind::from_file_name(&meshgrove_core::listing::file_name(path));
        let (tx, rx) = crossbeam_channel::bounded(1);
        let file = path.to_path_buf();
        let spawned = std::thread::Builder::new().name("mesh-head-start".into()).spawn(move || {
            let began = std::time::Instant::now();
            let read = read(&file, kind, None);
            let read_ms = began.elapsed().as_millis();
            let _ = tx.send((read.and_then(|(mesh, info)| prepare(&mesh, info)), read_ms));
        });
        if let Err(e) = spawned {
            log::warn!("no head start for the first model: {e}");
        }
        HeadStart { key: (path.to_path_buf(), None), kind, read: rx, began: std::time::Instant::now() }
    }

    /// Waits for the model and uploads it.
    fn finish(self, device: &wgpu::Device) -> (MeshKey, Result<Loaded, String>) {
        let Ok((read, read_ms)) = self.read.recv() else {
            // Its thread never ran: read now, as any other model.
            let loaded = load(&self.key.0, self.kind, None, device);
            return (self.key, loaded);
        };
        let loaded = read.map(|(prepared, info)| upload(&prepared, info, device));
        if let Ok(loaded) = &loaded {
            log_loaded(&self.key.0, loaded, read_ms, self.began);
        }
        (self.key, loaded)
    }
}

impl MeshStore {
    pub fn new(device: wgpu::Device, ctx: egui::Context) -> MeshStore {
        MeshStore::with_head_start(device, ctx, None)
    }

    /// As [`MeshStore::new`], with a model whose reading began before (see [`HeadStart`]).
    pub fn with_head_start(device: wgpu::Device, ctx: egui::Context, head: Option<HeadStart>) -> MeshStore {
        let pending: Pending = Arc::default();
        let (tx, results) = unbounded();
        let queue = pending.clone();
        // Asked for from the first frame on: it is on its way already.
        let slots = head.iter().map(|h| (h.key.clone(), Slot { state: MeshState::Loading, last_used: 0 })).collect();
        std::thread::Builder::new()
            .name("mesh-loader".into())
            .spawn(move || {
                if let Some(head) = head {
                    if tx.send(head.finish(&device)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
                loop {
                    let (key, kind) = {
                        let (lock, wake) = &*queue;
                        let mut q = lock.lock().unwrap();
                        loop {
                            if let Some(job) = q.pop() {
                                break job;
                            }
                            q = wake.wait(q).unwrap();
                        }
                    };
                    let result = load(&key.0, kind, key.1, &device);
                    if tx.send((key, result)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("spawning mesh loader");
        MeshStore { slots, pending, results, frame: 0 }
    }

    /// Call once per frame before any `request`: collects finished loads and evicts.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        while let Ok((key, result)) = self.results.try_recv() {
            let state = match result {
                Ok(loaded) => MeshState::Ready(loaded),
                Err(e) => MeshState::Failed(e),
            };
            self.slots.insert(key, Slot { state, last_used: self.frame });
        }
        self.evict();
    }

    /// The model's state; starts loading it if it is unknown. Marks it as in use this frame.
    pub fn request(&mut self, path: &Path, kind: Kind, plate: Option<usize>) -> &MeshState {
        let key: MeshKey = (path.to_path_buf(), plate);
        if !self.slots.contains_key(&key) {
            let dropped = {
                let (lock, wake) = &*self.pending;
                let mut q = lock.lock().unwrap();
                q.push((key.clone(), kind));
                let dropped = (q.len() > MAX_PENDING).then(|| q.remove(0).0);
                wake.notify_one();
                dropped
            };
            // Forget the dropped request so asking for it again re-queues it.
            if let Some(old) = dropped {
                self.slots.remove(&old);
            }
            self.slots.insert(key.clone(), Slot { state: MeshState::Loading, last_used: self.frame });
        }
        let slot = self.slots.get_mut(&key).expect("inserted above");
        slot.last_used = self.frame;
        &slot.state
    }

    /// Drops the loaded models whose path matches, so they load again (after a setting that
    /// changes how they are read).
    pub fn forget_where(&mut self, matches: impl Fn(&Path) -> bool) {
        self.slots.retain(|(path, _), slot| matches!(slot.state, MeshState::Loading) || !matches(path));
    }

    fn evict(&mut self) {
        let mut used: usize = self.slots.values().filter_map(|s| ready_bytes(&s.state)).sum();
        while used > VRAM_BUDGET {
            let victim = self
                .slots
                .iter()
                .filter(|(_, s)| s.last_used < self.frame && ready_bytes(&s.state).is_some())
                .min_by_key(|(_, s)| s.last_used)
                .map(|(p, _)| p.clone());
            let Some(victim) = victim else { break };
            if let Some(slot) = self.slots.remove(&victim) {
                used -= ready_bytes(&slot.state).unwrap_or(0);
            }
        }
    }
}

fn ready_bytes(state: &MeshState) -> Option<usize> {
    match state {
        MeshState::Ready(l) => Some(l.mesh.byte_size()),
        _ => None,
    }
}

fn load(path: &Path, kind: Kind, plate: Option<usize>, device: &wgpu::Device) -> Result<Loaded, String> {
    let began = std::time::Instant::now();
    let (mesh, info) = read(path, kind, plate)?;
    let read_ms = began.elapsed().as_millis();
    let (prepared, info) = prepare(&mesh, info)?;
    let loaded = upload(&prepared, info, device);
    log_loaded(path, &loaded, read_ms, began);
    Ok(loaded)
}

/// How long a model took: reading its file, and all of it from the request until it is on the
/// GPU (a head start waits for the GPU in between).
fn log_loaded(path: &Path, loaded: &Loaded, read_ms: u128, began: std::time::Instant) {
    let (name, triangles) = (meshgrove_core::listing::file_name(path), loaded.info.triangles);
    let all = began.elapsed().as_millis();
    log::info!("loaded {name}: {triangles} triangles, read in {read_ms} ms, ready after {all} ms");
}

/// The model's triangles on the CPU (for measuring), as a soup in the same recentred
/// coordinates as its `GpuMesh`.
pub fn load_triangles(path: &Path, kind: Kind, plate: Option<usize>) -> Result<Vec<[f32; 3]>, String> {
    let (mesh, _) = read(path, kind, plate)?;
    let center = mesh.bounds().ok_or_else(|| t!("error.empty_model").into_owned())?.center();
    Ok(mesh.to_soup().into_iter().map(|p| (glam::Vec3::from(p) - center).to_array()).collect())
}

fn read(path: &Path, kind: Kind, plate: Option<usize>) -> Result<(meshgrove_core::Mesh, ModelInfo), String> {
    if kind == Kind::Step {
        let mesh = loaders::step::load(path).map_err(|e| e.to_string())?;
        return Ok((mesh, ModelInfo::plain()));
    }
    let bytes = meshgrove_core::archive::read(path).map_err(|e| e.to_string())?;
    let (mesh, info) = match kind {
        Kind::ThreeMf => {
            let model = threemf::parse_plate(&bytes, plate).map_err(|e| e.to_string())?;
            let plates = threemf::plate_images(&bytes)
                .into_iter()
                .map(|(n, img)| {
                    let size = [img.width() as usize, img.height() as usize];
                    (n, egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
                })
                .collect();
            let info = ModelInfo {
                triangles: model.triangles,
                size_mm: glam::Vec3::ZERO,
                title: model.title,
                designer: model.designer,
                application: model.application,
                plates,
                plate: model.plate,
                plate_count: model.plates.len(),
                filaments: model.filaments,
            };
            (model.mesh, info)
        }
        _ => {
            // OBJ materials and textures, glTF buffers: read from next to the model.
            let sibling = |name: &str| meshgrove_core::archive::read_sibling(path, name);
            (loaders::load_mesh_with(kind, &bytes, &sibling).map_err(|e| e.to_string())?, ModelInfo::plain())
        }
    };
    Ok((mesh, info))
}

/// The work on a read model that needs no GPU: its facts, and its mesh made ready to upload.
fn prepare(mesh: &meshgrove_core::Mesh, mut info: ModelInfo) -> Read {
    // 3MF files report their own count: split paint must not inflate it.
    if info.triangles == 0 {
        info.triangles = mesh.triangle_count();
    }
    info.size_mm = mesh.bounds().map(|b| b.size()).unwrap_or_default();
    let prepared = PreparedMesh::of(mesh).ok_or_else(|| t!("error.empty_model").into_owned())?;
    Ok((prepared, info))
}

fn upload(prepared: &PreparedMesh, info: ModelInfo, device: &wgpu::Device) -> Loaded {
    Loaded { mesh: Arc::new(prepared.upload(device)), info: Arc::new(info) }
}
