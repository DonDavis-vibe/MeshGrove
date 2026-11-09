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
use meshgrove_plugin_api::ui::{BundleOutcome, Effect, Event, Node, PickInto, View};
use meshgrove_plugin_api::{FileType, HostInfo};
use meshgrove_plugins::jobs::{JobEvent, JobPool};
use meshgrove_plugins::{CancelToken, Opened, Plugin, Registry, Settings, StagedBundle};

use crate::library::{Applied, LibraryService};
use crate::plugin_mesh::{Gpu, PreparedMesh, overlay_colors, plain_colors, prepare};
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
    /// Files for the library: put in at once (`apply`) or suggested. `title` is the bundle's;
    /// `staged` holds it with its files, or why it could not be taken.
    Bundle {
        plugin: String,
        panel: String,
        apply: bool,
        title: String,
        staged: Result<Box<StagedBundle>, String>,
    },
    /// A file opened with a plugin, and what the plugin asks for.
    Opened {
        plugin: String,
        effects: Result<Vec<Effect>, String>,
    },
    /// A file the user picked for a panel (`Effect::OpenFileDialog`), copied into its folders:
    /// its name and where the plugin finds it, or why it could not be taken.
    Picked {
        plugin: String,
        panel: String,
        id: String,
        file: Result<(String, String), String>,
    },
}

/// Whether file `name` has one of `extensions` (given without the dot; case does not matter).
/// None given: every file fits.
pub fn extension_fits(name: &str, extensions: &[String]) -> bool {
    let name = name.to_lowercase();
    extensions.is_empty() || extensions.iter().any(|e| name.ends_with(&format!(".{}", e.to_lowercase())))
}

/// Copies a file the user picked into a panel's `/tmp` (as it is called; one of that name there
/// is replaced) or below the plugin's `/data` (into `picked`, where a file of that name with
/// other content stays and the new one gets a number). The file's name and the path the plugin
/// finds the copy at.
pub fn take_picked(
    from: &Path,
    extensions: &[String],
    into: PickInto,
    tmp: &Path,
    data: &Path,
) -> Result<(String, String), String> {
    let name = meshgrove_core::listing::file_name(from);
    if !from.is_file() || name.is_empty() || !extension_fits(&name, extensions) {
        return Err(t!("plugins.not_a_file").into_owned());
    }
    let io = |e: std::io::Error| format!("{name}: {e}");
    match into {
        PickInto::Tmp => {
            std::fs::create_dir_all(tmp).map_err(io)?;
            std::fs::copy(from, tmp.join(&name)).map_err(io)?;
            Ok((name.clone(), format!("/tmp/{name}")))
        }
        PickInto::Data => {
            let dir = data.join("picked");
            std::fs::create_dir_all(&dir).map_err(io)?;
            let same = |there: &Path| {
                let sizes = (there.metadata().map(|m| m.len()).ok(), from.metadata().map(|m| m.len()).ok());
                sizes.0.is_some() && sizes.0 == sizes.1 && std::fs::read(there).ok() == std::fs::read(from).ok()
            };
            // Picked before: the copy that is there serves.
            let target = match dir.join(&name) {
                there if same(&there) => there,
                _ => {
                    let free = meshgrove_core::import::free_target(&dir, &name, &[]);
                    std::fs::copy(from, &free).map_err(io)?;
                    free
                }
            };
            Ok((name, format!("/data/picked/{}", meshgrove_core::listing::file_name(&target))))
        }
    }
}

enum Input {
    Event(Event),
    Render,
    Snapshot(Sender<Result<String, String>>),
}

