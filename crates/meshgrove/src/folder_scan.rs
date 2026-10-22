//! Looks into folders on disk (a few levels deep) for the models that get pictured on the folder's
//! tile. One background thread; moving to another folder drops what is still queued.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam_channel::{Receiver, Sender, unbounded};
use meshgrove_core::listing::{self, Entry};

struct Job {
    generation: u64,
    dir: PathBuf,
    depth: u8,
    limit: usize,
}

pub struct FolderScanner {
    jobs: Sender<Job>,
    results: Receiver<(u64, PathBuf, Vec<Entry>)>,
    generation: Arc<AtomicU64>,
    found: HashMap<PathBuf, Vec<Entry>>,
    requested: HashSet<PathBuf>,
    /// The folder whose subfolders are being pictured.
    folder: PathBuf,
}

impl FolderScanner {
    pub fn new(ctx: egui::Context) -> FolderScanner {
        let (jobs, job_rx) = unbounded::<Job>();
        let (result_tx, results) = unbounded();
        let generation = Arc::new(AtomicU64::new(0));
        let current = generation.clone();
        std::thread::Builder::new()
            .name("folder-scan".into())
            .spawn(move || {
                for job in job_rx {
                    if job.generation != current.load(Ordering::Relaxed) {
                        continue;
                    }
                    let entries = listing::scan_previews(&job.dir, job.depth, job.limit);
                    if job.generation == current.load(Ordering::Relaxed) {
                        let _ = result_tx.send((job.generation, job.dir, entries));
                        ctx.request_repaint();
                    }
                }
            })
            .expect("spawn folder scanner");
        FolderScanner {
            jobs,
            results,
            generation,
            found: HashMap::new(),
            requested: HashSet::new(),
            folder: PathBuf::new(),
        }
    }

    /// Forgets everything found so far (new folder, F5, changed settings).
    pub fn reset(&mut self, folder: &Path) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.found.clear();
        self.requested.clear();
        self.folder = folder.to_path_buf();
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// What was found for `dir`, or queues the search and returns `None`.
    pub fn get_or_request(&mut self, dir: &Path, depth: u8, limit: usize) -> Option<&Vec<Entry>> {
        if !self.found.contains_key(dir) && self.requested.insert(dir.to_path_buf()) {
            let generation = self.generation.load(Ordering::Relaxed);
            let _ = self.jobs.send(Job { generation, dir: dir.to_path_buf(), depth, limit });
        }
        self.found.get(dir)
    }

    /// Takes in finished searches; true if there were any.
    pub fn poll(&mut self) -> bool {
        let current = self.generation.load(Ordering::Relaxed);
        let mut any = false;
        while let Ok((generation, dir, entries)) = self.results.try_recv() {
            if generation == current {
                self.found.insert(dir, entries);
                any = true;
            }
        }
        any
    }
}
