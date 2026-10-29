//! Background library service: owns the index's write connection, scans the library roots at
//! start, watches them with inotify for changes, and records thumbnail facts. The UI talks to it
//! through commands and gets events back; it reads the index through its own connection.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use meshgrove_core::ai::proposals::{Decision, ProposalStore};
use meshgrove_core::cache::ThumbMeta;
use meshgrove_core::changes::{ApplyOp, ChangeReport, TargetState, UndoReport};
use meshgrove_core::checks::MeshChecks;
use meshgrove_core::config::Config;
use meshgrove_core::db::{self, Counts, Error, Library};
use meshgrove_core::lists::{ListKind, PrintResult};
use meshgrove_core::scan::{self, Force};
use meshgrove_core::userdata;
use meshgrove_core::{copies, import, placement};
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
    /// A rescan the user asked for is done: how many folders were listed, how many changed.
    Rescanned { dir: PathBuf, folders: usize, changed: usize },
    /// Files whose thumbnail facts are missing or stale, to process in the background.
    Pending(Vec<Job>),
    /// Tags, notes, lists or prints were changed (by the user or an import).
    UserDataChanged,
    /// Printability checks were stored.
    ChecksStored,
    /// Suggestions were accepted, or a change was undone or redone.
    Applied(Applied),
    /// Result of an export or import, for a message.
    Transfer(Result<Transfer, String>),
    /// Files were brought in from an import folder; `trash` are archives unpacked on the way,
    /// for the trash.
    Imported { title: String, report: ChangeReport, trash: Vec<PathBuf> },
    /// Files from the clipboard were copied into `dir`; `skipped` sources were gone or links.
    Copied { dir: PathBuf, report: ChangeReport, skipped: usize },
    /// Move suggestions by tag were made (or not): how many, how many files had none, and
    /// whether the user asked for them.
    Placed { proposed: usize, skipped: usize, explicit: bool },
}

/// What became of accepting suggestions, an undo or a redo.
pub enum Applied {
    Accepted { title: String, report: ChangeReport },
    Undone { title: String, report: UndoReport },
    Redone { title: String, report: ChangeReport },
    Failed(String),
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
    /// Lists this folder again, with `true` every folder below it as well (and reports the result).
    Rescan(PathBuf, bool),
    StoreMeta(PathBuf, String, ThumbMeta),
    /// Printability checks by content hash.
    StoreChecks(Vec<(String, MeshChecks)>),
    /// Takes these files out of the inbox; `None`: everything.
    InboxDone(Option<Vec<PathBuf>>),
    /// Applies what the user picked of a batch of suggestions and records the decisions.
    Accept {
        batch: i64,
        uid: String,
        title: String,
        ops: Vec<ApplyOp>,
        decisions: Vec<(i64, Decision)>,
    },
    /// Records decisions without changing the library (rejected suggestions).
    Decide {
        batch: i64,
        decisions: Vec<(i64, Decision)>,
    },
    Undo(i64),
    Redo(i64),
    /// Applies changes at once (the chat in automatic mode) and answers how it went.
    ApplyNow {
        origin: String,
        title: String,
        ops: Vec<ApplyOp>,
        reply: Sender<Result<ChangeReport, String>>,
    },
    Tag(Vec<PathBuf>, String),
    Untag(Vec<PathBuf>, String),
    SetNote(Vec<PathBuf>, String),
    List(ListOp),
    Export(PathBuf),
    Import(PathBuf),
    /// Answered once everything sent before is stored (and backed up).
    Flush(Sender<()>),
    /// The settings as they are now (the landing folder, the placement rules).
    Configure(Box<Config>),
    /// Brings files in from an import folder, as planned.
    ImportFiles {
        plan: import::Plan,
        title: String,
    },
    /// Copies files from the clipboard into a folder, as planned; `index`: the folder is in a
    /// library and is listed afterwards.
    CopyFiles {
        plan: copies::Plan,
        title: String,
        index: bool,
    },
    /// Suggests moving these files into the folders of their tags.
    PlaceByTag(Vec<PathBuf>),
}

