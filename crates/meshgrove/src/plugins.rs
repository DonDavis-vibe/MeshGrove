//! The user's plugins in the window: loaded as config.toml says (on a thread, compiling can take
//! a moment), their open panels — each driven by a worker thread of its own, so the window never
//! waits for a plugin —, the jobs the panels start, and what is installed, for the settings.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use meshgrove_core::ai::dispatch::ToolProvider;
use meshgrove_core::bundle::ResolvedBundle;
use meshgrove_core::config::Config;
use meshgrove_plugin_api::ui::{BundleOutcome, Effect, Event, Node, View};
use meshgrove_plugin_api::{Bundle, FileType, HostInfo, MeshData};
use meshgrove_plugins::jobs::{JobEvent, JobPool};
use meshgrove_plugins::{CancelToken, Opened, Plugin, Registry, Settings, StagedBundle, Staging};

use crate::library::{Applied, LibraryService};
use crate::ui::plugin_view::PanelUiState;
use crate::ui::settings_plugins::PluginRow;
use crate::ui::viewer::{MeshSource, ModelViewer};

/// How long closing a panel waits for its state.
const SNAPSHOT_WAIT: Duration = Duration::from_secs(3);
/// Largest file opened with a plugin.
const MAX_OPEN_FILE: u64 = 64 << 20;

/// What a panel asks of the window.
pub enum PanelAction {
    Effect {
        plugin: String,
        panel: String,
        effect: Effect,
    },
    /// Files for the library: put in at once (`apply`) or suggested; `staging` holds them.
    Bundle {
        plugin: String,
        panel: String,
        apply: bool,
        bundle: Bundle,
        staging: Result<Staging, String>,
    },
    /// A file opened with a plugin, and what the plugin asks for.
    Opened {
        plugin: String,
        effects: Result<Vec<Effect>, String>,
    },
}

enum Input {
    Event(Event),
    Render,
    Snapshot(Sender<Result<String, String>>),
}

enum Output {
    Opened { out: PathBuf, tmp: PathBuf },
    View(View),
    Effects(Vec<Effect>),
    Bundle { apply: bool, bundle: Bundle, staging: Result<Staging, String> },
    Mesh { key: String, mesh: Option<MeshData> },
    Failed(String),
}

/// A 3D view of a panel: the viewer and the mesh it shows, uploaded when it arrives.
pub struct PanelMesh {
    pub viewer: ModelViewer,
    arrived: Option<Option<MeshData>>,
    /// The middle of the mesh in the plugin's coordinates: the viewer shows it recentred there.
    centre: glam::Vec3,
}

impl PanelMesh {
    /// Shows the space the model is meant to fit, given in the plugin's coordinates.
    pub fn set_frame(&mut self, frame: Option<[[f32; 3]; 2]>) {
        let frame = frame.map(|[min, max]| {
            let (min, max) = (glam::Vec3::from(min), glam::Vec3::from(max));
            meshgrove_core::Aabb { min: min.min(max) - self.centre, max: min.max(max) - self.centre }
        });
        self.viewer.set_frame(frame);
    }
}

pub struct RunningJob {
    pub id: String,
    pub label: String,
    pub fraction: f32,
    pub message: String,
    cancel: CancelToken,
}

pub struct OpenPanel {
    pub plugin: String,
    pub panel: String,
    pub title: String,
    inputs: Sender<Input>,
    outputs: Receiver<Output>,
    /// The panel's `/out` and `/tmp`, once it is open.
    folders: Option<(PathBuf, PathBuf)>,
    /// The library files the user opened with the panel: it and its jobs may read these.
    pub opened: Opened,
    pub view: Option<View>,
    pub error: Option<String>,
    pub ui: PanelUiState,
    pub meshes: HashMap<String, PanelMesh>,
    pub jobs: Vec<RunningJob>,
    job_events: (Sender<JobEvent>, Receiver<JobEvent>),
    /// An event is on its way and the plugin has not rendered since.
    pub busy: bool,
    render_at: Option<Instant>,
}

impl OpenPanel {
    pub fn send(&mut self, event: Event) {
        self.busy = true;
        let _ = self.inputs.send(Input::Event(event));
    }

    /// The host path of a file the plugin named (`/out/…` or `/tmp/…`).
    pub fn host_path(&self, guest: &str) -> Option<PathBuf> {
        let (out, tmp) = self.folders.as_ref()?;
        let (base, rest) = match guest.split_once('/').map(|(_, r)| r)?.split_once('/') {
            Some(("out", rest)) => (out, rest),
            Some(("tmp", rest)) => (tmp, rest),
            _ => return None,
        };
        let ok = rest.split('/').all(|part| !part.is_empty() && part != "." && part != "..");
        ok.then(|| base.join(rest))
    }

    /// The panel's `/tmp`.
    pub fn tmp(&self) -> Option<&Path> {
        self.folders.as_ref().map(|(_, tmp)| tmp.as_path())
    }

