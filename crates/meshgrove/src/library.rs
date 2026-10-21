//! Background library service: owns the index's write connection, scans the library roots at
//! start, watches them with inotify for changes, and records thumbnail facts. The UI talks to it
//! through commands and gets events back; it reads the index through its own connection.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use meshgrove_core::cache::ThumbMeta;
use meshgrove_core::db::{Counts, Library};
use meshgrove_core::{scan, userdata};
use notify::{EventKind, RecursiveMode, Watcher};

use crate::thumbs::Job;

/// Changes are collected this long before rescanning, so copying a pack in triggers one scan.
const SETTLE: Duration = Duration::from_millis(1500);
/// Full (incremental, ~1 ms) rescans as a safety net: often when inotify is unavailable (its
/// per-user instance limit is easily exhausted on a busy desktop), rarely otherwise.
const RESCAN_WITHOUT_WATCHER: Duration = Duration::from_secs(60);
const RESCAN_WITH_WATCHER: Duration = Duration::from_secs(600);
/// Daily backups of tags and notes kept in `userdata::backup_dir()`.
const KEEP_BACKUPS: usize = 7;

pub enum LibEvent {
    /// A scan finished; the contents of these folders changed.
    Scanned { changed: Vec<PathBuf> },
    /// Files whose thumbnail facts are missing or stale, to process in the background.
    Pending(Vec<Job>),
    /// Tags or notes were changed (by the user or an import).
    UserDataChanged,
    /// Result of an export or import, for a message.
    Transfer(Result<Transfer, String>),
}

pub enum Transfer {
    /// Tags and notes of this many files were written to the file.
    Exported(PathBuf, usize),
    /// Entries read from an export and merged into the library.
    Imported(usize),
}

#[derive(Debug, Clone, Default)]
pub struct LibStatus {
    pub scanning: bool,
    pub counts: Counts,
    pub error: Option<String>,
}

enum Cmd {
    SetRoots(Vec<PathBuf>),
    Changed(Vec<PathBuf>),
    StoreMeta(PathBuf, String, ThumbMeta),
    Tag(Vec<PathBuf>, String),
    Untag(Vec<PathBuf>, String),
    SetNote(Vec<PathBuf>, String),
    Export(PathBuf),
    Import(PathBuf),
    /// Answered once everything sent before is stored (and backed up).
    Flush(Sender<()>),
}

pub struct LibraryService {
    tx: Sender<Cmd>,
    events: Receiver<LibEvent>,
    status: Arc<Mutex<LibStatus>>,
}

impl LibraryService {
    pub fn start(db_path: PathBuf, roots: Vec<PathBuf>, ctx: egui::Context) -> LibraryService {
        let (tx, rx) = unbounded();
        let (event_tx, events) = unbounded();
        let status = Arc::new(Mutex::new(LibStatus::default()));
        let watch_tx = tx.clone();
        let shared = status.clone();
        std::thread::Builder::new()
            .name("library".into())
            .spawn(move || {
                let worker = Worker { status: shared, events: event_tx, ctx, roots: Vec::new() };
                worker.run(&db_path, rx, watch_tx);
            })
            .expect("spawning library thread");
        let service = LibraryService { tx, events, status };
        service.set_roots(roots);
        service
    }

    pub fn set_roots(&self, roots: Vec<PathBuf>) {
        let _ = self.tx.send(Cmd::SetRoots(roots));
    }

    pub fn store_meta(&self, path: PathBuf, key: String, meta: ThumbMeta) {
        let _ = self.tx.send(Cmd::StoreMeta(path, key, meta));
    }

    pub fn tag(&self, paths: Vec<PathBuf>, tag: String) {
        let _ = self.tx.send(Cmd::Tag(paths, tag));
    }

    pub fn untag(&self, paths: Vec<PathBuf>, tag: String) {
        let _ = self.tx.send(Cmd::Untag(paths, tag));
    }

    pub fn set_note(&self, paths: Vec<PathBuf>, text: String) {
        let _ = self.tx.send(Cmd::SetNote(paths, text));
    }

    /// Writes all tags and notes to a JSON file.
    pub fn export(&self, path: PathBuf) {
        let _ = self.tx.send(Cmd::Export(path));
    }

    /// Merges tags and notes from an exported JSON file.
    pub fn import(&self, path: PathBuf) {
        let _ = self.tx.send(Cmd::Import(path));
    }

