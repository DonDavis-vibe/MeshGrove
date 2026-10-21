//! Models on the GPU for the 3D viewer and the hover turntable. Loads happen on a background
//! thread (newest request first, one file at a time — easy on the HDD and on RAM), results are
//! shared by path and the least recently used ones are dropped once over the VRAM budget.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use crossbeam_channel::{Receiver, unbounded};
use stlb_core::Kind;
use stlb_core::loaders::{self, threemf};
use stlb_render::{GpuMesh, upload_mesh};

/// Kinds the viewer and turntable can show.
pub fn viewable(kind: Kind) -> bool {
    matches!(kind, Kind::Stl | Kind::ThreeMf | Kind::Step)
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

impl MeshStore {
    pub fn new(device: wgpu::Device, ctx: egui::Context) -> MeshStore {
        let pending: Pending = Arc::default();
        let (tx, results) = unbounded();
        let queue = pending.clone();
        std::thread::Builder::new()
            .name("mesh-loader".into())
            .spawn(move || {
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
        MeshStore { slots: HashMap::new(), pending, results, frame: 0 }
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
    if kind == Kind::Step {
        let mesh = loaders::step::load(path).map_err(|e| e.to_string())?;
        return finish(mesh, ModelInfo::plain(), device);
    }
    let bytes = stlb_core::archive::read(path).map_err(|e| e.to_string())?;
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
                triangles: 0,
                size_mm: glam::Vec3::ZERO,
                title: model.title,
                designer: model.designer,
                application: model.application,
                plates,
                plate: model.plate,
                plate_count: model.plates.len(),
            };
            (model.mesh, info)
        }
        _ => (loaders::load_mesh(kind, &bytes).map_err(|e| e.to_string())?, ModelInfo::plain()),
    };
    drop(bytes);
    finish(mesh, info, device)
}

fn finish(mesh: stlb_core::Mesh, mut info: ModelInfo, device: &wgpu::Device) -> Result<Loaded, String> {
    info.triangles = mesh.triangle_count();
    info.size_mm = mesh.bounds().map(|b| b.size()).unwrap_or_default();
    let gpu = upload_mesh(device, &mesh).ok_or_else(|| t!("error.empty_model").into_owned())?;
    Ok(Loaded { mesh: Arc::new(gpu), info: Arc::new(info) })
}