    /// Shows the meshes that arrived (needs the GPU, so done while drawing).
    pub fn upload_meshes(&mut self, device: &wgpu::Device) {
        for (key, m) in &mut self.meshes {
            let Some(arrived) = m.arrived.take() else { continue };
            let Some(data) = arrived.filter(|d| d.triangles() > 0) else {
                m.viewer.set_mesh(None);
                continue;
            };
            let mesh = meshgrove_core::Mesh {
                positions: data.positions.as_chunks::<3>().0.to_vec(),
                indices: Some(data.indices.clone()),
                colors: None,
                vertex_colors: None,
            };
            let (Some(gpu), Some(bounds)) = (meshgrove_render::upload_mesh(device, &mesh), mesh.bounds()) else {
                continue;
            };
            // Measuring works in the uploaded, recentred coordinates.
            let center = bounds.center();
            let soup = mesh.to_soup().into_iter().map(|p| (glam::Vec3::from(p) - center).to_array()).collect();
            let source = MeshSource::Memory { name: key.clone(), soup: Arc::new(soup) };
            m.centre = center;
            m.viewer.update_mesh(Arc::new(gpu), source);
        }
    }
}

/// The plugins, loaded or being loaded.
pub struct PluginHost {
    base: PathBuf,
    registry: Arc<Registry>,
    rows: Vec<PluginRow>,
    loading: Option<Receiver<(Arc<Registry>, Vec<PluginRow>)>>,
    /// Loaded at least once: the panels of the saved layout can open.
    pub ready: bool,
    /// What the plugins were last told about the host.
    info: Option<HostInfo>,
    pub panels: Vec<OpenPanel>,
    jobs: Option<JobPool>,
    opened: (Sender<PanelAction>, Receiver<PanelAction>),
    ctx: egui::Context,
}

impl PluginHost {
    pub fn new(ctx: egui::Context) -> PluginHost {
        PluginHost {
            base: meshgrove_plugins::default_base().unwrap_or_else(|| PathBuf::from("plugins")),
            registry: Arc::new(Registry::default()),
            rows: Vec::new(),
            loading: None,
            ready: false,
            info: None,
            panels: Vec::new(),
            jobs: None,
            opened: unbounded(),
            ctx,
        }
    }

    pub fn base(&self) -> &Path {
        &self.base
    }

    /// Loads the plugins of `config` again, on a thread; they read the library in `db` (and the
    /// settings in the user's config.toml).
    pub fn reload(&mut self, config: &Config, info: HostInfo, db: &Path) {
        self.info = Some(info.clone());
        let (tx, rx) = bounded(1);
        let (config, base, ctx) = (config.clone(), self.base.clone(), self.ctx.clone());
        let library: Arc<dyn meshgrove_plugins::LibraryAccess> =
            Arc::new(meshgrove_plugins::LibraryReader::new(db.to_path_buf(), None));
        std::thread::Builder::new()
            .name("plugins".into())
            .spawn(move || {
                let registry = Registry::load(&config, &base, &info, Some(library));
                let rows = rows(&config, &base, &registry);
                let _ = tx.send((Arc::new(registry), rows));
                ctx.request_repaint();
            })
            .expect("a thread for loading plugins");
        self.loading = Some(rx);
    }

    /// What the plugins were last told about the host.
    pub fn info(&self) -> Option<&HostInfo> {
        self.info.as_ref()
    }

    /// Tells the open panels that language, theme or printers changed.
    pub fn host_changed(&mut self, info: &HostInfo) {
        for p in &mut self.panels {
            p.send(Event::HostChanged { info: info.clone() });
        }
    }

    /// Tells a panel what became of its bundle (it may have been closed since).
    pub fn bundle_finished(&mut self, plugin: &str, panel: &str, title: String, outcome: BundleOutcome) {
        if let Some(p) = self.find(plugin, panel) {
            p.send(Event::BundleFinished { title, outcome });
        }
    }

    /// The plugins' tools for the chat.
    pub fn tools(&self) -> Option<Arc<dyn ToolProvider>> {
        (!self.registry.plugins().is_empty()).then(|| self.registry.clone() as Arc<dyn ToolProvider>)
    }

    /// What is installed, for the settings page.
    pub fn rows(&self) -> &[PluginRow] {
        &self.rows
    }

    pub fn plugin(&self, id: &str) -> Option<&Arc<Plugin>> {
        self.registry.get(id)
    }

    /// The panels of the loaded plugins: (plugin, panel, title, plugin name).
    pub fn panel_defs(&self) -> Vec<(String, String, String, String)> {
        let mut out = Vec::new();
        for p in self.registry.plugins() {
            for def in &p.manifest().panels {
                out.push((p.id().to_owned(), def.id.clone(), def.title.clone(), p.manifest().name.clone()));
            }
        }
        out
    }