    /// Waits (at most `timeout`) until earlier commands are done — before the app quits.
    pub fn flush(&self, timeout: Duration) {
        let (done, wait) = crossbeam_channel::bounded(1);
        if self.tx.send(Cmd::Flush(done)).is_ok() {
            let _ = wait.recv_timeout(timeout);
        }
    }

    /// Files were changed by the app itself (e.g. moved to the trash): rescan their folders.
    pub fn changed(&self, paths: Vec<PathBuf>) {
        let _ = self.tx.send(Cmd::Changed(paths));
    }

    pub fn try_event(&self) -> Option<LibEvent> {
        self.events.try_recv().ok()
    }

    pub fn status(&self) -> LibStatus {
        self.status.lock().unwrap().clone()
    }
}

struct Worker {
    status: Arc<Mutex<LibStatus>>,
    events: Sender<LibEvent>,
    ctx: egui::Context,
    roots: Vec<PathBuf>,
}

impl Worker {
    fn run(mut self, db_path: &Path, rx: Receiver<Cmd>, watch_tx: Sender<Cmd>) {
        let mut lib = match Library::open(db_path) {
            Ok(lib) => lib,
            Err(e) => {
                self.fail(format!("{}: {e}", db_path.display()));
                return;
            }
        };
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res
                && is_change(&event.kind)
            {
                let _ = watch_tx.send(Cmd::Changed(event.paths));
            }
        })
        .map_err(|e| log::warn!("file watching unavailable: {e}"))
        .ok();

        let interval = if watcher.is_some() { RESCAN_WITH_WATCHER } else { RESCAN_WITHOUT_WATCHER };
        let mut next_full = Instant::now() + interval;
        let mut dirty: BTreeSet<PathBuf> = BTreeSet::new();
        loop {
            let now = Instant::now();
            let deadline = if dirty.is_empty() { next_full } else { (now + SETTLE).min(next_full) };
            let cmd = rx.recv_timeout(deadline.saturating_duration_since(now));
            match cmd {
                Ok(Cmd::SetRoots(roots)) => {
                    if let Some(w) = watcher.as_mut() {
                        for old in &self.roots {
                            let _ = w.unwatch(old);
                        }
                        for root in &roots {
                            if let Err(e) = w.watch(root, RecursiveMode::Recursive) {
                                log::warn!("cannot watch {}: {e}", root.display());
                            }
                        }
                    }
                    self.roots = roots;
                    if let Err(e) = lib.retain_roots(&self.roots) {
                        log::warn!("index cleanup failed: {e}");
                    }
                    let starts: Vec<(PathBuf, PathBuf)> = self.roots.iter().map(|r| (r.clone(), r.clone())).collect();
                    self.scan(&mut lib, &starts, false);
                    backup(&lib);
                    next_full = Instant::now() + interval;
                }
                Ok(Cmd::Changed(paths)) => {
                    for path in paths {
                        // A changed file means its folder must be listed again; a changed folder itself too.
                        let dir =
                            if path.is_dir() { path } else { path.parent().map(Path::to_path_buf).unwrap_or(path) };
                        dirty.insert(dir);
                    }
                }
                Ok(Cmd::Tag(paths, tag)) => {
                    if let Err(e) = lib.tag(&paths, &tag) {
                        log::warn!("tagging failed: {e}");
                    }
                    self.send(LibEvent::UserDataChanged);
                }
                Ok(Cmd::Untag(paths, tag)) => {
                    if let Err(e) = lib.untag(&paths, &tag) {
                        log::warn!("removing tag failed: {e}");
                    }
                    self.send(LibEvent::UserDataChanged);
                }
                Ok(Cmd::SetNote(paths, text)) => {
                    if let Err(e) = lib.set_note(&paths, &text) {
                        log::warn!("saving note failed: {e}");
                    }
                    self.send(LibEvent::UserDataChanged);
                }
                Ok(Cmd::Export(path)) => {
                    let result = lib.export_user_data().map_err(|e| e.to_string()).and_then(|data| {
                        userdata::write(&data, &path).map_err(|e| e.to_string())?;
                        Ok(Transfer::Exported(path, data.entries.len()))
                    });
                    self.send(LibEvent::Transfer(result));
                }
                Ok(Cmd::Import(path)) => {
                    let result = userdata::read(&path).map_err(|e| e.to_string()).and_then(|data| {
                        lib.import_user_data(&data).map(Transfer::Imported).map_err(|e| e.to_string())
                    });
                    self.send(LibEvent::Transfer(result));
                    self.send(LibEvent::UserDataChanged);
                }
                Ok(Cmd::Flush(done)) => {
                    backup(&lib);
                    let _ = done.send(());
                }
                Ok(Cmd::StoreMeta(path, key, meta)) => {
                    if let Err(e) = lib.store_meta(&path, &key, &meta) {
                        log::warn!("storing facts for {} failed: {e}", path.display());
                    }
                    self.refresh_counts(&lib);
                }
                Err(RecvTimeoutError::Timeout) if Instant::now() >= next_full => {
                    dirty.clear();
                    let starts: Vec<(PathBuf, PathBuf)> = self.roots.iter().map(|r| (r.clone(), r.clone())).collect();
                    self.scan(&mut lib, &starts, false);
                    backup(&lib);
                    next_full = Instant::now() + interval;
                }
                Err(RecvTimeoutError::Timeout) => {
                    let starts: Vec<(PathBuf, PathBuf)> = std::mem::take(&mut dirty)
                        .into_iter()
                        .filter_map(|dir| {
                            let root =
                                self.roots.iter().filter(|r| dir.starts_with(r)).max_by_key(|r| r.as_os_str().len())?;
                            Some((root.clone(), dir))
                        })
                        .collect();
                    self.scan(&mut lib, &starts, true);
                }
                Err(RecvTimeoutError::Disconnected) => {
                    backup(&lib);
                    return;
                }
            }
        }
    }

    /// Scans each (root, start folder) pair and reports what changed.
    fn scan(&self, lib: &mut Library, starts: &[(PathBuf, PathBuf)], force: bool) {
        self.status.lock().unwrap().scanning = true;
        self.ctx.request_repaint();
        let mut changed = Vec::new();
        let never = AtomicBool::new(false);
        for (root, start) in starts {
            match scan::scan(lib, root, start, force, &never) {
                Ok(report) if report.changed_dirs.is_empty() => {}
                Ok(report) => {
                    log::info!(
                        "scanned {}: {} folders, {} changed",
                        start.display(),
                        report.dirs_visited,
                        report.changed_dirs.len()
                    );
                    changed.extend(report.changed_dirs);
                }
                Err(e) => log::warn!("scan of {} failed: {e}", start.display()),
            }
        }
        let pending = lib.needing_meta().unwrap_or_default();
        let jobs =
            pending.into_iter().map(|(e, key)| Job { key, path: e.path, kind: e.kind, background: true }).collect();
        let _ = self.events.send(LibEvent::Scanned { changed });
        let _ = self.events.send(LibEvent::Pending(jobs));
        self.status.lock().unwrap().scanning = false;
        self.refresh_counts(lib);
    }

    fn send(&self, event: LibEvent) {
        let _ = self.events.send(event);
        self.ctx.request_repaint();
    }

    fn refresh_counts(&self, lib: &Library) {
        if let Ok(counts) = lib.counts() {
            self.status.lock().unwrap().counts = counts;
        }
        self.ctx.request_repaint();
    }

    fn fail(&mut self, error: String) {
        log::error!("library index unavailable: {error}");
        self.status.lock().unwrap().error = Some(error);
        self.ctx.request_repaint();
    }
}

