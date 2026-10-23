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