    /// The loaded plugins that open the files named `names` (one file, or a selection every
    /// file of which fits): (id, name, the file type that takes them).
    pub fn openers(&self, names: &[&str]) -> Vec<(String, String, FileType)> {
        self.registry
            .plugins()
            .iter()
            .filter_map(|p| {
                let opener = p.manifest().opener(names)?;
                Some((p.id().to_owned(), p.manifest().name.clone(), opener.clone()))
            })
            .collect()
    }

    pub fn open_keys(&self) -> Vec<(String, String)> {
        self.panels.iter().map(|p| (p.plugin.clone(), p.panel.clone())).collect()
    }

    pub fn find(&mut self, plugin: &str, panel: &str) -> Option<&mut OpenPanel> {
        self.panels.iter_mut().find(|p| p.plugin == plugin && p.panel == panel)
    }

    fn state_file(&self, plugin: &str, panel: &str) -> PathBuf {
        self.base.join(plugin).join("state").join(format!("{panel}.json"))
    }

    /// Where the ids of the library files opened with a panel are kept.
    fn opened_file(&self, plugin: &str, panel: &str) -> PathBuf {
        self.base.join(plugin).join("state").join(format!("{panel}.opened.json"))
    }

    /// Hands library files the user opened with a plugin (by their ids) to its panel, opening
    /// it if need be. From now on the panel and its jobs may read exactly these files.
    pub fn open_models(&mut self, plugin: &str, panel: &str, ids: Vec<String>) -> Result<(), String> {
        self.open(plugin, panel, None)?;
        let file = self.opened_file(plugin, panel);
        let open = self.find(plugin, panel).ok_or("the panel is closed")?;
        open.opened.add(&ids);
        if let Err(e) = open.opened.save(&file) {
            log::warn!("keeping what was opened with {plugin}/{panel}: {e}");
        }
        open.send(Event::ModelsOpened { ids });
        Ok(())
    }

    /// Opens a panel with `state`, or as it was closed last; one already open starts again
    /// with `state` (a file opened with it), or stays as it is.
    pub fn open(&mut self, plugin: &str, panel: &str, state: Option<String>) -> Result<(), String> {
        if self.find(plugin, panel).is_some() {
            if state.is_none() {
                return Ok(());
            }
            self.panels.retain(|p| !(p.plugin == plugin && p.panel == panel));
        }
        let p = self.registry.get(plugin).ok_or_else(|| t!("plugins.not_loaded", id = plugin).into_owned())?.clone();
        let title = p.manifest().panels.iter().find(|d| d.id == panel).map(|d| d.title.clone());
        let title = title.ok_or_else(|| format!("{plugin} has no panel {panel}"))?;
        let state = state.or_else(|| std::fs::read_to_string(self.state_file(plugin, panel)).ok());
        // What the user opened with the panel before stays readable for it.
        let opened = Opened::load(&self.opened_file(plugin, panel));
        let (inputs, input_rx) = unbounded();
        let (output_tx, outputs) = unbounded();
        let (name, ctx, reads) = (panel.to_owned(), self.ctx.clone(), opened.clone());
        std::thread::Builder::new()
            .name(format!("plugin-ui-{plugin}"))
            .spawn(move || work(&p, &name, state, reads, &input_rx, &output_tx, &ctx))
            .map_err(|e| e.to_string())?;
        self.panels.push(OpenPanel {
            plugin: plugin.to_owned(),
            panel: panel.to_owned(),
            title,
            inputs,
            outputs,
            folders: None,
            opened,
            view: None,
            error: None,
            ui: PanelUiState::default(),
            meshes: HashMap::new(),
            jobs: Vec::new(),
            job_events: unbounded(),
            busy: true,
            render_at: None,
        });
        Ok(())
    }

    /// Closes a panel, keeping its state for next time.
    pub fn close(&mut self, plugin: &str, panel: &str) {
        let Some(i) = self.panels.iter().position(|p| p.plugin == plugin && p.panel == panel) else { return };
        let p = self.panels.remove(i);
        self.save_state(&p);
        for job in &p.jobs {
            job.cancel.cancel();
        }
    }

    /// Keeps the state of every open panel (Meshgrove closes).
    pub fn save_all(&self) {
        for p in &self.panels {
            self.save_state(p);
        }
    }

