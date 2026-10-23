use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use egui::{Button, Key, KeyboardShortcut, Modifiers, RichText, Ui};
use meshgrove_core::cache::{ThumbCache, ThumbMeta};
use meshgrove_core::config::{Config, DoubleClick, GalleryView, PrintFilter, Program, Slicer, SortKey, TileTags};
use meshgrove_core::db::Library;
use meshgrove_core::listing::{self, Entry};
use meshgrove_core::lists::{ListInfo, ListKind, PrintResult, PrintStatus};
use meshgrove_core::printer::Bed;
use meshgrove_core::slicers;
use meshgrove_core::{Kind, query};
use meshgrove_render::Renderer;

use crate::actions;
use crate::folder_scan::FolderScanner;
use crate::i18n;
use crate::library::{LibEvent, LibraryService, ListOp, Transfer};
use crate::meshes::{self, MeshState, MeshStore};
use crate::nav::{Listing, Navigator};
use crate::print_export;
use crate::text_preview::TextPreview;
use crate::thumbs::{ThumbResult, ThumbService};
use crate::ui::SlicerList;
use crate::ui::details::{
    self, DetailsContext, DetailsOutput, ListsOfSelection, ModelStatus, NoteEditor, PrintInfo, TagEditor,
};
use crate::ui::duplicates::{self, DupAction, GroupInfo};
use crate::ui::gallery::{Action, Arrange, Gallery, ListSettings, MenuContext};
use crate::ui::layout::{Layout, Pane};
use crate::ui::model_window::{self, ModelWindow, WindowAction, WindowContext};
use crate::ui::settings::{AppInfo, SettingsChange, SettingsDialog};
use crate::ui::sidebar::{LibraryInfo, ListExport, Sidebar, SidebarAction};
use crate::ui::tags::TagStyle;
use crate::ui::turntable::Turntable;
use crate::ui::viewer::{MeshSource, ModelViewer};
use egui_tiles::Tree;

