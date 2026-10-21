use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use crossbeam_channel::{Receiver, bounded};
use egui::{Button, Key, KeyboardShortcut, Modifiers, RichText, Ui};
use stlb_core::cache::{ThumbCache, ThumbMeta};
use stlb_core::config::{Config, Slicer, SortKey};
use stlb_core::db::Library;
use stlb_core::listing::{self, Entry};
use stlb_core::{Kind, query};
use stlb_render::Renderer;

use crate::actions;
use crate::i18n;
use crate::library::{LibEvent, LibraryService};
use crate::meshes::{self, MeshState, MeshStore};
use crate::nav::{Listing, Navigator};
use crate::thumbs::{ThumbResult, ThumbService};
use crate::ui::details::{self, DetailsContext, ModelStatus, TagEditor};
use crate::ui::duplicates::{self, DupAction, GroupInfo};
use crate::ui::gallery::{Action, Arrange, Gallery, MenuContext};
use crate::ui::settings::{AppInfo, SettingsChange, SettingsDialog};
use crate::ui::sidebar::{LibraryInfo, Sidebar, SidebarAction};
use crate::ui::turntable::Turntable;
use crate::ui::viewer::ModelViewer;

/// Wait this long after the selection settles before loading a model into the viewer,
/// so arrowing through a folder of 200 MB miniatures doesn't load every one of them.
const VIEWER_DEBOUNCE: Duration = Duration::from_millis(250);
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);
const TILE_RANGE: std::ops::RangeInclusive<f32> = 96.0..=384.0;
const MAX_RESULTS: usize = 5000;
/// Texture uploads per frame; more would stutter when a whole folder comes out of the cache at once.
const UPLOADS_PER_FRAME: usize = 24;

/// Files that were (to be) moved to the trash, and how it went.
type TrashResult = (Vec<PathBuf>, Result<(), String>);

/// What the gallery shows.
enum View {
    /// The current folder, as listed by the navigator.
    Folder,
    /// Library search results.
    Search(Vec<Entry>),
    /// Every file below `root` ("include subfolders"); `None` while the folders are walked.
    Flatten { root: PathBuf, entries: Option<Vec<Entry>> },
    /// Groups of duplicate models from the library index.
    Duplicates { entries: Vec<Entry>, groups: Vec<GroupInfo> },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Folder,
    Library,
}