    fn save_state(&self, p: &OpenPanel) {
        if p.error.is_some() || p.view.is_none() {
            return;
        }
        let (reply, answer) = bounded(1);
        if p.inputs.send(Input::Snapshot(reply)).is_err() {
            return;
        }
        if let Ok(Ok(state)) = answer.recv_timeout(SNAPSHOT_WAIT) {
            let file = self.state_file(&p.plugin, &p.panel);
            let written =
                file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&file, state));
            if let Err(e) = written {
                log::warn!("keeping the state of {}/{}: {e}", p.plugin, p.panel);
            }
        }
    }

    /// Starts jobs of a panel, writing into its `/out`.
    pub fn start_jobs(&mut self, plugin: &str, panel: &str, jobs: Vec<meshgrove_plugin_api::ui::JobSpec>) {
        let Some(p) = self.registry.get(plugin).cloned() else { return };
        let pool = self.jobs.get_or_insert_with(JobPool::with_cores);
        let Some(open) = self.panels.iter_mut().find(|o| o.plugin == plugin && o.panel == panel) else { return };
        let Some((out, _)) = open.folders.clone() else { return };
        let labels: Vec<(String, String)> =
            jobs.iter().map(|j| (j.id.clone(), j.label.clone().unwrap_or_else(|| j.kind.clone()))).collect();
        for (id, cancel) in pool.start(&p, jobs, &out, &open.opened, &open.job_events.0) {
            let label = labels.iter().find(|(j, _)| *j == id).map(|(_, l)| l.clone()).unwrap_or_default();
            open.jobs.push(RunningJob { id, label, fraction: 0.0, message: String::new(), cancel });
        }
    }

    pub fn cancel_job(&mut self, plugin: &str, panel: &str, job: &str) {
        if let Some(open) = self.find(plugin, panel) {
            open.jobs.iter().filter(|j| j.id == job).for_each(|j| j.cancel.cancel());
        }
    }

    pub fn render_after(&mut self, plugin: &str, panel: &str, after: Duration) {
        if let Some(open) = self.find(plugin, panel) {
            open.render_at = Some(Instant::now() + after);
            self.ctx.request_repaint_after(after);
        }
    }

    /// Opens a library file with a plugin, on a thread; the effects come with [`PluginHost::poll`].
    pub fn open_file(&self, plugin: &str, path: &Path) {
        let Some(p) = self.registry.get(plugin).cloned() else { return };
        let (path, tx, ctx) = (path.to_path_buf(), self.opened.0.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let name = meshgrove_core::listing::file_name(&path);
            let effects = match std::fs::metadata(&path) {
                Ok(m) if m.len() > MAX_OPEN_FILE => Err(t!("plugins.file_too_big").into_owned()),
                Ok(_) => std::fs::read(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| p.open_file(&name, &bytes).map_err(|e| e.to_string())),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(PanelAction::Opened { plugin: p.id().to_owned(), effects });
            ctx.request_repaint();
        });
    }

    /// Takes in what arrived: loaded plugins, new views, meshes, job progress; returns what the
    /// panels ask of the window. `true` with it: the plugins were (re)loaded.
    pub fn poll(&mut self) -> (Vec<PanelAction>, bool) {
        let mut actions = Vec::new();
        let mut reloaded = false;
        if let Some(rx) = &self.loading
            && let Ok((registry, rows)) = rx.try_recv()
        {
            self.loading = None;
            self.registry = registry;
            self.rows = rows;
            self.ready = true;
            reloaded = true;
            // Panels of plugins that are gone (switched off, removed) close.
            let loaded = |id: &str| self.registry.get(id).is_some();
            self.panels.retain(|p| loaded(&p.plugin));
        }
        let mut running = false;
        for p in &mut self.panels {
            while let Ok(output) = p.outputs.try_recv() {
                match output {
                    Output::Opened { out, tmp } => p.folders = Some((out, tmp)),
                    Output::View(view) => {
                        p.ui.new_view();
                        p.view = Some(view);
                        p.busy = false;
                        p.error = None;
                    }
                    Output::Effects(effects) => actions.extend(effects.into_iter().map(|effect| PanelAction::Effect {
                        plugin: p.plugin.clone(),
                        panel: p.panel.clone(),
                        effect,
                    })),
                    Output::Bundle { apply, bundle, staging } => {
                        let (plugin, panel) = (p.plugin.clone(), p.panel.clone());
                        actions.push(PanelAction::Bundle { plugin, panel, apply, bundle, staging });
                    }
                    Output::Mesh { key, mesh } => {
                        let m = p.meshes.entry(key).or_insert_with(|| {
                            let mut viewer = ModelViewer::new();
                            viewer.set_tools(true);
                            PanelMesh { viewer, arrived: None, centre: glam::Vec3::ZERO }
                        });
                        m.arrived = Some(mesh);
                    }
                    Output::Failed(e) => {
                        p.error = Some(e);
                        p.busy = false;
                    }
                }
            }
            // Job progress: only the newest per job goes to the plugin.
            let mut progress: Vec<(String, f32, String)> = Vec::new();
            while let Ok(event) = p.job_events.1.try_recv() {
                match event {
                    JobEvent::Progress { job, fraction, message } => {
                        if let Some(j) = p.jobs.iter_mut().find(|j| j.id == job) {
                            j.fraction = fraction;
                            j.message.clone_from(&message);
                        }
                        progress.retain(|(j, _, _)| *j != job);
                        progress.push((job, fraction, message));
                    }
                    JobEvent::Finished { job, result } => {
                        p.jobs.retain(|j| j.id != job);
                        progress.retain(|(j, _, _)| *j != job);
                        let (result, error) = match result {
                            Ok(v) => (Some(v), None),
                            Err(e) => (None, Some(e.to_string())),
                        };
                        p.send(Event::JobFinished { job, result, error });
                    }
                }
            }
            for (job, fraction, message) in progress {
                p.send(Event::JobProgress { job, fraction, message });
            }
            if p.render_at.is_some_and(|at| Instant::now() >= at) {
                p.render_at = None;
                let _ = p.inputs.send(Input::Render);
            }
            running |= !p.jobs.is_empty();
        }
        if running {
            self.ctx.request_repaint_after(Duration::from_millis(100));
        }
        while let Ok(action) = self.opened.1.try_recv() {
            actions.push(action);
        }
        (actions, reloaded)
    }
}

