//! The plugins in the window: their panels as tabs, what the panels ask for (effects), the
//! "Plugins" menu, opening files with a plugin, and installing, granting and removing plugins.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

use egui::{RichText, Ui};
use meshgrove_core::ai::proposals::{self, Origin, ProposalStore};
use meshgrove_core::config::{AccessClass, PluginConfig};
use meshgrove_plugin_api::ui::{BundleOutcome, Effect, Event, Level, PickInto};
use meshgrove_plugin_api::{FileType, HostInfo, Permission, Receive};
use meshgrove_plugins::{StagedBundle, Target};

use super::App;
use crate::plugins::{BundleRoute, PanelAction, settle_bundle};
use crate::ui::folder_pick::{self, FolderPick};
use crate::ui::plugin_view::{self, MeshViewArgs};
use crate::ui::settings::SettingsChange;

/// A path the user is asked for by typing it.
pub(super) enum PathPrompt {
    /// Where the files of a plugin go while there is no library yet: that folder becomes one.
    /// `title` is the question, `bundle_title` goes back to the panel with the outcome.
    Library {
        plugin: String,
        panel: String,
        apply: bool,
        staged: Box<StagedBundle>,
        title: String,
        bundle_title: String,
        input: String,
    },
}

/// A panel waiting for the user to choose a folder (`Effect::PickFolder`); the first one asked
/// for is shown, the others follow.
pub(super) struct FolderAsk {
    plugin: String,
    panel: String,
    id: String,
    pick: FolderPick,
}

/// What the system's file dialog was opened for.
enum DialogFor {
    /// A plugin's file, to install it.
    Install,
    /// Files for a panel (`Effect::OpenFileDialog`).
    Pick { plugin: String, panel: String, id: String, extensions: Vec<String>, into: PickInto },
    /// Where to save a file a plugin wrote (`Effect::SaveFile`).
    Save { from: PathBuf },
}

/// What the user picked in a dialog; nothing when they closed it.
type Picked = Pin<Box<dyn Future<Output = Vec<PathBuf>>>>;

/// The system's file dialog (rfd: the desktop portal on Linux), for installing a plugin and for
/// what panels ask (a file to open, a place to save). It is asked from the UI thread and its
/// answer polled each frame, so the window keeps drawing meanwhile. One is open at a time;
/// those asked for meanwhile follow.
pub(super) struct PluginDialog {
    /// Set up with the window as its parent.
    base: rfd::AsyncFileDialog,
    open: Option<(DialogFor, Picked)>,
    waiting: std::collections::VecDeque<(DialogFor, Box<dyn FnOnce() -> Picked>)>,
}

impl PluginDialog {
    pub(super) fn new(cc: &eframe::CreationContext<'_>) -> PluginDialog {
        let base = rfd::AsyncFileDialog::new().set_parent(cc);
        PluginDialog { base, open: None, waiting: std::collections::VecDeque::new() }
    }

    /// Opens a dialog (`start` does), or queues it behind the one that is open.
    fn ask(&mut self, what: DialogFor, start: impl FnOnce() -> Picked + 'static) {
        match self.open {
            None => self.open = Some((what, start())),
            Some(_) => self.waiting.push_back((what, Box::new(start))),
        }
    }

    /// The answer of the open dialog, once it is there; the next one waiting opens then.
    fn answer(&mut self, ctx: &egui::Context) -> Option<(DialogFor, Vec<PathBuf>)> {
        let (_, picked) = self.open.as_mut()?;
        let waker = Waker::from(Arc::new(Repaint(ctx.clone())));
        let Poll::Ready(files) = picked.as_mut().poll(&mut Context::from_waker(&waker)) else { return None };
        let (what, _) = self.open.take()?;
        self.open = self.waiting.pop_front().map(|(what, start)| (what, start()));
        Some((what, files))
    }
}

/// The paths of what a dialog answered.
/// What a panel is called in the "Plugins" menu: its title, and with several plugins the
/// plugin's name in front of it – unless the title says the name already ("Bingrove: Bingrove").
fn menu_label(plugin: &str, title: &str, several: bool) -> String {
    if !several || title.to_lowercase().contains(&plugin.to_lowercase()) {
        title.to_owned()
    } else {
        format!("{plugin}: {title}")
    }
}

fn paths(files: impl IntoIterator<Item = rfd::FileHandle>) -> Vec<PathBuf> {
    files.into_iter().map(|f| f.path().to_path_buf()).collect()
}

