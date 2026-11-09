//! Background jobs a panel starts (`Effect::StartJobs`): each in a fresh instance of the plugin,
//! several at once on a pool of threads, with progress, results and cancelling. They have no
//! deadline; the user cancels them.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;

use crossbeam_channel::Sender;
use meshgrove_plugin_api::ui::JobSpec;

use crate::Error;
use crate::library::Opened;
use crate::plugin::Plugin;
use crate::state::{CancelToken, Progress};

#[derive(Debug)]
pub enum JobEvent {
    Progress { job: String, fraction: f32, message: String },
    Finished { job: String, result: Result<serde_json::Value, Error> },
}

struct Job {
    plugin: Arc<Plugin>,
    spec: JobSpec,
    out: PathBuf,
    opened: Opened,
    cancel: CancelToken,
    events: Sender<JobEvent>,
}

pub struct JobPool {
    queue: Sender<Job>,
    _workers: Vec<JoinHandle<()>>,
}

impl JobPool {
    /// A pool of `workers` threads; dropping it lets running jobs finish.
    pub fn new(workers: usize) -> JobPool {
        let (queue, jobs) = crossbeam_channel::unbounded::<Job>();
        let workers = (0..workers.max(1))
            .map(|i| {
                let jobs = jobs.clone();
                std::thread::Builder::new()
                    .name(format!("plugin-job-{i}"))
                    .spawn(move || jobs.iter().for_each(run))
                    .expect("a thread for plugin jobs")
            })
            .collect();
        JobPool { queue, _workers: workers }
    }

    /// As many threads as the computer has cores, at most 8.
    pub fn with_cores() -> JobPool {
        JobPool::new(std::thread::available_parallelism().map_or(2, |n| n.get().min(8)))
    }

    /// Starts `jobs` of `plugin`, all writing into `out` and reading the library files in
    /// `opened` as the panel that starts them does; their progress and results arrive on
    /// `events`. Each job's cancel token, by its id.
    pub fn start(
        &self,
        plugin: &Arc<Plugin>,
        jobs: Vec<JobSpec>,
        out: &Path,
        opened: &Opened,
        events: &Sender<JobEvent>,
    ) -> Vec<(String, CancelToken)> {
        let mut tokens = Vec::new();
        for spec in jobs {
            let cancel = CancelToken::new();
            tokens.push((spec.id.clone(), cancel.clone()));
            let job = Job {
                plugin: plugin.clone(),
                spec,
                out: out.to_path_buf(),
                opened: opened.clone(),
                cancel,
                events: events.clone(),
            };
            if let Err(e) = self.queue.send(job) {
                let job = e.into_inner();
                let _ = job.events.send(JobEvent::Finished { job: job.spec.id, result: Err(Error::Cancelled) });
            }
        }
        tokens
    }
}

fn run(job: Job) {
    let result = if job.cancel.is_cancelled() {
        Err(Error::Cancelled)
    } else {
        let (events, id) = (job.events.clone(), job.spec.id.clone());
        let progress: Progress = Arc::new(move |fraction, message: &str| {
            let _ = events.send(JobEvent::Progress { job: id.clone(), fraction, message: message.to_owned() });
        });
        job.plugin.run_job_with(&job.spec.kind, &job.spec.params, &job.out, &job.opened, &job.cancel, Some(progress))
    };
    let _ = job.events.send(JobEvent::Finished { job: job.spec.id, result });
}