/// Cleans the plugins folder `base` up, on a thread: staging folders that no change in the
/// journal of `db` (a redo copies from them) and no waiting suggestion names, folders a crash
/// left behind, and compiled code of another wasmtime or of plugins no longer installed
/// (`installed`: their hashes).
pub fn sweep(base: PathBuf, db: PathBuf, installed: Vec<String>) {
    std::thread::Builder::new()
        .name("plugin-sweep".into())
        .spawn(move || {
            let cache = meshgrove_plugins::engine::trim_cache(&base.join(".cache"), &installed);
            // Without the journal nothing can be told apart: the staging folders stay.
            let Some(lib) = meshgrove_core::db::Library::open_reader(&db) else { return };
            let store = meshgrove_core::ai::proposals::ProposalStore::default_location()
                .filter(|p| p.exists())
                .map(|p| meshgrove_core::ai::proposals::ProposalStore::open(&p));
            if matches!(store, Some(Err(_))) {
                return;
            }
            let store = store.and_then(Result::ok);
            let referenced = |dir: &Path| {
                let text = serde_json::to_string(dir).unwrap_or_default();
                let text = text.trim_matches('"');
                lib.journal_mentions(text).unwrap_or(true)
                    || store.as_ref().is_some_and(|s| s.waiting_mentions(text).unwrap_or(true))
            };
            let swept = meshgrove_plugins::staging::sweep(&base, &referenced, Duration::from_secs(3600));
            log::info!(
                "plugins folder cleaned up: {} staging folders, {} leftovers, {cache} compiled files",
                swept.staging,
                swept.leftovers
            );
        })
        .expect("a thread for cleaning up the plugins folder");
}

/// What is installed: every plugin of `config`, loaded or (to show what it is) read once.
fn rows(config: &Config, base: &Path, registry: &Registry) -> Vec<PluginRow> {
    config
        .plugins
        .iter()
        .map(|entry| {
            let problem = registry.problems().iter().find(|(id, _)| *id == entry.id).map(|(_, why)| why.clone());
            let loaded = registry.get(&entry.id).cloned();
            let read = match &loaded {
                Some(p) => Ok(p.clone()),
                None => Plugin::load(&entry.file, Settings::new(base)).map(Arc::new).map_err(|e| e.to_string()),
            };
            match read {
                Ok(p) => {
                    let m = p.manifest();
                    PluginRow {
                        id: entry.id.clone(),
                        name: m.name.clone(),
                        version: m.version.clone(),
                        api: m.api.clone(),
                        description: m.description.clone(),
                        file: entry.file.clone(),
                        hash: Some(p.hash().to_owned()),
                        permissions: m.permissions.clone(),
                        panels: m.panels.iter().map(|d| d.title.clone()).collect(),
                        file_types: m.file_types.iter().map(|t| t.pattern.clone()).collect(),
                        tools: p.tools().iter().map(|t| t.name.clone()).collect(),
                        loaded: loaded.is_some(),
                        problem: problem.filter(|_| entry.enabled),
                    }
                }
                Err(e) => PluginRow {
                    id: entry.id.clone(),
                    name: entry.id.clone(),
                    file: entry.file.clone(),
                    problem: Some(e),
                    ..PluginRow::default()
                },
            }
        })
        .collect()
}