/// Wakes the UI when the dialog answers.
struct Repaint(egui::Context);

impl Wake for Repaint {
    fn wake(self: Arc<Self>) {
        self.0.request_repaint();
    }
}

/// How many folders plugins came from are remembered for the search.
const PLUGIN_FOLDERS: usize = 4;

impl App {
    pub(super) fn plugin_info(&self) -> HostInfo {
        let language = if rust_i18n::locale().starts_with("de") { "de" } else { "en" };
        meshgrove_plugins::registry::host_info(&self.config, language, self.egui_ctx.theme() == egui::Theme::Dark)
    }

    pub(super) fn reload_plugins(&mut self) {
        let info = self.plugin_info();
        self.plugins.reload(&self.config, info, &self.db_path);
    }

    /// Takes in what the plugins did since the last frame.
    pub(super) fn poll_plugins(&mut self) {
        if let Some((what, files)) = self.plugin_dialog.answer(&self.egui_ctx) {
            self.dialog_answered(what, files);
        }
        // Language, theme or printers changed: the plugins hear of it (tools load anew with it).
        let info = self.plugin_info();
        if self.plugins.info().is_some_and(|known| *known != info) {
            self.plugins.host_changed(&info);
            self.plugins.reload(&self.config, info, &self.db_path);
        }
        // Jobs the plugins' tools started: done ones say so, running ones keep the status bar fresh.
        for done in self.plugins.deferred().take_finished() {
            let name = done.plugin_name;
            match done.outcome {
                Ok(title) => self.toast(t!("plugins.job_done", name = name, title = title).into_owned()),
                Err(error) => self.toast(t!("plugins.job_failed", name = name, error = error).into_owned()),
            }
        }
        if !self.plugins.deferred().running().is_empty() {
            self.egui_ctx.request_repaint_after(Duration::from_millis(250));
        }
        let (actions, reloaded) = self.plugins.poll();
        if reloaded {
            // The panels of the last run (kept in the layout) open again.
            for (plugin, panel) in self.layout.plugin_panes() {
                if self.plugins.plugin(&plugin).is_some()
                    && let Err(e) = self.plugins.open(&plugin, &panel, None)
                {
                    log::warn!("opening {plugin}/{panel}: {e}");
                }
            }
            self.menu_for.clear(); // "open with" may have changed
        }
        for action in actions {
            self.plugin_action(action);
        }
        if self.plugins.ready {
            self.layout.sync_plugins(&self.plugins.open_keys());
        }
    }

    /// Does what a file dialog was opened for, with what the user picked there.
    fn dialog_answered(&mut self, what: DialogFor, files: Vec<PathBuf>) {
        let Some(first) = files.first().cloned() else { return };
        match what {
            DialogFor::Install => self.apply_settings(vec![SettingsChange::InstallPlugin(first)]),
            DialogFor::Pick { plugin, panel, id, extensions, into } => {
                // Copied on a thread; each file comes back as `PanelAction::Picked`.
                if let Err(e) = self.plugins.pick_files((&plugin, &panel), &id, files, extensions, into) {
                    self.toast(format!("{}: {e}", self.plugin_name(&plugin)));
                }
            }
            // The dialog has asked before it names a file that is there.
            DialogFor::Save { from } => match std::fs::copy(&from, &first) {
                Ok(_) => self.toast(t!("plugins.saved", path = first.display().to_string()).into_owned()),
                Err(e) => self.toast(format!("{}: {e}", first.display())),
            },
        }
    }

    fn plugin_name(&self, id: &str) -> String {
        self.plugins.plugin(id).map_or_else(|| id.to_owned(), |p| p.manifest().name.clone())
    }

    fn plugin_action(&mut self, action: PanelAction) {
        match action {
            PanelAction::Effect { plugin, panel, effect } => self.plugin_effect(&plugin, Some(&panel), effect),
            PanelAction::Opened { plugin, effects } => match effects {
                Ok(effects) => effects.into_iter().for_each(|e| self.plugin_effect(&plugin, None, e)),
                Err(e) => self.toast(format!("{}: {e}", self.plugin_name(&plugin))),
            },
            PanelAction::Picked { plugin, panel, id, file } => match file {
                Ok((name, path)) => {
                    if let Some(open) = self.plugins.find(&plugin, &panel) {
                        open.send(Event::FilePicked { id, name, path });
                    }
                }
                Err(e) => self.toast(format!("{}: {e}", self.plugin_name(&plugin))),
            },
            PanelAction::Bundle { plugin, panel, apply, title, staged } => {
                let outcome = match staged {
                    Ok(staged) => self.settle_staged(&plugin, &panel, apply, *staged),
                    Err(error) => {
                        self.toast(t!("plugins.bundle_failed", error = error).into_owned());
                        Some(BundleOutcome::Failed { error })
                    }
                };
                if let Some(outcome) = outcome {
                    self.plugins.bundle_finished(&plugin, &panel, title, outcome);
                }
            }
        }
    }

