//! Background printability checks for library models the thumbnail pipeline did not check:
//! thumbnails that came from the cache (made before 0.5) and meshes too large to check while
//! their thumbnail was made. One file at a time, and only while the thumbnail pipeline has
//! nothing to do, so browsing stays quick on spinning disks.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};
use meshgrove_core::Kind;
use meshgrove_core::checks::{self, MeshChecks};

/// A file to check: its path, kind and content hash (what the result is stored under).
pub struct CheckJob {
    pub path: PathBuf,
    pub kind: Kind,
    pub hash: String,
}

pub struct Checker {
    jobs: Sender<Vec<CheckJob>>,
    results: Receiver<(String, MeshChecks)>,
    idle: Arc<AtomicBool>,
    /// Jobs handed over and not finished yet.
    pending: Arc<AtomicUsize>,
}

impl Checker {
    pub fn start(ctx: egui::Context) -> Checker {
        let (jobs, job_rx) = unbounded::<Vec<CheckJob>>();
        let (result_tx, results) = unbounded();
        let idle = Arc::new(AtomicBool::new(false));
        let pending = Arc::new(AtomicUsize::new(0));
        let (thread_idle, thread_pending) = (idle.clone(), pending.clone());
        std::thread::Builder::new()
            .name("mesh-checks".into())
            .spawn(move || {
                for batch in job_rx {
                    for job in batch {
                        // Wait for a quiet moment; a dropped app ends the thread through the channel.
                        while !thread_idle.load(Ordering::Relaxed) {
                            std::thread::sleep(Duration::from_millis(250));
                        }
                        let result = match meshgrove_core::archive::read(&job.path) {
                            Ok(bytes) => checks::check_file(&job.path, job.kind, &bytes),
                            Err(e) => {
                                log::info!("not checked, unreadable: {}: {e}", job.path.display());
                                None
                            }
                        };
                        thread_pending.fetch_sub(1, Ordering::SeqCst);
                        if let Some(c) = result {
                            if result_tx.send((job.hash, c)).is_err() {
                                return;
                            }
                            ctx.request_repaint();
                        }
                    }
                }
            })
            .expect("spawning the check thread");
        Checker { jobs, results, idle, pending }
    }

    /// Whether the checks may use the disk now (the thumbnail pipeline is idle).
    pub fn set_idle(&self, idle: bool) {
        self.idle.store(idle, Ordering::Relaxed);
    }

    pub fn queue(&self, jobs: Vec<CheckJob>) {
        self.pending.fetch_add(jobs.len(), Ordering::SeqCst);
        let _ = self.jobs.send(jobs);
    }

    /// Nothing queued or running.
    pub fn is_empty(&self) -> bool {
        self.pending.load(Ordering::SeqCst) == 0
    }

    pub fn try_recv(&self) -> Option<(String, MeshChecks)> {
        self.results.try_recv().ok()
    }
}

/// What a file's check is filed under: its path, size and time of change, so that a file saved
/// anew is checked anew.
type Shown = (PathBuf, u64, Option<std::time::SystemTime>);
type Asked = Arc<(std::sync::Mutex<Vec<(Shown, Kind)>>, std::sync::Condvar)>;

/// Requests beyond this wait no longer: the user has moved on to other models.
const MAX_ASKED: usize = 4;
/// Results kept; the oldest go first.
const MAX_KEPT: usize = 256;

/// The checks of the models that model windows show. A window may show a file the library does
/// not hold, or run without a library at all (a start with model files alone), so these are
/// worked out here and not looked up: on one thread, one file at a time, the newest request
/// first, as the models themselves load (`MeshStore`).
pub struct ShownChecks {
    /// `None`: the file could not be read.
    known: std::collections::HashMap<Shown, Option<MeshChecks>>,
    /// In the order they came in.
    kept: std::collections::VecDeque<Shown>,
    /// Queued or being checked.
    asked: std::collections::HashSet<Shown>,
    queue: Asked,
    results: Receiver<(Shown, Option<MeshChecks>)>,
}

