//! The import view: looking at the import folders (a downloads folder) for new 3D-print files,
//! showing them, and bringing the picked ones into the library's landing folder.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crossbeam_channel::{Receiver, bounded};
use meshgrove_core::Kind;
use meshgrove_core::cache::ThumbCache;
use meshgrove_core::config::ImportAtStart;
use meshgrove_core::db::Library;
use meshgrove_core::import::{self, Plan, PlanItem, Scan};
use meshgrove_core::listing;

use super::inbox::{Batch, Source};
use super::{App, View};
use crate::ui::import::{ImportAction, ImportGroupInfo, ImportItemInfo};

/// What the app keeps about the import folders between frames.
#[derive(Default)]
pub(super) struct ImportState {
    /// What the import folders held when last looked at.
    pub scans: Vec<Scan>,
    rx: Option<Receiver<Vec<Scan>>>,
    /// The first look of this launch was acted on (the toast, or the automatic import).
    announced: bool,
    /// Candidates the user unticked, kept across looks.
    unpicked: HashSet<PathBuf>,
    /// An import whose files the AI sorts in once it is done.
    classify: Option<PendingClassify>,
}

/// What to hand the AI once an import is done.
#[derive(Debug, Clone, PartialEq)]
struct PendingClassify {
    /// The import's title in the history, which its event carries.
    title: String,
    drops: Vec<Landed>,
}

/// Where files of an import went that belong together.
#[derive(Debug, Clone, PartialEq)]
struct Landed {
    title: String,
    /// A folder or an archive: everything in it, found in the index once it is there.
    folder: Option<PathBuf>,
    /// Else the loose files.
    files: Vec<PathBuf>,
}

/// The drops of `plan`: each folder and archive on its own, the loose files (from the import
/// folders named `loose`) together.
fn landed(plan: &Plan, loose: &str) -> Vec<Landed> {
    let mut drops = Vec::new();
    let mut files = Vec::new();
    for item in &plan.items {
        let (dest, whole) = match item {
            // By its name: the archive is not there yet.
            PlanItem::Move { to, folder, .. } => {
                (to, *folder || Kind::from_file_name(&listing::file_name(to)) == Kind::Archive)
            }
            PlanItem::Extract { into, .. } => (into, true),
        };
        if whole {
            drops.push(Landed { title: listing::file_name(dest), folder: Some(dest.clone()), files: Vec::new() });
        } else {
            files.push(dest.clone());
        }
    }
    if !files.is_empty() {
        drops.insert(0, Landed { title: loose.to_owned(), folder: None, files });
    }
    drops
}

impl ImportState {
    pub fn scanning(&self) -> bool {
        self.rx.is_some()
    }

    /// Files worth importing, over all folders.
    pub fn importable(&self) -> usize {
        self.scans.iter().flat_map(|s| &s.candidates).filter(|c| c.importable()).count()
    }
}

impl App {
    /// Looks at the import folders on a thread; the index and the thumbnail cache say what the
    /// library has already.
    pub(super) fn start_import_scan(&mut self) {
        let folders = self.config.import.folders.clone();
        let Some(landing) = self.config.landing_dir() else {
            self.import.scans.clear();
            return;
        };
        if folders.is_empty() || self.import.rx.is_some() {
            return;
        }
        let db = self.db_path.clone();
        let (tx, rx) = bounded(1);
        self.import.rx = Some(rx);
        let ctx = self.egui_ctx.clone();
        std::thread::Builder::new()
            .name("import-scan".into())
            .spawn(move || {
                let lib = Library::open_reader(&db);
                let cache = ThumbCache::default_location().map(ThumbCache::new);
                let now = SystemTime::now();
                let scans = folders
                    .iter()
                    .filter_map(|f| import::scan(f, &landing, lib.as_ref(), cache.as_ref(), now).ok())
                    .collect();
                let _ = tx.send(scans);
                ctx.request_repaint();
            })
            .expect("spawning import scan");
    }