enum Output {
    Opened {
        out: PathBuf,
        tmp: PathBuf,
    },
    View(View),
    Effects(Vec<Effect>),
    Bundle {
        apply: bool,
        title: String,
        staged: Result<Box<StagedBundle>, String>,
    },
    /// The triangles of a mesh view, prepared (and uploaded, with a GPU) on the worker.
    Mesh {
        key: String,
        mesh: Option<Arc<PreparedMesh>>,
    },
    /// The colours of a mesh view's overlay, one per triangle; `written` when the worker put
    /// them on the GPU already.
    Overlay {
        key: String,
        colors: Arc<Vec<[u8; 4]>>,
        written: bool,
    },
    Failed(String),
}

/// A 3D view of a panel: the viewer and the mesh it shows, swapped in when it arrives.
pub struct PanelMesh {
    pub viewer: ModelViewer,
    arrived: Option<Option<Arc<PreparedMesh>>>,
    /// Colours of an overlay waiting to be shown, and whether they are on the GPU already.
    overlay: Option<(Arc<Vec<[u8; 4]>>, bool)>,
    /// What is shown: the mesh and its upload.
    shown: Option<(Arc<PreparedMesh>, Arc<meshgrove_render::GpuMesh>)>,
    /// The middle of the mesh in the plugin's coordinates: the viewer shows it recentred there.
    pub centre: glam::Vec3,
}

impl PanelMesh {
    fn new() -> PanelMesh {
        let mut viewer = ModelViewer::new();
        viewer.set_tools(true);
        PanelMesh { viewer, arrived: None, overlay: None, shown: None, centre: glam::Vec3::ZERO }
    }

    /// Takes in what arrived: a mesh (prepared on the worker, so this is only a swap unless
    /// there was no GPU there) and an overlay's colours. Needs the GPU, so done while drawing.
    fn show_arrived(&mut self, name: &str, device: &wgpu::Device, queue: &wgpu::Queue) {
        if let Some(arrived) = self.arrived.take() {
            match arrived {
                None => {
                    self.viewer.set_mesh(None);
                    self.shown = None;
                }
                Some(prepared) => {
                    let gpu = prepared.gpu.clone().unwrap_or_else(|| Arc::new(prepared.upload(device)));
                    let source = MeshSource::Memory { name: name.to_owned(), soup: prepared.soup.clone() };
                    self.centre = prepared.centre;
                    self.viewer.update_mesh(gpu.clone(), source);
                    self.viewer.set_picking(Some(prepared.tree.clone()));
                    self.shown = Some((prepared, gpu));
                }
            }
        }
        if let Some((colors, written)) = self.overlay.take()
            && let Some((prepared, gpu)) = &self.shown
            && colors.len() == prepared.triangles()
        {
            // Written by the worker only onto its own upload.
            if !written || prepared.gpu.is_none() {
                gpu.set_triangle_colors(queue, &colors);
            }
            self.viewer.redraw();
        }
    }

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

    /// Shows the meshes and overlays that arrived (needs the GPU, so done while drawing).
    pub fn upload_meshes(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        for (key, m) in &mut self.meshes {
            m.show_arrived(key, device, queue);
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
    /// The window's GPU, for the panels' workers to upload their meshes with.
    gpu: Option<Gpu>,
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
            gpu: None,
        }
    }