pub struct App {
    config: Config,
    config_error: Option<String>,
    nav: Navigator,
    view: View,
    sidebar: Sidebar,
    gallery: Gallery,
    thumbs: ThumbService,
    library: LibraryService,
    db_path: PathBuf,
    /// Read connection to the index (search, folder mosaics); the service owns the writer.
    /// Opened after the first scan has created the index.
    index: Option<Library>,
    renderer: Renderer,
    viewer: ModelViewer,
    model: ModelStatus,
    meshes: MeshStore,
    turntable: Turntable,
    /// Plate preview textures of the model in the viewer.
    plates: Option<(PathBuf, Vec<(usize, egui::TextureHandle)>)>,
    selection_changed: Instant,
    /// Build plate shown in the viewer for multi-plate projects (`None` = default plate).
    viewer_plate: Option<usize>,
    last_selected: Option<usize>,
    search: String,
    scope: Scope,
    search_changed: Option<Instant>,
    focus_search: bool,
    flatten: bool,
    flatten_rx: Option<Receiver<(PathBuf, Vec<Entry>)>>,
    select_after_load: Option<PathBuf>,
    gpu_name: String,
    rate: RateMeter,
    screenshot: Option<crate::screenshot::ScreenshotHook>,
    slicer: Option<Slicer>,
    /// Context-menu facts for `menu_for` (recomputed when the selection changes).
    menu: MenuContext,
    menu_for: Vec<PathBuf>,
    selection_tags: Vec<(String, usize)>,
    all_tags: Vec<(String, usize)>,
    tags_dirty: bool,
    tag_input: String,
    focus_tags: bool,
    /// Duplicate groups in the library (for the sidebar) and when to recount them.
    duplicate_groups: usize,
    duplicates_due: Option<Instant>,
    last_processed: usize,
    trash_confirm: Option<Vec<PathBuf>>,
    trash_rx: Option<Receiver<TrashResult>>,
    /// Short message in the status bar (result of an action) and when it was set.
    toast: Option<(String, Instant)>,
    settings: SettingsDialog,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        config: Config,
        config_error: Option<String>,
        start: Option<PathBuf>,
    ) -> anyhow::Result<App> {
        let rs = cc.wgpu_render_state.as_ref().context("eframe was not started with the wgpu renderer")?;
        // Ctrl +/- and Ctrl+wheel resize tiles instead of zooming the whole UI.
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        use_system_ui_font(&cc.egui_ctx);

        let slicer = config.slicer.clone().or_else(Slicer::detect);
        match &slicer {
            Some(s) => log::info!("slicer: {} ({})", s.name, s.program.display()),
            None => log::info!("no slicer found; models open with the default application"),
        }
        let cache = ThumbCache::new(ThumbCache::default_location().context("no cache directory")?);
        let db_path = Library::default_location().context("no data directory")?;
        let library = LibraryService::start(db_path.clone(), config.library_roots.clone(), cc.egui_ctx.clone());
        let (start_dir, select) = match start {
            // A file opens its folder with the file selected; an archive opens like a folder.
            Some(p) if p.is_file() && !stlb_core::archive::is_archive_file(&p) => {
                (p.parent().map(Path::to_path_buf).unwrap_or_default(), Some(p))
            }
            Some(p) => (p, None),
            None => (
                config
                    .last_folder
                    .clone()
                    .filter(|p| p.is_dir())
                    .or_else(|| config.library_roots.first().cloned())
                    .or_else(dirs::home_dir)
                    .unwrap_or_else(|| PathBuf::from("/")),
                None,
            ),
        };
        Ok(App {
            nav: Navigator::new(cc.egui_ctx.clone(), start_dir),
            view: View::Folder,
            sidebar: Sidebar::new(),
            gallery: Gallery::default(),
            thumbs: ThumbService::new(cc.egui_ctx.clone(), cache),
            library,
            db_path,
            index: None,
            renderer: Renderer::new(&rs.device, &rs.adapter),
            viewer: ModelViewer::new(),
            model: ModelStatus::Idle,
            meshes: MeshStore::new(rs.device.clone(), cc.egui_ctx.clone()),
            turntable: Turntable::default(),
            plates: None,
            selection_changed: Instant::now(),
            viewer_plate: None,
            last_selected: None,
            search: String::new(),
            scope: Scope::Folder,
            search_changed: None,
            focus_search: false,
            flatten: false,
            flatten_rx: None,
            select_after_load: select,
            gpu_name: rs.adapter.get_info().name,
            rate: RateMeter::default(),
            screenshot: crate::screenshot::ScreenshotHook::from_env(),
            slicer,
            menu: MenuContext::default(),
            menu_for: Vec::new(),
            selection_tags: Vec::new(),
            all_tags: Vec::new(),
            tags_dirty: true,
            tag_input: String::new(),
            focus_tags: false,
            duplicate_groups: 0,
            duplicates_due: Some(Instant::now()),
            last_processed: 0,
            trash_confirm: None,
            trash_rx: None,
            toast: None,
            settings: SettingsDialog::default(),
            config,
            config_error,
        })
    }

    fn save_config(&mut self) {
        self.config.last_folder = Some(self.nav.current().to_path_buf());
        if let Err(e) = self.config.save() {
            log::warn!("could not save settings: {e}");
        }
    }

    /// The entries the gallery currently shows.
    fn entries(&self) -> &[Entry] {
        view_entries(&self.view, &self.nav)
    }

    fn selected_entry(&self) -> Option<&Entry> {
        self.entries().get(self.gallery.selected?)
    }

    fn arrange(&self) -> Arrange {
        let filter = if self.scope == Scope::Folder { self.search.clone() } else { String::new() };
        Arrange {
            filter,
            only_models: self.config.only_models,
            sort: self.config.sort_by,
            descending: self.config.sort_descending,
        }
    }

    /// Hands the current entries (and, in folder view, the folders' mosaics) to the gallery.
    fn refresh_gallery(&mut self, reset: bool) {
        let entries = self.entries().to_vec();
        let mut mosaics = HashMap::new();
        if let (View::Folder, Some(index)) = (&self.view, &self.index) {
            for (i, e) in entries.iter().enumerate().filter(|(_, e)| e.is_dir() || e.kind == Kind::Archive) {
                let preview = index.folder_preview(&e.path, 4).unwrap_or_default();
                if !preview.is_empty() {
                    mosaics.insert(i, preview);
                }
            }
        }
        // Search hits, subfolder views and archive contents come from several folders.
        self.gallery.show_folders =
            !matches!(self.view, View::Folder) || stlb_core::archive::is_archive_file(self.nav.current());
        self.gallery.set_entries(&entries, mosaics, &self.thumbs);
        if reset {
            self.gallery.reset_view();
        }
        let arrange = self.arrange();
        self.gallery.apply_filter(&entries, &arrange);
        if let Some(target) = self.select_after_load.take_if(|t| entries.iter().any(|e| e.path == *t)) {
            self.gallery.select(entries.iter().position(|e| e.path == target));
        }
    }

    fn reapply_filter(&mut self) {
        let entries = self.entries().to_vec();
        let arrange = self.arrange();
        self.gallery.apply_filter(&entries, &arrange);
    }

    /// Leaves search results (and the search text) before navigating somewhere.
    fn leave_search(&mut self) {
        if matches!(self.view, View::Search(_)) || self.scope == Scope::Library {
            self.search.clear();
            self.search_changed = None;
            self.view = View::Folder;
        }
    }

    fn navigate(&mut self, path: PathBuf) {
        self.leave_search();
        self.nav.open(path);
    }

    fn open_entry(&mut self, index: usize) {
        let Some(entry) = self.entries().get(index).cloned() else { return };
        if entry.is_dir() || entry.kind == Kind::Archive {
            self.navigate(entry.path);
            return;
        }
        // Files inside archives are extracted first so other programs can open them.
        match stlb_core::archive::materialize(&entry.path) {
            Ok(file) => {
                if let Err(e) = std::process::Command::new("xdg-open").arg(&file).spawn() {
                    log::warn!("xdg-open failed: {e}");
                }
            }
            Err(e) => log::warn!("cannot extract {}: {e}", entry.path.display()),
        }
    }

    /// Opens the folder containing a search/flatten result, with the file selected.
    fn show_in_folder(&mut self, index: usize) {
        let Some(entry) = self.entries().get(index).cloned() else { return };
        let Some(parent) = entry.path.parent().map(Path::to_path_buf) else { return };
        self.flatten = false;
        self.leave_search();
        self.view = View::Folder;
        self.select_after_load = Some(entry.path);
        if parent == self.nav.current() {
            self.nav.reload();
        } else {
            self.nav.open(parent);
        }
    }

    fn on_listing_arrived(&mut self) {
        if matches!(self.view, View::Search(_)) {
            return; // the folder changed underneath the results; it is shown when the search ends
        }
        if self.flatten {
            self.start_flatten();
        } else {
            self.view = View::Folder;
            self.refresh_gallery(true);
        }
    }

    /// Walks every folder below the current one in the background.
    fn start_flatten(&mut self) {
        let root = self.nav.current().to_path_buf();
        self.view = View::Flatten { root: root.clone(), entries: None };
        self.gallery.set_entries(&[], HashMap::new(), &self.thumbs);
        let arrange = self.arrange();
        self.gallery.apply_filter(&[], &arrange);
        let (tx, rx) = bounded(1);
        self.flatten_rx = Some(rx);
        std::thread::Builder::new()
            .name("flatten".into())
            .spawn(move || {
                let mut files = Vec::new();
                let mut stack = vec![root.clone()];
                while let Some(dir) = stack.pop() {
                    for e in listing::list_dir(&dir).unwrap_or_default() {
                        if e.is_dir() {
                            stack.push(e.path);
                        } else if files.len() < MAX_RESULTS {
                            files.push(e);
                        }
                    }
                }
                let _ = tx.send((root, files));
            })
            .expect("spawning flatten walker");
    }

    fn poll_flatten(&mut self) {
        let Some(rx) = &self.flatten_rx else { return };
        let Ok((root, files)) = rx.try_recv() else { return };
        self.flatten_rx = None;
        if let View::Flatten { root: wanted, entries } = &mut self.view
            && *wanted == root
        {
            *entries = Some(files);
            self.refresh_gallery(true);
        }
    }

    fn run_search(&mut self) {
        if self.search.trim().is_empty() {
            if matches!(self.view, View::Search(_)) {
                self.view = View::Folder;
                self.on_listing_arrived();
            }
            return;
        }
        let results = match &self.index {
            Some(index) => index.search(&query::parse(&self.search), MAX_RESULTS).unwrap_or_else(|e| {
                log::warn!("search failed: {e}");
                Vec::new()
            }),
            None => Vec::new(),
        };
        self.view = View::Search(results);
        self.refresh_gallery(true);
    }

    /// Moves finished thumbnails into the gallery and their facts into the library index.
    fn receive_thumbnails(&mut self, ctx: &egui::Context) {
        let mut uploads = 0;
        while uploads < UPLOADS_PER_FRAME {
            let Some(result) = self.thumbs.try_recv() else { return };
            match &result {
                ThumbResult::Ready { job, image, meta } => {
                    uploads += usize::from(image.is_some());
                    self.library.store_meta(job.path.clone(), job.key.clone(), meta.clone());
                }
                ThumbResult::Failed { job, .. } => {
                    self.library.store_meta(job.path.clone(), job.key.clone(), ThumbMeta::default());
                }
            }
            self.gallery.accept(ctx, &result);
        }
        ctx.request_repaint(); // budget used up: more results may be waiting
    }

    fn library_events(&mut self) {
        while let Some(event) = self.library.try_event() {
            match event {
                LibEvent::Scanned { changed } => {
                    let first = self.index.is_none();
                    if first {
                        self.index = Library::open_reader(&self.db_path);
                        self.menu_for.clear(); // re-evaluate what the selection allows (tags need the index)
                        self.duplicates_due = Some(Instant::now());
                    }
                    if changed.is_empty() && !first {
                        continue; // periodic safety-net scan found nothing new
                    }
                    if changed.iter().any(|d| d == self.nav.current()) {
                        self.nav.reload();
                    } else if matches!(self.view, View::Folder) {
                        self.refresh_gallery(false); // the index may now know the folders' contents
                    }
                }
                LibEvent::Pending(jobs) => {
                    if !jobs.is_empty() {
                        log::info!("{} library files need thumbnails/facts", jobs.len());
                    }
                    self.thumbs.set_background(jobs);
                    self.duplicates_due = Some(Instant::now());
                    self.tags_dirty = true;
                }
                LibEvent::TagsChanged => self.tags_dirty = true,
            }
        }
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        let entries = self.entries();
        self.gallery.selection().into_iter().filter_map(|i| entries.get(i)).map(|e| e.path.clone()).collect()
    }

    fn in_library(&self, path: &Path) -> bool {
        self.config.library_roots.iter().any(|r| path.starts_with(r))
    }

    /// Context menu and tag facts follow the selection.
    fn update_selection_facts(&mut self) {
        let paths = self.selected_paths();
        if paths != self.menu_for {
            self.menu = MenuContext {
                slicer: self.slicer.as_ref().map(|s| s.name.clone()),
                trash_blocked: actions::trash_blocked(&paths),
                can_tag: self.index.is_some() && !paths.is_empty() && paths.iter().all(|p| self.in_library(p)),
            };
            self.menu_for = paths;
            self.tags_dirty = true;
        }
        if std::mem::take(&mut self.tags_dirty)
            && let Some(index) = &self.index
        {
            self.all_tags = index.all_tags().unwrap_or_default();
            self.selection_tags = index.tags_of(&self.menu_for).unwrap_or_default();
        }
    }

    /// Recounts duplicates now and then while the library is being processed.
    fn update_duplicates(&mut self) {
        let processed = self.library.status().counts.processed;
        if processed != self.last_processed && self.duplicates_due.is_none() {
            self.duplicates_due = Some(Instant::now() + Duration::from_secs(3));
        }
        let Some(due) = self.duplicates_due else { return };
        if Instant::now() < due {
            return;
        }
        self.duplicates_due = None;
        self.last_processed = processed;
        let Some(index) = &self.index else { return };
        let groups = index.duplicate_groups().unwrap_or_default();
        self.duplicate_groups = groups.len();
        let map = groups.iter().flat_map(|g| g.files.iter().map(move |f| (f.path.clone(), g.files.len()))).collect();
        self.gallery.set_duplicates(map);
    }

    fn show_duplicates(&mut self) {
        let Some(index) = &self.index else { return };
        let (mut entries, mut groups) = (Vec::new(), Vec::new());
        for g in index.duplicate_groups().unwrap_or_default() {
            let start = entries.len();
            let keep = start + duplicates::suggest_keep(&g.files);
            let others: Vec<PathBuf> =
                g.files.iter().enumerate().filter(|(i, _)| start + i != keep).map(|(_, f)| f.path.clone()).collect();
            let (wasted, exact) = (g.wasted(), g.exact);
            entries.extend(g.files);
            groups.push(GroupInfo {
                range: start..entries.len(),
                exact,
                keep,
                wasted,
                blocked: actions::trash_blocked(&others),
            });
        }
        self.leave_search();
        self.flatten = false;
        self.view = View::Duplicates { entries, groups };
        self.refresh_gallery(true);
    }

    fn handle_action(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::Open(index) => self.open_entry(index),
            Action::ShowInFolder(index) => self.show_in_folder(index),
            Action::OpenInSlicer => {
                let files: Vec<PathBuf> = self.selected_paths();
                let models: Vec<PathBuf> = files
                    .into_iter()
                    .filter(|p| stlb_core::Kind::from_file_name(&listing::file_name(p)).is_model())
                    .collect();
                if let Some(slicer) = &self.slicer {
                    let message = match actions::open_in_slicer(slicer, &models) {
                        Ok(()) => t!("toast.slicer_started", name = slicer.name, count = models.len()),
                        Err(e) => t!("toast.slicer_failed", name = slicer.name, error = e.to_string()),
                    };
                    self.toast(message.into_owned());
                }
            }
            Action::ShowInFileManager => {
                if let Err(e) = actions::show_in_file_manager(&self.selected_paths()) {
                    self.toast(t!("toast.file_manager_failed", error = e.to_string()).into_owned());
                }
            }
            Action::CopyPaths => {
                let paths = self.selected_paths();
                let text: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
                ctx.copy_text(text.join("\n"));
                self.toast(t!("toast.copied", count = paths.len()).into_owned());
            }
            Action::EditTags => self.focus_tags = true,
            Action::Trash => self.request_trash(self.selected_paths()),
        }
    }

    fn request_trash(&mut self, paths: Vec<PathBuf>) {
        match actions::trash_blocked(&paths) {
            None => self.trash_confirm = Some(paths),
            Some(reason) if !reason.is_empty() => self.toast(reason),
            Some(_) => {}
        }
    }

    fn apply_settings(&mut self, changes: Vec<SettingsChange>) {
        for change in changes {
            match change {
                SettingsChange::Language(lang) => {
                    i18n::init(lang.as_deref());
                    self.config.language = lang;
                }
                SettingsChange::AddRoot(path) => {
                    if !self.config.library_roots.contains(&path) {
                        self.config.library_roots.push(path);
                        self.library.set_roots(self.config.library_roots.clone());
                    }
                }
                SettingsChange::RemoveRoot(path) => {
                    self.config.library_roots.retain(|r| r != &path);
                    self.library.set_roots(self.config.library_roots.clone());
                }
                SettingsChange::Slicer(slicer) => {
                    self.config.slicer = slicer.clone();
                    self.slicer = slicer.or_else(Slicer::detect);
                    self.menu_for.clear();
                    let name = self
                        .slicer
                        .as_ref()
                        .map_or_else(|| t!("settings.slicer_none").into_owned(), |s| s.name.clone());
                    self.toast(t!("toast.slicer_set", name = name).into_owned());
                }
                SettingsChange::ClearCache => {
                    if let Some(dir) = crate::ui::settings::cache_dir() {
                        for sub in ["thumbs", "step", "extracted"] {
                            let _ = std::fs::remove_dir_all(dir.join(sub));
                        }
                    }
                    self.toast(t!("toast.cache_cleared").into_owned());
                }
            }
            self.save_config();
        }
    }

    fn toast(&mut self, message: String) {
        self.toast = Some((message, Instant::now()));
    }

    /// Confirmation dialog for moving files to the trash; the move runs on a thread.
    fn trash_dialog(&mut self, ctx: &egui::Context) {
        let Some(paths) = &self.trash_confirm else { return };
        let mut decision = None;
        egui::Modal::new(egui::Id::unique("trash-confirm")).show(ctx, |ui| {
            ui.set_max_width(460.0);
            ui.heading(t!("trash.title", count = paths.len()));
            ui.add_space(4.0);
            for p in paths.iter().take(8) {
                ui.label(format!("• {}", listing::file_name(p)));
            }
            if paths.len() > 8 {
                ui.weak(t!("trash.more", count = paths.len() - 8));
            }
            ui.add_space(4.0);
            ui.weak(t!("trash.explain"));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(RichText::new(t!("trash.confirm")).strong()).clicked()
                    || ui.input(|i| i.key_pressed(Key::Enter))
                {
                    decision = Some(true);
                }
                if ui.button(t!("trash.cancel")).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    decision = Some(false);
                }
            });
        });
        match decision {
            Some(true) => {
                let paths = self.trash_confirm.take().unwrap_or_default();
                let (tx, rx) = bounded(1);
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let result = trash::delete_all(&paths).map_err(|e| e.to_string());
                    let _ = tx.send((paths, result));
                    ctx.request_repaint();
                });
                self.trash_rx = Some(rx);
            }
            Some(false) => self.trash_confirm = None,
            None => {}
        }
    }

    fn poll_trash(&mut self) {
        let Some(rx) = &self.trash_rx else { return };
        let Ok((paths, result)) = rx.try_recv() else { return };
        self.trash_rx = None;
        let message = match &result {
            Ok(()) => t!("toast.trashed", count = paths.len()),
            Err(e) => t!("toast.trash_failed", error = e),
        };
        self.toast(message.into_owned());
        self.library.changed(paths);
        self.duplicates_due = Some(Instant::now() + Duration::from_secs(2));
        match self.view {
            View::Duplicates { .. } => self.show_duplicates(),
            _ => self.nav.reload(),
        }
    }

    /// Keeps the 3D viewer in sync with the selection (debounced, loads run in the mesh store).
    fn update_viewer(&mut self, ctx: &egui::Context) {
        if self.gallery.selected != self.last_selected {
            self.last_selected = self.gallery.selected;
            self.selection_changed = Instant::now();
            self.viewer_plate = None;
        }
        let wanted = self.selected_entry().filter(|e| meshes::viewable(e.kind)).map(|e| (e.path.clone(), e.kind));
        let Some((path, kind)) = wanted else {
            self.viewer.set_mesh(None);
            self.model = ModelStatus::Idle;
            return;
        };
        let waited = self.selection_changed.elapsed();
        if waited < VIEWER_DEBOUNCE {
            ctx.request_repaint_after(VIEWER_DEBOUNCE - waited);
            self.viewer.set_mesh(None);
            self.model = ModelStatus::Loading;
            return;
        }
        match self.meshes.request(&path, kind, self.viewer_plate) {
            MeshState::Loading => {
                self.viewer.set_mesh(None);
                self.model = ModelStatus::Loading;
            }
            MeshState::Failed(e) => {
                self.viewer.set_mesh(None);
                self.model = ModelStatus::Failed(e.clone());
            }
            MeshState::Ready(loaded) => {
                let loaded = loaded.clone();
                self.viewer.set_mesh(Some(loaded.mesh));
                if self.plates.as_ref().is_none_or(|(p, _)| *p != path) {
                    let textures = loaded
                        .info
                        .plates
                        .iter()
                        .map(|(n, img)| {
                            (*n, ctx.load_texture(format!("plate-{n}"), img.clone(), egui::TextureOptions::LINEAR))
                        })
                        .collect();
                    self.plates = Some((path, textures));
                }
                self.model = ModelStatus::Ready(loaded.info);
            }
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.egui_wants_keyboard_input();
        let pressed = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        if pressed(Modifiers::ALT, Key::ArrowLeft) {
            self.leave_search();
            self.nav.back();
        }
        if pressed(Modifiers::ALT, Key::ArrowRight) {
            self.leave_search();
            self.nav.forward();
        }
        if pressed(Modifiers::ALT, Key::ArrowUp) || (!typing && pressed(Modifiers::NONE, Key::Backspace)) {
            self.leave_search();
            self.nav.up();
        }
        if !typing && pressed(Modifiers::NONE, Key::Delete) {
            let paths = self.selected_paths();
            self.request_trash(paths);
        }
        if pressed(Modifiers::COMMAND, Key::Comma) {
            self.settings.open(self.slicer.as_ref());
        }
        if pressed(Modifiers::NONE, Key::F5) {
            self.nav.reload();
            self.sidebar.refresh();
        }
        if pressed(Modifiers::COMMAND | Modifiers::SHIFT, Key::F) {
            self.set_scope(Scope::Library);
            self.focus_search = true;
        }
        if pressed(Modifiers::COMMAND, Key::F) {
            self.set_scope(Scope::Folder);
            self.focus_search = true;
        }
        if pressed(Modifiers::COMMAND, Key::Plus) || pressed(Modifiers::COMMAND, Key::Equals) {
            self.config.tile_size = (self.config.tile_size + 32.0).min(*TILE_RANGE.end());
        }
        if pressed(Modifiers::COMMAND, Key::Minus) {
            self.config.tile_size = (self.config.tile_size - 32.0).max(*TILE_RANGE.start());
        }
    }

    fn set_scope(&mut self, scope: Scope) {
        if self.scope == scope {
            return;
        }
        self.scope = scope;
        match scope {
            Scope::Library => self.search_changed = Some(Instant::now() - SEARCH_DEBOUNCE),
            Scope::Folder => {
                if matches!(self.view, View::Search(_)) {
                    self.view = View::Folder;
                    self.on_listing_arrived();
                } else {
                    self.reapply_filter();
                }
            }
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let nav_button = |ui: &mut Ui, icon: &str, enabled: bool, tip: std::borrow::Cow<str>| {
                ui.add_enabled(enabled, Button::new(RichText::new(icon).size(16.0))).on_hover_text(tip).clicked()
            };
            if nav_button(ui, "⬅", self.nav.can_go_back(), t!("nav.back")) {
                self.leave_search();
                self.nav.back();
            }
            if nav_button(ui, "➡", self.nav.can_go_forward(), t!("nav.forward")) {
                self.leave_search();
                self.nav.forward();
            }
            if nav_button(ui, "⬆", self.nav.can_go_up(), t!("nav.up")) {
                self.leave_search();
                self.nav.up();
            }
            if nav_button(ui, "🔄", true, t!("nav.reload")) {
                self.nav.reload();
                self.sidebar.refresh();
            }
            ui.separator();

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(RichText::new("⚙").size(16.0)).on_hover_text(t!("settings.open")).clicked() {
                    self.settings.open(self.slicer.as_ref());
                }
                self.view_menu(ui);
                self.search_field(ui);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    egui::ScrollArea::horizontal().id_salt("breadcrumb").show(ui, |ui| self.breadcrumb(ui));
                });
            });
        });
    }

    /// "Ansicht" menu: sorting, "models only", "include subfolders", tile size.
    fn view_menu(&mut self, ui: &mut Ui) {
        let mut changed = false;
        let mut flatten = self.flatten;
        ui.menu_button(format!("{} ▾", t!("view.menu")), |ui| {
            ui.label(RichText::new(t!("gallery.sort")).weak());
            let names = [
                (SortKey::Name, t!("sort.name")),
                (SortKey::Modified, t!("sort.modified")),
                (SortKey::Size, t!("sort.size")),
                (SortKey::Type, t!("sort.type")),
            ];
            for (key, name) in names {
                changed |= ui.radio_value(&mut self.config.sort_by, key, name).changed();
            }
            changed |= ui.checkbox(&mut self.config.sort_descending, t!("gallery.descending")).changed();
            ui.separator();
            changed |= ui.checkbox(&mut self.config.only_models, t!("gallery.only_models")).changed();
            ui.checkbox(&mut flatten, t!("gallery.flatten")).on_hover_text(t!("gallery.flatten_hint"));
            ui.separator();
            ui.label(RichText::new(t!("gallery.tile_size")).weak());
            ui.add(egui::Slider::new(&mut self.config.tile_size, TILE_RANGE).show_value(false));
        });
        if flatten != self.flatten {
            self.set_flatten(flatten);
        }
        if changed {
            self.reapply_filter();
        }
    }

    fn set_flatten(&mut self, on: bool) {
        self.flatten = on;
        self.leave_search();
        if on {
            self.start_flatten();
        } else {
            self.flatten_rx = None;
            self.view = View::Folder;
            self.refresh_gallery(true);
        }
    }

    fn search_field(&mut self, ui: &mut Ui) {
        let hint = match self.scope {
            Scope::Folder => t!("search.folder_hint"),
            Scope::Library => t!("search.library_hint"),
        };
        let field = ui.add(egui::TextEdit::singleline(&mut self.search).hint_text(hint).desired_width(220.0));
        if std::mem::take(&mut self.focus_search) {
            field.request_focus();
        }
        let mut changed = field.changed();
        if field.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
            self.search.clear();
            field.surrender_focus();
            changed = true;
        }
        let mut scope = self.scope;
        ui.selectable_value(&mut scope, Scope::Library, "📚").on_hover_text(t!("search.scope_library"));
        ui.selectable_value(&mut scope, Scope::Folder, "📁").on_hover_text(t!("search.scope_folder"));
        if scope != self.scope {
            self.set_scope(scope);
        }
        if changed {
            match self.scope {
                Scope::Folder => self.reapply_filter(),
                Scope::Library => self.search_changed = Some(Instant::now()),
            }
        }
    }

    fn breadcrumb(&mut self, ui: &mut Ui) {
        match &self.view {
            View::Search(results) => {
                let text = t!("search.results", count = i18n::count(results.len()));
                ui.label(RichText::new(format!("🔍 {text}")).strong());
                if ui.button(t!("search.close")).clicked() {
                    self.leave_search();
                    self.on_listing_arrived();
                }
                return;
            }
            View::Flatten { entries, .. } => {
                let text = match entries {
                    Some(e) => t!("gallery.flatten_count", count = i18n::count(e.len())),
                    None => t!("gallery.loading"),
                };
                if ui.button(format!("🗂 {text}  ✕")).on_hover_text(t!("gallery.flatten_off")).clicked() {
                    self.set_flatten(false);
                }
            }
            View::Duplicates { groups, .. } => {
                ui.label(RichText::new(format!("⧉ {}", t!("dupes.title", count = groups.len()))).strong());
                if ui.button(t!("search.close")).clicked() {
                    self.view = View::Folder;
                    self.on_listing_arrived();
                }
                return;
            }
            View::Folder => {}
        }
        let current = self.nav.current().to_path_buf();
        let parts: Vec<PathBuf> =
            current.ancestors().map(Path::to_path_buf).collect::<Vec<_>>().into_iter().rev().collect();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                ui.weak("›");
            }
            let name = if part.parent().is_none() { "/".to_owned() } else { listing::file_name(part) };
            let last = i + 1 == parts.len();
            if ui
                .add(Button::new(if last { RichText::new(name).strong() } else { RichText::new(name) }).frame(false))
                .clicked()
                && !last
            {
                self.navigate(part.clone());
            }
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let entries = self.entries();
            let models = entries.iter().filter(|e| e.kind.is_model()).count();
            ui.label(t!(
                "status.items",
                shown = i18n::count(self.gallery.shown_count()),
                total = i18n::count(entries.len())
            ));
            ui.separator();
            ui.label(t!("status.models", count = i18n::count(models)));
            let (done, total) = self.thumbs.progress();
            let rate = self.rate.sample(done);
            if total > 0 && done < total {
                ui.separator();
                ui.add(egui::ProgressBar::new(done as f32 / total as f32).desired_width(120.0).desired_height(10.0));
                ui.label(t!("status.thumbs", done = i18n::count(done), total = i18n::count(total)));
                if rate > 0.0 {
                    ui.weak(t!("status.rate", rate = format!("{rate:.0}")));
                }
            }
            if let Some((message, at)) = &self.toast
                && at.elapsed() < Duration::from_secs(6)
            {
                ui.separator();
                ui.label(RichText::new(message).color(ui.visuals().selection.stroke.color));
                ui.ctx().request_repaint_after(Duration::from_secs(1));
            }
            if let Some(err) = &self.config_error {
                ui.separator();
                ui.colored_label(ui.visuals().warn_fg_color, t!("error.config", error = err));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.weak(t!("status.gpu", name = self.gpu_name));
                ui.separator();
                self.library_status(ui);
            });
        });
    }

    fn library_status(&self, ui: &mut Ui) {
        if self.config.library_roots.is_empty() {
            return;
        }
        let status = self.library.status();
        if let Some(error) = &status.error {
            ui.colored_label(ui.visuals().error_fg_color, t!("library.error", error = error));
            return;
        }
        let c = status.counts;
        if c.processed < c.processable {
            ui.weak(t!("library.processing", done = i18n::count(c.processed), total = i18n::count(c.processable)));
        }
        ui.weak(t!("library.files", files = i18n::count(c.files), models = i18n::count(c.models)));
        if status.scanning {
            ui.spinner().on_hover_text(t!("library.scanning"));
        }
    }

    fn central(&mut self, ui: &mut Ui, rs: &egui_wgpu::RenderState) {
        let message = match (&self.view, &self.nav.listing) {
            (View::Folder, Listing::Loading) | (View::Flatten { entries: None, .. }, _) => {
                ui.centered_and_justified(|ui| ui.spinner().on_hover_text(t!("gallery.loading")));
                return;
            }
            (View::Folder, Listing::Error(e)) => Some(t!("error.read_dir", error = e).into_owned()),
            (View::Search(results), _) if results.is_empty() => {
                Some(if self.index.is_none() { t!("search.unavailable") } else { t!("search.none") }.into_owned())
            }
            (View::Duplicates { .. }, _) => None,
            _ if self.entries().is_empty() => Some(t!("gallery.empty").into_owned()),
            _ => None,
        };
        if let Some(message) = message {
            ui.centered_and_justified(|ui| ui.weak(message));
            return;
        }
        let zoom = ui.input(|i| i.zoom_delta());
        if zoom != 1.0 && ui.ui_contains_pointer() {
            self.config.tile_size = (self.config.tile_size * zoom).clamp(*TILE_RANGE.start(), *TILE_RANGE.end());
        }
        if let View::Duplicates { entries, groups } = &self.view {
            match duplicates::show(ui, entries, groups, &self.gallery) {
                Some(DupAction::Select(i)) => self.gallery.select(Some(i)),
                Some(DupAction::Open(i)) => self.open_entry(i),
                Some(DupAction::TrashOthers(g)) => {
                    let paths: Vec<PathBuf> = groups[g]
                        .range
                        .clone()
                        .filter(|&i| i != groups[g].keep)
                        .map(|i| entries[i].path.clone())
                        .collect();
                    self.request_trash(paths);
                }
                None => {}
            }
            return;
        }
        let entries = view_entries(&self.view, &self.nav);
        let output = self.gallery.show(ui, entries, self.config.tile_size, &self.thumbs, &self.menu);
        if let Some(action) = output.action {
            self.handle_action(ui.ctx(), action);
        }
        self.turntable.show(ui, output.hovered, output.viewport, &mut self.meshes, rs, &self.renderer);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let rs = frame.wgpu_render_state().expect("wgpu renderer").clone();
        if self.nav.poll() {
            self.on_listing_arrived();
        }
        self.poll_flatten();
        self.poll_trash();
        self.library_events();
        self.update_selection_facts();
        self.update_duplicates();
        self.trash_dialog(&ctx);
        self.receive_thumbnails(&ctx);
        self.shortcuts(&ctx);
        if let Some(changed) = self.search_changed {
            if changed.elapsed() >= SEARCH_DEBOUNCE {
                self.search_changed = None;
                if self.scope == Scope::Library {
                    self.run_search();
                }
            } else {
                ctx.request_repaint_after(SEARCH_DEBOUNCE);
            }
        }
        self.meshes.begin_frame();
        self.update_viewer(&ctx);
        if let Some(hook) = &mut self.screenshot {
            if std::mem::take(&mut hook.settings) {
                self.settings.open(self.slicer.as_ref());
            }
            let hook = self.screenshot.as_mut().expect("checked above");
            if hook.duplicates && self.index.is_some() && self.duplicate_groups > 0 {
                hook.duplicates = false;
                self.show_duplicates();
            }
            let hook = self.screenshot.as_mut().expect("checked above");
            if hook.flatten && !matches!(self.nav.listing, Listing::Loading) {
                hook.flatten = false;
                self.set_flatten(true);
            }
            let hook = self.screenshot.as_mut().expect("checked above");
            if let Some(query) = hook.search.take() {
                self.scope = Scope::Library;
                self.search = query;
                self.search_changed = Some(Instant::now());
            }
            hook.tick(&ctx);
        }

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(3.0);
            self.toolbar(ui);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));

        let sidebar_action = egui::Panel::left("sidebar")
            .default_size(250.0)
            .max_size(420.0)
            .show(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                let info = LibraryInfo {
                    tags: &self.all_tags,
                    duplicate_groups: self.duplicate_groups,
                    showing_duplicates: matches!(self.view, View::Duplicates { .. }),
                };
                self.sidebar.show(ui, &self.config, self.nav.current(), info)
            })
            .inner;
        match sidebar_action {
            Some(SidebarAction::Navigate(path)) => self.navigate(path),
            Some(SidebarAction::AddRoot(path)) => {
                if !self.config.library_roots.contains(&path) {
                    self.config.library_roots.push(path);
                    self.save_config();
                    self.library.set_roots(self.config.library_roots.clone());
                }
            }
            Some(SidebarAction::RemoveRoot(path)) => {
                self.config.library_roots.retain(|r| r != &path);
                self.save_config();
                self.library.set_roots(self.config.library_roots.clone());
            }
            Some(SidebarAction::SearchTag(tag)) => {
                self.scope = Scope::Library;
                self.search = if tag.contains(' ') { format!("tag:\"{tag}\"") } else { format!("tag:{tag}") };
                self.run_search();
            }
            Some(SidebarAction::ShowDuplicates) => self.show_duplicates(),
            None => {}
        }

        let focus_tags = std::mem::take(&mut self.focus_tags);
        let details_out = egui::Panel::right("details")
            .default_size(340.0)
            .show(ui, |ui| {
                let entries = view_entries(&self.view, &self.nav);
                let entry = self.gallery.selected.and_then(|i| entries.get(i));
                let thumb = self.gallery.selected.and_then(|i| self.gallery.thumb(i));
                let picked = self.gallery.selection();
                let selection = (picked.len(), picked.iter().filter_map(|&i| entries.get(i)).map(|e| e.size).sum());
                details::show(
                    ui,
                    DetailsContext {
                        entry,
                        selection,
                        tags: TagEditor {
                            enabled: self.menu.can_tag,
                            on_selection: &self.selection_tags,
                            all: &self.all_tags,
                            input: &mut self.tag_input,
                            focus: focus_tags,
                        },
                        thumb,
                        model: &self.model,
                        plates: self.plates.as_ref().map_or(&[][..], |(_, p)| p.as_slice()),
                        viewer: &mut self.viewer,
                        render_state: &rs,
                        renderer: &self.renderer,
                    },
                )
            })
            .inner;
        if details_out.clicked_plate.is_some() {
            self.viewer_plate = details_out.clicked_plate;
        }
        if let Some(tag) = details_out.add_tag {
            self.library.tag(self.menu_for.clone(), tag);
        }
        if let Some(tag) = details_out.remove_tag {
            self.library.untag(self.menu_for.clone(), tag);
        }

        egui::CentralPanel::default().show(ui, |ui| self.central(ui, &rs));

        let info = AppInfo { gpu: &self.gpu_name, db_path: &self.db_path };
        let changes = self.settings.show(&ctx, &self.config, self.slicer.as_ref(), info);
        self.apply_settings(changes);
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        if let Some(hook) = &mut self.screenshot {
            hook.inject(raw);
        }
    }

    fn on_exit(&mut self) {
        self.save_config();
    }
}