    /// Takes a finished look: says what is new the first time, or imports it right away.
    pub(super) fn poll_import_scan(&mut self) {
        let Some(rx) = &self.import.rx else { return };
        let Ok(scans) = rx.try_recv() else { return };
        self.import.rx = None;
        self.import.scans = scans;
        let count = self.import.importable();
        if count > 0 && !std::mem::replace(&mut self.import.announced, true) {
            match self.config.import.at_start {
                ImportAtStart::Auto => self.import_now(false, false),
                _ => self.toast(t!("import.toast_new", count = count).into_owned()),
            }
        }
        if matches!(self.view, View::Import { .. }) {
            self.show_import();
        }
    }

    pub(super) fn show_import(&mut self) {
        let (mut entries, mut groups) = (Vec::new(), Vec::new());
        for scan in &self.import.scans {
            let start = entries.len();
            let mut items = Vec::new();
            for c in &scan.candidates {
                entries.push(c.entry.clone());
                items.push(ImportItemInfo {
                    picked: c.importable() && !self.import.unpicked.contains(&c.entry.path),
                    in_library: c.in_library.clone(),
                    busy: c.busy,
                    name_taken: c.name_taken,
                    archive: c.archive,
                    folder: c.folder,
                });
            }
            groups.push(ImportGroupInfo {
                range: start..entries.len(),
                folder: scan.folder.clone(),
                title: listing::file_name(&scan.folder),
                items,
            });
        }
        self.leave_search();
        self.flatten = false;
        self.view = View::Import { entries, groups };
        self.refresh_gallery(true);
    }

    /// Where imported files land, as shown ("3D_druck/Import").
    pub(super) fn import_target(&self) -> String {
        match self.config.import_root() {
            Some(root) => format!("{}/{}", listing::file_name(root), self.config.landing_name()),
            None => String::new(),
        }
    }

    pub(super) fn import_action(&mut self, action: Option<ImportAction>) {
        let View::Import { entries, groups } = &self.view else { return };
        match action {
            Some(ImportAction::Select(i)) => self.gallery.select(Some(i)),
            Some(ImportAction::Open(i)) => self.open_entry(i),
            Some(ImportAction::Toggle(i)) => {
                if let Some(e) = entries.get(i) {
                    let path = e.path.clone();
                    if !self.import.unpicked.remove(&path) {
                        self.import.unpicked.insert(path);
                    }
                    self.show_import();
                }
            }
            Some(ImportAction::ToggleGroup(n, on)) => {
                let paths: Vec<PathBuf> = groups
                    .get(n)
                    .map(|g| g.range.clone().filter_map(|i| entries.get(i)).map(|e| e.path.clone()).collect())
                    .unwrap_or_default();
                for path in paths {
                    if on {
                        self.import.unpicked.remove(&path);
                    } else {
                        self.import.unpicked.insert(path);
                    }
                }
                self.show_import();
            }
            Some(ImportAction::Archives(choice)) => {
                self.config.import.archives = choice;
                self.save_config();
            }
            Some(ImportAction::Library(root)) => {
                // As in the settings; the names taken depend on the landing folder.
                self.config.import.library = Some(root);
                self.save_config();
                self.start_import_scan();
            }
            Some(ImportAction::ImportPicked) => self.import_now(true, false),
            Some(ImportAction::ImportAndClassify) => self.import_now(true, true),
            Some(ImportAction::ImportAll) => self.import_now(false, false),
            Some(ImportAction::Reload) => self.start_import_scan(),
            Some(ImportAction::OpenSettings) => self.settings.open_at("library"),
            None => {}
        }
    }

