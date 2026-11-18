//! Looking for a newer Meshgrove and putting it in place (docs/guide.md, "Updates").
//!
//! Nothing is sent until the user says yes once ([`Config::update_check`]): then one request to
//! GitHub at start and once a day, with this program's name and nothing about the user. A newer
//! release shows as a pill in the title bar; its window has the notes and, where Meshgrove can
//! replace itself safely, the one click that does it. Builds without a package marker (a plain
//! `cargo build`) never look. Modelled on SolidCraft's updater (Apache-2.0,
//! `crates/ui-egui/src/update.rs`).
//!
//! [`Config::update_check`]: meshgrove_core::config::Config::update_check

mod install;
mod kind;
mod net;
mod release;
mod state;
mod ui;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crossbeam_channel::{Receiver, bounded};

pub use kind::InstallKind;
use net::{FetchError, Net, Progress};
use release::Release;
use state::Memo;

/// `MESHGROVE_UPDATE_CHECK=0` turns every check off, the question included (managed machines).
const OFF: &str = "MESHGROVE_UPDATE_CHECK";
/// `MESHGROVE_UPDATE_FEED=<file>`: a local file read instead of GitHub's answer, for tests and
/// pictures; its packages may come from 127.0.0.1.
const FEED: &str = "MESHGROVE_UPDATE_FEED";

/// How this copy was installed, worked out once.
pub fn kind() -> &'static InstallKind {
    static KIND: OnceLock<InstallKind> = OnceLock::new();
    KIND.get_or_init(|| kind::Probe::with_process(|probe| kind::detect(kind::PACKAGE, probe)))
}

/// Whether this copy looks for updates at all: a release package or a build of `install.sh`,
/// not turned off, and not a test run of the hooks (unless it reads a feed).
pub fn available() -> bool {
    let off = std::env::var_os(OFF).is_some_and(|v| v == "0");
    let feed = std::env::var_os(FEED).is_some();
    let hooks = std::env::var_os("MESHGROVE_SCREENSHOT").is_some();
    kind().checks() && !off && (feed || !hooks)
}

/// The state of the last check, for the About page.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Idle,
    Checking,
    UpToDate,
    Failed(String),
}

/// What the About page shows of the update check.
#[derive(Debug, Clone)]
pub struct Info {
    pub available: bool,
    pub status: Status,
    /// Unix seconds of the last answer.
    pub last_check: Option<u64>,
    /// A newer version found.
    pub newer: Option<String>,
}

/// What the app has to do after the update windows.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The user answered the question once and for all (the settings keep it).
    Answered(bool),
    /// Close Meshgrove: the installer takes over, or the new version starts once Meshgrove has
    /// saved ([`Updater::relaunch_now`]).
    Quit,
}

/// Where an update is.
enum Job {
    Downloading {
        progress: Arc<Progress>,
        rx: Receiver<Result<install::Outcome, String>>,
    },
    /// In place: starting this program runs the new version.
    Ready(PathBuf),
    /// The installer runs: close now.
    Installer,
    /// The disk image is open (macOS).
    Opened,
    /// The installer ended before it ran (no administrator rights): nothing changed, and the
    /// update can be tried again.
    Declined,
    Failed(String),
}

pub struct Updater {
    available: bool,
    feed: Option<PathBuf>,
    memo: Memo,
    /// `None` with a feed: a test run remembers nothing.
    memo_path: Option<PathBuf>,
    status: Status,
    /// A newer release than this program.
    newer: Option<Release>,
    /// A check under way; `true` for "Check now".
    check: Option<(bool, Receiver<Result<Release, FetchError>>)>,
    last_manual: u64,
    /// "Check now" was clicked; the next frame asks.
    wanted: bool,
    job: Option<Job>,
    /// The update window is open.
    window: bool,
    /// The question is open.
    asking: bool,
    /// Started once Meshgrove has saved and closed.
    relaunch: Option<PathBuf>,
    notes: egui_commonmark::CommonMarkCache,
}

impl Updater {
    pub fn new() -> Updater {
        if let InstallKind::WindowsPortable(dir) = kind() {
            install::clean_up(dir);
        }
        let feed = std::env::var_os(FEED).map(PathBuf::from);
        let memo_path = if feed.is_some() { None } else { Memo::path() };
        let memo = memo_path.as_deref().map(Memo::load).unwrap_or_default();
        Updater {
            available: available(),
            feed,
            memo,
            memo_path,
            status: Status::Idle,
            newer: None,
            check: None,
            last_manual: 0,
            wanted: false,
            job: None,
            window: false,
            asking: false,
            relaunch: None,
            notes: Default::default(),
        }
    }

    pub fn info(&self) -> Info {
        Info {
            available: self.available,
            status: self.status.clone(),
            last_check: self.memo.last_check,
            newer: self.newer.as_ref().map(|r| r.version.to_string()),
        }
    }