impl ShownChecks {
    pub fn start(ctx: egui::Context) -> ShownChecks {
        let queue: Asked = Arc::default();
        let (tx, results) = unbounded();
        let waiting = queue.clone();
        std::thread::Builder::new()
            .name("shown-checks".into())
            .spawn(move || {
                loop {
                    let (key, kind) = {
                        let (lock, wake) = &*waiting;
                        let mut q = lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        loop {
                            if let Some(job) = q.pop() {
                                break job;
                            }
                            q = wake.wait(q).unwrap_or_else(std::sync::PoisonError::into_inner);
                        }
                    };
                    let began = std::time::Instant::now();
                    let result = match meshgrove_core::archive::read(&key.0) {
                        Ok(bytes) => checks::check_file(&key.0, kind, &bytes),
                        Err(e) => {
                            log::info!("not checked, unreadable: {}: {e}", key.0.display());
                            None
                        }
                    };
                    let name = meshgrove_core::listing::file_name(&key.0);
                    log::info!("checked {name} in {} ms", began.elapsed().as_millis());
                    if tx.send((key, result)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("spawning the thread of the shown models' checks");
        ShownChecks { known: Default::default(), kept: Default::default(), asked: Default::default(), queue, results }
    }

    /// The check of this model, and whether it is still to come (then it is on its way). A
    /// kind that is not checked has neither.
    pub fn of(&mut self, entry: &meshgrove_core::listing::Entry) -> (Option<&MeshChecks>, bool) {
        if !checks::checked(entry.kind) {
            return (None, false);
        }
        while let Ok((key, result)) = self.results.try_recv() {
            self.asked.remove(&key);
            if self.known.insert(key.clone(), result).is_none() {
                self.kept.push_back(key);
            }
            if self.kept.len() > MAX_KEPT
                && let Some(oldest) = self.kept.pop_front()
            {
                self.known.remove(&oldest);
            }
        }
        let key: Shown = (entry.path.clone(), entry.size, entry.modified);
        if !self.known.contains_key(&key) && self.asked.insert(key.clone()) {
            let (lock, wake) = &*self.queue;
            let mut q = lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            q.push((key.clone(), entry.kind));
            // Forgotten here as well, so that asking again queues it again.
            if q.len() > MAX_ASKED {
                self.asked.remove(&q.remove(0).0);
            }
            wake.notify_one();
        }
        match self.known.get(&key) {
            Some(result) => (result.as_ref(), false),
            None => (None, true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meshgrove_core::listing::Entry;

    /// Asks until the check is there.
    fn checked(shown: &mut ShownChecks, entry: &Entry) -> Option<MeshChecks> {
        let until = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            let (result, coming) = shown.of(entry);
            if !coming {
                return result.cloned();
            }
            assert!(std::time::Instant::now() < until, "no check of {}", entry.name);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn stl(dir: &std::path::Path, name: &str, triangles: &[[[f32; 3]; 3]]) -> Entry {
        let mut text = String::from("solid test\n");
        for t in triangles {
            text.push_str("facet normal 0 0 0\nouter loop\n");
            for [x, y, z] in t {
                text.push_str(&format!("vertex {x} {y} {z}\n"));
            }
            text.push_str("endloop\nendfacet\n");
        }
        text.push_str("endsolid test\n");
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        Entry::from_path(&path).unwrap()
    }

    /// A tetrahedron, 10 mm along each axis.
    fn tetrahedron() -> Vec<[[f32; 3]; 3]> {
        let [o, x, y, z] = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0], [0.0, 0.0, 10.0]];
        vec![[o, y, x], [o, x, z], [o, z, y], [x, y, z]]
    }

    #[test]
    fn a_shown_model_is_checked_in_the_background() {
        let dir = tempfile::tempdir().unwrap();
        let mut shown = ShownChecks::start(egui::Context::default());
        let closed = stl(dir.path(), "closed.stl", &tetrahedron());
        // Not there at once: it is on its way.
        assert_eq!(shown.of(&closed), (None, true));
        let checks = checked(&mut shown, &closed).expect("a check");
        assert!(checks.is_sound() && checks.shells == 1, "{checks:?}");
        assert!((checks.volume_mm3.unwrap() - 1000.0 / 6.0).abs() < 0.1, "{checks:?}");
        // Known from then on.
        assert!(matches!(shown.of(&closed), (Some(_), false)));

        // A face missing: three open edges.
        let open = stl(dir.path(), "open.stl", &tetrahedron()[..3]);
        let checks = checked(&mut shown, &open).expect("a check");
        assert_eq!(checks.open_edges, 3, "{checks:?}");
        assert_eq!(checks.volume_mm3, None);
    }

    #[test]
    fn a_changed_file_is_checked_anew_and_other_kinds_not_at_all() {
        let dir = tempfile::tempdir().unwrap();
        let mut shown = ShownChecks::start(egui::Context::default());
        let before = stl(dir.path(), "part.stl", &tetrahedron());
        assert!(checked(&mut shown, &before).unwrap().is_sound());
        let after = stl(dir.path(), "part.stl", &tetrahedron()[..3]);
        assert_ne!(before.size, after.size);
        assert_eq!(checked(&mut shown, &after).unwrap().open_edges, 3);

        // STEP tessellations are not checked: nothing is said, and nothing is to come.
        let step = dir.path().join("part.step");
        std::fs::write(&step, "ISO-10303-21;").unwrap();
        assert_eq!(shown.of(&Entry::from_path(&step).unwrap()), (None, false));
        // A file that is gone has no check, and none to come.
        let gone = Entry { path: dir.path().join("gone.stl"), ..before };
        assert_eq!(checked(&mut shown, &gone), None);
    }
}