    /// Brings the candidates in: every importable one, or (`only_picked`) those still ticked;
    /// with `classify` the AI is asked for their tags once they are in.
    fn import_now(&mut self, only_picked: bool, classify: bool) {
        let (Some(root), Some(landing)) = (self.config.import_root().map(Path::to_path_buf), self.config.landing_dir())
        else {
            return;
        };
        // One plan over all folders, so two files of the same name get their numbers.
        let mut all = Scan { folder: PathBuf::new(), candidates: Vec::new() };
        let mut picked = Vec::new();
        let mut from: Vec<String> = Vec::new();
        for scan in &self.import.scans {
            let mut any = false;
            for c in &scan.candidates {
                if c.importable() && !(only_picked && self.import.unpicked.contains(&c.entry.path)) {
                    picked.push(all.candidates.len());
                    any = true;
                }
                all.candidates.push(c.clone());
            }
            if any {
                from.push(listing::file_name(&scan.folder));
            }
        }
        let plan = import::plan(&all, &root, &landing, self.config.import.archives, &picked);
        if plan.items.is_empty() {
            self.toast(t!("import.nothing").into_owned());
            return;
        }
        let from = from.join(", ");
        let title = t!("import.journal_title", from = from, to = self.import_target(), count = plan.items.len());
        let title = title.into_owned();
        self.import.classify = classify.then(|| PendingClassify { title: title.clone(), drops: landed(&plan, &from) });
        self.library.import_files(plan, title);
    }

    /// Tags from the AI after an import: `None` without AI, `Some(false)` while the AI may not
    /// see the library the files go to.
    pub(super) fn import_ai(&self) -> Option<bool> {
        if !self.chat_available() {
            return None;
        }
        let scope = meshgrove_core::ai::Scope::new(&self.config, self.chat_class());
        Some(self.config.landing_dir().is_some_and(|l| scope.allows(&l)))
    }

    /// After the import called `title`: asks the AI for the tags of its files, if it was to.
    pub(super) fn classify_imported(&mut self, title: &str) {
        let Some(pending) = self.import.classify.take_if(|p| p.title == title) else { return };
        let Some(index) = &self.index else { return };
        let batches: Vec<Batch> = pending
            .drops
            .into_iter()
            .filter_map(|d| {
                // What came in, as the index has it now (the landing folder was listed first).
                let paths: Vec<PathBuf> = match &d.folder {
                    Some(folder) => index.files_below(folder).unwrap_or_default(),
                    None => d.files.into_iter().filter(|p| index.entry_of(p).ok().flatten().is_some()).collect(),
                };
                (!paths.is_empty()).then_some(Batch { title: d.title, folder: d.folder, paths })
            })
            .collect();
        if batches.is_empty() {
            self.toast(t!("import.classify_nothing").into_owned());
            return;
        }
        self.classify(batches, Source::Import);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meshgrove_core::ai::proposals::Target;

    fn target(path: &str) -> Target {
        Target { path: path.into(), hash: None, size: 1, mtime_ns: 1 }
    }

    #[test]
    fn an_import_lands_as_its_folders_archives_and_loose_files() {
        let landing = PathBuf::from("/lib/Import");
        let item = |to: &str, folder: bool| PlanItem::Move {
            from: target(&format!("/dl/{to}")),
            to: landing.join(to),
            folder,
        };
        let plan = Plan {
            root: "/lib".into(),
            landing: landing.clone(),
            items: vec![
                item("rose.stl", false),
                item("Kunde Meier", true),
                item("kit.zip", false),
                PlanItem::Extract { archive: target("/dl/pack.zip"), into: landing.join("pack") },
                item("halter.gcode", false),
            ],
            trash: Vec::new(),
        };
        let whole = |name: &str| Landed { title: name.into(), folder: Some(landing.join(name)), files: Vec::new() };
        let loose = Landed {
            title: "Downloads".into(),
            folder: None,
            files: vec![landing.join("rose.stl"), landing.join("halter.gcode")],
        };
        assert_eq!(landed(&plan, "Downloads"), [loose, whole("Kunde Meier"), whole("kit.zip"), whole("pack")]);
        let only_folder = Plan { items: vec![item("Kunde Meier", true)], ..plan };
        assert_eq!(landed(&only_folder, "Downloads"), [whole("Kunde Meier")], "no empty group of loose files");
    }
}