    /// Every frame: collects what the threads found, and with `auto` (the user said yes) starts
    /// the check when it is due and asks for a frame when the next one will be.
    pub fn tick(&mut self, ctx: &egui::Context, auto: bool) {
        if !self.available {
            return;
        }
        self.poll_check();
        self.poll_job();
        if std::mem::take(&mut self.wanted) {
            self.check_now(ctx);
        }
        if auto && self.check.is_none() {
            let now = state::now();
            if self.memo.due(now) {
                self.start(ctx, false);
            } else {
                ctx.request_repaint_after(Duration::from_secs(self.memo.wait(now).clamp(1, state::HOUR)));
            }
        }
        if matches!(self.job, Some(Job::Downloading { .. })) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// "Check now" (the About page): the next frame asks.
    pub fn request_check(&mut self) {
        self.wanted = true;
    }

    /// "Check now": an explicit click, so it asks even with the automatic check off. Not while a
    /// check runs, not twice in half a minute, not while GitHub asked to wait.
    fn check_now(&mut self, ctx: &egui::Context) {
        let now = state::now();
        if !self.available || self.check.is_some() || now < self.last_manual + state::MANUAL_GAP {
            return;
        }
        if self.memo.rate_limited(now) {
            let wait = self.memo.rate_limited_until.unwrap_or(now).saturating_sub(now);
            self.status = Status::Failed(FetchError::RateLimited(wait).to_string());
            return;
        }
        self.last_manual = now;
        self.start(ctx, true);
    }

    /// Opens the window of the newer version (the About page's button).
    pub fn show(&mut self) {
        if self.newer.is_some() {
            self.window = true;
        }
    }

    fn start(&mut self, ctx: &egui::Context, manual: bool) {
        let (tx, rx) = bounded(1);
        let (feed, ctx) = (self.feed.clone(), ctx.clone());
        let spawned = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            let text = match &feed {
                Some(path) => {
                    std::fs::read_to_string(path).map_err(|e| FetchError::Failed(format!("{}: {e}", path.display())))
                }
                None => Net::new(false).text(net::LATEST, 1 << 20),
            };
            let _ = tx.send(text.and_then(|text| Release::parse(&text, feed.is_some()).map_err(FetchError::Failed)));
            ctx.request_repaint();
        });
        match spawned {
            Ok(_) => {
                self.check = Some((manual, rx));
                self.status = Status::Checking;
            }
            Err(e) => self.status = Status::Failed(e.to_string()),
        }
    }

    fn poll_check(&mut self) {
        let Some((manual, rx)) = &self.check else { return };
        let Ok(result) = rx.try_recv() else { return };
        let manual = *manual;
        self.check = None;
        let now = state::now();
        match result {
            Ok(latest) if release::is_newer(&latest.version, &release::current()) => {
                self.memo.answered(now);
                if manual {
                    self.memo.show_again();
                    self.window = true;
                }
                if self.newer.as_ref().is_none_or(|r| r.version != latest.version) {
                    self.job = None;
                }
                self.newer = Some(latest);
                self.status = Status::Idle;
            }
            Ok(_) => {
                self.memo.answered(now);
                self.newer = None;
                self.status = Status::UpToDate;
            }
            Err(FetchError::RateLimited(wait)) => {
                self.memo.limited(now, wait);
                self.status = Status::Failed(FetchError::RateLimited(wait).to_string());
            }
            Err(e) => {
                log::info!("update check: {e}");
                self.memo.failed(now);
                self.status = Status::Failed(e.to_string());
            }
        }
        self.save_memo();
    }

    fn poll_job(&mut self) {
        let Some(Job::Downloading { rx, .. }) = &self.job else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.job = match result {
            Ok(install::Outcome::Restart(program)) => Some(Job::Ready(program)),
            Ok(install::Outcome::Installer) => Some(Job::Installer),
            Ok(install::Outcome::Opened) => Some(Job::Opened),
            Err(e) if e == net::CANCELLED => None,
            Err(e) if e == install::DECLINED => Some(Job::Declined),
            Err(e) => {
                log::warn!("update: {e}");
                Some(Job::Failed(e))
            }
        };
    }

    /// Downloads and puts in place the newer version, in a thread of its own.
    fn install(&mut self, ctx: &egui::Context) {
        let Some(release) = self.newer.clone() else { return };
        let progress = Arc::new(Progress::default());
        let (tx, rx) = bounded(1);
        let (ctx, shared, local) = (ctx.clone(), progress.clone(), self.feed.is_some());
        let spawned = std::thread::Builder::new().name("update-install".into()).spawn(move || {
            let _ = tx.send(install::run(&Net::new(local), &release, kind(), &shared));
            ctx.request_repaint();
        });
        self.job = Some(match spawned {
            Ok(_) => Job::Downloading { progress, rx },
            Err(e) => Job::Failed(e.to_string()),
        });
    }

    fn save_memo(&self) {
        if let Some(path) = &self.memo_path {
            self.memo.save(path);
        }
    }

    /// After Meshgrove has saved and closed (`on_exit`): starts the new version if the user
    /// asked for the restart.
    pub fn relaunch_now(&mut self) {
        if let Some(program) = self.relaunch.take() {
            install::relaunch(&program);
        }
    }
}