/// Wait this long after the selection settles before loading a model into the viewer,
/// so arrowing through a folder of 200 MB miniatures doesn't load every one of them.
const VIEWER_DEBOUNCE: Duration = Duration::from_millis(250);
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);
/// A note is saved this long after the last keystroke.
const NOTE_DEBOUNCE: Duration = Duration::from_millis(800);
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
    /// The files of a list (favourites, print list, collection).
    List { id: i64, entries: Vec<Entry> },
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
    /// The selected readme or note, or the selected folder's readme.
    text: TextPreview,
    /// Models opened in windows of their own.
    model_windows: Vec<ModelWindow>,
    /// The large preview over the window (Space).
    preview_open: bool,
    preview_viewer: ModelViewer,
    /// Models found inside folders on disk, for the folder tiles (when enabled in the settings).
    folder_scan: FolderScanner,
    scan_pending: bool,
    scan_refreshed: Instant,
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
    /// The slicers models can be sent to, and which one is the default.
    slicers: Vec<Slicer>,
    slicer_names: Vec<String>,
    default_slicer: usize,
    /// Mesh editor or CAD program for "open in …".
    editor: Option<Program>,
    /// Context-menu facts for `menu_for` (recomputed when the selection changes).
    menu: MenuContext,
    menu_for: Vec<PathBuf>,
    selection_tags: Vec<(String, usize)>,
    all_tags: Vec<(String, usize)>,
    tags_dirty: bool,
    /// Favourites, print lists and collections, for the sidebar and the menus.
    lists: Vec<ListInfo>,
    /// Lists holding the selection, with how many of its files each holds.
    selection_lists: Vec<(ListInfo, usize)>,
    /// The selection's print log, summed up.
    selection_print: PrintStatus,
    /// Lists, tile marks and tag colours must be read again (user data changed).
    marks_dirty: bool,
    /// How tags are drawn (coloured or not, chosen colours).
    tag_style: TagStyle,
    /// Name typed for a new automatic collection (saving a search).
    new_list_name: String,
    tag_input: String,
    focus_tags: bool,
    layout: Layout,
    /// Note of the selection as shown in the editor; saved shortly after the last keystroke.
    note_input: String,
    note_mixed: bool,
    note_edited: Option<Instant>,
    note_dirty: bool,
    /// Printer bed for the size check (from the settings or the slicer).
    bed: Option<Bed>,
    /// Duplicate groups in the library (for the sidebar) and when to recount them.
    duplicate_groups: usize,
    duplicates_due: Option<Instant>,
    last_processed: usize,
    trash_confirm: Option<Vec<PathBuf>>,
    trash_rx: Option<Receiver<TrashResult>>,
    /// Short message in the status bar (result of an action) and when it was set.
    toast: Option<(String, Instant)>,
    /// Messages from actions that finish on a thread (e.g. copying files).
    messages: (Sender<String>, Receiver<String>),
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

        // Slicers never chosen in the settings: take all that are installed.
        let mut config = config;
        if config.slicers.is_none() {
            let found: Vec<Slicer> = slicers::detect_all().into_iter().map(|d| d.program).collect();
            if !found.is_empty() {
                config.default_slicer = found.first().map(|s| s.name.clone());
                config.slicers = Some(found);
            }
        }
        let slicers = config.slicers.clone().unwrap_or_default();
        for s in &slicers {
            log::info!("slicer: {} ({})", s.name, s.program.display());
        }
        if slicers.is_empty() {
            log::info!("no slicer found; models open with the default application");
        }
        let default_slicer = Config::default_slicer_index(&slicers, config.default_slicer.as_deref());
        let slicer_names = slicers.iter().map(|s| s.name.clone()).collect();
        let editor = config.editor.clone().or_else(Program::detect_editor);
        if let Some(e) = &editor {
            log::info!("mesh editor: {} ({})", e.name, e.program.display());
        }
        let bed = config.printer.clone().or_else(Bed::detect);
        if let Some(b) = &bed {
            log::info!("printer bed: {} ({} × {} × {} mm)", b.name, b.x, b.y, b.z);
        }
        let cache = ThumbCache::new(ThumbCache::default_location().context("no cache directory")?);
        let db_path = Library::default_location().context("no data directory")?;
        let library = LibraryService::start(db_path.clone(), config.library_roots.clone(), cc.egui_ctx.clone());
        let (start_dir, select) = match start {
            // A file opens its folder with the file selected; an archive opens like a folder.
            Some(p) if p.is_file() && !meshgrove_core::archive::is_archive_file(&p) => {
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
            gallery: Gallery::with_bed(bed.clone()),
            thumbs: ThumbService::new(cc.egui_ctx.clone(), cache),
            library,
            db_path,
            index: None,
            renderer: Renderer::new(&rs.device, &rs.adapter),
            viewer: ModelViewer::new(),
            text: TextPreview::default(),
            model_windows: Vec::new(),
            preview_open: false,
            preview_viewer: {
                let mut v = ModelViewer::new();
                v.set_quality(config.preview_quality);
                v
            },
            folder_scan: FolderScanner::new(cc.egui_ctx.clone()),
            scan_pending: false,
            scan_refreshed: Instant::now(),
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
            slicers,
            slicer_names,
            default_slicer,
            editor,
            menu: MenuContext::default(),
            menu_for: Vec::new(),
            selection_tags: Vec::new(),
            all_tags: Vec::new(),
            tags_dirty: true,
            lists: Vec::new(),
            selection_lists: Vec::new(),
            selection_print: PrintStatus::default(),
            marks_dirty: true,
            tag_style: TagStyle { colored: config.colored_tags, ..Default::default() },
            new_list_name: String::new(),
            tag_input: String::new(),
            focus_tags: false,
            layout: Layout::load(),
            note_input: String::new(),
            note_mixed: false,
            note_edited: None,
            note_dirty: true,
            bed,
            duplicate_groups: 0,
            duplicates_due: Some(Instant::now()),
            last_processed: 0,
            trash_confirm: None,
            trash_rx: None,
            toast: None,
            messages: unbounded(),
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
            print: self.config.print_filter,
        }
    }

    /// Hands the current entries (and, in folder view, the folders' mosaics) to the gallery.
    fn refresh_gallery(&mut self, reset: bool) {
        if !matches!(self.view, View::List { .. }) {
            self.gallery.set_list_counts(HashMap::new());
        }
        let entries = self.entries().to_vec();
        let mut mosaics = HashMap::new();
        let fp = self.config.folder_previews;
        self.gallery.cycle_previews = fp.enabled && fp.cycle;
        if matches!(self.view, View::Folder) {
            if reset || self.folder_scan.folder() != self.nav.current() {
                self.folder_scan.reset(self.nav.current());
            }
            // More than four candidates only when they are cycled through.
            let limit = if fp.enabled && fp.cycle { 8 } else { 4 };
            for (i, e) in entries.iter().enumerate().filter(|(_, e)| e.is_dir() || e.kind == Kind::Archive) {
                let mut preview = match &self.index {
                    Some(index) => index.folder_preview(&e.path, limit).unwrap_or_default(),
                    None => Vec::new(),
                };
                if preview.is_empty() && fp.enabled && e.is_dir() {
                    preview = self.folder_scan.get_or_request(&e.path, fp.depth, limit).cloned().unwrap_or_default();
                }
                if self.config.only_models {
                    preview.retain(|f| f.kind.is_printable());
                }
                if !preview.is_empty() {
                    mosaics.insert(i, preview);
                }
            }
        }
        // Search hits, subfolder views and archive contents come from several folders.
        self.gallery.show_folders =
            !matches!(self.view, View::Folder) || meshgrove_core::archive::is_archive_file(self.nav.current());
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
        if matches!(self.view, View::List { .. }) {
            self.view = View::Folder;
        }
        if matches!(self.view, View::Search(_)) || self.scope == Scope::Library {
            self.search.clear();
            self.search_changed = None;
            self.view = View::Folder;
        }
    }

    fn navigate(&mut self, path: PathBuf) {
        self.leave_search();
        if path == self.nav.current() && matches!(self.view, View::Duplicates { .. } | View::List { .. }) {
            // The folder is already read; going "there" means leaving the duplicates or the list.
            self.view = View::Folder;
            self.on_listing_arrived();
            return;
        }
        self.nav.open(path);
    }

    fn open_entry(&mut self, index: usize) {
        let Some(entry) = self.entries().get(index).cloned() else { return };
        if entry.is_dir() || entry.kind == Kind::Archive {
            self.navigate(entry.path);
            return;
        }
        if meshes::viewable(entry.kind) {
            // As set in the settings, where that program opens the model; else its own window.
            let kind = entry.kind;
            match self.config.double_click {
                DoubleClick::Preview => {
                    if self.gallery.selected != Some(index) {
                        self.gallery.select(Some(index));
                    }
                    self.open_preview();
                }
                DoubleClick::Slicer if !self.slicers.is_empty() && kind.is_sliceable() => {
                    self.open_in_slicer(vec![entry.path]);
                }
                DoubleClick::Editor if self.editor.as_ref().is_some_and(|e| e.opens(kind)) => {
                    self.open_in_editor(vec![entry.path]);
                }
                DoubleClick::DefaultApp => self.open_with_default_app(&entry),
                _ => self.open_model_window(entry),
            }
            return;
        }
        self.open_with_default_app(&entry);
    }

    /// Shows a model in a window of its own; one that is already open comes to the front.
    fn open_model_window(&mut self, entry: Entry) {
        match self.model_windows.iter_mut().find(|w| w.path() == entry.path) {
            Some(window) => window.raise(),
            None => {
                let mut window = ModelWindow::new(entry);
                if let Some(hook) = &mut self.screenshot {
                    if !hook.measure.is_empty() {
                        window.script_measure(std::mem::take(&mut hook.measure));
                    }
                    if let Some(view) = hook.window_view.take() {
                        window.script_view(&view);
                    }
                }
                self.model_windows.push(window);
            }
        }
    }

    fn open_with_default_app(&mut self, entry: &Entry) {
        // Files inside archives are extracted first so other programs can open them.
        match meshgrove_core::archive::materialize(&entry.path) {
            Ok(file) => {
                if let Err(e) = actions::open_with_default_app(&file) {
                    log::warn!("cannot open {}: {e}", file.display());
                }
            }
            Err(e) => log::warn!("cannot extract {}: {e}", entry.path.display()),
        }
    }

    /// Hands the models among `files` to the default slicer (several at once if it allows).
    fn open_in_slicer(&mut self, files: Vec<PathBuf>) {
        self.open_in_slicer_with(files, self.default_slicer);
    }

    /// Hands the models among `files` to the slicer number `index` of the list.
    fn open_in_slicer_with(&mut self, files: Vec<PathBuf>, index: usize) {
        let models: Vec<PathBuf> =
            files.into_iter().filter(|p| Kind::from_file_name(&listing::file_name(p)).is_sliceable()).collect();
        let Some(slicer) = self.slicers.get(index) else { return };
        if models.is_empty() {
            return;
        }
        let message = match actions::open_in_slicer(slicer, &models) {
            Ok(actions::SlicerOpened::Started) => t!("toast.slicer_started", name = slicer.name, count = models.len()),
            Ok(actions::SlicerOpened::HandedOver) => {
                t!("toast.slicer_handed_over", name = slicer.name, count = models.len())
            }
            Err(e) => t!("toast.slicer_failed", name = slicer.name, error = e.to_string()),
        };
        self.toast(message.into_owned());
    }

    /// Opens the models among `files` that the mesh editor reads.
    fn open_in_editor(&mut self, files: Vec<PathBuf>) {
        let Some(editor) = &self.editor else { return };
        let models: Vec<PathBuf> =
            files.into_iter().filter(|p| editor.opens(Kind::from_file_name(&listing::file_name(p)))).collect();
        if models.is_empty() {
            return;
        }
        let message = match actions::open_in_editor(editor, &models) {
            Ok(()) => t!("toast.slicer_started", name = editor.name, count = models.len()),
            Err(e) => t!("toast.slicer_failed", name = editor.name, error = e.to_string()),
        };
        self.toast(message.into_owned());
    }

    /// Draws the model windows; handles what they ask for and forgets the closed ones.
    fn show_model_windows(&mut self, ctx: &egui::Context, rs: &egui_wgpu::RenderState) {
        if self.model_windows.is_empty() {
            return;
        }
        let mut actions = Vec::new();
        let mut cx = WindowContext {
            meshes: &mut self.meshes,
            render_state: rs,
            renderer: &self.renderer,
            bed: self.bed.as_ref(),
            slicers: SlicerList { names: &self.slicer_names, default: self.default_slicer },
            editor: self.editor.as_ref(),
            quality: self.config.preview_quality,
            can_log_prints: self.index.is_some(),
            icon: model_window::icon(crate::app_icon),
        };
        self.model_windows.retain_mut(|window| window.show(ctx, &mut cx, &mut actions));
        for action in actions {
            match action {
                WindowAction::OpenInSlicer(path, index) => self.open_in_slicer_with(vec![path], index),
                WindowAction::OpenInEditor(path) => self.open_in_editor(vec![path]),
                WindowAction::Print(path, result) => self.record_print(vec![path], result),
                WindowAction::Message(message) => self.toast(message),
            }
        }
    }

    fn open_preview(&mut self) {
        if self.selected_entry().is_some_and(|e| meshes::viewable(e.kind)) {
            self.preview_viewer.reset_view();
            self.preview_open = true;
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

    /// Reads the current folder again; the selected file stays selected. Duplicates and lists
    /// stay on screen while the folder underneath is read.
    fn reload(&mut self) {
        if matches!(self.view, View::Folder) && self.select_after_load.is_none() {
            self.select_after_load = self.selected_entry().map(|e| e.path.clone());
        }
        if matches!(self.view, View::Duplicates { .. } | View::List { .. }) {
            self.nav.refresh();
        } else {
            self.nav.reload();
        }
    }

    fn on_listing_arrived(&mut self) {
        if matches!(self.view, View::Search(_)) {
            return; // the folder changed underneath the results; it is shown when the search ends
        }
        if self.nav.refreshed() && matches!(self.view, View::Duplicates { .. } | View::List { .. }) {
            return; // likewise: only the folder underneath was read again
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
                        self.marks_dirty = true;
                    }
                    if changed.is_empty() && !first {
                        continue; // periodic safety-net scan found nothing new
                    }
                    if changed.iter().any(|d| d == self.nav.current()) {
                        self.reload();
                    } else if matches!(self.view, View::Folder) {
                        self.refresh_gallery(false); // the index may now know the folders' contents
                    }
                    if matches!(self.view, View::Duplicates { .. }) {
                        self.show_duplicates(); // files were added, changed or trashed
                    }
                }
                LibEvent::Pending(jobs) => {
                    if !jobs.is_empty() {
                        log::info!("{} library files need thumbnails/facts", jobs.len());
                    }
                    self.thumbs.set_background(jobs);
                    self.duplicates_due = Some(Instant::now());
                    self.tags_dirty = true;
                    self.marks_dirty = true;
                }
                LibEvent::UserDataChanged => {
                    self.tags_dirty = true;
                    self.marks_dirty = true;
                }
                LibEvent::Rescanned { dir, folders, changed } => {
                    let folder = listing::file_name(&dir);
                    let message = match changed {
                        0 => t!("toast.rescanned_same", folder = folder, folders = i18n::count(folders)),
                        n => t!("toast.rescanned", folder = folder, folders = i18n::count(folders), changed = n),
                    };
                    self.toast(message.into_owned());
                }
                LibEvent::Transfer(result) => {
                    let message = match result {
                        Ok(Transfer::Exported(path, n)) => {
                            t!("toast.exported", count = n, path = path.display().to_string())
                        }
                        Ok(Transfer::Imported(n)) => {
                            self.note_dirty = true;
                            t!("toast.imported", count = n)
                        }
                        Err(e) => t!("toast.transfer_failed", error = e),
                    };
                    self.toast(message.into_owned());
                }
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
            self.save_note(); // belongs to the previous selection
            self.note_dirty = true;
            self.menu = MenuContext {
                slicers: self.slicer_names.clone(),
                default_slicer: self.default_slicer,
                editor: self
                    .editor
                    .as_ref()
                    .filter(|e| paths.iter().any(|p| e.opens(Kind::from_file_name(&listing::file_name(p)))))
                    .map(|e| e.name.clone()),
                trash_blocked: actions::trash_blocked(&paths),
                can_tag: self.index.is_some() && !paths.is_empty() && paths.iter().all(|p| self.in_library(p)),
                lists: menu_lists(&self.lists),
                favorite: self.all_favorite(&paths),
                current_list: self.current_list(),
                rescan: self.index.is_some()
                    && matches!(paths.as_slice(), [p] if self.in_library(p)
                        && (p.is_dir() || Kind::from_file_name(&listing::file_name(p)) == Kind::Archive)),
            };
            self.menu_for = paths;
            self.tags_dirty = true;
        }
        if std::mem::take(&mut self.tags_dirty)
            && let Some(index) = &self.index
        {
            self.all_tags = index.all_tags().unwrap_or_default();
            self.selection_tags = index.tags_of(&self.menu_for).unwrap_or_default();
            self.selection_lists = index.lists_of(&self.menu_for).unwrap_or_default();
            self.selection_print = index.print_status(&self.menu_for).unwrap_or_default();
        }
        if self.note_edited.is_none()
            && std::mem::take(&mut self.note_dirty)
            && let Some(index) = &self.index
        {
            let (note, mixed) = index.notes_of(&self.menu_for).unwrap_or_default();
            self.note_input = note.unwrap_or_default();
            self.note_mixed = mixed;
        }
    }

    /// Re-reads lists, tile marks and tag colours after user data changed.
    fn refresh_marks(&mut self) {
        if !self.marks_dirty {
            return;
        }
        let Some(index) = &self.index else { return };
        self.marks_dirty = false;
        self.lists = index.lists().unwrap_or_default();
        self.tag_style.chosen = index.tag_colors().unwrap_or_default();
        self.gallery.tag_style = self.tag_style.clone();
        self.gallery.set_marks(index.marks().unwrap_or_default());
        self.menu.lists = menu_lists(&self.lists);
        self.menu.favorite = self.all_favorite(&self.menu_for);
        self.reapply_filter(); // the print filter follows the marks
        if let View::List { id, .. } = self.view {
            if self.lists.iter().any(|l| l.id == id) {
                self.show_list(id, false); // its contents may have changed
            } else {
                self.view = View::Folder; // deleted
                self.on_listing_arrived();
            }
        }
    }

    /// Ctrl+D and the star: favourites on, or off when every selected file already is one.
    fn toggle_favorite(&mut self) {
        if !self.menu.can_tag {
            return;
        }
        let paths = self.selected_paths();
        let on = !self.all_favorite(&paths);
        self.library.list(ListOp::Favorite(paths, on));
    }

    /// Writes a print list to the documents folder: a PDF checklist (then opened, to print it)
    /// or a CSV (then shown in the file manager).
    fn export_list(&mut self, id: i64, format: ListExport) {
        let (Some(index), Some(list)) = (&self.index, self.lists.iter().find(|l| l.id == id)) else { return };
        let cache = ThumbCache::default_location().map(ThumbCache::new);
        let rows: Vec<print_export::Row> = index
            .list_entries(id)
            .unwrap_or_default()
            .into_iter()
            .map(|e| {
                let thumb = cache
                    .as_ref()
                    .and_then(|c| c.load(&ThumbCache::key(&e.entry.path, e.entry.size, e.entry.modified, None)))
                    .and_then(Result::ok);
                print_export::Row {
                    name: e.entry.name.clone(),
                    folder: e.entry.path.parent().map(|p| p.display().to_string()).unwrap_or_default(),
                    quantity: e.quantity,
                    done: e.done,
                    size_mm: thumb.as_ref().and_then(|t| t.meta.size_mm),
                    filaments: thumb.as_ref().map(|t| t.meta.filaments.clone()).unwrap_or_default(),
                    picture: thumb.filter(|_| format == ListExport::Pdf).map(|t| t.image),
                }
            })
            .collect();
        let today = meshgrove_core::userdata::date(std::time::SystemTime::now());
        let dir = dirs::document_dir().or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("."));
        let stem = format!("{} {} {today}", t!("print.export_file"), print_export::file_name_part(&list.name));
        let copies: u32 = rows.iter().map(|r| r.quantity).sum();
        let result = match format {
            ListExport::Csv => {
                let header = print_export::CsvHeader {
                    name: &t!("print.col_name"),
                    folder: &t!("print.col_folder"),
                    quantity: &t!("print.col_quantity"),
                    done: &t!("print.col_done"),
                    size: &t!("print.col_size"),
                    filaments: &t!("print.col_filaments"),
                };
                let separator = if rust_i18n::locale().starts_with("de") { ';' } else { ',' };
                let path = dir.join(format!("{stem}.csv"));
                std::fs::write(&path, print_export::csv(&rows, &header, separator)).map(|_| path)
            }
            ListExport::Pdf => {
                let subtitle = t!("print.pdf_subtitle", date = today, models = rows.len(), copies = copies);
                let text =
                    print_export::PdfText { title: &list.name, subtitle: &subtitle, page: &t!("print.pdf_page") };
                let path = dir.join(format!("{stem}.pdf"));
                print_export::pdf(&rows, &text, print_export::font(system_ui_font()))
                    .map_err(std::io::Error::other)
                    .and_then(|bytes| std::fs::write(&path, bytes))
                    .map(|_| path)
            }
        };
        let message = match result {
            Ok(path) => {
                let shown = match format {
                    ListExport::Pdf => actions::open_with_default_app(&path),
                    ListExport::Csv => actions::show_in_file_manager(std::slice::from_ref(&path)),
                };
                if let Err(e) = shown {
                    log::warn!("could not show the export: {e}");
                }
                t!("toast.list_exported", path = path.display().to_string())
            }
            Err(e) => t!("toast.list_export_failed", error = e.to_string()),
        };
        self.toast(message.into_owned());
    }

    /// The print list shown now, if any.
    fn current_print_list(&self) -> Option<&ListInfo> {
        let View::List { id, .. } = self.view else { return None };
        self.lists.iter().find(|l| l.id == id && l.kind == ListKind::Print)
    }

    /// Logs a print of the models and G-code among `paths`; on a shown print list it counts there too.
    fn record_print(&mut self, paths: Vec<PathBuf>, result: PrintResult) {
        let models: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| self.in_library(p) && Kind::from_file_name(&listing::file_name(p)).is_printable())
            .collect();
        if models.is_empty() || self.index.is_none() {
            return;
        }
        let message = match result {
            PrintResult::Printed => t!("toast.printed", count = models.len()),
            PrintResult::Failed => t!("toast.print_failed", count = models.len()),
        };
        self.toast(message.into_owned());
        let list = self.current_print_list().map(|l| l.id);
        self.library.list(ListOp::Print(models, result, list));
    }

    /// Every one of `paths` is a favourite.
    fn all_favorite(&self, paths: &[PathBuf]) -> bool {
        !paths.is_empty() && paths.iter().all(|p| self.gallery.marks_of(p).is_some_and(|m| m.favorite))
    }

    /// The hand-made list shown now, for "remove from …".
    fn current_list(&self) -> Option<(i64, String)> {
        let View::List { id, .. } = self.view else { return None };
        self.lists.iter().find(|l| l.id == id && l.query.is_none()).map(|l| (l.id, list_title(l)))
    }

    /// Shows the files of a list; `reset` when opened (rather than refreshed).
    fn show_list(&mut self, id: i64, reset: bool) {
        let Some(index) = &self.index else { return };
        let listed = index.list_entries(id).unwrap_or_default();
        let print_list = self.lists.iter().any(|l| l.id == id && l.kind == ListKind::Print);
        let counts = if print_list {
            listed.iter().map(|e| (e.entry.path.clone(), (e.quantity, e.done))).collect()
        } else {
            HashMap::new()
        };
        let entries: Vec<Entry> = listed.into_iter().map(|e| e.entry).collect();
        if reset {
            self.leave_search();
            self.flatten = false;
        }
        self.view = View::List { id, entries };
        self.menu_for.clear(); // "remove from this list" now applies
        self.refresh_gallery(reset);
        self.gallery.set_list_counts(counts);
        self.reapply_filter(); // finished entries sink
    }

    /// Stores an edited note for the files it was typed for.
    fn save_note(&mut self) {
        if self.note_edited.take().is_some() && self.menu.can_tag {
            self.library.set_note(self.menu_for.clone(), self.note_input.clone());
            self.note_mixed = false;
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
        self.show_duplicates_without(&[]);
    }

    /// The duplicate groups, leaving out `gone`: files just moved to the trash, which the index
    /// lists until it has noticed (a moment later, when the view is built again).
    fn show_duplicates_without(&mut self, gone: &[PathBuf]) {
        let Some(index) = &self.index else { return };
        let (mut entries, mut groups) = (Vec::new(), Vec::new());
        for mut g in index.duplicate_groups().unwrap_or_default() {
            g.files.retain(|f| !gone.contains(&f.path));
            if g.files.len() < 2 {
                continue;
            }
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
            Action::ToggleFavorite => self.toggle_favorite(),
            Action::Print(result) => self.record_print(self.selected_paths(), result),
            Action::AddToList(id) => {
                let paths = self.selected_paths();
                if let Some(list) = self.lists.iter().find(|l| l.id == id) {
                    self.toast(t!("toast.list_added", count = paths.len(), list = list_title(list)).into_owned());
                }
                self.library.list(ListOp::Add(id, paths));
            }
            Action::NewList(kind, name) => {
                let paths = self.selected_paths();
                self.toast(t!("toast.list_added", count = paths.len(), list = name).into_owned());
                self.library.list(ListOp::Create(kind, name, paths));
            }
            Action::RemoveFromList(id) => self.library.list(ListOp::Remove(id, self.selected_paths())),
            Action::Open(index) => self.open_entry(index),
            Action::OpenWithDefaultApp(index) => {
                if let Some(entry) = self.entries().get(index).cloned() {
                    self.open_with_default_app(&entry);
                }
            }
            Action::Preview => self.open_preview(),
            Action::SortBy(key) => {
                if self.config.sort_by == key {
                    self.config.sort_descending = !self.config.sort_descending;
                } else {
                    self.config.sort_by = key;
                    self.config.sort_descending = false;
                }
                self.reapply_filter();
            }
            Action::Rescan => {
                if let [dir] = self.selected_paths().as_slice() {
                    self.library.rescan(dir.clone(), true);
                }
            }
            Action::ShowInFolder(index) => self.show_in_folder(index),
            Action::OpenInSlicer => self.open_in_slicer(self.selected_paths()),
            Action::OpenInSlicerWith(index) => self.open_in_slicer_with(self.selected_paths(), index),
            Action::OpenInEditor => self.open_in_editor(self.selected_paths()),
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
            Action::CopyFiles => {
                let paths = self.selected_paths();
                let (tx, ctx) = (self.messages.0.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let message = match actions::copy_files(&paths) {
                        Ok(n) => t!("toast.files_copied", count = n),
                        Err(e) => {
                            // No Wayland data-control support: at least the paths as text.
                            log::warn!("copying files failed: {e}");
                            let text: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
                            ctx.copy_text(text.join("\n"));
                            t!("toast.copied", count = paths.len())
                        }
                    };
                    let _ = tx.send(message.into_owned());
                    ctx.request_repaint();
                });
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
                SettingsChange::Slicers { list, default } => {
                    self.default_slicer = Config::default_slicer_index(&list, default.as_deref());
                    self.slicer_names = list.iter().map(|s| s.name.clone()).collect();
                    self.config.slicers = Some(list.clone());
                    self.config.default_slicer = list.get(self.default_slicer).map(|s| s.name.clone());
                    self.slicers = list;
                    self.menu_for.clear();
                    let name = self
                        .slicers
                        .get(self.default_slicer)
                        .map_or_else(|| t!("settings.slicer_none").into_owned(), |s| s.name.clone());
                    self.toast(t!("toast.slicer_set", name = name).into_owned());
                }
                SettingsChange::Editor(editor) => {
                    self.config.editor = editor.clone();
                    self.editor = editor.or_else(Program::detect_editor);
                    self.menu_for.clear();
                    let name = self
                        .editor
                        .as_ref()
                        .map_or_else(|| t!("settings.editor_none").into_owned(), |e| e.name.clone());
                    self.toast(t!("toast.editor_set", name = name).into_owned());
                }
                SettingsChange::Printer(bed) => {
                    self.config.printer = bed.clone();
                    self.bed = bed.or_else(Bed::detect);
                    self.gallery.bed = self.bed.clone();
                    let name =
                        self.bed.as_ref().map_or_else(|| t!("settings.printer_none").into_owned(), |b| b.name.clone());
                    self.toast(t!("toast.printer_set", name = name).into_owned());
                }
                SettingsChange::FolderPreviews(fp) => {
                    self.config.folder_previews = fp;
                    self.folder_scan.reset(self.nav.current());
                    self.refresh_gallery(false);
                }
                SettingsChange::LockPanes(locked) => self.config.lock_panes = locked,
                SettingsChange::TagDisplay { colored, on_tiles, count, style } => {
                    self.config.colored_tags = colored;
                    self.config.tags_on_tiles = on_tiles;
                    self.config.tile_tag_count = count;
                    self.config.tile_tags = style;
                    self.tag_style.colored = colored;
                    self.gallery.tag_style = self.tag_style.clone();
                }
                SettingsChange::ResetLayout => self.layout.reset(),
                SettingsChange::DoubleClick(action) => self.config.double_click = action,
                SettingsChange::PreviewQuality(quality) => {
                    self.config.preview_quality = quality;
                    self.preview_viewer.set_quality(quality);
                }
                SettingsChange::GltfUnit(unit) => {
                    self.config.gltf_unit = unit;
                    meshgrove_core::config::set_gltf_unit(unit);
                    // New sizes: reload shown glTF models, renew their facts and previews.
                    let is_gltf = |p: &Path| Kind::from_file_name(&listing::file_name(p)) == Kind::Gltf;
                    self.meshes.forget_where(is_gltf);
                    self.library.set_roots(self.config.library_roots.clone());
                    self.refresh_gallery(false);
                    self.menu_for.clear();
                }
                SettingsChange::Export(path) => {
                    self.save_note();
                    self.library.export(path);
                }
                SettingsChange::Import(path) => self.library.import(path),
                SettingsChange::OpenBackups => {
                    if let Some(dir) = meshgrove_core::userdata::backup_dir() {
                        let _ = std::fs::create_dir_all(&dir);
                        let _ = actions::open_with_default_app(&dir);
                    }
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
        // What is gone, even if moving the rest failed.
        let gone: Vec<PathBuf> = paths.iter().filter(|p| !p.exists()).cloned().collect();
        self.library.changed(paths);
        self.duplicates_due = Some(Instant::now() + Duration::from_secs(2));
        match self.view {
            View::Duplicates { .. } => self.show_duplicates_without(&gone),
            _ => self.reload(),
        }
    }

    /// The small viewer and the large preview show the same model.
    fn set_viewer_mesh(&mut self, mesh: Option<(std::sync::Arc<meshgrove_render::GpuMesh>, MeshSource)>) {
        self.preview_viewer.set_mesh(mesh.clone());
        self.viewer.set_mesh(mesh);
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
            self.set_viewer_mesh(None);
            self.model = ModelStatus::Idle;
            return;
        };
        let waited = self.selection_changed.elapsed();
        if waited < VIEWER_DEBOUNCE {
            ctx.request_repaint_after(VIEWER_DEBOUNCE - waited);
            self.set_viewer_mesh(None);
            self.model = ModelStatus::Loading;
            return;
        }
        match self.meshes.request(&path, kind, self.viewer_plate) {
            MeshState::Loading => {
                self.set_viewer_mesh(None);
                self.model = ModelStatus::Loading;
            }
            MeshState::Failed(e) => {
                let e = e.clone();
                self.set_viewer_mesh(None);
                self.model = ModelStatus::Failed(e);
            }
            MeshState::Ready(loaded) => {
                let loaded = loaded.clone();
                let source = MeshSource { path: path.clone(), kind, plate: self.viewer_plate };
                self.set_viewer_mesh(Some((loaded.mesh, source)));
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
        // The mouse's back / forward buttons (4 and 5), as in a file manager.
        let (mouse_back, mouse_forward) = ctx.input(|i| {
            (
                i.pointer.button_pressed(egui::PointerButton::Extra1),
                i.pointer.button_pressed(egui::PointerButton::Extra2),
            )
        });
        if mouse_back && !self.preview_open {
            self.leave_search();
            self.nav.back();
        }
        if mouse_forward && !self.preview_open {
            self.leave_search();
            self.nav.forward();
        }
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
        if self.preview_open && pressed(Modifiers::NONE, Key::Escape) {
            self.preview_open = false;
        }
        if !typing && pressed(Modifiers::NONE, Key::Space) {
            if self.preview_open {
                self.preview_open = false;
            } else {
                self.open_preview();
            }
        }
        if !typing && pressed(Modifiers::NONE, Key::Delete) {
            let paths = self.selected_paths();
            self.request_trash(paths);
        }
        if pressed(Modifiers::COMMAND, Key::Comma) {
            self.settings.open(self.editor.as_ref(), self.bed.as_ref());
        }
        if pressed(Modifiers::COMMAND, Key::Num1) {
            self.config.gallery_view = GalleryView::Grid;
        }
        if pressed(Modifiers::COMMAND, Key::Num2) {
            self.config.gallery_view = GalleryView::List;
        }
        if pressed(Modifiers::NONE, Key::F5) {
            self.reload();
            self.sidebar.refresh();
            match self.view {
                View::Duplicates { .. } => self.show_duplicates(),
                View::List { id, .. } => self.show_list(id, false),
                _ => {}
            }
            // The index too: what the watcher missed (copied on another system, a network share).
            let current = self.nav.current().to_path_buf();
            if matches!(self.view, View::Folder) && self.in_library(&current) {
                self.library.rescan(current, false);
            }
        }
        if pressed(Modifiers::COMMAND | Modifiers::SHIFT, Key::F) {
            self.set_scope(Scope::Library);
            self.focus_search = true;
        }
        if pressed(Modifiers::COMMAND, Key::D) {
            self.toggle_favorite();
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
                self.reload();
                self.sidebar.refresh();
            }
            ui.separator();

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(RichText::new("⚙").size(16.0)).on_hover_text(t!("settings.open")).clicked() {
                    self.settings.open(self.editor.as_ref(), self.bed.as_ref());
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
        let models_only = self.config.only_models;
        let mut flatten = self.flatten;
        ui.menu_button(format!("{} ▾", t!("view.menu")), |ui| {
            ui.radio_value(&mut self.config.gallery_view, GalleryView::Grid, t!("view.grid"));
            ui.radio_value(&mut self.config.gallery_view, GalleryView::List, t!("view.list"));
            ui.separator();
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
            ui.label(RichText::new(t!("print.filter")).weak());
            for (filter, name) in [
                (PrintFilter::All, t!("print.filter_all")),
                (PrintFilter::Unprinted, t!("print.filter_unprinted")),
                (PrintFilter::Printed, t!("print.filter_printed")),
                (PrintFilter::Failed, t!("print.filter_failed")),
            ] {
                changed |= ui.radio_value(&mut self.config.print_filter, filter, name).changed();
            }
            ui.separator();
            ui.label(RichText::new(t!("gallery.tile_size")).weak());
            ui.add(egui::Slider::new(&mut self.config.tile_size, TILE_RANGE).show_value(false));
        });
        if flatten != self.flatten {
            self.set_flatten(flatten);
        }
        if changed && models_only != self.config.only_models {
            self.refresh_gallery(false); // the folder tiles show models only, too
        } else if changed {
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
                    return;
                }
                if self.index.is_some() {
                    let save = egui::containers::menu::MenuButton::new(t!("lists.save_search").into_owned());
                    save.config(crate::ui::typing_menu()).ui(ui, |ui| {
                        ui.label(t!("lists.save_search_hint"));
                        let field = ui
                            .add(egui::TextEdit::singleline(&mut self.new_list_name).hint_text(t!("lists.name_hint")));
                        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        if (ui.button(t!("lists.create")).clicked() || enter) && !self.new_list_name.trim().is_empty() {
                            let name = std::mem::take(&mut self.new_list_name);
                            self.toast(t!("toast.list_created", list = name).into_owned());
                            self.library.list(ListOp::CreateSmart(name, self.search.clone()));
                            ui.close();
                        }
                    });
                }
                return;
            }
            View::List { id, entries } => {
                let (id, count) = (*id, entries.len());
                if let Some(list) = self.lists.iter().find(|l| l.id == id) {
                    ui.label(RichText::new(format!("{} {}", list_icon(list), list_title(list))).strong());
                }
                ui.weak(t!("lists.count", count = i18n::count(count)));
                if ui.button(t!("lists.close")).clicked() {
                    self.view = View::Folder;
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
            let name = listing::file_name(part); // roots ("/", "C:\") show as they are
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
                // Preview size, as in a file manager's status bar (also Ctrl + mouse wheel over the files).
                ui.spacing_mut().slider_width = 110.0;
                ui.add(egui::Slider::new(&mut self.config.tile_size, TILE_RANGE).show_value(false))
                    .on_hover_text(t!("gallery.tile_size"));
                ui.weak("🔍");
                ui.separator();
                // Right to left: list, then grid.
                for (view, icon, hint) in
                    [(GalleryView::List, "☰", t!("view.list_hint")), (GalleryView::Grid, "⊞", t!("view.grid_hint"))]
                {
                    let on = self.config.gallery_view == view;
                    if ui.add(Button::new(icon).selected(on).frame_when_inactive(false)).on_hover_text(hint).clicked() {
                        self.config.gallery_view = view;
                    }
                }
                ui.separator();
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
            (View::List { id, entries }, _) if entries.is_empty() => {
                let favorites = self.lists.iter().any(|l| l.id == *id && l.kind == ListKind::Favorites);
                Some(if favorites { t!("lists.favorites_empty") } else { t!("lists.empty") }.into_owned())
            }
            _ if self.entries().is_empty() => Some(t!("gallery.empty").into_owned()),
            _ => None,
        };
        if let Some(message) = message {
            ui.centered_and_justified(|ui| ui.weak(message));
            return;
        }
        let zoom = ui.input(|i| i.zoom_delta());
        if zoom != 1.0 && ui.rect_contains_pointer(ui.max_rect()) {
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
        let output = match self.config.gallery_view {
            GalleryView::Grid => self.gallery.show(ui, entries, self.config.tile_size, &self.thumbs, &self.menu),
            GalleryView::List => self.gallery.show_list(
                ui,
                entries,
                self.config.tile_size,
                &self.thumbs,
                &self.menu,
                ListSettings {
                    columns: &mut self.config.list_columns,
                    sort: self.config.sort_by,
                    descending: self.config.sort_descending,
                },
            ),
        };
        if let Some(action) = output.action {
            self.handle_action(ui.ctx(), action);
        }
        self.turntable.show(ui, output.hovered, output.viewport, &mut self.meshes, rs, &self.renderer);
    }
}

static PREVIEW_BACKDROP: std::sync::LazyLock<egui::Id> =
    std::sync::LazyLock::new(|| egui::Id::unique("preview_backdrop"));
static PREVIEW_PANEL: std::sync::LazyLock<egui::Id> = std::sync::LazyLock::new(|| egui::Id::unique("preview_panel"));

impl App {
    fn pane_places(&mut self, ui: &mut Ui) {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        let active_tags =
            if matches!(self.view, View::Search(_)) { query::parse(&self.search).tags } else { Vec::new() };
        let info = LibraryInfo {
            tags: &self.all_tags,
            duplicate_groups: self.duplicate_groups,
            showing_duplicates: matches!(self.view, View::Duplicates { .. }),
            lists: &self.lists,
            showing_list: match self.view {
                View::List { id, .. } => Some(id),
                _ => None,
            },
            active_tags: &active_tags,
            has_index: self.index.is_some(),
            tag_style: &self.tag_style,
        };
        match self.sidebar.show(ui, &self.config, self.nav.current(), info) {
            Some(SidebarAction::Navigate(path)) => self.navigate(path),
            Some(SidebarAction::AddRoot(path)) => {
                if !self.config.library_roots.contains(&path) {
                    self.config.library_roots.push(path);
                    self.save_config();
                    self.library.set_roots(self.config.library_roots.clone());
                }
            }
            Some(SidebarAction::Rescan(path)) => self.library.rescan(path, true),
            Some(SidebarAction::RemoveRoot(path)) => {
                self.config.library_roots.retain(|r| r != &path);
                self.save_config();
                self.library.set_roots(self.config.library_roots.clone());
            }
            Some(SidebarAction::SearchTags(tags)) => {
                self.scope = Scope::Library;
                let terms: Vec<String> = tags
                    .iter()
                    .map(|t| if t.contains(' ') { format!("tag:\"{t}\"") } else { format!("tag:{t}") })
                    .collect();
                self.search = terms.join(" ");
                self.run_search();
            }
            Some(SidebarAction::ShowDuplicates) => self.show_duplicates(),
            Some(SidebarAction::OpenList(id)) => self.show_list(id, true),
            Some(SidebarAction::CreateList(kind, name)) => {
                self.toast(t!("toast.list_created", list = name).into_owned());
                self.library.list(ListOp::Create(kind, name, Vec::new()));
            }
            Some(SidebarAction::RenameList(id, name)) => self.library.list(ListOp::Rename(id, name)),
            Some(SidebarAction::DeleteList(id)) => self.library.list(ListOp::Delete(id)),
            Some(SidebarAction::TagColor(tag, color)) => self.library.list(ListOp::TagColor(tag, color)),
            Some(SidebarAction::ArchiveList(id, archived)) => {
                let message = if archived { t!("toast.list_finished") } else { t!("toast.list_reopened") };
                self.toast(message.into_owned());
                self.library.list(ListOp::Archive(id, archived));
            }
            Some(SidebarAction::ExportList(id, format)) => self.export_list(id, format),
            None => {}
        }
    }

    fn details_context<'a>(&'a mut self, rs: &'a egui_wgpu::RenderState, focus_tags: bool) -> DetailsContext<'a> {
        let entries = view_entries(&self.view, &self.nav);
        let entry = self.gallery.selected.and_then(|i| entries.get(i));
        let thumb = self.gallery.selected.and_then(|i| self.gallery.thumb(i));
        let picked = self.gallery.selection();
        let selection = (picked.len(), picked.iter().filter_map(|&i| entries.get(i)).map(|e| e.size).sum());
        // On a shown print list: its name and the selected file's copies.
        let on_list = match (&self.view, entry) {
            (View::List { id, .. }, Some(e)) => self
                .lists
                .iter()
                .find(|l| l.id == *id && l.kind == ListKind::Print)
                .zip(self.gallery.list_counts_of(&e.path))
                .map(|(list, (wanted, done))| (list.name.as_str(), wanted, done)),
            _ => None,
        };
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
            note: NoteEditor { enabled: self.menu.can_tag, text: &mut self.note_input, mixed: self.note_mixed },
            bed: self.bed.as_ref(),
            slicers: SlicerList { names: &self.slicer_names, default: self.default_slicer },
            editor: self.editor.as_ref(),
            lists: ListsOfSelection { lists: &self.selection_lists, favorite: self.menu.favorite },
            print: PrintInfo { enabled: self.menu.can_tag, status: &self.selection_print, on_list },
            tag_style: &self.tag_style,
            thumb,
            model: &self.model,
            plates: self.plates.as_ref().map_or(&[][..], |(_, p)| p.as_slice()),
            viewer: &mut self.viewer,
            text: &mut self.text,
            render_state: rs,
            renderer: &self.renderer,
        }
    }

    /// The large model view over (almost) the whole window; click outside, Esc or Space closes it.
    /// At first only the model shows; ☰ adds the file name, the viewer's tools and the hints.
    fn preview_overlay(&mut self, ctx: &egui::Context, rs: &egui_wgpu::RenderState) {
        if !self.preview_open {
            return;
        }
        let Some(name) =
            self.selected_entry().filter(|e| meshes::viewable(e.kind)).map(|e| listing::file_name(&e.path))
        else {
            self.preview_open = false;
            return;
        };
        let controls = self.config.preview_controls;
        self.preview_viewer.set_tools(controls);
        let mut toggle_controls = false;
        let screen = ctx.content_rect();
        let mut close = false;
        egui::Area::new(*PREVIEW_BACKDROP).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
            let response = ui.allocate_rect(screen, egui::Sense::click());
            ui.painter().rect_filled(screen, 0.0, egui::Color32::from_black_alpha(190));
            close |= response.clicked();
        });
        let panel = screen.shrink(20.0);
        let mut message = None;
        egui::Area::new(*PREVIEW_PANEL).order(egui::Order::Foreground).fixed_pos(panel.min).show(ctx, |ui| {
            egui::Frame::new()
                .fill(ui.visuals().panel_fill)
                .corner_radius(8.0)
                .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                .inner_margin(10.0)
                .show(ui, |ui| {
                    ui.set_min_size(panel.size() - egui::vec2(20.0, 20.0));
                    ui.set_max_size(panel.size() - egui::vec2(20.0, 20.0));
                    if controls {
                        ui.horizontal(|ui| {
                            ui.add(egui::Label::new(RichText::new(name).heading().strong()).truncate());
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button("✕").on_hover_text(t!("preview.close")).clicked() {
                                    close = true;
                                }
                            });
                        });
                    }
                    let hint = if controls { ui.text_style_height(&egui::TextStyle::Small) + 6.0 } else { 0.0 };
                    let mut size = ui.available_size();
                    size.y = (size.y - hint).max(80.0);
                    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                    match &self.model {
                        ModelStatus::Loading => {
                            child.centered_and_justified(|ui| ui.spinner());
                        }
                        ModelStatus::Failed(e) => {
                            child.centered_and_justified(|ui| ui.colored_label(ui.visuals().error_fg_color, e));
                        }
                        _ => message = self.preview_viewer.ui(&mut child, rs, &self.renderer),
                    }
                    let corner =
                        egui::Rect::from_min_size(rect.right_top() + egui::vec2(-30.0, 2.0), egui::vec2(28.0, 24.0));
                    let tip = if controls { t!("preview.controls_hide") } else { t!("preview.controls_show") };
                    toggle_controls = ui.put(corner, egui::Button::new("☰")).on_hover_text(tip).clicked();
                    if controls {
                        ui.add(egui::Label::new(RichText::new(t!("preview.hint")).weak()).truncate());
                    }
                });
        });
        if let Some(message) = message {
            self.toast(message);
        }
        if toggle_controls {
            self.config.preview_controls = !controls;
            self.save_config();
        }
        if close {
            self.preview_open = false;
        }
    }

    fn apply_details_output(&mut self, out: DetailsOutput) {
        if out.toggle_favorite {
            self.toggle_favorite();
        }
        if let Some(id) = out.open_list {
            self.show_list(id, true);
        }
        if let Some(result) = out.print {
            self.record_print(self.selected_paths(), result);
        }
        if out.undo_print {
            self.toast(t!("toast.print_undone").into_owned());
            self.library.list(ListOp::UndoPrint(self.selected_paths()));
        }
        if let (Some(quantity), Some(list)) = (out.quantity, self.current_print_list()) {
            self.library.list(ListOp::Quantity(list.id, self.selected_paths(), quantity));
        }
        if out.clicked_plate.is_some() {
            self.viewer_plate = out.clicked_plate;
        }
        if let Some(tag) = out.add_tag {
            self.library.tag(self.menu_for.clone(), tag);
        }
        if let Some(tag) = out.remove_tag {
            self.library.untag(self.menu_for.clone(), tag);
        }
        if out.note_changed {
            self.note_edited = Some(Instant::now());
        }
        if let Some(message) = out.message {
            self.toast(message);
        }
        if let Some(index) = out.open_in_slicer {
            self.open_in_slicer_with(self.selected_paths(), index);
        }
        if out.open_in_editor {
            self.open_in_editor(self.selected_paths());
        }
        if let Some(path) = out.open_file
            && let Err(e) = actions::open_with_default_app(&path)
        {
            log::warn!("cannot open {}: {e}", path.display());
        }
    }
}

