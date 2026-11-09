//! Jobs a tool hands over (`Bundle::job`): a tool that would take longer than its deadline
//! answers at once and names a job; the host runs it in the background, one at a time, as the
//! AI that called the tool, and when it is done puts its files into the review. Shared by the
//! window and `meshgrove-mcp`, and kept across reloads of the plugins.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use meshgrove_core::config::AccessClass;

use crate::jobs::JobPool;
use crate::plugin::{Plugin, StagedBundle};
use crate::staging::new_uid;
use crate::state::{CancelToken, Progress};

/// Beside a staging folder (`<uid>/.job`, next to `out`) while a job writes into it: cleaning
/// up leaves the folder alone while the marker is fresh ([`crate::staging::sweep`]).
pub const MARKER: &str = ".job";
/// How long a marker counts as fresh; a running job touches it now and then.
pub const MARKER_FRESH: Duration = Duration::from_secs(24 * 3600);
/// How often a running job touches its marker, at most.
const TOUCH_EVERY: Duration = Duration::from_secs(60);

/// What becomes of a job's files once it is done: resolved and proposed for the review. The
/// title of the bundle, or why it did not work; on an error it removes the staging folder.
pub type Deliver = Box<dyn FnOnce(StagedBundle) -> Result<String, String> + Send>;

/// A job under way, as the window shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct JobStatus {
    pub id: String,
    pub plugin: String,
    /// The plugin's name as Meshgrove shows it.
    pub plugin_name: String,
    pub tool: String,
    pub fraction: f32,
    pub message: String,
}

/// A job that is done: its bundle's title when it waits in the review, or why not.
#[derive(Debug, Clone, PartialEq)]
pub struct Finished {
    pub id: String,
    pub plugin: String,
    pub plugin_name: String,
    pub outcome: Result<String, String>,
}

struct Running {
    status: JobStatus,
    cancel: CancelToken,
}

#[derive(Default)]
struct State {
    running: Vec<Running>,
    finished: Vec<Finished>,
}

/// The tool-started jobs of a process: one worker, so they do not compete for memory.
#[derive(Default)]
pub struct DeferredJobs {
    pool: OnceLock<JobPool>,
    state: Mutex<State>,
    idle: Condvar,
}

impl DeferredJobs {
    pub fn new() -> Arc<DeferredJobs> {
        Arc::new(DeferredJobs::default())
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Starts the job of `staged` (tool `tool` of `plugin` called by an AI of access class
    /// `class`); `deliver` takes its files when it is done. The job's id.
    pub fn start(
        self: &Arc<Self>,
        plugin: Arc<Plugin>,
        tool: &str,
        staged: StagedBundle,
        class: AccessClass,
        deliver: Deliver,
    ) -> String {
        let id = new_uid();
        let marker = staged.staging.out().parent().map(|dir| dir.join(MARKER));
        if let Some(marker) = &marker
            && let Err(e) = std::fs::write(marker, id.as_bytes())
        {
            log::warn!("marking the staging folder of job {id}: {e}");
        }
        let cancel = CancelToken::new();
        let status = JobStatus {
            id: id.clone(),
            plugin: plugin.id().to_owned(),
            plugin_name: plugin.manifest().name.clone(),
            tool: tool.to_owned(),
            fraction: 0.0,
            message: String::new(),
        };
        self.state().running.push(Running { status: status.clone(), cancel: cancel.clone() });
        let me = self.clone();
        let job = id.clone();
        let task = move || {
            let touched = Mutex::new(Instant::now());
            let (reporter, job_id, marker_at) = (me.clone(), job.clone(), marker.clone());
            let progress: Progress = Arc::new(move |fraction, message: &str| {
                reporter.progress(&job_id, fraction, message);
                let mut last = touched.lock().unwrap_or_else(|e| e.into_inner());
                if last.elapsed() >= TOUCH_EVERY {
                    *last = Instant::now();
                    if let Some(marker) = &marker_at {
                        let now = std::time::SystemTime::now();
                        let _ = std::fs::File::options().write(true).open(marker).and_then(|f| f.set_modified(now));
                    }
                }
            });
            let outcome = match plugin.finish_job(staged, class, &cancel, Some(progress)) {
                Ok(staged) => {
                    if let Some(marker) = &marker {
                        let _ = std::fs::remove_file(marker);
                    }
                    deliver(staged)
                }
                Err(e) => Err(e.to_string()),
            };
            me.finish(&job, outcome);
        };
        let pool = self.pool.get_or_init(|| JobPool::new(1));
        if !pool.spawn(task) {
            self.finish(&id, Err("the jobs of this process have stopped".into()));
        }
        id
    }

    fn progress(&self, id: &str, fraction: f32, message: &str) {
        if let Some(r) = self.state().running.iter_mut().find(|r| r.status.id == id) {
            r.status.fraction = fraction;
            r.status.message = message.to_owned();
        }
    }

    fn finish(&self, id: &str, outcome: Result<String, String>) {
        let mut state = self.state();
        if let Some(i) = state.running.iter().position(|r| r.status.id == id) {
            let status = state.running.remove(i).status;
            if let Err(e) = &outcome {
                log::warn!("job {id} of {}: {e}", status.plugin);
            }
            state.finished.push(Finished {
                id: id.to_owned(),
                plugin: status.plugin,
                plugin_name: status.plugin_name,
                outcome,
            });
        }
        self.idle.notify_all();
    }

    /// The jobs under way, oldest first.
    pub fn running(&self) -> Vec<JobStatus> {
        self.state().running.iter().map(|r| r.status.clone()).collect()
    }

    /// Cancels job `id`; whether it was running.
    pub fn cancel(&self, id: &str) -> bool {
        let state = self.state();
        let job = state.running.iter().find(|r| r.status.id == id);
        job.inspect(|r| r.cancel.cancel()).is_some()
    }

    /// The jobs done since the last look.
    pub fn take_finished(&self) -> Vec<Finished> {
        std::mem::take(&mut self.state().finished)
    }

    /// Waits until no job runs, at most `timeout`; whether none does.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let until = Instant::now() + timeout;
        let mut state = self.state();
        while !state.running.is_empty() {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            state = self.idle.wait_timeout(state, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        true
    }
}

/// Whether the staging folder `dir` (`<uid>`, holding `out`) has a fresh marker: a job writes
/// into it.
pub fn running_in(dir: &Path) -> bool {
    let marker: PathBuf = dir.join(MARKER);
    std::fs::metadata(marker).and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|e| e < MARKER_FRESH))
}