    /// Does what a panel (or a file opened with `plugin`, then without `panel`) asks for.
    fn plugin_effect(&mut self, plugin: &str, panel: Option<&str>, effect: Effect) {
        let name = self.plugin_name(plugin);
        match effect {
            Effect::Toast { text, level } => {
                let mark = match level {
                    Level::Warn => "⚠ ",
                    Level::Error => "✖ ",
                    _ => "",
                };
                self.toast(format!("{mark}{name}: {text}"));
            }
            Effect::OpenPanel { panel: id, state } => match self.plugins.open(plugin, &id, state) {
                Ok(()) => self.layout.focus_plugin(plugin, &id),
                Err(e) => self.toast(format!("{name}: {e}")),
            },
            Effect::SelectModel { id } => match self.plugin_model(plugin, panel, &id) {
                Some(path) => self.reveal(&path),
                None => self.toast(t!("plugins.unknown_model", id = id).into_owned()),
            },
            effect => {
                let Some(panel) = panel else { return };
                match effect {
                    Effect::StartJobs { jobs } => self.plugins.start_jobs(plugin, panel, jobs),
                    Effect::CancelJob { job } => self.plugins.cancel_job(plugin, panel, &job),
                    Effect::RepaintAfter { ms } => {
                        self.plugins.render_after(plugin, panel, Duration::from_millis(u64::from(ms.max(16))));
                    }
                    Effect::SaveFile { path, name: file } => {
                        let from = self.plugins.find(plugin, panel).and_then(|p| p.host_path(&path));
                        match from.filter(|f| f.is_file()) {
                            Some(from) => {
                                let file =
                                    meshgrove_core::fsops::check_name(&file).map_or("plugin-file".into(), |()| file);
                                let mut dialog = self
                                    .plugin_dialog
                                    .base
                                    .clone()
                                    .set_title(t!("plugins.save_as"))
                                    .set_file_name(file);
                                if let Some(dir) = dirs::download_dir().or_else(dirs::home_dir) {
                                    dialog = dialog.set_directory(dir);
                                }
                                self.plugin_dialog.ask(DialogFor::Save { from }, move || {
                                    Box::pin(async move { paths(dialog.save_file().await) })
                                });
                            }
                            None => self.toast(format!("{name}: {}", t!("plugins.no_such_file", path = path))),
                        }
                    }
                    Effect::PickFolder { id, title } => self.ask_folder(plugin, panel, id, title),
                    Effect::OpenFileDialog { id, title, extensions, multiple, into } => {
                        let title = title.unwrap_or_else(|| t!("plugins.pick_file").into_owned());
                        let mut dialog = self.plugin_dialog.base.clone().set_title(title);
                        if !extensions.is_empty() {
                            // Linux dialogs match the pattern by case.
                            let cases: Vec<String> =
                                extensions.iter().flat_map(|e| [e.to_lowercase(), e.to_uppercase()]).collect();
                            let shown: Vec<String> = extensions.iter().map(|e| format!("*.{e}")).collect();
                            dialog = dialog.add_filter(shown.join(", "), &cases);
                        }
                        // Where the user is looking now, if that is a folder on disk.
                        let here = self.nav.current().to_path_buf();
                        if here.is_dir() {
                            dialog = dialog.set_directory(here);
                        }
                        let what = DialogFor::Pick {
                            plugin: plugin.to_owned(),
                            panel: panel.to_owned(),
                            id,
                            extensions,
                            into,
                        };
                        self.plugin_dialog.ask(what, move || {
                            Box::pin(async move {
                                match multiple {
                                    true => paths(dialog.pick_files().await.unwrap_or_default()),
                                    false => paths(dialog.pick_file().await),
                                }
                            })
                        });
                    }
                    _ => {}
                }
            }
        }
    }