/// Three dots centred in the pane's header bar; dragging them moves the pane like its tab does.
/// Returns whether a drag started on them.
fn grip_handle(ui: &Ui) -> bool {
    const BAR: f32 = 24.0;
    let pane = ui.max_rect();
    if pane.width() < 200.0 {
        return false;
    }
    let bar = egui::Rect::from_center_size(egui::pos2(pane.center().x, pane.top() - BAR / 2.0), egui::vec2(36.0, BAR));
    // The pane's Ui is clipped to the pane, so paint and listen on the whole screen instead.
    let ctx = ui.ctx();
    let (hovered, pressed) =
        ctx.input(|i| (i.pointer.hover_pos().is_some_and(|p| bar.contains(p)), i.pointer.primary_pressed()));
    let color = if hovered { ui.visuals().widgets.hovered.fg_stroke.color } else { ui.visuals().weak_text_color() };
    if hovered {
        ctx.set_cursor_icon(egui::CursorIcon::Grab);
    }
    let painter = egui::Painter::new(ctx.clone(), ui.layer_id(), ctx.content_rect());
    let line = egui::Stroke::new(1.0, ui.visuals().widgets.inactive.bg_fill);
    painter.hline(pane.x_range(), pane.top() - 0.5, line);
    for i in -1..=1 {
        painter.circle_filled(bar.center() + egui::vec2(i as f32 * 6.0, 0.0), 1.5, color);
    }
    hovered && pressed
}

