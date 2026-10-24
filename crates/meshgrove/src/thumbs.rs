//! Background thumbnail pipeline, built for libraries on spinning disks:
//!
//! ```text
//! queue ──► 2 cache-lookup threads (SSD) ──hit──► UI
//!   │            └─miss─► queue (stage Generate)
//!   └──► 1 disk reader (one file at a time, visible first) ──► 3 workers (parse / decode)
//!                                                              └─► 1 GPU thread (render) ──► cache + UI
//! ```
//! Visible tiles always jump the queue; opening another folder drops everything not started yet.
//! Background jobs (the rest of the library, for its index) only run when nothing else waits.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use meshgrove_core::cache::{ThumbCache, ThumbMeta, Thumbnail};
use meshgrove_core::thumbnail::ThumbError;
use meshgrove_core::{Kind, Mesh};
use meshgrove_render::{HeadlessRenderer, ViewParams};

pub const THUMB_PX: u32 = 384;
const LOOKUP_THREADS: usize = 2;
const WORKER_THREADS: usize = 3;

#[derive(Debug, Clone)]
pub struct Job {
    pub key: String,
    pub path: PathBuf,
    pub kind: Kind,
    /// Library indexing work: lowest priority, survives folder changes, no picture needed.
    pub background: bool,
}

pub enum ThumbResult {
    /// `image` is `None` for background jobs.
    Ready { job: Job, image: Option<egui::ColorImage>, meta: ThumbMeta },
    Failed {
        job: Job,
        error: String,
        /// The file's content hash when it was read but cannot be shown (a broken model), so
        /// the library can still note it.
        content_hash: Option<String>,
    },
}

pub use meshgrove_core::thumbnail::supported as has_thumbnail;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Lookup,
    Generate,
}

struct Queued {
    job: Job,
    stage: Stage,
    order: usize,
    visible: bool,
}

#[derive(Default)]
struct Queue {
    items: Vec<Queued>,
    shutdown: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
    generation: AtomicU64,
    done: AtomicUsize,
    total: AtomicUsize,
    started: Mutex<std::time::Instant>,
}

impl Shared {
    /// Blocks until a job of `stage` is available: visible tiles, then the rest of the folder,
    /// then background work.
    fn pop(&self, stage: Stage) -> Option<(Queued, u64)> {
        let mut q = self.queue.lock().unwrap();
        loop {
            if q.shutdown {
                return None;
            }
            let best = q
                .items
                .iter()
                .enumerate()
                .filter(|(_, it)| it.stage == stage)
                .min_by_key(|(_, it)| (it.job.background, !it.visible, it.order))
                .map(|(i, _)| i);
            if let Some(i) = best {
                return Some((q.items.swap_remove(i), self.generation.load(Ordering::SeqCst)));
            }
            q = self.wake.wait(q).unwrap();
        }
    }

    fn current(&self, job: &Job, generation: u64) -> bool {
        job.background || self.generation.load(Ordering::SeqCst) == generation
    }

    fn requeue(&self, mut item: Queued, generation: u64, stage: Stage) {
        let mut q = self.queue.lock().unwrap();
        if self.current(&item.job, generation) {
            item.stage = stage;
            q.items.push(item);
            self.wake.notify_all();
        }
    }

    fn finish(&self, job: &Job, generation: u64) {
        if !job.background && self.generation.load(Ordering::SeqCst) == generation {
            let done = self.done.fetch_add(1, Ordering::SeqCst) + 1;
            if done == self.total.load(Ordering::SeqCst) {
                let secs = self.started.lock().unwrap().elapsed().as_secs_f32();
                log::info!("all {done} thumbnails of this folder ready after {secs:.1} s");
            }
        }
    }
}

type RenderRequest = (Mesh, Sender<Result<image::RgbaImage, String>>);

pub struct ThumbService {
    shared: Arc<Shared>,
    results: Receiver<ThumbResult>,
}

