//! Settings → "KI & MCP": switching AI on, what each library shows to which AI, setting up the
//! user's assistant, and what it looked at.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use egui::{RichText, Ui};
use meshgrove_core::ai::proposals::{AccessEntry, ProposalStore};
use meshgrove_core::ai::snippets::{self, Client};
use meshgrove_core::config::{AccessClass, AiAccess, Config};
use meshgrove_core::listing;

use super::settings::SettingsChange;
use crate::i18n;

/// The AI part of the settings window's state.
pub struct AiSettings {
    client: Client,
    local: bool,
    /// The newest accesses and the suggestions waiting, read when the page is opened.
    log: Vec<AccessEntry>,
    pending: usize,
    loaded: bool,
    /// "Delete AI data" was clicked once.
    confirm_clear: bool,
}

impl Default for AiSettings {
    fn default() -> AiSettings {
        AiSettings { client: Client::ClaudeDesktop, local: false, log: Vec::new(), pending: 0, loaded: false, confirm_clear: false }
    }
}

impl AiSettings {
    /// Reads the log again next time the page shows.
    pub fn reset(&mut self) {
        self.loaded = false;
        self.confirm_clear = false;
    }

    fn load(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let store = ProposalStore::default_location().filter(|p| p.exists()).and_then(|p| ProposalStore::open(&p).ok());
        self.log = store.as_ref().and_then(|s| s.access_log(20).ok()).unwrap_or_default();
        self.pending = store.as_ref().and_then(|s| s.pending_count().ok()).unwrap_or(0);
    }

    pub fn main(&mut self, ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
        self.load();
        ui.label(RichText::new(t!("settings.ai")).strong());
        let mut on = config.ai.enabled;
        if ui.checkbox(&mut on, t!("settings.ai_enable")).changed() {
            changes.push(SettingsChange::AiEnabled(on));
        }
        ui.weak(t!("settings.ai_hint"));
        if config.ai.enabled {
            ui.horizontal(|ui| {
                if ui.button(t!("settings.ai_review", count = self.pending)).clicked() {
                    changes.push(SettingsChange::ShowAiReview);
                }
                if ui.button(t!("settings.ai_history")).clicked() {
                    changes.push(SettingsChange::ShowAiHistory);
                }
            });
        }
    }