/// Draws the panes of the dockable layout with the app's state.
struct PaneHost<'a> {
    app: &'a mut App,
    rs: &'a egui_wgpu::RenderState,
    /// Panes can only be resized: nothing to drag, and single panes have no header.
    locked: bool,
    reset: bool,
}

impl egui_tiles::Behavior<Pane> for PaneHost<'_> {
    fn pane_ui(&mut self, ui: &mut Ui, _tile_id: egui_tiles::TileId, pane: &mut Pane) -> egui_tiles::UiResponse {
        let rs = self.rs;
        let grip = !self.locked && grip_handle(ui);
        ui.shrink_clip_rect(ui.max_rect());
        // The margins the side and central panels had before the panes.
        let margin = if *pane == Pane::Files { egui::Margin::same(8) } else { egui::Margin::symmetric(8, 2) };
        egui::Frame::new().inner_margin(margin).show(ui, |ui| match pane {
            Pane::Places => self.app.pane_places(ui),
            Pane::Files => self.app.central(ui, rs),
            Pane::Preview => {
                let cx = self.app.details_context(rs, false);
                let out = details::preview(ui, cx);
                self.app.apply_details_output(out);
            }
            Pane::Info => {
                let focus = std::mem::take(&mut self.app.focus_tags);
                let cx = self.app.details_context(rs, focus);
                let out = details::info(ui, cx);
                self.app.apply_details_output(out);
            }
        });
        if grip { egui_tiles::UiResponse::DragStarted } else { egui_tiles::UiResponse::None }
    }

    fn tab_bar_hline_stroke(&self, visuals: &egui::Visuals) -> egui::Stroke {
        egui::Stroke::new(1.0, visuals.widgets.inactive.bg_fill)
    }

    fn tab_bar_color(&self, visuals: &egui::Visuals) -> egui::Color32 {
        visuals.panel_fill
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        pane.title().into()
    }

    fn is_tab_closable(&self, _tiles: &egui_tiles::Tiles<Pane>, _tile_id: egui_tiles::TileId) -> bool {
        false
    }

    fn is_tile_draggable(&self, _tiles: &egui_tiles::Tiles<Pane>, _tile_id: egui_tiles::TileId) -> bool {
        !self.locked
    }

    /// Stacked panes keep their tabs when locked, or one of them could not be reached any more.
    fn is_tab_bar_visible(&self, _tile_id: egui_tiles::TileId, tabs: &egui_tiles::Tabs) -> bool {
        !self.locked || tabs.children.len() > 1
    }

    fn simplification_options(&self) -> egui_tiles::SimplificationOptions {
        egui_tiles::SimplificationOptions { all_panes_must_have_tabs: true, ..Default::default() }
    }

    fn top_bar_right_ui(
        &mut self,
        tiles: &egui_tiles::Tiles<Pane>,
        ui: &mut Ui,
        _tile_id: egui_tiles::TileId,
        tabs: &egui_tiles::Tabs,
        _scroll_offset: &mut f32,
    ) {
        let holds_files =
            tabs.children.iter().any(|&c| matches!(tiles.get(c), Some(egui_tiles::Tile::Pane(Pane::Files))));
        if holds_files && ui.small_button("↺").on_hover_text(t!("pane.reset")).clicked() {
            self.reset = true;
        }
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
        while let Ok(message) = self.messages.1.try_recv() {
            self.toast(message);
        }
        self.library_events();
        self.update_selection_facts();
        self.refresh_marks();
        self.gallery.tags_on_tiles = self.config.tags_on_tiles;
        self.gallery.tile_tag_count = usize::from(self.config.tile_tag_count);
        // Dots only say something in colour.
        self.gallery.tag_dots = self.config.tile_tags == TileTags::Dots && self.config.colored_tags;
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
        if let Some(edited) = self.note_edited {
            if edited.elapsed() >= NOTE_DEBOUNCE {
                self.save_note();
            } else {
                ctx.request_repaint_after(NOTE_DEBOUNCE);
            }
        }
        if self.folder_scan.poll() {
            self.scan_pending = true;
        }
        if self.scan_pending {
            // Several folders finish close together: redraw them in one go.
            if self.scan_refreshed.elapsed() >= Duration::from_millis(300) {
                self.scan_pending = false;
                self.scan_refreshed = Instant::now();
                self.refresh_gallery(false);
            } else {
                ctx.request_repaint_after(Duration::from_millis(300));
            }
        }
        self.meshes.begin_frame();
        self.update_viewer(&ctx);
        let selected = self.selected_entry().cloned();
        self.text.follow(selected.as_ref(), &ctx);
        if let Some(hook) = &mut self.screenshot {
            if std::mem::take(&mut hook.settings) {
                self.settings.open(self.editor.as_ref(), self.bed.as_ref());
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

        let fill = ui.visuals().panel_fill;
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(fill)).show(ui, |ui| {
            let mut tree = std::mem::replace(&mut self.layout.tree, Tree::empty(egui::Id::unique("layout")));
            let locked = self.config.lock_panes;
            let mut host = PaneHost { app: self, rs: &rs, locked, reset: false };
            tree.ui(&mut host, ui);
            let reset = host.reset;
            self.layout.tree = tree;
            self.layout.drawn();
            if reset {
                self.layout.reset();
            } else if ctx.input(|i| i.pointer.any_released()) {
                // Moving a pane or a divider ends with the button let go.
                self.layout.save();
            }
        });

        self.preview_overlay(&ctx, &rs);

        let info = AppInfo { gpu: &self.gpu_name, db_path: &self.db_path };
        let changes = self.settings.show(
            &ctx,
            &self.config,
            (&self.slicers, self.default_slicer),
            self.editor.as_ref(),
            self.bed.as_ref(),
            info,
        );
        self.apply_settings(changes);
        self.show_model_windows(&ctx, &rs);
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        if let Some(hook) = &mut self.screenshot {
            hook.inject(raw);
        }
    }

    fn on_exit(&mut self) {
        self.layout.save();
        self.save_note();
        self.library.flush(Duration::from_secs(3));
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
        View::List { entries, .. } => entries,
    }
}