/// Borrows only the view and the navigator, so other fields stay free for mutation.
fn view_entries<'a>(view: &'a View, nav: &'a Navigator) -> &'a [Entry] {
    match view {
        View::Folder => nav.entries(),
        View::Search(entries) => entries,
        View::Flatten { entries, .. } => entries.as_deref().unwrap_or(&[]),
        View::Duplicates { entries, .. } => entries,
    }
}

/// Use the desktop's UI font (Noto Sans on KDE) so the app does not look foreign;
/// glyphs it lacks still fall back to egui's bundled fonts and the system fallback.
fn use_system_ui_font(ctx: &egui::Context) {
    let Ok(out) = std::process::Command::new("fc-match").args(["-f", "%{file}", "sans-serif:style=Regular"]).output()
    else {
        return;
    };
    let path = String::from_utf8_lossy(&out.stdout).into_owned();
    let Ok(bytes) = std::fs::read(&path) else { return };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("system-ui".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.insert(0, "system-ui".into());
    }
    ctx.set_fonts(fonts);
    log::info!("UI font: {path}");
}

/// Thumbnails per second over the last few seconds, for the status bar.
#[derive(Default)]
struct RateMeter {
    samples: VecDeque<(Instant, usize)>,
}

impl RateMeter {
    fn sample(&mut self, done: usize) -> f32 {
        let now = Instant::now();
        if self.samples.back().is_some_and(|&(_, d)| done < d) {
            self.samples.clear(); // new folder, counter restarted
        }
        if self.samples.back().is_none_or(|&(t, _)| now - t > Duration::from_millis(250)) {
            self.samples.push_back((now, done));
        }
        while self.samples.len() > 2 && now - self.samples[0].0 > Duration::from_secs(3) {
            self.samples.pop_front();
        }
        match (self.samples.front(), self.samples.back()) {
            (Some(&(t0, d0)), Some(&(t1, d1))) if t1 > t0 => (d1 - d0) as f32 / (t1 - t0).as_secs_f32(),
            _ => 0.0,
        }
    }
}
