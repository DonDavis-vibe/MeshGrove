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
use meshgrove_plugin_api::ui::{BundleOutcome, Effect, Event, Level};
use meshgrove_plugin_api::{Bundle, HostInfo, Permission};
use meshgrove_plugins::{StagedBundle, Staging};

use super::App;
use crate::plugins::{BundleRoute, PanelAction, settle_bundle};
use crate::ui::plugin_view::{self, MeshViewArgs};
use crate::ui::settings::SettingsChange;

/// A path the user is asked for (the app has no system file dialogs).
pub(super) enum PathPrompt {
    /// A file for a panel (`Effect::OpenFileDialog`).
    Open { plugin: String, panel: String, id: String, title: String, input: String, extensions: Vec<String> },
    /// Where to save a file a plugin wrote (`Effect::SaveFile`).
    Save { from: PathBuf, input: String },
}

/// The system's file dialog for picking a plugin (rfd: the desktop portal on Linux). It is asked
/// from the UI thread and its answer polled each frame, so the window keeps drawing meanwhile.
pub(super) struct PluginDialog {
    /// Set up with the window as its parent.
    base: rfd::AsyncFileDialog,
    open: Option<Pin<Box<dyn Future<Output = Option<rfd::FileHandle>>>>>,
}

impl PluginDialog {
    pub(super) fn new(cc: &eframe::CreationContext<'_>) -> PluginDialog {
        PluginDialog { base: rfd::AsyncFileDialog::new().set_parent(cc), open: None }
    }
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
        if let Some(open) = &mut self.plugin_dialog.open {
            let waker = Waker::from(Arc::new(Repaint(self.egui_ctx.clone())));
            if let Poll::Ready(picked) = open.as_mut().poll(&mut Context::from_waker(&waker)) {
                self.plugin_dialog.open = None;
                if let Some(file) = picked {
                    self.apply_settings(vec![SettingsChange::InstallPlugin(file.path().to_path_buf())]);
                }
            }
        }
        // Language, theme or printers changed: the plugins hear of it (tools load anew with it).
        let info = self.plugin_info();
        if self.plugins.info().is_some_and(|known| *known != info) {
            self.plugins.host_changed(&info);
            self.plugins.reload(&self.config, info, &self.db_path);
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
            PanelAction::Bundle { plugin, panel, apply, bundle, staging } => {
                let title = bundle.title.clone();
                let outcome = match staging {
                    Ok(staging) => self.plugin_bundle(&plugin, &panel, apply, bundle, staging),
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
            Effect::SelectModel { id } => {
                let scope = meshgrove_core::ai::Scope::new(&self.config, AccessClass::Local);
                match scope.resolve(&id) {
                    Some(path) => self.reveal(&path),
                    None => self.toast(t!("plugins.unknown_model", id = id).into_owned()),
                }
            }
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
                                let dir = dirs::download_dir().or_else(dirs::home_dir).unwrap_or_default();
                                let file =
                                    meshgrove_core::fsops::check_name(&file).map_or("plugin-file".into(), |()| file);
                                let input = dir.join(file).display().to_string();
                                self.plugin_prompt = Some(PathPrompt::Save { from, input });
                            }
                            None => self.toast(format!("{name}: {}", t!("plugins.no_such_file", path = path))),
                        }
                    }
                    Effect::OpenFileDialog { id, title, extensions } => {
                        let input = self.nav.current().display().to_string();
                        let title = title.unwrap_or_else(|| t!("plugins.pick_file").into_owned());
                        let (plugin, panel) = (plugin.to_owned(), panel.to_owned());
                        self.plugin_prompt = Some(PathPrompt::Open { plugin, panel, id, title, input, extensions });
                    }
                    _ => {}
                }
            }
        }
    }

    /// Files from a panel: into the library now, or into the review. The outcome, or `None`
    /// when the library's thread will answer (see `applied`).
    fn plugin_bundle(
        &mut self,
        plugin: &str,
        panel: &str,
        apply: bool,
        bundle: Bundle,
        staging: Staging,
    ) -> Option<BundleOutcome> {
        let allowed = self.plugins.plugin(plugin).is_some_and(|p| p.may(Permission::Bundles));
        let staged = StagedBundle { plugin: plugin.to_owned(), bundle, staging };
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
        if self.config.import_root().is_none()
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

    /// The tab of a plugin panel.
    pub(super) fn plugin_pane(&mut self, ui: &mut Ui, rs: &egui_wgpu::RenderState, plugin: &str, panel: &str) {
        let renderer = &self.renderer;
        let space = self.space.as_ref();
        let Some(open) = self.plugins.panels.iter_mut().find(|p| p.plugin == plugin && p.panel == panel) else {
            ui.weak(t!("plugins.loading"));
            return;
        };
        open.upload_meshes(&rs.device);
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
                    if ui.small_button("✕").on_hover_text(t!("plugins.cancel_job")).clicked() {
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
            ui.allocate_ui(size, |ui| {
                ui.set_min_size(size);
                if let Some(input) = space {
                    m.viewer.apply_space(input);
                }
                m.viewer.ui(ui, rs, renderer);
            });
        };
        let events = plugin_view::show(ui, &view, &mut open.ui, &mut draw_mesh);
        for event in events {
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
                let label = if several { format!("{name}: {title}") } else { title.clone() };
                if ui.button(label).clicked() {
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
        if let Some(path) = ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf())) {
            match prompt {
                PathPrompt::Open { input, .. } | PathPrompt::Save { input, .. } => *input = path.display().to_string(),
            }
        }
        egui::Modal::new(egui::Id::unique("plugin_prompt")).show(ctx, |ui| {
            ui.set_max_width(520.0);
            let (title, input) = match prompt {
                PathPrompt::Open { title, input, .. } => (title.clone(), input),
                PathPrompt::Save { input, .. } => (t!("plugins.save_as").into_owned(), input),
            };
            ui.heading(title);
            ui.weak(t!("plugins.path_hint"));
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
            Some(false) => self.plugin_prompt = None,
            None => {}
        }
    }

    fn answer_prompt(&mut self, prompt: PathPrompt) -> Result<(), String> {
        match prompt {
            PathPrompt::Save { from, input } => {
                let to = PathBuf::from(input.trim());
                if to.exists() {
                    return Err(t!("plugins.exists", path = to.display().to_string()).into_owned());
                }
                std::fs::copy(&from, &to).map_err(|e| e.to_string())?;
                self.toast(t!("plugins.saved", path = to.display().to_string()).into_owned());
            }
            PathPrompt::Open { plugin, panel, id, input, extensions, .. } => {
                let from = PathBuf::from(input.trim());
                let name = meshgrove_core::listing::file_name(&from);
                let ext_ok = extensions.is_empty()
                    || extensions.iter().any(|e| name.to_lowercase().ends_with(&format!(".{}", e.to_lowercase())));
                if !from.is_file() || !ext_ok {
                    return Err(t!("plugins.not_a_file").into_owned());
                }
                let open = self.plugins.find(&plugin, &panel).ok_or("the panel is closed")?;
                let tmp = open.tmp().ok_or("the panel is not open yet")?.to_path_buf();
                std::fs::copy(&from, tmp.join(&name)).map_err(|e| e.to_string())?;
                open.send(Event::FilePicked { id, path: format!("/tmp/{name}"), name });
            }
        }
        Ok(())
    }

    /// Opens a library file with a plugin.
    pub(super) fn open_with_plugin(&mut self, plugin: &str, path: &Path) {
        self.plugins.open_file(plugin, path);
    }

    /// The plugins that open the one selected file: (id, name).
    pub(super) fn plugin_openers(&self, paths: &[PathBuf]) -> Vec<(String, String)> {
        match paths {
            [p] if p.is_file() => self.plugins.openers(&meshgrove_core::listing::file_name(p)),
            _ => Vec::new(),
        }
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
                    self.plugin_dialog.open = Some(Box::pin(dialog.pick_file()));
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
