//! Settings → "KI & MCP": switching AI on, what each library shows to which AI, who answers the
//! chat (an agent found on this computer, or the user's own endpoint), connecting the AI apps,
//! and what they looked at.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crossbeam_channel::{Receiver, bounded};
use egui::Ui;
use meshgrove_core::ai::agent_setup::{Change, Outcome};
use meshgrove_core::ai::guess_local;
use meshgrove_core::ai::proposals::{AccessEntry, ProposalStore};
use meshgrove_core::ai::snippets::{self, Client};
use meshgrove_core::config::{AUTO_CONNECT_LAST, AccessClass, AgentConfig, AiAccess, Config, EndpointConfig};
use meshgrove_core::listing;

use super::picker::{self, Choice, Picker};
use super::settings::SettingsChange;
use super::settings_kit::{self as kit, check, hint, row};
use crate::app::ModelList;
use crate::assist::agents::{self, Detection, Login};
use crate::assist::connect;
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
    /// The endpoint as being edited, the key typed, and the connection test.
    form: Option<EndpointConfig>,
    key: String,
    test: Option<Receiver<Result<Vec<String>, String>>>,
    /// The endpoint's models, listed for the address in `listed_for` (the picker lists by itself
    /// once the form shows, and again after a test).
    models: ModelList,
    listed_for: Option<String>,
    tested: Option<Result<String, String>>,
    /// The assistants found and what connecting would do for each, read when the page shows.
    assistants: Option<Vec<Outcome>>,
    /// An agent being added by hand.
    new_agent: AgentConfig,
}

impl Default for AiSettings {
    fn default() -> AiSettings {
        AiSettings {
            client: Client::ClaudeDesktop,
            local: false,
            log: Vec::new(),
            pending: 0,
            loaded: false,
            confirm_clear: false,
            form: None,
            key: String::new(),
            test: None,
            models: ModelList::default(),
            listed_for: None,
            tested: None,
            assistants: None,
            new_agent: AgentConfig::default(),
        }
    }
}

impl AiSettings {
    /// Reads the log again next time the page shows.
    pub fn reset(&mut self) {
        self.loaded = false;
        self.confirm_clear = false;
        self.form = None;
        self.key.clear();
        self.tested = None;
        self.listed_for = None;
        self.assistants = None;
    }