/// Today's backup of tags and notes (only written when they changed since the last one).
fn backup(lib: &Library) {
    let Some(dir) = userdata::backup_dir() else { return };
    let today = userdata::date(std::time::SystemTime::now());
    let result = lib
        .export_user_data()
        .map_err(|e| e.to_string())
        .and_then(|data| userdata::rolling_backup(&data, &dir, &today, KEEP_BACKUPS).map_err(|e| e.to_string()));
    match result {
        Ok(Some(path)) => log::info!("backed up tags and notes to {}", path.display()),
        Ok(None) => {}
        Err(e) => log::warn!("backup of tags and notes failed: {e}"),
    }
}

/// Reads and attribute changes (atime, chmod) are not content changes — and reading files for
/// thumbnails must not trigger rescans.
fn is_change(kind: &EventKind) -> bool {
    use notify::event::ModifyKind;
    match kind {
        EventKind::Create(_) | EventKind::Remove(_) => true,
        EventKind::Modify(ModifyKind::Metadata(_)) => false,
        EventKind::Modify(_) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, MetadataKind, ModifyKind, RenameMode};

    #[test]
    fn only_content_changes_trigger_rescans() {
        assert!(is_change(&EventKind::Create(CreateKind::File)));
        assert!(is_change(&EventKind::Modify(ModifyKind::Name(RenameMode::Both))));
        assert!(!is_change(&EventKind::Access(AccessKind::Read)));
        assert!(!is_change(&EventKind::Modify(ModifyKind::Metadata(MetadataKind::AccessTime))));
    }
}
