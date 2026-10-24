//! Notices suggestions that `meshgrove-mcp` (or the chat) left in the suggestion store: a
//! thread that looks every two seconds whether the store changed, and only then wakes the UI.
//! It runs only while AI is switched on.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use meshgrove_core::ai::proposals::ProposalStore;

const POLL: Duration = Duration::from_secs(2);

pub struct ProposalWatcher {
    pending: Arc<AtomicUsize>,
    /// Counts the store's changes, so views know to read it again.
    generation: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
}

impl ProposalWatcher {
    /// Also lets old undecided suggestions expire and trims the access log, once.
    pub fn start(path: PathBuf, ctx: egui::Context, (log_days, expire_days): (u16, u16)) -> ProposalWatcher {
        let pending = Arc::new(AtomicUsize::new(0));
        let generation = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (p, g, s) = (pending.clone(), generation.clone(), stop.clone());
        std::thread::Builder::new()
            .name("proposals".into())
            .spawn(move || {
                let mut store: Option<ProposalStore> = None;
                let mut seen = None;
                while !s.load(Ordering::Relaxed) {
                    // The store appears when an assistant first suggests something.
                    if store.is_none() && path.exists() {
                        store = ProposalStore::open(&path).map_err(|e| log::warn!("suggestion store: {e}")).ok();
                        if let Some(st) = &mut store
                            && let Err(e) = st.prune(SystemTime::now(), log_days, expire_days)
                        {
                            log::warn!("cleaning up the suggestion store: {e}");
                        }
                    }
                    if let Some(st) = &store {
                        let version = st.data_version().ok();
                        if version != seen {
                            seen = version;
                            p.store(st.pending_count().unwrap_or(0), Ordering::Relaxed);
                            g.fetch_add(1, Ordering::Relaxed);
                            ctx.request_repaint();
                        }
                    }
                    std::thread::sleep(POLL);
                }
            })
            .expect("spawning the proposals thread");
        ProposalWatcher { pending, generation, stop }
    }

    /// Batches waiting for the user.
    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::Relaxed)
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }
}

impl Drop for ProposalWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