    /// Who answers the chat: the agents found on this computer and the ones added by hand,
    /// the endpoint when set up; one of them picked.
    pub fn assistant(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        detection: Option<&Detection>,
        changes: &mut Vec<SettingsChange>,
    ) {
        hint(ui, t!("assistant.hint"));
        if !config.ai.enabled {
            return;
        }
        let entries = agents::entries(&config.ai, detection);
        let current = agents::current(&config.ai, detection).map(|e| e.id).unwrap_or_default();
        if detection.is_none() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak(t!("assistant.looking"));
            });
        }
        if entries.is_empty() {
            hint(ui, t!("assistant.none"));
        }
        for e in &entries {
            ui.horizontal(|ui| {
                if ui.radio(e.id == current, &e.name).clicked() && e.id != current {
                    changes.push(SettingsChange::Assistant(e.id.clone()));
                }
                ui.weak(&e.detail);
                if e.found
                    && let Some(f) = detection.and_then(|d| d.found.iter().find(|f| f.id == e.id))
                {
                    let (text, color) = match f.login {
                        Login::LooksLoggedIn => (t!("assistant.login_found"), kit::GOOD),
                        Login::KeyInEnvironment => (t!("assistant.login_key"), kit::GOOD),
                        Login::NotLoggedIn => (t!("assistant.login_missing"), ui.visuals().warn_fg_color),
                        Login::Unknown => (t!("assistant.login_unknown"), ui.visuals().weak_text_color()),
                    };
                    kit::pill(ui, text, color).on_hover_text(f.argv.join(" "));
                }
                if e.is_endpoint() {
                    kit::pill(ui, t!("assistant.endpoint_pill"), ui.visuals().weak_text_color());
                }
                if !e.found && !e.is_endpoint() && ui.small_button("✖").on_hover_text(t!("assistant.remove")).clicked()
                {
                    changes.push(SettingsChange::RemoveAgent(e.id.clone()));
                }
            });

            if let Some(problem) = &e.problem {
                hint(ui, t!("assistant.cannot_start", error = problem.clone()));
            }
        }
        // What happens when the chat opens: wait for Connect, connect the last pick, or one agent.
        let agents: Vec<&agents::Entry> = entries.iter().filter(|e| !e.is_endpoint() && e.problem.is_none()).collect();
        if !agents.is_empty() {
            let current = config.ai.auto_connect.as_str();
            let label = |value: &str| -> String {
                match value {
                    "" => t!("assistant.auto_connect_off").into_owned(),
                    AUTO_CONNECT_LAST => t!("assistant.auto_connect_last").into_owned(),
                    id => agents.iter().find(|e| e.id == id).map_or_else(|| id.to_owned(), |e| e.name.clone()),
                }
            };
            row(ui, &t!("assistant.auto_connect"), |ui| {
                egui::ComboBox::from_id_salt("auto_connect").selected_text(label(current)).width(220.0).show_ui(
                    ui,
                    |ui| {
                        for value in ["", AUTO_CONNECT_LAST].into_iter().chain(agents.iter().map(|e| e.id.as_str())) {
                            if ui.selectable_label(value == current, label(value)).clicked() && value != current {
                                changes.push(SettingsChange::AutoConnect(value.to_owned()));
                            }
                        }
                    },
                );
            });
            hint(ui, t!("assistant.auto_connect_hint"));
        }
        // The known agents not found, and how to get them.
        if let Some(d) = detection {
            let missing: Vec<&agents::Known> =
                agents::KNOWN.iter().filter(|k| !d.found.iter().any(|f| f.id == k.id)).collect();
            if !missing.is_empty() {
                egui::CollapsingHeader::new(t!("assistant.missing", count = missing.len())).show(ui, |ui| {
                    for k in missing {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(k.name);
                            ui.weak(k.install);
                        });
                    }
                    hint(ui, t!("assistant.missing_hint"));
                });
            }
        }
        if config.ai.endpoint.is_none() {
            hint(ui, t!("assistant.endpoint_hint"));
        }
        ui.add_space(4.0);
        egui::CollapsingHeader::new(t!("assistant.add")).show(ui, |ui| {
            row(ui, &t!("assistant.add_name"), |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.new_agent.name).desired_width(220.0));
            });
            row(ui, &t!("assistant.add_command"), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_agent.command)
                        .hint_text("opencode acp")
                        .desired_width(300.0),
                );
            });
            ui.checkbox(&mut self.new_agent.local, t!("endpoint.local")).on_hover_text(t!("endpoint.local_hint"));
            let ready = !self.new_agent.command.trim().is_empty();
            if ui.add_enabled(ready, egui::Button::new(t!("assistant.add_button"))).clicked() {
                let mut agent = std::mem::take(&mut self.new_agent);
                agent.id = agents::new_id(&config.ai);
                agent.command = agent.command.trim().to_owned();
                if agent.name.trim().is_empty() {
                    agent.name = agents::short_command(&agent.command);
                }
                changes.push(SettingsChange::AddAgent(agent));
            }
            hint(ui, t!("assistant.add_hint"));
        });
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
        let mut on = config.ai.enabled;
        if check(ui, &mut on, t!("settings.ai_enable"), t!("settings.ai_hint")) {
            changes.push(SettingsChange::AiEnabled(on));
        }
        if config.ai.enabled {
            let mut auto = config.ai.auto_accept_since.is_some();
            if check(ui, &mut auto, t!("settings.ai_auto_accept"), t!("settings.ai_auto_accept_hint")) {
                changes.push(SettingsChange::AiAutoAccept(auto));
            }
            let mut cross = config.ai.cross_library_moves;
            if check(ui, &mut cross, t!("settings.ai_cross_moves"), t!("settings.ai_cross_moves_hint")) {
                changes.push(SettingsChange::AiCrossLibraryMoves(cross));
            }
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
        if config.library_roots.is_empty() {
            hint(ui, t!("settings.ai_no_libraries"));
        } else {
            self.library_grid(ui, config, changes);
        }
        hint(ui, t!("settings.ai_libraries_hint"));
    }

    fn library_grid(&self, ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
        egui::Grid::new("ai_libraries").num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
            for root in &config.library_roots {
                let mut library = config.ai.library(root);
                let before = library.clone();
                ui.label(listing::file_name(root)).on_hover_text(root.display().to_string());
                egui::ComboBox::from_id_salt(("ai_access", root)).selected_text(access_name(library.access)).show_ui(
                    ui,
                    |ui| {
                        for access in [AiAccess::None, AiAccess::LocalOnly, AiAccess::Cloud] {
                            ui.selectable_value(&mut library.access, access, access_name(access));
                        }
                    },
                );
                ui.add_enabled(
                    library.access != AiAccess::None,
                    egui::Checkbox::new(&mut library.images, t!("settings.ai_images")),
                )
                .on_hover_text(t!("settings.ai_images_hint"));
                ui.end_row();
                if library != before {
                    changes.push(SettingsChange::AiLibrary(library));
                }
            }
        });
    }

    /// The AI assistants on this computer: Meshgrove adds itself to them (the idea and its rules
    /// come from SolidCraft), and the entry to copy for setting one up by hand.
    pub fn mcp(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        (armed, busy): (bool, bool),
        changes: &mut Vec<SettingsChange>,
    ) {
        if !config.ai.enabled {
            return;
        }
        hint(ui, t!("settings.mcp_hint"));
        let (exe, appimage) = mcp_binary();
        let server = appimage.clone().unwrap_or_else(|| exe.clone());
        if appimage.is_none() && !exe.exists() {
            ui.colored_label(ui.visuals().warn_fg_color, t!("settings.mcp_missing", path = exe.display().to_string()));
        }
        ui.add_space(2.0);
        let mut on = config.ai.connect_apps;
        if check(ui, &mut on, t!("settings.mcp_connect_apps"), t!("settings.mcp_connect_hint")) {
            changes.push(SettingsChange::AiConnectApps(on));
            self.assistants = None;
        }
        if !armed {
            hint(ui, t!("settings.mcp_connect_off_here"));
        }
        // What is there, read again once a run has finished.
        if busy {
            self.assistants = None;
        } else if self.assistants.is_none() {
            self.assistants = Some(connect::status());
        }
        ui.add_space(4.0);
        match &self.assistants {
            None => {
                ui.spinner();
            }
            Some(found) if found.is_empty() => hint(ui, t!("settings.mcp_none_found")),
            Some(found) => {
                for o in found {
                    let (text, color) = match &o.result {
                        Ok(Some(Change::Unchanged)) => (t!("settings.mcp_state_connected"), kit::GOOD),
                        Ok(Some(Change::Updated)) => (t!("settings.mcp_state_stale"), ui.visuals().text_color()),
                        Ok(Some(Change::Custom)) => (t!("settings.mcp_state_custom"), ui.visuals().text_color()),
                        Err(_) => (t!("settings.mcp_state_unreadable"), ui.visuals().warn_fg_color),
                        _ => (t!("settings.mcp_state_not_yet"), ui.visuals().weak_text_color()),
                    };
                    row(ui, o.name(), |ui| {
                        let pill = kit::pill(ui, text, color);
                        match &o.result {
                            Err(e) => pill.on_hover_text(format!("{}\n{e}", o.file.display())),
                            Ok(_) => pill.on_hover_text(o.file.display().to_string()),
                        };
                    });
                }
                let missing = found.iter().any(|o| matches!(o.result, Ok(Some(Change::Added | Change::Updated))));
                ui.add_space(2.0);
                if ui
                    .add_enabled(armed && !busy && missing, egui::Button::new(t!("settings.mcp_connect_now")))
                    .clicked()
                {
                    changes.push(SettingsChange::AiConnectNow);
                    self.assistants = None;
                }
            }
        }
        let none_found = self.assistants.as_ref().is_some_and(Vec::is_empty);
        egui::CollapsingHeader::new(t!("settings.mcp_by_hand")).default_open(none_found).show(ui, |ui| {
            hint(ui, t!("settings.mcp_found", path = server.display().to_string()));
            row(ui, &t!("settings.mcp_client"), |ui| {
                egui::ComboBox::from_id_salt("mcp_client").selected_text(self.client.name()).show_ui(ui, |ui| {
                    for client in Client::ALL {
                        ui.selectable_value(&mut self.client, client, client.name());
                    }
                });
            });
            row(ui, &t!("settings.mcp_model_runs"), |ui| {
                ui.radio_value(&mut self.local, false, t!("settings.mcp_cloud"));
                ui.radio_value(&mut self.local, true, t!("settings.mcp_local"));
            });
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
                hint(ui, t!("settings.mcp_paste_terminal"));
            } else if let Some(file) = self.client.config_files().first() {
                hint(ui, t!("settings.mcp_paste_file", path = file.display().to_string()));
            }
        });
    }

    /// The user's own OpenAI-compatible endpoint for the chat and the one-click actions.
    pub fn endpoint(&mut self, ui: &mut Ui, config: &Config, changes: &mut Vec<SettingsChange>) {
        hint(ui, t!("endpoint.hint"));
        if !config.ai.enabled {
            return;
        }
        if let Some(rx) = &self.test
            && let Ok(result) = rx.try_recv()
        {
            self.test = None;
            self.tested = Some(match result {
                Ok(models) => {
                    let text = t!("endpoint.test_ok", count = models.len()).into_owned();
                    self.models.choices = crate::app::endpoint_choices(&models);
                    self.models.note = models.is_empty().then(|| t!("assistant.no_models").into_owned());
                    Ok(text)
                }
                Err(e) => {
                    self.models.note = Some(t!("assistant.models_failed", error = e.clone()).into_owned());
                    Err(e)
                }
            });
        }
        if self.form.is_none() {
            match &config.ai.endpoint {
                Some(e) => self.form = Some(e.clone()),
                None => {
                    if ui.button(t!("endpoint.set_up")).clicked() {
                        self.form = Some(EndpointConfig::default());
                    }
                    return;
                }
            }
        }
        let Some(form) = self.form.as_mut() else { return };
        ui.add_space(2.0);
        row(ui, &t!("endpoint.preset"), |ui| {
            let name = PRESETS.iter().find(|p| p.0 == form.preset).map_or(form.preset.clone(), |p| preset_name(p.0));
            egui::ComboBox::from_id_salt("endpoint_preset").selected_text(name).show_ui(ui, |ui| {
                for (id, url) in PRESETS {
                    if ui.selectable_label(form.preset == *id, preset_name(id)).clicked() {
                        form.preset = (*id).to_owned();
                        if !url.is_empty() {
                            form.base_url = (*url).to_owned();
                            form.local = guess_local(url);
                        }
                    }
                }
            });
        });
        row(ui, &t!("endpoint.url"), |ui| {
            if ui
                .add(egui::TextEdit::singleline(&mut form.base_url).id(*ENDPOINT_URL_FIELD).desired_width(300.0))
                .changed()
            {
                form.local = guess_local(&form.base_url);
            }
        });
        // The models are listed once for each address the form shows (never per keystroke:
        // only when the address field is left), with the key as kept.
        let address_settled = !ui.memory(|m| m.has_focus(*ENDPOINT_URL_FIELD));
        if address_settled && self.listed_for.as_deref() != Some(form.base_url.as_str()) && self.test.is_none() {
            self.listed_for = Some(form.base_url.clone());
            self.models = ModelList { note: Some(t!("assistant.listing").into_owned()), ..Default::default() };
            self.test = Some(list_models(ui.ctx(), form, &self.key));
            self.tested = None;
        }
        row(ui, &t!("endpoint.model"), |ui| {
            let choices: Vec<Choice> = self
                .models
                .choices
                .iter()
                .map(|c| Choice { value: &c.value, label: &c.label, tip: &c.tip, group: c.group.as_deref() })
                .collect();
            let current = if form.model.is_empty() { "qwen3:8b".to_owned() } else { form.model.clone() };
            let picked = picker::show(
                ui,
                Picker {
                    id: "endpoint_model",
                    current: &current,
                    current_value: &form.model,
                    hint: &t!("picker.search_hint"),
                    choices: &choices,
                    custom: true,
                    note: self.models.note.as_deref(),
                    width: Some(300.0),
                    enabled: true,
                },
            );
            if let Some(p) = picked.inner {
                form.model = p.value().to_owned();
            }
        });
        row(ui, &t!("endpoint.key"), |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.key)
                        .password(true)
                        .hint_text(t!("endpoint.key_hint"))
                        .desired_width(220.0),
                );
                if ui.add_enabled(!self.key.trim().is_empty(), egui::Button::new(t!("endpoint.key_save"))).clicked() {
                    changes.push(SettingsChange::EndpointKey {
                        url: form.base_url.clone(),
                        key: self.key.trim().to_owned(),
                    });
                    form.key_in_keyring = true;
                    self.key.clear();
                }
                if form.key_in_keyring {
                    ui.weak(t!("endpoint.key_stored"));
                }
            });
        });
        row(ui, &t!("endpoint.key_env"), |ui| {
            ui.add(egui::TextEdit::singleline(&mut form.key_env).hint_text("OPENAI_API_KEY").desired_width(220.0));
        });
        row(ui, &t!("endpoint.effort"), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut form.reasoning_effort)
                    .hint_text(t!("endpoint.effort_field_hint"))
                    .desired_width(220.0),
            )
            .on_hover_text(t!("endpoint.effort_hint"));
        });
        ui.checkbox(&mut form.local, t!("endpoint.local")).on_hover_text(t!("endpoint.local_hint"));
        ui.checkbox(&mut form.vision, t!("endpoint.vision")).on_hover_text(t!("endpoint.vision_hint"));
        ui.checkbox(&mut form.tools, t!("endpoint.tools")).on_hover_text(t!("endpoint.tools_hint"));
        if !form.local {
            ui.colored_label(ui.visuals().warn_fg_color, t!("endpoint.cloud_note"));
        }
        let mut remove = false;
        ui.horizontal(|ui| {
            if ui.add_enabled(self.test.is_none(), egui::Button::new(t!("endpoint.test"))).clicked() {
                self.listed_for = Some(form.base_url.clone());
                self.test = Some(list_models(ui.ctx(), form, &self.key));
                self.tested = None;
            }
            if self.test.is_some() {
                ui.spinner();
            }
            let ready = !form.base_url.trim().is_empty() && !form.model.trim().is_empty();
            if ui.add_enabled(ready, egui::Button::new(t!("settings.apply"))).clicked() {
                changes.push(SettingsChange::Endpoint(Some(form.clone())));
            }
            if config.ai.endpoint.is_some() && ui.button(t!("endpoint.remove")).clicked() {
                changes.push(SettingsChange::Endpoint(None));
                remove = true;
            }
        });
        if remove {
            self.form = None;
        }
        match &self.tested {
            Some(Ok(text)) => {
                ui.label(format!("✔ {text}"));
            }
            Some(Err(e)) => {
                ui.colored_label(ui.visuals().error_fg_color, format!("✖ {e}"));
            }
            None => {}
        }
    }

    pub fn log(&mut self, ui: &mut Ui, changes: &mut Vec<SettingsChange>) {
        self.load();
        if self.log.is_empty() {
            hint(ui, t!("settings.ai_log_empty"));
        } else {
            egui::Grid::new("ai_log").num_columns(5).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
                for e in &self.log {
                    let at = SystemTime::UNIX_EPOCH + Duration::from_nanos(e.at_ns.max(0) as u64);
                    ui.weak(i18n::datetime(at));
                    let client = e.client.as_deref().map(client_name).unwrap_or_else(|| e.source.clone());
                    ui.label(format!(
                        "{client} · {}",
                        if e.access_class == "local" { t!("review.local") } else { t!("review.cloud") }
                    ));
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

/// The address field, whose focus says whether the address is being typed.
static ENDPOINT_URL_FIELD: std::sync::LazyLock<egui::Id> =
    std::sync::LazyLock::new(|| egui::Id::unique("endpoint_url"));

/// Asks the endpoint for its models on a thread: with the key typed, else the one kept.
fn list_models(ctx: &egui::Context, form: &EndpointConfig, typed: &str) -> Receiver<Result<Vec<String>, String>> {
    let (tx, rx) = bounded(1);
    let key = Some(typed.trim().to_owned())
        .filter(|k| !k.is_empty())
        .or_else(|| crate::assist::secret::load(&form.base_url, form.key_in_keyring, &form.key_env));
    let endpoint = crate::assist::client::Endpoint::new(&form.base_url, &form.model, key, Duration::from_secs(15));
    let ctx = ctx.clone();
    std::thread::Builder::new()
        .name("models".into())
        .spawn(move || {
            let _ = tx.send(endpoint.models().map_err(|e| e.to_string()));
            ctx.request_repaint();
        })
        .expect("spawning a thread");
    rx
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
    meshgrove_core::ai::agent_setup::client_name(id).to_owned()
}

/// `meshgrove-mcp` next to this program, and the AppImage this runs from (if it does).
pub fn mcp_binary() -> (PathBuf, Option<PathBuf>) {
    let name = if cfg!(windows) { "meshgrove-mcp.exe" } else { "meshgrove-mcp" };
    let exe =
        std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(name))).unwrap_or_else(|| name.into());
    (exe, std::env::var_os("APPIMAGE").map(PathBuf::from))
}

/// Endpoints people run or subscribe to, with their usual address.
const PRESETS: &[(&str, &str)] = &[
    ("ollama", "http://localhost:11434/v1"),
    ("lmstudio", "http://localhost:1234/v1"),
    ("openai", "https://api.openai.com/v1"),
    ("openrouter", "https://openrouter.ai/api/v1"),
    ("custom", ""),
];

fn preset_name(id: &str) -> String {
    match id {
        "ollama" => "Ollama".into(),
        "lmstudio" => "LM Studio".into(),
        "openai" => "OpenAI".into(),
        "openrouter" => "OpenRouter".into(),
        _ => t!("endpoint.custom").into_owned(),
    }
}
