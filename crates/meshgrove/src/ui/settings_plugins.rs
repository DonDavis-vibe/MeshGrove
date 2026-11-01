//! Settings → "Plugins": what is installed, what each plugin asks for and what the user grants,
//! switching them on and off, installing a `.wasm` file and removing one.

use std::path::PathBuf;

use egui::{RichText, Ui};
use meshgrove_core::config::{Config, LibraryRead, PluginGrants};
use meshgrove_plugin_api::Permission;

use super::settings::SettingsChange;
use super::settings_kit::{self as kit, hint};

/// An installed plugin as the settings show it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PluginRow {
    pub id: String,
    pub name: String,
    pub version: String,
    /// The plugin API it was built for.
    pub api: String,
    pub description: String,
    pub file: PathBuf,
    /// blake3 of the file as it is now (`None`: it cannot be read).
    pub hash: Option<String>,
    /// What it asks for.
    pub permissions: Vec<Permission>,
    /// The titles of its panels, its file types and its tools.
    pub panels: Vec<String>,
    pub file_types: Vec<String>,
    pub tools: Vec<String>,
    /// Running now.
    pub loaded: bool,
    /// Why it does not run although switched on (or cannot be read at all).
    pub problem: Option<String>,
}

#[derive(Default)]
pub struct PluginSettings {
    error: Option<String>,
    /// "Remove" was clicked once for this plugin.
    confirm_remove: Option<String>,
    /// The plugin search ran since the settings opened (or a plugin came or went).
    searched: bool,
    /// What it found, and the search under way.
    #[cfg(feature = "plugins")]
    found: Vec<meshgrove_plugins::discovery::Found>,
    #[cfg(feature = "plugins")]
    search_rx: Option<crossbeam_channel::Receiver<Vec<meshgrove_plugins::discovery::Found>>>,
}

impl PluginSettings {
    /// The next look at the page searches for plugins again.
    pub fn search_again(&mut self) {
        self.searched = false;
    }