/// Changes to favourites, lists and tag colours.
pub enum ListOp {
    Favorite(Vec<PathBuf>, bool),
    /// A new list (or the existing one of that name), with these files on it.
    Create(ListKind, String, Vec<PathBuf>),
    /// An automatic collection: name and saved search.
    CreateSmart(String, String),
    Rename(i64, String),
    Delete(i64),
    Add(i64, Vec<PathBuf>),
    Remove(i64, Vec<PathBuf>),
    TagColor(String, Option<u8>),
    /// Copies wanted on a print list.
    Quantity(i64, Vec<PathBuf>, u32),
    /// Logs a print; on a print list, printed also counts a copy as done.
    Print(Vec<PathBuf>, PrintResult, Option<i64>),
    /// Takes back the newest logged print of each file.
    UndoPrint(Vec<PathBuf>),
    Archive(i64, bool),
    /// Files went to the slicer: on top of the last prints, which keep this many.
    Sent(Vec<PathBuf>, usize),
    /// The last prints keep this many (none: the list goes).
    KeepRecent(usize),
}

impl ListOp {
    fn apply(self, lib: &mut Library) -> Result<(), Error> {
        match self {
            ListOp::Favorite(paths, on) => lib.set_favorite(&paths, on),
            ListOp::Create(kind, name, paths) => {
                let id = lib.create_list(kind, &name)?;
                lib.add_to_list(id, &paths).map(|_| ())
            }
            ListOp::CreateSmart(name, query) => lib.create_smart_list(&name, &query).map(|_| ()),
            ListOp::Rename(id, name) => lib.rename_list(id, &name).map(|_| ()),
            ListOp::Delete(id) => lib.delete_list(id),
            ListOp::Add(id, paths) => lib.add_to_list(id, &paths).map(|_| ()),
            ListOp::Remove(id, paths) => lib.remove_from_list(id, &paths),
            ListOp::TagColor(tag, color) => lib.set_tag_color(&tag, color),
            ListOp::Quantity(id, paths, quantity) => lib.set_quantity(id, &paths, quantity),
            ListOp::Print(paths, result, list) => lib.record_print(&paths, result, list),
            ListOp::UndoPrint(paths) => lib.undo_print(&paths),
            ListOp::Archive(id, archived) => lib.set_list_archived(id, archived),
            ListOp::Sent(paths, keep) => lib.record_sent(&paths, keep),
            ListOp::KeepRecent(keep) => lib.keep_recent(keep),
        }
    }
}

/// Applies changes through the library's one writer from another thread, and waits for it.
#[derive(Clone)]
pub struct Applier(Sender<Cmd>);

impl Applier {
    /// Copies files into a folder, as planned; answered with [`LibEvent::Copied`].
    pub fn copy_files(&self, plan: copies::Plan, title: String, index: bool) {
        let _ = self.0.send(Cmd::CopyFiles { plan, title, index });
    }

    pub fn apply(&self, origin: &str, title: &str, ops: Vec<ApplyOp>) -> Result<ChangeReport, String> {
        let (reply, answer) = crossbeam_channel::bounded(1);
        self.0
            .send(Cmd::ApplyNow { origin: origin.to_owned(), title: title.to_owned(), ops, reply })
            .map_err(|_| "the library is closed".to_owned())?;
        answer.recv_timeout(Duration::from_secs(60)).map_err(|_| "the library did not answer".to_owned())?
    }
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
                let worker = Worker { status: shared, events: event_tx, ctx, roots: Vec::new(), config: None };
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

    pub fn applier(&self) -> Applier {
        Applier(self.tx.clone())
    }

    pub fn accept(&self, batch: i64, uid: String, title: String, ops: Vec<ApplyOp>, decisions: Vec<(i64, Decision)>) {
        let _ = self.tx.send(Cmd::Accept { batch, uid, title, ops, decisions });
    }

