//! The import view: looking at the import folders (a downloads folder) for new 3D-print files,
//! showing them, and bringing the picked ones into the library's landing folder.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crossbeam_channel::{Receiver, bounded};
use meshgrove_core::cache::ThumbCache;
use meshgrove_core::config::ImportAtStart;
use meshgrove_core::db::Library;
use meshgrove_core::import::{self, Scan};
use meshgrove_core::listing;

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
                ImportAtStart::Auto => self.import_now(false),
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
            Some(ImportAction::ImportPicked) => self.import_now(true),
            Some(ImportAction::ImportAll) => self.import_now(false),
            Some(ImportAction::Reload) => self.start_import_scan(),
            Some(ImportAction::OpenSettings) => self.settings.open_at("library"),
            None => {}
        }
    }

    /// Brings the candidates in: every importable one, or (`only_picked`) those still ticked.
    fn import_now(&mut self, only_picked: bool) {
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
        let title =
            t!("import.journal_title", from = from.join(", "), to = self.import_target(), count = plan.items.len());
        self.library.import_files(plan, title.into_owned());
    }
}