impl ThumbService {
    pub fn new(ctx: egui::Context, cache: ThumbCache) -> ThumbService {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue::default()),
            wake: Condvar::new(),
            generation: AtomicU64::new(0),
            done: AtomicUsize::new(0),
            total: AtomicUsize::new(0),
            started: Mutex::new(std::time::Instant::now()),
        });
        let (result_tx, results) = unbounded();
        let send = move |r: ThumbResult| {
            let _ = result_tx.send(r);
            ctx.request_repaint();
        };

        for i in 0..LOOKUP_THREADS {
            let (shared, cache, send) = (shared.clone(), cache.clone(), send.clone());
            spawn(&format!("thumb-lookup-{i}"), move || {
                while let Some((item, generation)) = shared.pop(Stage::Lookup) {
                    match cache.load(&item.job.key) {
                        Some(Ok(thumb)) => {
                            shared.finish(&item.job, generation);
                            send(ready(item.job, thumb));
                        }
                        Some(Err(error)) => {
                            shared.finish(&item.job, generation);
                            send(ThumbResult::Failed { job: item.job, error, content_hash: None });
                        }
                        None => shared.requeue(item, generation, Stage::Generate),
                    }
                }
            });
        }

        // A small buffer keeps the disk busy without piling up hundreds of MB of model data.
        let (work_tx, work_rx) = bounded::<(Job, u64, std::io::Result<Vec<u8>>)>(2);
        {
            let shared = shared.clone();
            spawn("thumb-reader", move || {
                while let Some((item, generation)) = shared.pop(Stage::Generate) {
                    // poppler reads PDFs on disk itself (only the first page); everything else is read here.
                    let pdf_on_disk =
                        item.job.kind == Kind::Pdf && meshgrove_core::archive::split(&item.job.path).is_none();
                    let bytes =
                        if pdf_on_disk { Ok(Vec::new()) } else { meshgrove_core::archive::read(&item.job.path) };
                    if work_tx.send((item.job, generation, bytes)).is_err() {
                        break;
                    }
                }
            });
        }

        let (render_tx, render_rx) = unbounded::<RenderRequest>();
        spawn("thumb-gpu", move || gpu_thread(render_rx));

        for i in 0..WORKER_THREADS {
            let (shared, cache, send) = (shared.clone(), cache.clone(), send.clone());
            let (work_rx, render_tx) = (work_rx.clone(), render_tx.clone());
            spawn(&format!("thumb-worker-{i}"), move || {
                for (job, generation, bytes) in work_rx {
                    let result = match &bytes {
                        Ok(b) => generate(&job, b, &render_tx),
                        Err(e) => Err(ThumbError::Transient(e.to_string())),
                    };
                    match result {
                        Ok(thumb) => {
                            if let Err(e) = cache.store(&job.key, &thumb) {
                                log::warn!("thumbnail cache write failed: {e}");
                            }
                            shared.finish(&job, generation);
                            send(ready(job, thumb));
                        }
                        Err(failure) => {
                            let (error, content_hash) = match failure {
                                ThumbError::Permanent(e) => {
                                    let _ = cache.store_failure(&job.key, &e);
                                    let hash = bytes
                                        .as_ref()
                                        .ok()
                                        .filter(|b| !b.is_empty() && meshgrove_core::checks::checked(job.kind))
                                        .map(|b| meshgrove_core::dedupe::content_hash(b));
                                    (e, hash)
                                }
                                ThumbError::Transient(e) => (e, None),
                            };
                            log::info!("no thumbnail for {}: {error}", job.path.display());
                            shared.finish(&job, generation);
                            send(ThumbResult::Failed { job, error, content_hash });
                        }
                    }
                }
            });
        }

        ThumbService { shared, results }
    }

    /// Replaces the pending foreground work (e.g. after opening another folder).
    pub fn set_jobs(&self, jobs: Vec<Job>) {
        let mut q = self.shared.queue.lock().unwrap();
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        self.shared.done.store(0, Ordering::SeqCst);
        self.shared.total.store(jobs.len(), Ordering::SeqCst);
        *self.shared.started.lock().unwrap() = std::time::Instant::now();
        let keys: std::collections::HashSet<&str> = jobs.iter().map(|j| j.key.as_str()).collect();
        // The foreground job will produce the same result (and its facts) sooner.
        q.items.retain(|it| it.job.background && !keys.contains(it.job.key.as_str()));
        q.items.extend(jobs.into_iter().enumerate().map(|(order, mut job)| {
            job.background = false;
            Queued { job, stage: Stage::Lookup, order, visible: false }
        }));
        self.shared.wake.notify_all();
    }

    /// Replaces the background work (library files whose facts are missing).
    pub fn set_background(&self, jobs: Vec<Job>) {
        let mut q = self.shared.queue.lock().unwrap();
        q.items.retain(|it| !it.job.background);
        let foreground: std::collections::HashSet<String> = q.items.iter().map(|it| it.job.key.clone()).collect();
        q.items.extend(jobs.into_iter().filter(|j| !foreground.contains(&j.key)).enumerate().map(
            |(order, mut job)| {
                job.background = true;
                Queued { job, stage: Stage::Lookup, order, visible: false }
            },
        ));
        self.shared.wake.notify_all();
    }

    /// Marks which jobs are on screen right now so they are served first.
    pub fn set_visible(&self, is_visible: impl Fn(&str) -> bool) {
        let mut q = self.shared.queue.lock().unwrap();
        for item in &mut q.items {
            item.visible = is_visible(&item.job.key);
        }
    }

    /// Nothing waits in the queue (the last jobs may still be running).
    pub fn is_idle(&self) -> bool {
        self.shared.queue.lock().unwrap().items.is_empty()
    }

    pub fn try_recv(&self) -> Option<ThumbResult> {
        self.results.try_recv().ok()
    }

    /// (finished, total) for the current folder.
    pub fn progress(&self) -> (usize, usize) {
        (self.shared.done.load(Ordering::SeqCst), self.shared.total.load(Ordering::SeqCst))
    }
}