    /// Meshgrove's folder chooser for a panel: in the libraries, starting where the panel's
    /// files went last, else at the root of the library imports go to.
    fn ask_folder(&mut self, plugin: &str, panel: &str, id: String, title: Option<String>) {
        // A double click on the panel's button asks twice before the chooser shows: one is enough.
        if self.folder_asks.iter().any(|a| a.plugin == plugin && a.panel == panel && a.id == id) {
            return;
        }
        let name = self.plugin_name(plugin);
        let libraries: Vec<folder_pick::Library> = meshgrove_core::ai::scope::aliases(&self.config.library_roots)
            .into_iter()
            .map(|(root, alias)| folder_pick::Library {
                read_only: meshgrove_core::fsinfo::is_read_only(&root),
                root,
                alias,
            })
            .collect();
        if libraries.iter().all(|l| l.read_only) {
            self.toast(format!("{name}: {}", t!("plugins.no_library")));
            return;
        }
        let start = self
            .plugins
            .last_target(plugin, panel)
            .map(|t| (t.root, t.dir))
            .or_else(|| self.config.import_root().map(|root| (root.to_path_buf(), root.to_path_buf())));
        let title = title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| t!("folder_pick.title").into_owned());
        let pick = FolderPick::new(title, name, libraries, start);
        self.folder_asks.push_back(FolderAsk { plugin: plugin.to_owned(), panel: panel.to_owned(), id, pick });
    }

    /// The folder chooser, while a panel waits for one.
    pub(super) fn folder_pick_modal(&mut self, ctx: &egui::Context) {
        let Some(ask) = self.folder_asks.front_mut() else { return };
        let Some(answer) = ask.pick.show(ctx) else { return };
        let FolderAsk { plugin, panel, id, .. } = self.folder_asks.pop_front().expect("shown above");
        // Cancelled: the panel hears nothing, as when a file dialog is closed.
        if let folder_pick::Answer::Chosen(chosen) = answer {
            let target = Target { root: chosen.root, dir: chosen.dir, label: chosen.label };
            if let Err(e) = self.plugins.folder_picked(&plugin, &panel, &id, target) {
                self.toast(format!("{}: {e}", self.plugin_name(&plugin)));
            }
        }
    }

    /// The library file behind an id a plugin named, as far as the plugin may know of it: a
    /// file the user opened with that panel, one its grant to read the library shows, or (as
    /// before there were grants) one local AI may see.
    fn plugin_model(&mut self, plugin: &str, panel: Option<&str>, id: &str) -> Option<PathBuf> {
        use meshgrove_core::ai::Scope;
        use meshgrove_plugins::{Caller, LibraryRead};
        let opened = panel.and_then(|panel| self.plugins.find(plugin, panel)).is_some_and(|p| p.opened.contains(id));
        if opened && let Some(path) = Scope::whole(&self.config).resolve(id) {
            return Some(path);
        }
        let read = match self.plugins.plugin(plugin) {
            Some(p) if p.may(Permission::LibraryRead) => p.settings().grants.library_read,
            _ => LibraryRead::None,
        };
        meshgrove_plugins::library::scope(&self.config, read, Caller::User)
            .and_then(|scope| scope.resolve(id))
            .or_else(|| Scope::new(&self.config, AccessClass::Local).resolve(id))
    }

    /// Files from a panel: into the library now, or into the review. The outcome, or `None`
    /// when the library's thread will answer (see `applied`).
    fn settle_staged(&mut self, plugin: &str, panel: &str, apply: bool, staged: StagedBundle) -> Option<BundleOutcome> {
        let allowed = self.plugins.plugin(plugin).is_some_and(|p| p.may(Permission::Bundles));
        // No library yet (a fresh install): ask where the files should go. That folder becomes
        // the library, and the bundle follows once it is chosen.
        // Files that go beside a library file or into a folder the user chose need no asking:
        // those are in a library.
        let placed = staged.is_placed();
        if allowed && !placed && self.config.import_root().is_none() && self.plugin_prompt.is_none() {
            let input = dirs::document_dir().or_else(dirs::home_dir).unwrap_or_default().join("Meshgrove");
            self.plugin_prompt = Some(PathPrompt::Library {
                plugin: plugin.to_owned(),
                panel: panel.to_owned(),
                apply,
                title: t!("plugins.pick_library", name = self.plugin_name(plugin)).into_owned(),
                bundle_title: staged.bundle.title.clone(),
                staged: Box::new(staged),
                input: input.display().to_string(),
            });
            return None;
        }
        let propose = |resolved: &_| {
            let origin = Origin { source: format!("plugin:{plugin}"), client: None, class: AccessClass::Local };
            ProposalStore::default_location()
                .ok_or_else(|| "no data folder".to_owned())
                .and_then(|path| ProposalStore::open(&path).map_err(|e| e.to_string()))
                .and_then(|mut store| {
                    proposals::propose_bundle(&self.config, &mut store, &origin, resolved).map_err(|e| e.to_string())
                })
                .map(|_| ())
        };
        let route = if apply {
            BundleRoute::Apply { library: &self.library, from: (plugin.to_owned(), panel.to_owned()) }
        } else {
            BundleRoute::Propose(&propose)
        };
        let mut outcome = settle_bundle(allowed, staged, &self.config, route);
        // No library yet (a fresh install): say so in the user's language, with the way out.
        if !placed
            && self.config.import_root().is_none()
            && let Some(BundleOutcome::Failed { error }) = &mut outcome
        {
            *error = t!("plugins.no_library").into_owned();
        }
        match &outcome {
            Some(BundleOutcome::Refused) => {
                let name = self.plugin_name(plugin);
                self.toast(t!("plugins.bundles_denied", name = name).into_owned());
            }
            Some(BundleOutcome::Failed { error }) => {
                self.toast(t!("plugins.bundle_failed", error = error).into_owned());
            }
            Some(BundleOutcome::Queued) => self.show_review(),
            _ => {}
        }
        outcome
    }

    /// The status bar's entries for the jobs plugins' tools started: name, progress and a
    /// button to cancel each.
    pub(super) fn plugin_jobs_status(&mut self, ui: &mut Ui) {
        for job in self.plugins.deferred().running() {
            ui.separator();
            let text = match job.message.is_empty() {
                true => job.plugin_name.clone(),
                false => format!("{}: {}", job.plugin_name, job.message),
            };
            ui.add(egui::ProgressBar::new(job.fraction.clamp(0.0, 1.0)).desired_width(90.0).desired_height(10.0))
                .on_hover_text(t!("plugins.job_hint", tool = job.tool.as_str()));
            ui.label(text);
            if ui.small_button("✖").on_hover_text(t!("plugins.cancel_job")).clicked() {
                self.plugins.deferred().cancel(&job.id);
            }
        }
    }

    /// The tab of a plugin panel.
    pub(super) fn plugin_pane(&mut self, ui: &mut Ui, rs: &egui_wgpu::RenderState, plugin: &str, panel: &str) {
        let renderer = &self.renderer;
        let space = self.space.as_ref();
        let Some(open) = self.plugins.panels.iter_mut().find(|p| p.plugin == plugin && p.panel == panel) else {
            ui.weak(t!("plugins.loading"));
            return;
        };
        open.upload_meshes(&rs.device, &rs.queue);
        if let Some(e) = open.error.clone() {
            ui.colored_label(ui.visuals().error_fg_color, format!("✖ {e}"));
            if ui.button(t!("plugins.reopen")).clicked() {
                let (plugin, panel) = (open.plugin.clone(), open.panel.clone());
                self.plugins.close(&plugin, &panel);
                if let Err(e) = self.plugins.open(&plugin, &panel, None) {
                    self.toast(e);
                }
            }
            return;
        }
        if !open.jobs.is_empty() {
            let mut cancel = None;
            for job in &open.jobs {
                ui.horizontal(|ui| {
                    if ui.small_button("✖").on_hover_text(t!("plugins.cancel_job")).clicked() {
                        cancel = Some(job.id.clone());
                    }
                    let text = if job.message.is_empty() {
                        job.label.clone()
                    } else {
                        format!("{}: {}", job.label, job.message)
                    };
                    ui.add(egui::ProgressBar::new(job.fraction.clamp(0.0, 1.0)).text(text));
                });
            }
            if let Some(job) = cancel {
                self.plugins.cancel_job(plugin, panel, &job);
            }
            ui.separator();
        }
        let Some(open) = self.plugins.find(plugin, panel) else { return };
        let Some(view) = open.view.clone() else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak(t!("plugins.loading"));
            });
            return;
        };
        // Painting on a 3D view waits while the panel works on an event.
        let busy = open.busy;
        let mut painted = Vec::new();
        let meshes = &mut open.meshes;
        let mut draw_mesh = |ui: &mut Ui, args: MeshViewArgs| {
            let size = egui::vec2(ui.available_width(), args.height);
            let Some(m) = meshes.get_mut(args.key) else {
                ui.allocate_ui(size, |ui| {
                    ui.centered_and_justified(|ui| ui.spinner());
                });
                return;
            };
            m.viewer.set_bed(args.bed);
            m.set_frame(args.frame);
            m.viewer.set_paint(args.paint.map(crate::plugin_mesh::paint_mode));
            ui.allocate_ui(size, |ui| {
                ui.set_min_size(size);
                if let Some(input) = space {
                    m.viewer.apply_space(input);
                }
                m.viewer.ui(ui, rs, renderer);
            });
            let centre = m.centre;
            painted.extend(
                m.viewer.take_paint(busy).into_iter().map(|out| crate::plugin_mesh::to_event(args.id, centre, out)),
            );
        };
        let events = plugin_view::show(ui, &view, &mut open.ui, &mut draw_mesh);
        for event in events.into_iter().chain(painted) {
            open.send(event);
        }
    }

    /// The "Plugins" menu: the loaded plugins' panels, and the settings.
    pub(super) fn plugins_menu(&mut self, ui: &mut Ui) {
        if self.config.plugins.is_empty() {
            return;
        }
        let defs = self.plugins.panel_defs();
        let several = defs.iter().map(|d| &d.0).collect::<std::collections::HashSet<_>>().len() > 1;
        ui.menu_button(format!("{} ▾", t!("plugins.menu")), |ui| {
            if defs.is_empty() {
                ui.label(RichText::new(t!("plugins.no_panels")).weak());
            }
            for (plugin, panel, title, name) in &defs {
                if ui.button(menu_label(name, title, several)).clicked() {
                    match self.plugins.open(plugin, panel, None) {
                        Ok(()) => self.layout.focus_plugin(plugin, panel),
                        Err(e) => self.toast(e),
                    }
                    ui.close();
                }
            }
            ui.separator();
            if ui.button(t!("plugins.settings")).clicked() {
                self.settings.open_at("plugins");
                ui.close();
            }
        });
    }

    /// The question for a path, while one is asked.
    pub(super) fn plugin_prompt_modal(&mut self, ctx: &egui::Context) {
        let Some(prompt) = &mut self.plugin_prompt else { return };
        let mut decision = None;
        let PathPrompt::Library { title, input, .. } = prompt;
        if let Some(path) = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf())) {
            *input = path.display().to_string();
        }
        egui::Modal::new(egui::Id::unique("plugin_prompt")).show(ctx, |ui| {
            ui.set_max_width(520.0);
            ui.heading(title.as_str());
            ui.weak(t!("plugins.pick_library_hint"));
            let r = ui.add(egui::TextEdit::singleline(input).desired_width(500.0));
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                decision = Some(true);
            }
            ui.horizontal(|ui| {
                if ui.button(t!("plugins.ok")).clicked() {
                    decision = Some(true);
                }
                if ui.button(t!("trash.cancel")).clicked() {
                    decision = Some(false);
                }
            });
        });
        match decision {
            Some(true) => {
                let prompt = self.plugin_prompt.take().expect("asked above");
                if let Err(e) = self.answer_prompt(prompt) {
                    self.toast(e);
                }
            }
            Some(false) => {
                // A bundle nobody wants a place for stays out; its panel hears why.
                let PathPrompt::Library { plugin, panel, staged, bundle_title, .. } =
                    self.plugin_prompt.take().expect("asked above");
                let _ = staged.staging.remove();
                let error = t!("plugins.no_library").into_owned();
                self.plugins.bundle_finished(&plugin, &panel, bundle_title, BundleOutcome::Failed { error });
            }
            None => {}
        }
    }

    fn answer_prompt(&mut self, prompt: PathPrompt) -> Result<(), String> {
        match prompt {
            PathPrompt::Library { plugin, panel, apply, staged, bundle_title, input, .. } => {
                let root = PathBuf::from(input.trim());
                if let Err(e) = std::fs::create_dir_all(&root).and_then(|()| root.canonicalize()) {
                    let _ = staged.staging.remove();
                    let error = e.to_string();
                    self.plugins.bundle_finished(
                        &plugin,
                        &panel,
                        bundle_title,
                        BundleOutcome::Failed { error: error.clone() },
                    );
                    return Err(t!("plugins.bundle_failed", error = error).into_owned());
                }
                self.add_library_folder(root);
                if let Some(outcome) = self.settle_staged(&plugin, &panel, apply, *staged) {
                    self.plugins.bundle_finished(&plugin, &panel, bundle_title, outcome);
                }
            }
        }
        Ok(())
    }

    /// Opens the selected files with a plugin: one file as its bytes, or (a file type that
    /// receives ids) the selection as library ids in the plugin's panel.
    pub(super) fn open_with_plugin(&mut self, plugin: &str, paths: &[PathBuf]) {
        let Some((_, _, opener)) = self.plugin_openers_of(paths).into_iter().find(|(id, ..)| id == plugin) else {
            return;
        };
        match (opener.receive, opener.panel) {
            (Receive::Bytes, _) => self.plugins.open_file(plugin, &paths[0]),
            (Receive::Ids, Some(panel)) => {
                let scope = meshgrove_core::ai::Scope::whole(&self.config);
                let ids = paths.iter().filter_map(|p| scope.id_of(p)).collect();
                match self.plugins.open_models(plugin, &panel, ids) {
                    Ok(()) => self.layout.focus_plugin(plugin, &panel),
                    Err(e) => self.toast(format!("{}: {e}", self.plugin_name(plugin))),
                }
            }
            // A manifest that is checked names a panel.
            (Receive::Ids, None) => {}
        }
    }

    /// The plugins that open the selection, with the file type that takes it: (id, name, type).
    /// Bytes are one file on disk; ids are files of the library (also inside archives), any
    /// number of them.
    fn plugin_openers_of(&self, paths: &[PathBuf]) -> Vec<(String, String, FileType)> {
        if paths.is_empty() || paths.iter().any(|p| p.is_dir()) {
            return Vec::new();
        }
        let names: Vec<String> = paths.iter().map(|p| meshgrove_core::listing::file_name(p)).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut openers = self.plugins.openers(&names);
        if openers.is_empty() {
            return openers;
        }
        let scope = meshgrove_core::ai::Scope::whole(&self.config);
        let in_library = paths.iter().all(|p| scope.id_of(p).is_some());
        openers.retain(|(_, _, opener)| match opener.receive {
            Receive::Bytes => paths[0].is_file(),
            Receive::Ids => in_library,
        });
        openers
    }

    /// The plugins that open the selection: (id, the entry in the context menu).
    pub(super) fn plugin_openers(&self, paths: &[PathBuf]) -> Vec<(String, String)> {
        self.plugin_openers_of(paths)
            .into_iter()
            .map(|(id, name, opener)| {
                let entry = opener.label.unwrap_or_else(|| t!("menu.open_with_plugin", name = name).into_owned());
                (id, entry)
            })
            .collect()
    }

    /// A change on the settings' plugins page; `true`: the plugins load again.
    pub(super) fn plugin_setting(&mut self, change: SettingsChange) -> bool {
        match change {
            SettingsChange::InstallPlugin(file) => {
                let installed = self.install_plugin(&file);
                self.settings.plugins_changed();
                match installed {
                    Ok(name) => {
                        self.toast(t!("plugins.installed", name = name).into_owned());
                        self.remember_plugin_folder(&file);
                    }
                    Err(e) => {
                        self.toast(t!("plugins.install_failed", error = e).into_owned());
                        return false;
                    }
                }
            }
            SettingsChange::PickPlugin => {
                if self.plugin_dialog.open.is_none() {
                    let mut dialog = self
                        .plugin_dialog
                        .base
                        .clone()
                        .set_title(t!("plugins.pick_title"))
                        // Linux dialogs match the pattern by case.
                        .add_filter(t!("plugins.filter"), &["wasm", "WASM"]);
                    let start =
                        self.config.plugin_folders.iter().find(|d| d.is_dir()).cloned().or_else(dirs::download_dir);
                    if let Some(dir) = start {
                        dialog = dialog.set_directory(dir);
                    }
                    self.plugin_dialog
                        .ask(DialogFor::Install, move || Box::pin(async move { paths(dialog.pick_file().await) }));
                }
                return false;
            }
            SettingsChange::OpenPluginFolder => {
                let base = self.plugins.base().to_path_buf();
                if let Err(e) =
                    std::fs::create_dir_all(&base).and_then(|()| crate::actions::open_with_default_app(&base))
                {
                    self.toast(format!("{}: {e}", base.display()));
                }
                return false;
            }
            SettingsChange::PluginEnabled { id, enabled } => {
                let hash = self.plugins.rows().iter().find(|r| r.id == id).and_then(|r| r.hash.clone());
                if let Some(entry) = self.config.plugins.iter_mut().find(|p| p.id == id) {
                    entry.enabled = enabled;
                    // Switched on as the file is now.
                    if let (true, Some(hash)) = (enabled, hash) {
                        entry.blake3 = hash;
                    }
                }
            }
            SettingsChange::PluginGrants { id, grants } => {
                if let Some(entry) = self.config.plugins.iter_mut().find(|p| p.id == id) {
                    entry.grants = grants;
                }
            }
            SettingsChange::RemovePlugin(id) => {
                for (plugin, panel) in self.plugins.open_keys() {
                    if plugin == id {
                        self.plugins.close(&plugin, &panel);
                    }
                }
                if let Some(entry) = self.config.plugins.iter().find(|p| p.id == id) {
                    // Only the file Meshgrove installed; data and staging folders stay for undo.
                    if entry.file.starts_with(self.plugins.base()) {
                        let _ = std::fs::remove_file(&entry.file);
                    }
                }
                self.config.plugins.retain(|p| p.id != id);
                // Its file may be offered again from where it came.
                self.settings.plugins_changed();
            }
            _ => return false,
        }
        true
    }

    /// The folder a plugin was installed from goes first in the remembered ones. Not Meshgrove's
    /// plugins folder, Downloads or an import folder: the search looks there anyway, and there
    /// other `.wasm` files are no plugins gone wrong.
    fn remember_plugin_folder(&mut self, file: &Path) {
        let Some(dir) = file.parent().filter(|d| !d.as_os_str().is_empty()) else { return };
        if dir.starts_with(self.plugins.base())
            || dirs::download_dir().is_some_and(|d| d == dir)
            || self.config.import.folders.iter().any(|f| f == dir)
        {
            return;
        }
        let folders = &mut self.config.plugin_folders;
        folders.retain(|d| d != dir);
        folders.insert(0, dir.to_path_buf());
        folders.truncate(PLUGIN_FOLDERS);
    }

    /// Copies the plugin into `<plugins>/<id>/` and sets it up in the config: switched on with
    /// nothing granted; an update keeps the grants, but stays off when it asks for more.
    fn install_plugin(&mut self, file: &Path) -> Result<String, String> {
        let settings = meshgrove_plugins::Settings::new(self.plugins.base());
        let plugin = meshgrove_plugins::Plugin::load(file, settings).map_err(|e| e.to_string())?;
        let m = plugin.manifest();
        let dir = self.plugins.base().join(&m.id);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let target = dir.join(format!("{}.wasm", m.id));
        if file.canonicalize().ok() != target.canonicalize().ok() {
            std::fs::copy(file, &target).map_err(|e| e.to_string())?;
        }
        let asks_more = |old: &PluginConfig| {
            let rows = self.plugins.rows();
            let before = rows.iter().find(|r| r.id == old.id).map(|r| r.permissions.clone()).unwrap_or_default();
            m.permissions.iter().any(|p| !before.contains(p))
        };
        match self.config.plugins.iter().position(|p| p.id == m.id) {
            Some(i) => {
                let more = asks_more(&self.config.plugins[i]);
                let entry = &mut self.config.plugins[i];
                entry.file = target;
                entry.blake3 = plugin.hash().to_owned();
                entry.enabled &= !more;
            }
            None => self.config.plugins.push(PluginConfig {
                id: m.id.clone(),
                file: target,
                blake3: plugin.hash().to_owned(),
                enabled: true,
                grants: Default::default(),
                memory_mb: None,
            }),
        }
        Ok(m.name.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::menu_label;

    #[test]
    fn the_plugins_menu_does_not_say_a_name_twice() {
        // One plugin: its panels by their titles alone.
        assert_eq!(menu_label("Bingrove", "Bingrove", false), "Bingrove");
        assert_eq!(menu_label("Bingrove", "Drawer", false), "Drawer");
        // Several: the plugin in front, so two "Settings" can be told apart …
        assert_eq!(menu_label("Bingrove", "Drawer", true), "Bingrove: Drawer");
        // … but not when the title is the name, or has it in it.
        assert_eq!(menu_label("Texgrove", "Texgrove", true), "Texgrove");
        assert_eq!(menu_label("Texgrove", "texgrove textures", true), "texgrove textures");
    }
}
