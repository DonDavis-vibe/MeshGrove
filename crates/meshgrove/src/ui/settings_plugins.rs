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
    /// The path of a `.wasm` file to install, as typed or dropped.
    install: String,
    error: Option<String>,
    /// "Remove" was clicked once for this plugin.
    confirm_remove: Option<String>,
}

impl PluginSettings {
    pub fn page(&mut self, ui: &mut Ui, config: &Config, rows: &[PluginRow], changes: &mut Vec<SettingsChange>) {
        if !cfg!(feature = "plugins") {
            hint(ui, t!("plugins.not_in_build"));
            return;
        }
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
        ui.add_space(10.0);
        // A file dropped on the window lands in the field.
        if let Some(path) = ui.ctx().input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf())) {
            self.install = path.display().to_string();
        }
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.install)
                    .hint_text(t!("plugins.install_hint"))
                    .desired_width(340.0),
            );
            if ui.button(t!("plugins.install")).clicked() {
                let path = PathBuf::from(self.install.trim());
                if path.is_file() {
                    changes.push(SettingsChange::InstallPlugin(path));
                    self.install.clear();
                    self.error = None;
                } else {
                    self.error = Some(t!("plugins.not_a_file").into_owned());
                }
            }
        });
        if let Some(e) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
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

/// Auto-accept means nothing without bundles.
fn normalized(mut grants: PluginGrants) -> PluginGrants {
    grants.auto_accept &= grants.bundles;
    grants
}