    pub fn decide(&self, batch: i64, decisions: Vec<(i64, Decision)>) {
        let _ = self.tx.send(Cmd::Decide { batch, decisions });
    }

    /// Takes back a change of the journal.
    pub fn undo(&self, change: i64) {
        let _ = self.tx.send(Cmd::Undo(change));
    }

    pub fn redo(&self, change: i64) {
        let _ = self.tx.send(Cmd::Redo(change));
    }

    /// Marks files of the inbox as done (`None`: all of them).
    pub fn inbox_done(&self, paths: Option<Vec<PathBuf>>) {
        let _ = self.tx.send(Cmd::InboxDone(paths));
    }

    pub fn store_checks(&self, checks: Vec<(String, MeshChecks)>) {
        let _ = self.tx.send(Cmd::StoreChecks(checks));
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

    pub fn list(&self, op: ListOp) {
        let _ = self.tx.send(Cmd::List(op));
    }

    /// Writes all tags and notes to a JSON file.
    pub fn export(&self, path: PathBuf) {
        let _ = self.tx.send(Cmd::Export(path));
    }

    /// Merges tags and notes from an exported JSON file.
    pub fn import(&self, path: PathBuf) {
        let _ = self.tx.send(Cmd::Import(path));
    }

    /// Hands the service the settings (again): the import and placement rules live there.
    pub fn configure(&self, config: Config) {
        let _ = self.tx.send(Cmd::Configure(Box::new(config)));
    }

    /// Brings files in from an import folder; answered with [`LibEvent::Imported`].
    pub fn import_files(&self, plan: import::Plan, title: String) {
        let _ = self.tx.send(Cmd::ImportFiles { plan, title });
    }

    /// Suggests moving these library files into the folders of their tags ([`LibEvent::Placed`]).
    pub fn place_by_tag(&self, paths: Vec<PathBuf>) {
        let _ = self.tx.send(Cmd::PlaceByTag(paths));
    }

    /// Waits (at most `timeout`) until earlier commands are done — before the app quits.
    pub fn flush(&self, timeout: Duration) {
        let (done, wait) = crossbeam_channel::bounded(1);
        if self.tx.send(Cmd::Flush(done)).is_ok() {
            let _ = wait.recv_timeout(timeout);
        }
    }

    /// Lists `dir` again whatever its modification time says (only for folders in the library);
    /// with `whole_tree` also every folder below it, answered with [`LibEvent::Rescanned`].
    pub fn rescan(&self, dir: PathBuf, whole_tree: bool) {
        let _ = self.tx.send(Cmd::Rescan(dir, whole_tree));
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
    /// The settings, once the app handed them over.
    config: Option<Config>,
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
        // Accepted moves take the thumbnails along.
        if let Some(dir) = meshgrove_core::cache::ThumbCache::default_location() {
            lib.set_thumb_cache(meshgrove_core::cache::ThumbCache::new(dir));
        }
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res
                && is_change(&event.kind)
            {
                let _ = watch_tx.send(Cmd::Changed(event.paths));
            }
        })
        .map_err(|e| log::warn!("file watching unavailable: {e}"))
        .ok();