impl Drop for ThumbService {
    fn drop(&mut self) {
        self.shared.queue.lock().unwrap().shutdown = true;
        self.shared.wake.notify_all();
    }
}

fn generate(job: &Job, bytes: &[u8], render_tx: &Sender<RenderRequest>) -> Result<Thumbnail, ThumbError> {
    meshgrove_core::thumbnail::make(&job.path, job.kind, bytes, THUMB_PX, |mesh| render(mesh, render_tx))
}

/// Draws on the GPU thread and waits for the picture.
fn render(mesh: Mesh, render_tx: &Sender<RenderRequest>) -> Result<image::RgbaImage, String> {
    let (reply_tx, reply_rx) = bounded(1);
    render_tx.send((mesh, reply_tx)).map_err(|_| "renderer stopped".to_owned())?;
    reply_rx.recv().map_err(|_| "renderer stopped".to_owned())?
}

fn gpu_thread(requests: Receiver<RenderRequest>) {
    let gpu = HeadlessRenderer::new().map_err(|e| e.to_string());
    match &gpu {
        Ok(gpu) => log::info!("thumbnail GPU: {} ({:?})", gpu.adapter_info.name, gpu.adapter_info.backend),
        Err(e) => log::error!("no GPU for thumbnails: {e}"),
    }
    let view = ViewParams::default();
    for (mesh, reply) in requests {
        let result = match &gpu {
            Ok(gpu) => gpu.render(&mesh, THUMB_PX, &view).map_err(|e| e.to_string()),
            Err(e) => Err(e.clone()),
        };
        drop(mesh); // free large meshes before the next one arrives
        let _ = reply.send(result);
    }
}

fn ready(job: Job, thumb: Thumbnail) -> ThumbResult {
    let image = (!job.background).then(|| {
        let size = [thumb.image.width() as usize, thumb.image.height() as usize];
        egui::ColorImage::from_rgba_unmultiplied(size, thumb.image.as_raw())
    });
    ThumbResult::Ready { job, image, meta: thumb.meta }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    thread::Builder::new().name(name.into()).spawn(f).expect("spawning a thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared_with(items: &[(&str, Stage, bool)]) -> Shared {
        let items = items
            .iter()
            .enumerate()
            .map(|(order, &(key, stage, visible))| Queued {
                job: Job { key: key.into(), path: key.into(), kind: Kind::Stl, background: false },
                stage,
                order,
                visible,
            })
            .collect();
        Shared {
            queue: Mutex::new(Queue { items, shutdown: false }),
            wake: Condvar::new(),
            generation: AtomicU64::new(0),
            done: AtomicUsize::new(0),
            total: AtomicUsize::new(0),
            started: Mutex::new(std::time::Instant::now()),
        }
    }

    fn pop_keys(shared: &Shared, stage: Stage, n: usize) -> Vec<String> {
        (0..n).map(|_| shared.pop(stage).unwrap().0.job.key).collect()
    }

    #[test]
    fn visible_jobs_first_then_folder_order() {
        let shared = shared_with(&[
            ("a", Stage::Lookup, false),
            ("b", Stage::Lookup, false),
            ("c", Stage::Lookup, true),
            ("d", Stage::Generate, true),
            ("e", Stage::Lookup, true),
        ]);
        assert_eq!(pop_keys(&shared, Stage::Lookup, 4), ["c", "e", "a", "b"]);
        assert_eq!(pop_keys(&shared, Stage::Generate, 1), ["d"]);
    }

    #[test]
    fn background_work_waits_and_survives_folder_changes() {
        let shared = Arc::new(shared_with(&[]));
        let service = ThumbService { shared: shared.clone(), results: unbounded().1 };
        let job = |k: &str| Job { key: k.into(), path: k.into(), kind: Kind::Stl, background: false };
        service.set_background(vec![job("lib1"), job("lib2"), job("a")]);
        service.set_jobs(vec![job("a"), job("b")]);
        assert_eq!(
            pop_keys(&shared, Stage::Lookup, 4),
            ["a", "b", "lib1", "lib2"],
            "duplicate 'a' runs once, in front"
        );

        service.set_background(vec![job("lib3")]);
        service.set_jobs(vec![job("c")]);
        assert_eq!(pop_keys(&shared, Stage::Lookup, 2), ["c", "lib3"]);
        std::mem::forget(service); // Drop would shut the shared queue down
    }

    #[test]
    fn stale_requeue_is_dropped() {
        let shared = shared_with(&[("a", Stage::Lookup, false)]);
        let (item, generation) = shared.pop(Stage::Lookup).unwrap();
        shared.generation.fetch_add(1, Ordering::SeqCst); // user opened another folder meanwhile
        shared.requeue(item, generation, Stage::Generate);
        assert!(shared.queue.lock().unwrap().items.is_empty());
    }
}