    pub fn page(&mut self, ui: &mut Ui, config: &Config, rows: &[PluginRow], changes: &mut Vec<SettingsChange>) {
        if !cfg!(feature = "plugins") {
            hint(ui, t!("plugins.not_in_build"));
            return;
        }
        #[cfg(feature = "plugins")]
        self.poll_search(config);
        hint(ui, t!("plugins.hint"));
        if rows.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new(t!("plugins.none")).weak());
        }
        for row in rows {
            let Some(entry) = config.plugin(&row.id) else { continue };
            ui.add_space(8.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.push_id(&row.id, |ui| self.plugin(ui, row, entry, changes));
            });
        }
        #[cfg(feature = "plugins")]
        self.found(ui, config, rows, changes);
        // A file dropped on the window is installed.
        if let Some(path) = ui.ctx().input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf())) {
            if path.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("wasm")) {
                changes.push(SettingsChange::InstallPlugin(path));
                self.error = None;
            } else {
                self.error = Some(t!("plugins.not_a_file").into_owned());
            }
        }
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            if ui.button(t!("plugins.browse")).clicked() {
                changes.push(SettingsChange::PickPlugin);
                self.error = None;
            }
            if ui.button(t!("plugins.open_folder")).clicked() {
                changes.push(SettingsChange::OpenPluginFolder);
            }
            #[cfg(feature = "plugins")]
            {
                let searching = self.search_rx.is_some();
                if ui.add_enabled(!searching, egui::Button::new(t!("plugins.search_again"))).clicked() {
                    self.searched = false;
                }
                if searching {
                    ui.spinner();
                    ui.label(RichText::new(t!("plugins.searching")).weak());
                }
            }
        });
        hint(ui, t!("plugins.where"));
        if let Some(e) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
    }

    /// Starts the search when the page shows and it has not run yet; takes in its answer.
    #[cfg(feature = "plugins")]
    fn poll_search(&mut self, config: &Config) {
        if let Some(rx) = &self.search_rx
            && let Ok(found) = rx.try_recv()
        {
            self.found = found;
            self.search_rx = None;
        }
        if self.searched || self.search_rx.is_some() {
            return;
        }
        self.searched = true;
        let Some(base) = meshgrove_plugins::default_base() else { return };
        let (tx, rx) = crossbeam_channel::bounded(1);
        let folders = search_folders(config, &base);
        let installed: Vec<String> = config.plugins.iter().map(|p| p.blake3.clone()).collect();
        let language = if rust_i18n::locale().starts_with("de") { "de" } else { "en" };
        let mut settings = meshgrove_plugins::Settings::new(&base);
        settings.info = meshgrove_plugins::registry::host_info(config, language, false);
        std::thread::spawn(move || {
            let _ = tx.send(meshgrove_plugins::discovery::discover(&folders, &settings, &installed));
        });
        self.search_rx = Some(rx);
    }

    /// The plugins the search found that are not installed, or newer than the installed ones,
    /// and the files in the plugins folders that are none.
    #[cfg(feature = "plugins")]
    fn found(&mut self, ui: &mut Ui, config: &Config, rows: &[PluginRow], changes: &mut Vec<SettingsChange>) {
        use meshgrove_plugins::discovery::older;
        let mut offers = Vec::new();
        let mut problems = Vec::new();
        for found in &self.found {
            match &found.plugin {
                Ok(offer) => {
                    let installed = config
                        .plugin(&offer.id)
                        .map(|_| rows.iter().find(|r| r.id == offer.id).map(|r| r.version.clone()).unwrap_or_default());
                    // An older file than the one installed is no offer.
                    if installed.as_deref().is_some_and(|v| older(&offer.version, v)) {
                        continue;
                    }
                    offers.push((found, offer, installed));
                }
                Err(problem) => problems.push((found, problem)),
            }
        }
        if offers.is_empty() && problems.is_empty() {
            if self.searched && self.search_rx.is_none() {
                ui.add_space(8.0);
                ui.label(RichText::new(t!("plugins.found_none")).weak());
            }
            return;
        }
        ui.add_space(12.0);
        ui.label(RichText::new(t!("plugins.found")).strong());
        for (found, offer, installed) in offers {
            ui.add_space(6.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.push_id(&found.path, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&offer.name).strong());
                        ui.label(RichText::new(format!("{} · {}", offer.version, offer.id)).weak());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let label = if installed.is_some() { t!("plugins.update") } else { t!("plugins.install") };
                            if ui.button(label).clicked() {
                                changes.push(SettingsChange::InstallPlugin(found.path.clone()));
                                self.error = None;
                            }
                        });
                    });
                    if !offer.description.is_empty() {
                        hint(ui, offer.description.clone());
                    }
                    let mut facts = vec![found.path.display().to_string()];
                    if let Some(version) = installed {
                        facts.push(t!("plugins.installed_version", version = version).into_owned());
                    }
                    ui.label(RichText::new(facts.join(" · ")).small().weak());
                });
            });
        }
        for (found, problem) in problems {
            let name = found.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let problem: String = problem.chars().take(200).collect();
            ui.label(RichText::new(format!("{name}: {problem}")).small().weak())
                .on_hover_text(found.path.display().to_string());
        }
    }

    fn plugin(
        &mut self,
        ui: &mut Ui,
        row: &PluginRow,
        entry: &meshgrove_core::config::PluginConfig,
        changes: &mut Vec<SettingsChange>,
    ) {
        let changed_file = row.hash.as_deref().is_some_and(|h| !h.eq_ignore_ascii_case(&entry.blake3));
        ui.horizontal(|ui| {
            let mut on = entry.enabled && !changed_file;
            if ui.checkbox(&mut on, RichText::new(&row.name).strong()).changed() {
                changes.push(SettingsChange::PluginEnabled { id: row.id.clone(), enabled: on });
            }
            ui.label(RichText::new(format!("{} · {}", row.version, row.id)).weak());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let confirm = self.confirm_remove.as_deref() == Some(row.id.as_str());
                let label = if confirm { t!("plugins.remove_confirm") } else { t!("plugins.remove") };
                if ui.small_button(label).clicked() {
                    if confirm {
                        changes.push(SettingsChange::RemovePlugin(row.id.clone()));
                        self.confirm_remove = None;
                    } else {
                        self.confirm_remove = Some(row.id.clone());
                    }
                }
            });
        });
        if !row.description.is_empty() {
            hint(ui, row.description.clone());
        }
        if changed_file && entry.enabled {
            ui.colored_label(ui.visuals().warn_fg_color, t!("plugins.changed"));
        } else if let Some(problem) = &row.problem {
            ui.colored_label(ui.visuals().error_fg_color, problem);
        }
        let mut grants = entry.grants.clone();
        let wants = |p: Permission| row.permissions.contains(&p);
        if wants(Permission::AiTools) {
            kit::check(ui, &mut grants.ai_tools, t!("plugins.grant_ai_tools"), t!("plugins.grant_ai_tools_hint"));
        }
        if wants(Permission::Bundles) {
            kit::check(ui, &mut grants.bundles, t!("plugins.grant_bundles"), t!("plugins.grant_bundles_hint"));
            ui.add_enabled_ui(grants.bundles, |ui| {
                kit::check(ui, &mut grants.auto_accept, t!("plugins.grant_auto"), t!("plugins.grant_auto_hint"));
            });
        }
        if wants(Permission::LibraryRead) {
            kit::row(ui, &t!("plugins.grant_library"), |ui| {
                let options = [
                    (LibraryRead::None, t!("plugins.library_none").into_owned()),
                    (LibraryRead::AiScope, t!("plugins.library_ai").into_owned()),
                    (LibraryRead::All, t!("plugins.library_all").into_owned()),
                ];
                kit::segmented(ui, &mut grants.library_read, &options);
            });
        }
        if grants != entry.grants {
            changes.push(SettingsChange::PluginGrants { id: row.id.clone(), grants: normalized(grants) });
        }
        let mut facts = vec![t!("plugins.api", version = row.api.clone()).into_owned()];
        if !row.panels.is_empty() {
            facts.push(t!("plugins.panels", names = row.panels.join(", ")).into_owned());
        }
        if !row.tools.is_empty() {
            facts.push(t!("plugins.tools", names = row.tools.join(", ")).into_owned());
        }
        if !row.file_types.is_empty() {
            facts.push(row.file_types.join(", "));
        }
        if let Some(hash) = &row.hash {
            facts.push(format!("blake3 {}", &hash[..hash.len().min(12)]));
        }
        ui.label(RichText::new(facts.join(" · ")).small().weak());
    }
}

/// Where the plugin search looks: Meshgrove's plugins folder and the folders plugins came from
/// (problems reported), then Downloads and the import folders (only plugins).
#[cfg(feature = "plugins")]
fn search_folders(config: &Config, base: &std::path::Path) -> Vec<meshgrove_plugins::discovery::Folder> {
    use meshgrove_plugins::discovery::Folder;
    let put = std::iter::once(base.to_path_buf()).chain(config.plugin_folders.iter().cloned());
    let other = dirs::download_dir().into_iter().chain(config.import.folders.iter().cloned());
    let mut folders: Vec<Folder> = Vec::new();
    for (path, report_problems) in put.map(|p| (p, true)).chain(other.map(|p| (p, false))) {
        if !folders.iter().any(|f| f.path == path) {
            folders.push(Folder { path, report_problems });
        }
    }
    folders
}

/// Auto-accept means nothing without bundles.
fn normalized(mut grants: PluginGrants) -> PluginGrants {
    grants.auto_accept &= grants.bundles;
    grants
}