/// Lists files can be put on from the menus: hand-made, not archived, not the favourites.
fn menu_lists(lists: &[ListInfo]) -> Vec<(i64, ListKind, String)> {
    lists
        .iter()
        .filter(|l| l.kind != ListKind::Favorites && l.query.is_none() && !l.archived)
        .map(|l| (l.id, l.kind, l.name.clone()))
        .collect()
}

/// The symbol in front of a list's name.
fn list_icon(list: &ListInfo) -> &'static str {
    match list.kind {
        ListKind::Favorites => "★",
        ListKind::Print => "🖨",
        ListKind::Collection if list.query.is_some() => "🔍",
        ListKind::Collection => "📁",
    }
}

/// A list's name as shown (the favourites have a translated one).
fn list_title(list: &ListInfo) -> String {
    match list.kind {
        ListKind::Favorites => t!("lists.favorites").into_owned(),
        _ => list.name.clone(),
    }
}

/// Use the desktop's UI font (Noto Sans on KDE) so the app does not look foreign;
/// glyphs it lacks still fall back to egui's bundled fonts and the system fallback.
fn use_system_ui_font(ctx: &egui::Context) {
    let Some(path) = system_ui_font() else { return };
    let Ok(bytes) = std::fs::read(&path) else { return };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("system-ui".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.insert(0, "system-ui".into());
    }
    ctx.set_fonts(fonts);
    log::info!("UI font: {}", path.display());
}

#[cfg(target_os = "linux")]
fn system_ui_font() -> Option<PathBuf> {
    let out = std::process::Command::new("fc-match").args(["-f", "%{file}", "sans-serif:style=Regular"]).output();
    Some(PathBuf::from(String::from_utf8_lossy(&out.ok()?.stdout).into_owned()))
}

#[cfg(windows)]
fn system_ui_font() -> Option<PathBuf> {
    let font = PathBuf::from(std::env::var_os("WINDIR")?).join("Fonts").join("segoeui.ttf");
    font.is_file().then_some(font)
}

/// macOS ships its UI font (SF Pro) only as a variable font; egui's bundled font stays.
#[cfg(not(any(target_os = "linux", windows)))]
fn system_ui_font() -> Option<PathBuf> {
    None
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