        let mut proposals = Proposals::default();
        proposals.reconcile(&lib);
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
                    self.scan(&mut lib, &starts, Force::Nothing);
                    backup(&lib);
                    next_full = Instant::now() + interval;
                }
                Ok(Cmd::Rescan(dir, whole_tree)) => {
                    let Some(root) = self.root_of(&dir) else { continue };
                    let force = if whole_tree { Force::Tree } else { Force::Start };
                    let (folders, changed) = self.scan(&mut lib, &[(root, dir.clone())], force);
                    if whole_tree {
                        self.send(LibEvent::Rescanned { dir, folders, changed });
                    }
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
                    match lib.tag(&paths, &tag) {
                        Ok(()) => self.place(&lib, &mut proposals, &paths, true),
                        Err(e) => log::warn!("tagging failed: {e}"),
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
                Ok(Cmd::List(op)) => {
                    if let Err(e) = op.apply(&mut lib) {
                        log::warn!("changing a list failed: {e}");
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
                Ok(Cmd::InboxDone(paths)) => {
                    let result = match &paths {
                        Some(paths) => lib.inbox_done(paths, "user").map(|done| done.len()),
                        None => lib.inbox_done_all("user"),
                    };
                    if let Err(e) = result {
                        log::warn!("inbox update failed: {e}");
                    }
                    self.send(LibEvent::UserDataChanged);
                }
                Ok(Cmd::Accept { batch, uid, title, ops, decisions }) => {
                    let result = lib.apply_change(&format!("proposal:{uid}"), &title, &ops);
                    let applied = match result {
                        Ok(report) => {
                            if let Some(store) = proposals.get()
                                && let Err(e) = store.decide(batch, &decisions)
                            {
                                log::warn!("recording the decisions failed: {e}");
                            }
                            self.place(&lib, &mut proposals, &tagged_paths(&lib, &ops), true);
                            Applied::Accepted { title, report }
                        }
                        Err(e) => Applied::Failed(e.to_string()),
                    };
                    self.send(LibEvent::Applied(applied));
                    self.send(LibEvent::UserDataChanged);
                }
                Ok(Cmd::ApplyNow { origin, title, ops, reply }) => {
                    let result = lib.apply_change(&origin, &title, &ops);
                    if result.is_ok() {
                        self.place(&lib, &mut proposals, &tagged_paths(&lib, &ops), true);
                    }
                    let _ = reply.send(result.map_err(|e| e.to_string()));
                    self.send(LibEvent::UserDataChanged);
                }
                Ok(Cmd::Configure(config)) => self.config = Some(*config),
                Ok(Cmd::PlaceByTag(paths)) => self.place(&lib, &mut proposals, &paths, false),
                Ok(Cmd::ImportFiles { plan, title }) => {
                    let trash = plan.trash.clone();
                    match lib.apply_change("import", &title, &import::to_ops(&plan)) {
                        Ok(report) => {
                            // The landing folder is listed now, whatever its time says: the files
                            // get their index rows (and, being new, land in the inbox).
                            if plan.landing.is_dir() {
                                self.scan(&mut lib, &[(plan.root.clone(), plan.landing.clone())], Force::Start);
                            }
                            self.send(LibEvent::Imported { title, report, trash });
                        }
                        Err(e) => self.send(LibEvent::Applied(Applied::Failed(e.to_string()))),
                    }
                    self.send(LibEvent::UserDataChanged);
                }
                Ok(Cmd::CopyFiles { plan, title, index }) => {
                    let skipped = plan.skipped.len();
                    match lib.apply_change("copy", &title, &copies::to_ops(&plan)) {
                        Ok(report) => {
                            // The folder is listed now, so the copies get their index rows.
                            if index && plan.dir.is_dir() {
                                self.scan(&mut lib, &[(plan.root.clone(), plan.dir.clone())], Force::Start);
                            }
                            self.send(LibEvent::Copied { dir: plan.dir, report, skipped });
                        }
                        Err(e) => self.send(LibEvent::Applied(Applied::Failed(e.to_string()))),
                    }
                    self.send(LibEvent::UserDataChanged);
                }
                Ok(Cmd::Decide { batch, decisions }) => {
                    if let Some(store) = proposals.get()
                        && let Err(e) = store.decide(batch, &decisions)
                    {
                        log::warn!("recording the decisions failed: {e}");
                    }
                }
                Ok(Cmd::Undo(id)) => {
                    let title = change_title(&lib, id);
                    let applied = match lib.undo_change(id) {
                        Ok(report) => {
                            // A batch taken back is no longer accepted.
                            if let Some((uid, store)) = proposal_uid(&lib, id).zip(proposals.get())
                                && let Ok(Some(batch)) = store.batch_by_uid(&uid)
                            {
                                let _ = store.mark_undone(batch);
                            }
                            Applied::Undone { title, report }
                        }
                        Err(e) => Applied::Failed(e.to_string()),
                    };
                    self.send(LibEvent::Applied(applied));
                    self.send(LibEvent::UserDataChanged);
                }
                Ok(Cmd::Redo(id)) => {
                    let title = change_title(&lib, id);
                    let uid = proposal_uid(&lib, id);
                    let applied = match lib.redo_change(id) {
                        Ok(report) => {
                            if let Some((uid, store)) = uid.zip(proposals.get())
                                && let Ok(Some(batch)) = store.batch_by_uid(&uid)
                            {
                                let _ = store.set_batch_state(batch, "accepted");
                            }
                            Applied::Redone { title, report }
                        }
                        Err(e) => Applied::Failed(e.to_string()),
                    };
                    self.send(LibEvent::Applied(applied));
                    self.send(LibEvent::UserDataChanged);
                }
                Ok(Cmd::StoreChecks(checks)) => {
                    for (hash, c) in &checks {
                        if let Err(e) = lib.store_checks(hash, c) {
                            log::warn!("storing a printability check failed: {e}");
                        }
                    }
                    self.send(LibEvent::ChecksStored);
                }
                Err(RecvTimeoutError::Timeout) if Instant::now() >= next_full => {
                    dirty.clear();
                    let starts: Vec<(PathBuf, PathBuf)> = self.roots.iter().map(|r| (r.clone(), r.clone())).collect();
                    self.scan(&mut lib, &starts, Force::Nothing);
                    backup(&lib);
                    next_full = Instant::now() + interval;
                }
                Err(RecvTimeoutError::Timeout) => {
                    let starts: Vec<(PathBuf, PathBuf)> = std::mem::take(&mut dirty)
                        .into_iter()
                        .filter_map(|dir| Some((self.root_of(&dir)?, dir)))
                        .collect();
                    self.scan(&mut lib, &starts, Force::Start);
                }
                Err(RecvTimeoutError::Disconnected) => {
                    backup(&lib);
                    return;
                }
            }
        }
    }

    /// The library root `dir` lies in (the innermost one).
    fn root_of(&self, dir: &Path) -> Option<PathBuf> {
        self.roots.iter().filter(|r| dir.starts_with(r)).max_by_key(|r| r.as_os_str().len()).cloned()
    }

    /// Scans each (root, start folder) pair and reports what changed. Returns how many folders
    /// were visited and how many of them had changed.
    fn scan(&self, lib: &mut Library, starts: &[(PathBuf, PathBuf)], force: Force) -> (usize, usize) {
        self.status.lock().unwrap().scanning = true;
        self.ctx.request_repaint();
        let mut changed = Vec::new();
        let mut visited = 0;
        let never = AtomicBool::new(false);
        for (root, start) in starts {
            // Until a library's first full scan is through, what it finds is not new (inbox).
            let baseline = lib.baseline_pending(root);
            lib.set_baseline_root(root, baseline);
            let result = scan::scan(lib, root, start, force, &never);
            lib.set_baseline_root(root, false);
            if baseline
                && start == root
                && result.is_ok()
                && let Err(e) = lib.finish_baseline(root)
            {
                log::warn!("could not note the first scan of {}: {e}", root.display());
            }
            match result {
                Ok(report) if report.changed_dirs.is_empty() => visited += report.dirs_visited,
                Ok(report) => {
                    visited += report.dirs_visited;
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
        // How old the index is, for readers in other processes (meshgrove-mcp).
        if let Err(e) = lib.meta_set("last_scan_ns", &db::nanos(Some(SystemTime::now())).to_string()) {
            log::warn!("could not note the scan time: {e}");
        }
        let pending = lib.needing_meta().unwrap_or_default();
        let jobs =
            pending.into_iter().map(|(e, key)| Job { key, path: e.path, kind: e.kind, background: true }).collect();
        let count = changed.len();
        let _ = self.events.send(LibEvent::Scanned { changed });
        let _ = self.events.send(LibEvent::Pending(jobs));
        self.status.lock().unwrap().scanning = false;
        self.refresh_counts(lib);
        (visited, count)
    }

    fn send(&self, event: LibEvent) {
        let _ = self.events.send(event);
        self.ctx.request_repaint();
    }

    /// Suggests moving `paths` into the folders of their tags (Settings → Tags): all of them when
    /// the user asked (`only_landing` false), else only those still in a landing folder, right
    /// after they were tagged.
    fn place(&self, lib: &Library, proposals: &mut Proposals, paths: &[PathBuf], only_landing: bool) {
        let Some(config) = &self.config else { return };
        if !config.placement.enabled {
            return;
        }
        let paths: Vec<PathBuf> = paths.iter().filter(|p| !only_landing || config.in_landing(p)).cloned().collect();
        if paths.is_empty() {
            return;
        }
        let (items, skipped) = match placement::proposals_for(lib, config, &paths) {
            Ok(found) => found,
            Err(e) => {
                log::warn!("placing by tag failed: {e}");
                return;
            }
        };
        let proposed = match (items.is_empty(), proposals.open_or_create()) {
            (false, Some(store)) => match placement::propose(store, &t!("placement.title"), items) {
                Ok(n) => n,
                Err(e) => {
                    log::warn!("suggesting the moves failed: {e}");
                    0
                }
            },
            _ => 0,
        };
        self.send(LibEvent::Placed { proposed, skipped: skipped.len(), explicit: !only_landing });
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

/// The suggestion store, opened once an assistant made one (most users never have one).
#[derive(Default)]
struct Proposals {
    store: Option<ProposalStore>,
}

impl Proposals {
    fn get(&mut self) -> Option<&mut ProposalStore> {
        if self.store.is_none() {
            let path = ProposalStore::default_location().filter(|p| p.exists());
            self.store =
                path.and_then(|p| ProposalStore::open(&p).map_err(|e| log::warn!("suggestion store: {e}")).ok());
        }
        self.store.as_mut()
    }

    /// The store, made when there is none yet (Meshgrove's own suggestions need one too).
    fn open_or_create(&mut self) -> Option<&mut ProposalStore> {
        if self.store.is_none() {
            let path = ProposalStore::default_location();
            self.store =
                path.and_then(|p| ProposalStore::open(&p).map_err(|e| log::warn!("suggestion store: {e}")).ok());
        }
        self.store.as_mut()
    }

    /// Batches still pending that the journal shows applied (Meshgrove stopped in between).
    fn reconcile(&mut self, lib: &Library) {
        let Some(store) = self.get() else { return };
        for b in store.batches(None, None, &["pending", "partly"], 1000).unwrap_or_default() {
            if lib.changes_by_origin(&format!("proposal:{}", b.uid)).is_ok_and(|c| !c.is_empty()) {
                let _ = store.set_batch_state(b.id, "accepted");
            }
        }
    }
}

/// The files the tag steps of `ops` were about, where they are now.
fn tagged_paths(lib: &Library, ops: &[ApplyOp]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for op in ops {
        if let ApplyOp::AddTag { target, .. } = op {
            match lib.resolve_target(target) {
                Ok(TargetState::Here) => out.push(target.path.clone()),
                Ok(TargetState::Moved(p)) => out.push(p),
                _ => {}
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn change_title(lib: &Library, id: i64) -> String {
    lib.changes(1000).unwrap_or_default().into_iter().find(|c| c.id == id).map(|c| c.title).unwrap_or_default()
}

/// The batch a journalled change came from, if it came from one.
fn proposal_uid(lib: &Library, id: i64) -> Option<String> {
    let change = lib.changes(1000).ok()?.into_iter().find(|c| c.id == id)?;
    change.origin.strip_prefix("proposal:").map(str::to_owned)
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