    pub fn libraries(&mut self, ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
        ui.label(RichText::new(t!("settings.ai_libraries")).strong());
        if !config.ai.enabled {
            ui.weak(t!("settings.ai_off"));
            return;
        }
        if config.library_roots.is_empty() {
            ui.weak(t!("settings.ai_no_libraries"));
        }
        egui::Grid::new("ai_libraries").num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
            for root in &config.library_roots {
                let mut library = config.ai.library(root);
                let before = library.clone();
                ui.label(listing::file_name(root)).on_hover_text(root.display().to_string());
                egui::ComboBox::from_id_salt(("ai_access", root))
                    .selected_text(access_name(library.access))
                    .show_ui(ui, |ui| {
                        for access in [AiAccess::None, AiAccess::LocalOnly, AiAccess::Cloud] {
                            ui.selectable_value(&mut library.access, access, access_name(access));
                        }
                    });
                ui.add_enabled(library.access != AiAccess::None, egui::Checkbox::new(&mut library.images, t!("settings.ai_images")))
                    .on_hover_text(t!("settings.ai_images_hint"));
                ui.end_row();
                if library != before {
                    changes.push(SettingsChange::AiLibrary(library));
                }
            }
        });
        ui.weak(t!("settings.ai_libraries_hint"));
    }

    pub fn mcp(&mut self, ui: &mut Ui, config: &Config) {
        ui.label(RichText::new(t!("settings.mcp")).strong());
        ui.weak(t!("settings.mcp_hint"));
        if !config.ai.enabled {
            ui.weak(t!("settings.ai_off"));
            return;
        }
        let (exe, appimage) = mcp_binary();
        match (&appimage, exe.exists()) {
            (Some(image), _) => ui.weak(t!("settings.mcp_found", path = image.display().to_string())),
            (None, true) => ui.weak(t!("settings.mcp_found", path = exe.display().to_string())),
            (None, false) => ui.colored_label(ui.visuals().warn_fg_color, t!("settings.mcp_missing", path = exe.display().to_string())),
        };
        ui.horizontal(|ui| {
            ui.label(t!("settings.mcp_client"));
            egui::ComboBox::from_id_salt("mcp_client").selected_text(self.client.name()).show_ui(ui, |ui| {
                for client in Client::ALL {
                    ui.selectable_value(&mut self.client, client, client.name());
                }
            });
        });
        ui.radio_value(&mut self.local, false, t!("settings.mcp_cloud"));
        ui.radio_value(&mut self.local, true, t!("settings.mcp_local"));
        let class = if self.local { AccessClass::Local } else { AccessClass::Cloud };
        let mut snippet = snippets::snippet(self.client, &exe, appimage.as_deref(), class);
        ui.add(
            egui::TextEdit::multiline(&mut snippet)
                .code_editor()
                .interactive(true)
                .desired_rows(if self.client.is_command() { 2 } else { 8 })
                .desired_width(f32::INFINITY),
        );
        if ui.button(t!("settings.mcp_copy")).clicked() {
            ui.ctx().copy_text(snippet.clone());
        }
        // Below the button, not beside it: text in a row would not wrap and widen the window.
        if self.client.is_command() {
            ui.weak(t!("settings.mcp_paste_terminal"));
        } else if let Some(file) = self.client.config_files().first() {
            ui.weak(t!("settings.mcp_paste_file", path = file.display().to_string()));
        }
        ui.weak(t!("settings.mcp_never_written"));
    }

    pub fn log(&mut self, ui: &mut Ui, changes: &mut Vec<SettingsChange>) {
        self.load();
        ui.label(RichText::new(t!("settings.ai_log")).strong());
        if self.log.is_empty() {
            ui.weak(t!("settings.ai_log_empty"));
        } else {
            egui::Grid::new("ai_log").num_columns(5).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
                for e in &self.log {
                    let at = SystemTime::UNIX_EPOCH + Duration::from_nanos(e.at_ns.max(0) as u64);
                    ui.weak(i18n::datetime(at));
                    let client = e.client.as_deref().map(client_name).unwrap_or_else(|| e.source.clone());
                    ui.label(format!("{client} · {}", if e.access_class == "local" { t!("review.local") } else { t!("review.cloud") }));
                    ui.monospace(&e.tool);
                    let mut what = t!("settings.ai_log_items", count = e.items).into_owned();
                    if e.images > 0 {
                        what += &format!(" · {}", t!("settings.ai_log_images", count = e.images));
                    }
                    ui.weak(what);
                    ui.weak(e.libraries.join(", "));
                    ui.end_row();
                }
            });
        }
        ui.horizontal(|ui| {
            if ui.button(t!("settings.ai_log_refresh")).clicked() {
                self.loaded = false;
            }
            let label = if self.confirm_clear { t!("settings.ai_clear_confirm") } else { t!("settings.ai_clear") };
            if ui.button(label).on_hover_text(t!("settings.ai_clear_hint")).clicked() {
                if self.confirm_clear {
                    changes.push(SettingsChange::ClearAiData);
                    self.confirm_clear = false;
                    self.loaded = false;
                } else {
                    self.confirm_clear = true;
                }
            }
        });
    }
}

fn access_name(access: AiAccess) -> String {
    match access {
        AiAccess::None => t!("settings.ai_access_none"),
        AiAccess::LocalOnly => t!("settings.ai_access_local"),
        AiAccess::Cloud => t!("settings.ai_access_cloud"),
    }
    .into_owned()
}

fn client_name(id: &str) -> String {
    Client::ALL.into_iter().find(|c| c.id() == id).map_or_else(|| id.to_owned(), |c| c.name().to_owned())
}

/// `meshgrove-mcp` next to this program, and the AppImage this runs from (if it does).
pub fn mcp_binary() -> (PathBuf, Option<PathBuf>) {
    let name = if cfg!(windows) { "meshgrove-mcp.exe" } else { "meshgrove-mcp" };
    let exe = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(name))).unwrap_or_else(|| name.into());
    (exe, std::env::var_os("APPIMAGE").map(PathBuf::from))
}