    /// The window's GPU: the panels' workers upload their meshes with it, off the UI thread.
    pub fn set_gpu(&mut self, device: wgpu::Device, queue: wgpu::Queue) {
        self.gpu = Some(Gpu { device, queue });
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
        let (name, ctx, reads, gpu) = (panel.to_owned(), self.ctx.clone(), opened.clone(), self.gpu.clone());
        std::thread::Builder::new()
            .name(format!("plugin-ui-{plugin}"))
            .spawn(move || work(&p, &name, state, reads, gpu, &input_rx, &output_tx, &ctx))
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

    /// Copies files the user picked for a panel into its folders, on a thread (they may be
    /// large); each comes back with [`PluginHost::poll`] as [`PanelAction::Picked`].
    pub fn pick_files(
        &mut self,
        (plugin, panel): (&str, &str),
        id: &str,
        files: Vec<PathBuf>,
        extensions: Vec<String>,
        into: PickInto,
    ) -> Result<(), String> {
        let data = self.registry.get(plugin).ok_or("the plugin is not loaded")?.data_dir();
        let open = self.find(plugin, panel).ok_or("the panel is closed")?;
        let tmp = open.tmp().ok_or("the panel is not open yet")?.to_path_buf();
        let (tx, ctx) = (self.opened.0.clone(), self.ctx.clone());
        let (plugin, panel, id) = (plugin.to_owned(), panel.to_owned(), id.to_owned());
        std::thread::spawn(move || {
            for from in files {
                let file = take_picked(&from, &extensions, into, &tmp, &data);
                let (plugin, panel, id) = (plugin.clone(), panel.clone(), id.clone());
                let _ = tx.send(PanelAction::Picked { plugin, panel, id, file });
                ctx.request_repaint();
            }
        });
        Ok(())
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
                    Output::Bundle { apply, title, staged } => {
                        let (plugin, panel) = (p.plugin.clone(), p.panel.clone());
                        actions.push(PanelAction::Bundle { plugin, panel, apply, title, staged });
                    }
                    Output::Mesh { key, mesh } => {
                        p.meshes.entry(key).or_insert_with(PanelMesh::new).arrived = Some(mesh)
                    }
                    Output::Overlay { key, colors, written } => {
                        if let Some(m) = p.meshes.get_mut(&key) {
                            m.overlay = Some((colors, written));
                        }
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

/// What a panel's worker has shown of its mesh views, by their keys: the revision and prepared
/// triangles of each, and the overlay (key, revision, palette) last applied to it.
#[derive(Default)]
struct Shown {
    revisions: HashMap<String, u64>,
    prepared: HashMap<String, Arc<PreparedMesh>>,
    overlays: HashMap<String, (String, u64, Vec<String>)>,
}

/// A panel's worker: opens it, then turns each event into effects, a new view and the meshes
/// and overlays whose revision changed. Meshes are prepared (and uploaded, with `gpu`) here,
/// off the window's thread.
#[allow(clippy::too_many_arguments)]
fn work(
    plugin: &Arc<Plugin>,
    panel: &str,
    state: Option<String>,
    opened: Opened,
    gpu: Option<Gpu>,
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
    let mut shown = Shown::default();
    let refresh = |shown: &mut Shown| {
        let view = match handle.render() {
            Ok(view) => view,
            Err(e) => return send(Output::Failed(e.to_string())),
        };
        type Overlay = Option<(String, u64, Vec<String>)>;
        let views: Vec<(String, u64, Overlay)> = view
            .root
            .walk()
            .into_iter()
            .filter_map(|n| match n {
                Node::MeshView { key, revision, overlay, .. } => Some((
                    key.clone(),
                    *revision,
                    overlay.as_ref().map(|o| (o.key.clone(), o.revision, o.palette.clone())),
                )),
                _ => None,
            })
            .collect();
        send(Output::View(view));
        for (key, revision, overlay) in views {
            let changed = shown.revisions.get(&key) != Some(&revision);
            if changed {
                match handle.mesh(&key) {
                    Ok(mesh) => {
                        let prepared = mesh.as_ref().and_then(|m| prepare(m, gpu.as_ref())).map(Arc::new);
                        match &prepared {
                            Some(p) => shown.prepared.insert(key.clone(), p.clone()),
                            None => shown.prepared.remove(&key),
                        };
                        send(Output::Mesh { key: key.clone(), mesh: prepared });
                    }
                    Err(e) => return send(Output::Failed(e.to_string())),
                }
                shown.revisions.insert(key.clone(), revision);
            }
            // A new mesh comes in the model's grey: its overlay is asked for again.
            let stale = changed || overlay.as_ref() != shown.overlays.get(&key);
            if !stale {
                continue;
            }
            let Some(prepared) = shown.prepared.get(&key).cloned() else {
                shown.overlays.remove(&key);
                continue;
            };
            let colors = match &overlay {
                Some((overlay_key, _, palette)) => match handle.mesh(overlay_key) {
                    Ok(classes) => {
                        let classes = classes.filter(|c| c.positions.is_empty()).map(|c| c.indices).unwrap_or_default();
                        let colors = overlay_colors(&classes, palette, prepared.triangles());
                        if colors.is_none() {
                            log::debug!(
                                "{}/{panel}: overlay {overlay_key} has {} classes for {} triangles, left out",
                                plugin.id(),
                                classes.len(),
                                prepared.triangles()
                            );
                        }
                        colors
                    }
                    Err(e) => return send(Output::Failed(e.to_string())),
                },
                // The overlay is gone: back to the grey, if one was shown.
                None if shown.overlays.contains_key(&key) => Some(plain_colors(prepared.triangles())),
                None => None,
            };
            match overlay {
                Some(o) => shown.overlays.insert(key.clone(), o),
                None => shown.overlays.remove(&key),
            };
            if let Some(colors) = colors {
                let written = match (&prepared.gpu, &gpu) {
                    (Some(mesh), Some(gpu)) => mesh.set_triangle_colors(&gpu.queue, &colors),
                    _ => false,
                };
                send(Output::Overlay { key, colors: Arc::new(colors), written });
            }
        }
    };
    let on_event = |event: &Event, shown: &mut Shown| match handle.event(event) {
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
                // What the panel and its jobs wrote goes with the bundle; the files it is put
                // beside are found here, off the window's thread.
                let title = bundle.title.clone();
                let staged = handle.stage(bundle).map(Box::new).map_err(|e| e.to_string());
                send(Output::Bundle { apply, title, staged });
            }
            if !rest.is_empty() {
                send(Output::Effects(rest));
            }
            refresh(shown);
        }
        Err(e) => send(Output::Failed(e.to_string())),
    };
    // Once before the first view, so a panel can start right away what takes longer than an
    // event may (a job), with what it is told about the host.
    on_event(&Event::HostChanged { info: plugin.settings().info.clone() }, &mut shown);
    for input in inputs {
        match input {
            Input::Event(event) => on_event(&event, &mut shown),
            Input::Render => refresh(&mut shown),
            Input::Snapshot(reply) => {
                let _ = reply.send(handle.snapshot().map_err(|e| e.to_string()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meshgrove_plugin_api::{Bundle, MeshData};
    use meshgrove_plugins::Staging;

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
    fn picked_files_are_copied_where_the_plugin_asked() {
        let dir = tempfile::tempdir().unwrap();
        let (tmp, data) = (dir.path().join("tmp"), dir.path().join("data"));
        let from = dir.path().join("Wabe.PNG");
        std::fs::write(&from, "wabe").unwrap();
        let png = vec!["png".to_owned(), "jpg".to_owned()];
        let pick = |from: &Path, into| take_picked(from, &png, into, &tmp, &data);

        assert_eq!(pick(&from, PickInto::Tmp), Ok(("Wabe.PNG".into(), "/tmp/Wabe.PNG".into())));
        assert_eq!(std::fs::read(tmp.join("Wabe.PNG")).unwrap(), b"wabe");
        // Into /data it stays, where the panel's jobs read it too; picked again, it is the same.
        let kept = Ok(("Wabe.PNG".into(), "/data/picked/Wabe.PNG".into()));
        assert_eq!(pick(&from, PickInto::Data), kept);
        assert_eq!(pick(&from, PickInto::Data), kept);
        assert_eq!(std::fs::read_dir(data.join("picked")).unwrap().count(), 1);
        // Another file of that name does not replace it.
        let other = dir.path().join("other").join("Wabe.PNG");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&other, "holz").unwrap();
        assert_eq!(pick(&other, PickInto::Data), Ok(("Wabe.PNG".into(), "/data/picked/Wabe (2).PNG".into())));
        assert_eq!(std::fs::read(data.join("picked").join("Wabe.PNG")).unwrap(), b"wabe");
        assert_eq!(std::fs::read(data.join("picked").join("Wabe (2).PNG")).unwrap(), b"holz");
        // In /tmp the newest counts, as it always did.
        assert_eq!(pick(&other, PickInto::Tmp), Ok(("Wabe.PNG".into(), "/tmp/Wabe.PNG".into())));
        assert_eq!(std::fs::read(tmp.join("Wabe.PNG")).unwrap(), b"holz");

        // What does not fit is not taken: another kind of file, a folder, nothing.
        let text = dir.path().join("notes.txt");
        std::fs::write(&text, "x").unwrap();
        for bad in [text.as_path(), dir.path(), &dir.path().join("missing.png")] {
            assert!(pick(bad, PickInto::Data).is_err(), "{}", bad.display());
        }
        assert!(take_picked(&text, &[], PickInto::Tmp, &tmp, &data).is_ok(), "no extensions asked for: any file");
        assert!(extension_fits("a.tar.GZ", &["tar.gz".into()]) && !extension_fits("png", &png));
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
        let worker = std::thread::spawn(move || work(&plugin, "box", None, opened, None, &input_rx, &output_tx, &ctx));
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
            _part: Option<usize>,
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

    /// The next output of a panel's worker that `want` picks, skipping the others.
    fn next_matching<T>(outputs: &Receiver<Output>, mut want: impl FnMut(Output) -> Option<T>) -> T {
        let until = Instant::now() + Duration::from_secs(60);
        loop {
            let left = until.saturating_duration_since(Instant::now());
            let output = outputs.recv_timeout(left).expect("the panel answers");
            if let Some(found) = want(output) {
                return found;
            }
        }
    }

    /// Everything the worker sends until it renders: (overlays, meshes) seen on the way.
    fn until_view(outputs: &Receiver<Output>) -> (Vec<Arc<Vec<[u8; 4]>>>, usize) {
        let (mut overlays, mut meshes) = (Vec::new(), 0);
        // The meshes and overlays come right after the view; the next view ends them.
        next_matching(outputs, |o| matches!(o, Output::View(_)).then_some(()));
        let until = Instant::now() + Duration::from_millis(1500);
        while let Ok(output) = outputs.recv_timeout(until.saturating_duration_since(Instant::now())) {
            match output {
                Output::Overlay { colors, written, .. } => {
                    assert!(!written, "no GPU in this worker");
                    overlays.push(colors);
                }
                Output::Mesh { .. } => meshes += 1,
                _ => {}
            }
        }
        (overlays, meshes)
    }

    /// hello-box's 3D view has an overlay: the worker asks for it with the mesh and again when
    /// its revision changes, and leaves out one of the wrong length.
    #[test]
    fn the_worker_brings_the_overlay_and_leaves_out_one_that_does_not_fit() {
        let Some(bytes) = test_plugin() else { return };
        let base = tempfile::tempdir().unwrap();
        let plugin = Arc::new(Plugin::from_bytes(&bytes, Settings::new(base.path())).unwrap());
        let (inputs, input_rx) = unbounded();
        let (output_tx, outputs) = unbounded();
        let ctx = egui::Context::default();
        let worker = std::thread::spawn(move || {
            work(&plugin, "box", None, Opened::default(), None, &input_rx, &output_tx, &ctx)
        });
        let grey = meshgrove_render::default_model_color();
        let mesh = next_matching(&outputs, |o| match o {
            Output::Mesh { key, mesh } if key == "box" => Some(mesh),
            _ => None,
        });
        let mesh = mesh.expect("the box");
        assert_eq!((mesh.triangles(), mesh.tree.len()), (12, 12));
        assert!(mesh.gpu.is_none());
        let colors = next_matching(&outputs, |o| match o {
            Output::Overlay { key, colors, .. } if key == "box" => Some(colors),
            _ => None,
        });
        assert_eq!(*colors, vec![grey; 12], "nothing tinted yet");
        // A click on triangle 4 (the front): the overlay comes anew, the front orange.
        let hit = meshgrove_plugin_api::ui::MeshHit { triangle: 4, point: [1.0, 0.0, 1.0], normal: [0.0, -1.0, 0.0] };
        inputs.send(Input::Event(Event::MeshPick { id: "preview".into(), hit: Some(hit), shift: false })).unwrap();
        let (overlays, meshes) = until_view(&outputs);
        assert_eq!((overlays.len(), meshes), (1, 0), "the box is as it was");
        let tinted: Vec<usize> = (0..12).filter(|&t| overlays[0][t] != grey).collect();
        assert_eq!(tinted, [4, 5]);
        assert_eq!(overlays[0][4], [255, 128, 0, 255]);
        // An overlay of five classes for twelve triangles: left out, the panel goes on.
        inputs.send(Input::Event(Event::Click { id: "bad_overlay".into() })).unwrap();
        let (overlays, _) = until_view(&outputs);
        assert!(overlays.is_empty(), "{} overlays", overlays.len());
        // A new box comes in grey and brings its overlay again (still the wrong length here).
        inputs.send(Input::Event(Event::Changed { id: "width".into(), value: serde_json::json!(33.0) })).unwrap();
        let (overlays, meshes) = until_view(&outputs);
        assert_eq!((overlays.len(), meshes), (0, 1));
        drop(inputs);
        worker.join().unwrap();
    }

    /// A preview of two million triangles: prepared and uploaded on the worker, so showing it
    /// takes the window's thread only a swap, and so does its overlay.
    #[test]
    fn a_large_preview_arrives_without_stalling_the_window() {
        let gpu = match meshgrove_render::HeadlessRenderer::new() {
            Ok(gpu) => gpu,
            Err(e) if std::env::var_os("MESHGROVE_REQUIRE_GPU").is_none() => return eprintln!("skipped: {e}"),
            Err(e) => panic!("{e}"),
        };
        let (device, queue, _) = gpu.parts();
        // A wavy sheet of 1000 × 1000 quads.
        let n = 1000u32;
        let positions = (0..=n)
            .flat_map(|y| (0..=n).map(move |x| [x as f32 * 0.1, y as f32 * 0.1, (x as f32 * 0.05).sin()]))
            .flatten()
            .collect();
        let indices = (0..n)
            .flat_map(|y| {
                (0..n).flat_map(move |x| {
                    let i = y * (n + 1) + x;
                    [i, i + 1, i + n + 2, i, i + n + 2, i + n + 1]
                })
            })
            .collect();
        let data = MeshData { positions, indices };
        assert_eq!(data.triangles(), 2_000_000);
        let worker_gpu = Gpu { device: device.clone(), queue: queue.clone() };
        let prepared = Arc::new(prepare(&data, Some(&worker_gpu)).unwrap());
        assert!(prepared.gpu.is_some(), "uploaded on the worker");
        let mut m = PanelMesh::new();
        m.arrived = Some(Some(prepared.clone()));
        let started = Instant::now();
        m.show_arrived("big", device, queue);
        let swap = started.elapsed();
        assert!(m.shown.is_some());
        assert!(swap < Duration::from_millis(16), "the window's thread spent {swap:?}");
        // Its overlay, written on the worker too.
        let colors = overlay_colors(&vec![1; 2_000_000], &["#00000000".into(), "#ff0000".into()], 2_000_000).unwrap();
        assert!(prepared.gpu.as_ref().unwrap().set_triangle_colors(queue, &colors));
        m.overlay = Some((Arc::new(colors), true));
        let started = Instant::now();
        m.show_arrived("big", device, queue);
        assert!(started.elapsed() < Duration::from_millis(16), "{:?}", started.elapsed());
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
        let staged = StagedBundle::new("test", bundle, staging);
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