/// Where a panel's bundle is to go.
pub enum BundleRoute<'a> {
    /// Into the library now; `from` (plugin, panel) comes back with the outcome.
    Apply { library: &'a LibraryService, from: (String, String) },
    /// Into the review.
    Propose(&'a dyn Fn(&ResolvedBundle) -> Result<(), String>),
}

/// Takes a panel's bundle as far as it goes here. The outcome, or `None` when it is on its way
/// to the library's thread, which answers with [`Applied`] once the change went through.
pub fn settle_bundle(
    allowed: bool,
    staged: StagedBundle,
    config: &Config,
    route: BundleRoute,
) -> Option<BundleOutcome> {
    if !allowed {
        let _ = staged.staging.remove();
        return Some(BundleOutcome::Refused);
    }
    let resolved = match staged.resolve_for(config) {
        Ok(resolved) => resolved,
        Err(e) => {
            let _ = staged.staging.remove();
            return Some(BundleOutcome::Failed { error: e.to_string() });
        }
    };
    match route {
        BundleRoute::Apply { library, from } => {
            let origin = format!("plugin:{}", staged.plugin);
            library.apply_bundle(origin, resolved.title.clone(), resolved.ops(), from);
            None
        }
        BundleRoute::Propose(propose) => Some(match propose(&resolved) {
            Ok(()) => BundleOutcome::Queued,
            Err(error) => {
                let _ = staged.staging.remove();
                BundleOutcome::Failed { error }
            }
        }),
    }
}

/// The panel to tell, the bundle's title and its outcome, if `applied` is about a bundle.
pub fn bundle_outcome(applied: &Applied) -> Option<((String, String), String, BundleOutcome)> {
    match applied {
        Applied::Bundle { title, report, from } => {
            let outcome = BundleOutcome::Applied { steps: report.applied as u32, skipped: report.skipped as u32 };
            Some((from.clone(), title.clone(), outcome))
        }
        Applied::BundleFailed { title, error, from } => {
            Some((from.clone(), title.clone(), BundleOutcome::Failed { error: error.clone() }))
        }
        _ => None,
    }
}

/// A panel's worker: opens it, then turns each event into effects, a new view and the meshes
/// whose revision changed.
fn work(
    plugin: &Arc<Plugin>,
    panel: &str,
    state: Option<String>,
    opened: Opened,
    inputs: &Receiver<Input>,
    outputs: &Sender<Output>,
    ctx: &egui::Context,
) {
    let send = |o: Output| {
        let _ = outputs.send(o);
        ctx.request_repaint();
    };
    let handle = match plugin.open_panel_with(panel, state, opened) {
        Ok(h) => h,
        Err(e) => return send(Output::Failed(e.to_string())),
    };
    send(Output::Opened { out: handle.out().to_path_buf(), tmp: handle.tmp().to_path_buf() });
    let mut revisions: HashMap<String, u64> = HashMap::new();
    let refresh = |revisions: &mut HashMap<String, u64>| match handle.render() {
        Ok(view) => {
            let wanted: Vec<(String, u64)> = view
                .root
                .walk()
                .into_iter()
                .filter_map(|n| match n {
                    Node::MeshView { key, revision, .. } if revisions.get(key) != Some(revision) => {
                        Some((key.clone(), *revision))
                    }
                    _ => None,
                })
                .collect();
            send(Output::View(view));
            for (key, revision) in wanted {
                match handle.mesh(&key) {
                    Ok(mesh) => send(Output::Mesh { key: key.clone(), mesh }),
                    Err(e) => return send(Output::Failed(e.to_string())),
                }
                revisions.insert(key, revision);
            }
        }
        Err(e) => send(Output::Failed(e.to_string())),
    };
    let on_event = |event: &Event, revisions: &mut HashMap<String, u64>| match handle.event(event) {
        Ok(effects) => {
            let mut rest = Vec::new();
            for effect in effects {
                let (apply, bundle) = match effect {
                    Effect::ApplyBundle { bundle } => (true, bundle),
                    Effect::ProposeBundle { bundle } => (false, bundle),
                    other => {
                        rest.push(other);
                        continue;
                    }
                };
                // What the panel and its jobs wrote goes with the bundle.
                let staging = handle.take_output().map_err(|e| e.to_string());
                send(Output::Bundle { apply, bundle, staging });
            }
            if !rest.is_empty() {
                send(Output::Effects(rest));
            }
            refresh(revisions);
        }
        Err(e) => send(Output::Failed(e.to_string())),
    };
    // Once before the first view, so a panel can start right away what takes longer than an
    // event may (a job), with what it is told about the host.
    on_event(&Event::HostChanged { info: plugin.settings().info.clone() }, &mut revisions);
    for input in inputs {
        match input {
            Input::Event(event) => on_event(&event, &mut revisions),
            Input::Render => refresh(&mut revisions),
            Input::Snapshot(reply) => {
                let _ = reply.send(handle.snapshot().map_err(|e| e.to_string()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example plugin built for tests (`scripts/build-plugins.sh --test-hooks`), if there is one.
    fn test_plugin() -> Option<Vec<u8>> {
        let path = std::env::var_os("MESHGROVE_TEST_PLUGIN").map(PathBuf::from).unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/plugins-test/wasm32-wasip2/release/hello_box.wasm")
        });
        match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(e) if std::env::var("MESHGROVE_REQUIRE_PLUGIN").as_deref() == Ok("1") => {
                panic!("{}: {e} (build it with scripts/build-plugins.sh --test-hooks)", path.display())
            }
            Err(_) => None,
        }
    }

    #[test]
    fn a_panel_hears_about_the_host_before_its_first_view() {
        let Some(bytes) = test_plugin() else { return };
        let base = tempfile::tempdir().unwrap();
        let mut settings = Settings::new(base.path());
        settings.info.language = "de".into();
        let plugin = Arc::new(Plugin::from_bytes(&bytes, settings).unwrap());
        let (inputs, input_rx) = unbounded();
        let (output_tx, outputs) = unbounded();
        let ctx = egui::Context::default();
        let opened = Opened::default();
        let worker = std::thread::spawn(move || work(&plugin, "box", None, opened, &input_rx, &output_tx, &ctx));
        let next = || outputs.recv_timeout(Duration::from_secs(60)).expect("the panel answers");
        assert!(matches!(next(), Output::Opened { .. }));
        match next() {
            Output::Effects(effects) => {
                assert!(matches!(&effects[..], [Effect::Toast { text, .. }] if text == "host: de"));
            }
            _ => panic!("what the panel does on hearing about the host comes first"),
        }
        assert!(matches!(next(), Output::View(_)), "then its first view");
        drop(inputs);
        worker.join().unwrap();
    }

    /// A library of one box, whoever asks.
    struct OneBox;

    impl meshgrove_plugins::LibraryAccess for OneBox {
        fn search(
            &self,
            _query: &str,
            _limit: u32,
            _read: meshgrove_plugins::LibraryRead,
            _caller: meshgrove_plugins::Caller,
        ) -> Result<serde_json::Value, String> {
            Ok(serde_json::json!([]))
        }

        fn describe(
            &self,
            id: &str,
            _read: meshgrove_plugins::LibraryRead,
            _caller: meshgrove_plugins::Caller,
        ) -> Result<serde_json::Value, String> {
            Ok(serde_json::json!({"id": id}))
        }

        fn triangles(
            &self,
            _id: &str,
            _max: u32,
            _read: meshgrove_plugins::LibraryRead,
            _caller: meshgrove_plugins::Caller,
        ) -> Result<MeshData, String> {
            Ok(MeshData::cuboid([10.0, 20.0, 30.0]))
        }
    }

    /// What a panel shows as the width of its box, once it has a view.
    fn width(host: &mut PluginHost) -> Option<f64> {
        host.poll();
        match host.find("hello-box", "box")?.view.as_ref()?.root.find("width")? {
            Node::NumberInput { value, .. } => Some(*value),
            _ => None,
        }
    }

    /// Files opened with a plugin reach its panel as ids, which it may read from then on, also
    /// after Meshgrove was closed in between; nothing was granted.
    #[test]
    fn files_opened_with_a_panel_stay_readable_after_a_restart() {
        let Some(bytes) = test_plugin() else { return };
        let dir = tempfile::tempdir().unwrap();
        let (base, file) = (dir.path().join("plugins"), dir.path().join("hello_box.wasm"));
        std::fs::write(&file, &bytes).unwrap();
        let blake3 = Plugin::from_bytes(&bytes, Settings::new(&base)).unwrap().hash().to_owned();
        let entry = meshgrove_core::config::PluginConfig {
            id: "hello-box".into(),
            file,
            blake3,
            enabled: true,
            grants: Default::default(),
            memory_mb: None,
        };
        let config = Config { plugins: vec![entry], ..Config::default() };
        let start = || {
            let mut host = PluginHost::new(egui::Context::default());
            host.base = base.clone();
            let info = Settings::new(&base).info;
            host.registry = Arc::new(Registry::load(&config, &base, &info, Some(Arc::new(OneBox))));
            assert_eq!(host.registry.problems(), [], "the plugin loads");
            host
        };
        let wait = |host: &mut PluginHost, wanted: f64| {
            let until = Instant::now() + Duration::from_secs(60);
            while width(host) != Some(wanted) {
                assert!(Instant::now() < until, "the panel shows {:?}, not {wanted}", width(host));
                std::thread::sleep(Duration::from_millis(5));
            }
        };

        let mut host = start();
        let names = ["a.stl", "b.stl"];
        let openers = host.openers(&names);
        assert_eq!(openers.len(), 1, "a selection of models opens with the plugin");
        assert_eq!(openers[0].2.label.as_deref(), Some("Box around this …"));
        assert!(host.openers(&["a.stl", "notes.txt"]).is_empty() && host.openers(&[]).is_empty());
        // Opening hands the ids over, and the panel reads the first file: 10 mm and air.
        host.open_models("hello-box", "box", vec!["Eigene/box.stl".into()]).unwrap();
        wait(&mut host, 14.0);
        let kept = base.join("hello-box").join("state").join("box.opened.json");
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), r#"["Eigene/box.stl"]"#);
        // A file nobody opened with it stays closed: the panel says so and keeps its box.
        host.find("hello-box", "box").unwrap().send(Event::ModelsOpened { ids: vec!["Eigene/other.stl".into()] });
        let until = Instant::now() + Duration::from_secs(60);
        let refusal = loop {
            assert!(Instant::now() < until, "no answer from the panel");
            let (actions, _) = host.poll();
            let toast = actions.into_iter().find_map(|a| match a {
                PanelAction::Effect { effect: Effect::Toast { text, .. }, .. } if !text.starts_with("host:") => {
                    Some(text)
                }
                _ => None,
            });
            match toast {
                Some(text) => break text,
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        };
        assert!(refusal.contains("has not allowed hello-box to read the library"), "{refusal}");
        drop(host);

        // The next run: the panel opens with what was opened before.
        let mut host = start();
        host.open("hello-box", "box", None).unwrap();
        let panel = host.find("hello-box", "box").unwrap();
        assert!(panel.opened.contains("Eigene/box.stl") && !panel.opened.contains("Eigene/other.stl"));
        panel.send(Event::ModelsOpened { ids: vec!["Eigene/box.stl".into()] });
        wait(&mut host, 14.0);
    }

    /// A library with one folder and a panel's bundle for it, which holds one file.
    struct Scene {
        _dir: tempfile::TempDir,
        root: PathBuf,
        db: PathBuf,
        config: Config,
        staged: StagedBundle,
    }

    fn scene() -> Scene {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib");
        std::fs::create_dir_all(root.join("Garten")).unwrap();
        let staging = Staging::new(&dir.path().join("plugins"), "test").unwrap();
        std::fs::write(staging.out().join("a.stl"), "solid a").unwrap();
        let bundle = Bundle { title: "Box".into(), folder: Some("Boxes".into()), ..Bundle::default() };
        let staged = StagedBundle { plugin: "test".into(), bundle, staging };
        let config = Config { library_roots: vec![root.clone()], ..Config::default() };
        Scene { db: dir.path().join("library.db"), root, config, staged, _dir: dir }
    }

    fn never(_: &ResolvedBundle) -> Result<(), String> {
        panic!("not to be proposed")
    }

    #[test]
    fn a_bundle_without_the_grant_is_refused() {
        let s = scene();
        let out = s.staged.staging.out().to_path_buf();
        let outcome = settle_bundle(false, s.staged, &s.config, BundleRoute::Propose(&never));
        assert_eq!(outcome, Some(BundleOutcome::Refused));
        assert!(!out.exists(), "its files are gone");
        assert!(!s.root.join("Boxes").exists(), "and the library is as it was");
    }

    /// `applied` means the change went through on the library's thread, not that it was sent there.
    #[test]
    fn an_applied_bundle_comes_back_from_the_librarys_thread() {
        let s = scene();
        let library = LibraryService::start(s.db.clone(), vec![s.root.clone()], egui::Context::default());
        let from = ("test".to_owned(), "panel".to_owned());
        let route = BundleRoute::Apply { library: &library, from: from.clone() };
        assert_eq!(settle_bundle(true, s.staged, &s.config, route), None, "not done yet");
        let until = Instant::now() + Duration::from_secs(30);
        let answer = loop {
            assert!(Instant::now() < until, "no answer from the library");
            match library.try_event() {
                Some(crate::library::LibEvent::Applied(applied)) => break bundle_outcome(&applied),
                Some(_) => {}
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        };
        let (to, title, outcome) = answer.expect("about the bundle");
        assert_eq!((to, title.as_str()), (from, "Box"));
        assert!(matches!(outcome, BundleOutcome::Applied { steps, skipped: 0 } if steps > 0), "{outcome:?}");
        assert!(s.root.join("Boxes").join("a.stl").is_file());
    }

    #[test]
    fn a_failure_says_why_and_is_no_refusal() {
        let mut s = scene();
        // A list naming a file the plugin did not write: the bundle does not resolve.
        s.staged.bundle.lists.push(meshgrove_plugin_api::BundleList {
            name: "Teile".into(),
            kind: Default::default(),
            items: vec![meshgrove_plugin_api::BundleItem { path: "missing.stl".into(), quantity: 1 }],
        });
        let out = s.staged.staging.out().to_path_buf();
        match settle_bundle(true, s.staged, &s.config, BundleRoute::Propose(&never)) {
            Some(BundleOutcome::Failed { error }) => assert!(error.contains("missing.stl"), "{error}"),
            other => panic!("{other:?}"),
        }
        assert!(!out.exists());
        // And one the library's thread could not apply.
        let failed = Applied::BundleFailed {
            title: "Box".into(),
            error: "disk full".into(),
            from: ("test".into(), "panel".into()),
        };
        let (_, _, outcome) = bundle_outcome(&failed).unwrap();
        assert_eq!(outcome, BundleOutcome::Failed { error: "disk full".into() });
        assert!(bundle_outcome(&Applied::Failed("not ours".into())).is_none());
    }

    #[test]
    fn a_proposed_bundle_waits_in_the_review() {
        let s = scene();
        let propose = |r: &ResolvedBundle| if r.title == "Box" { Ok(()) } else { Err("wrong".to_owned()) };
        assert_eq!(
            settle_bundle(true, s.staged, &s.config, BundleRoute::Propose(&propose)),
            Some(BundleOutcome::Queued)
        );
        let s = scene();
        let out = s.staged.staging.out().to_path_buf();
        let fail = |_: &ResolvedBundle| Err("no data folder".to_owned());
        assert_eq!(
            settle_bundle(true, s.staged, &s.config, BundleRoute::Propose(&fail)),
            Some(BundleOutcome::Failed { error: "no data folder".into() })
        );
        assert!(!out.exists());
    }
}
