//! The chat answered by an AI agent of this computer (Claude Code, Codex, OpenCode …) instead of
//! the endpoint: the agent runs as a child process, reaches the library through the `meshgrove`
//! MCP server it is given, and what it suggests shows as cards in the chat like the endpoint's.
//! Also the pickers' lists: which assistant answers, and which model it uses.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use base64::Engine;
use crossbeam_channel::{Receiver, bounded};
use meshgrove_core::ai::dispatch;
use meshgrove_core::ai::proposals::{self, Decision, Picked as PickedAll, ProposalKind};
use meshgrove_core::config::{AccessClass, ChatMode};

use super::App;
use super::chat::Pending;
use crate::assist::acp::{self, AuthKind, AuthMethod, ConfigOption, PermissionOption, PromptPart, ToolStatus};
use crate::assist::acp_client::{AcpBackend, Event};
use crate::assist::actions::Action;
use crate::assist::agents::{self, Entry};
use crate::assist::chat::agent_note;
use crate::assist::{connect, secret};
use crate::ui::chat::{AuthRow, Card, ChatMsg, OptionRow, Question};
use crate::ui::picker::Picked;

/// Earlier messages of a conversation told to an agent whose session starts in the middle of it.
const CATCH_UP: usize = 12;

/// A model the pickers offer.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelChoice {
    pub value: String,
    pub label: String,
    pub tip: String,
    pub group: Option<String>,
}

/// The reasoning efforts offered for an endpoint (empty: the service's default); the services
/// differ, and the settings take any other word.
pub const EFFORTS: [&str; 6] = ["", "low", "medium", "high", "xhigh", "max"];

/// The models of the assistant answering the chat.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelList {
    pub current: String,
    pub choices: Vec<ModelChoice>,
    /// Still listing, or why there are no others.
    pub note: Option<String>,
}

/// The agent answering the chat, while it runs.
pub struct AgentChat {
    pub entry: Entry,
    backend: AcpBackend,
    /// A session is open.
    ready: bool,
    /// A message waiting for the session.
    queued: Option<Vec<PromptPart>>,
    /// The conversation the agent's session belongs to; another one gets a new session, told
    /// what was said before.
    session_chat: Option<i64>,
    pub models: ModelList,
    /// The session option that picks the model (`None`: the older `session/set_model`).
    model_option: Option<String>,
    /// The agent's other session options (reasoning effort, mode …), as last listed.
    options: Vec<ConfigOption>,
    /// The agent wants the user to sign in, in these ways.
    pub auth: Vec<AuthMethod>,
    /// What the agent is up to, for the pane.
    pub status: Option<String>,
    /// Tool call id → its row in the answer being written.
    tool_rows: HashMap<String, usize>,
    /// When the message being answered went out (Unix nanoseconds).
    turn_started: i64,
    /// Permission questions the agent asked and the user has not answered.
    pub questions: Vec<Question>,
}

/// The endpoint's models, listed in the background for the picker.
pub struct EndpointModels {
    base_url: String,
    rx: Option<Receiver<Result<Vec<String>, String>>>,
    pub list: ModelList,
}

fn now_ns() -> i64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos() as i64).unwrap_or(0)
}

/// The agents' working folder: an empty folder of Meshgrove's own, never the user's files.
fn work_dir() -> PathBuf {
    let dir = dirs::data_dir().map(|d| d.join("meshgrove")).unwrap_or_else(std::env::temp_dir).join("agent-work");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

impl App {
    /// Who answers the chat: the endpoint or an agent (`None`: nothing set up or found).
    pub(super) fn assistant(&self) -> Option<Entry> {
        agents::current(&self.config.ai, self.agents.as_ref())
    }

    /// The endpoint is set up: the one-click actions and the tag suggestions need it.
    pub(super) fn endpoint_available(&self) -> bool {
        self.config.ai.enabled && self.config.ai.endpoint.as_ref().is_some_and(|e| !e.base_url.trim().is_empty())
    }

    /// Looks for the agents installed here, off the window thread (once AI is on).
    pub(super) fn detect_agents(&mut self) {
        if !self.config.ai.enabled || self.agents.is_some() || self.agents_rx.is_some() {
            return;
        }
        let (tx, rx) = bounded(1);
        let ctx = self.egui_ctx.clone();
        std::thread::Builder::new()
            .name("agents".into())
            .spawn(move || {
                let _ = tx.send(agents::detect());
                ctx.request_repaint();
            })
            .expect("spawning a thread");
        self.agents_rx = Some(rx);
    }

    /// Takes the agents found, when the search is done.
    pub(super) fn poll_agents(&mut self) {
        if let Some(rx) = &self.agents_rx
            && let Ok(found) = rx.try_recv()
        {
            self.agents_rx = None;
            log::info!(
                "AI agents found: {:?} ({} ms)",
                found.found.iter().map(|f| f.id).collect::<Vec<_>>(),
                found.took_ms
            );
            self.agents = Some(found);
        }
    }

    /// Everything the pickers offer: (id, name) of each assistant.
    pub(super) fn assistant_rows(&self) -> Vec<(String, String)> {
        agents::entries(&self.config.ai, self.agents.as_ref()).into_iter().map(|e| (e.id, e.name)).collect()
    }

    /// Starts the picked agent (the *Connect* button, or an agent set to connect by itself).
    /// The endpoint has nothing to start.
    pub(super) fn connect_agent(&mut self, retry: bool) {
        if let Some(entry) = self.assistant().filter(|e| !e.is_endpoint()) {
            self.ensure_agent(&entry, retry);
        }
    }

    /// Whether the picked assistant can answer now: the endpoint always, an agent once it runs.
    pub(super) fn assistant_connected(&self) -> bool {
        match self.assistant() {
            Some(e) if e.is_endpoint() => true,
            Some(e) => self.chat.agent.as_ref().is_some_and(|a| a.entry.id == e.id),
            None => false,
        }
    }

    /// The chat talks to this assistant from now on.
    pub(super) fn set_assistant(&mut self, id: String) {
        if self.config.ai.assistant == id {
            return;
        }
        self.config.ai.assistant = id;
        self.save_config();
        self.stop_agent();
    }

    /// Ends the running agent (the assistant changed, or Meshgrove quits).
    pub(super) fn stop_agent(&mut self) {
        if let Some(mut agent) = self.chat.agent.take() {
            agent.backend.shutdown(false);
        }
        self.chat.streaming = None;
    }

    pub(super) fn shutdown_chat(&mut self) {
        if let Some(mut agent) = self.chat.agent.take() {
            agent.backend.shutdown(true);
        }
    }

    /// Starts `entry`'s agent unless it is running already; says what went wrong in a toast.
    /// One that failed is not tried again until asked (`retry`: a message is being sent).
    pub(super) fn ensure_agent(&mut self, entry: &Entry, retry: bool) {
        if self.chat.agent.as_ref().is_some_and(|a| a.entry.id == entry.id && a.entry.argv == entry.argv) {
            return;
        }
        if !retry && self.chat.agent_failed.as_deref() == Some(entry.id.as_str()) {
            return;
        }
        self.chat.agent_failed = None;
        self.stop_agent();
        let server = connect::server().map(|s| acp::mcp_server(&s.program, &s.lead, &entry.id, entry.local));
        if server.is_none() {
            self.toast(t!("assistant.no_mcp").into_owned());
        }
        let ctx = self.egui_ctx.clone();
        match AcpBackend::start(&entry.argv, &work_dir(), server, agent_note(&self.config), move || {
            ctx.request_repaint()
        }) {
            Ok(backend) => {
                self.chat.agent = Some(AgentChat {
                    entry: entry.clone(),
                    backend,
                    ready: false,
                    queued: None,
                    session_chat: None,
                    models: ModelList { note: Some(t!("assistant.connecting").into_owned()), ..Default::default() },
                    model_option: None,
                    options: Vec::new(),
                    auth: Vec::new(),
                    status: Some(t!("assistant.connecting").into_owned()),
                    tool_rows: HashMap::new(),
                    turn_started: 0,
                    questions: Vec::new(),
                });
            }
            Err(e) => {
                let hint = entry.problem.clone().unwrap_or(e);
                self.chat.agent_failed = Some(entry.id.clone());
                self.toast(t!("assistant.start_failed", name = entry.name.clone(), error = hint).into_owned());
            }
        }
    }

    /// The models to pick from, for the assistant answering now.
    pub(super) fn chat_models(&mut self) -> ModelList {
        let Some(entry) = self.assistant() else { return ModelList::default() };
        if entry.is_endpoint() {
            return self.endpoint_models();
        }
        self.chat.agent.as_ref().filter(|a| a.entry.id == entry.id).map(|a| a.models.clone()).unwrap_or_default()
    }

    /// The endpoint's models, asked for once per address in the background.
    fn endpoint_models(&mut self) -> ModelList {
        let Some(e) = self.config.ai.endpoint.clone() else { return ModelList::default() };
        let stale = self.chat.endpoint_models.as_ref().is_none_or(|m| m.base_url != e.base_url);
        if stale {
            let (tx, rx) = bounded(1);
            let key = secret::load(&e.base_url, e.key_in_keyring, &e.key_env);
            let endpoint = crate::assist::client::Endpoint::new(&e.base_url, &e.model, key, Duration::from_secs(15));
            let ctx = self.egui_ctx.clone();
            std::thread::Builder::new()
                .name("models".into())
                .spawn(move || {
                    let _ = tx.send(endpoint.models().map_err(|e| e.to_string()));
                    ctx.request_repaint();
                })
                .expect("spawning a thread");
            self.chat.endpoint_models = Some(EndpointModels {
                base_url: e.base_url.clone(),
                rx: Some(rx),
                list: ModelList {
                    current: e.model.clone(),
                    note: Some(t!("assistant.listing").into_owned()),
                    ..Default::default()
                },
            });
        }
        let models = self.chat.endpoint_models.as_mut().expect("just set");
        if let Some(rx) = &models.rx
            && let Ok(result) = rx.try_recv()
        {
            models.rx = None;
            match result {
                Ok(ids) => {
                    models.list.choices = endpoint_choices(&ids);
                    models.list.note = ids.is_empty().then(|| t!("assistant.no_models").into_owned());
                }
                Err(why) => models.list.note = Some(t!("assistant.models_failed", error = why).into_owned()),
            }
        }
        models.list.current = e.model;
        models.list.clone()
    }

    /// The selectors beside the model: an endpoint's reasoning effort, an agent's other session
    /// options (its reasoning effort, mode …).
    pub(super) fn chat_options(&self) -> Vec<OptionRow> {
        let Some(entry) = self.assistant() else { return Vec::new() };
        if entry.is_endpoint() {
            let current =
                self.config.ai.endpoint.as_ref().map(|e| e.reasoning_effort.trim().to_owned()).unwrap_or_default();
            let default = t!("chat.option_default").into_owned();
            let mut choices: Vec<(String, String, String)> = EFFORTS
                .iter()
                .map(|e| ((*e).to_owned(), if e.is_empty() { default.clone() } else { (*e).to_owned() }, String::new()))
                .collect();
            // An effort typed in the settings that is none of the usual ones stays pickable.
            if !current.is_empty() && !choices.iter().any(|c| c.0 == current) {
                choices.push((current.clone(), current.clone(), String::new()));
            }
            return vec![OptionRow {
                id: "reasoning_effort".into(),
                name: t!("endpoint.effort").into_owned(),
                description: t!("endpoint.effort_hint").into_owned(),
                current,
                choices,
            }];
        }
        let Some(agent) = self.chat.agent.as_ref().filter(|a| a.entry.id == entry.id) else { return Vec::new() };
        agent
            .options
            .iter()
            .map(|o| OptionRow {
                id: o.id.clone(),
                name: if o.name.is_empty() { o.id.clone() } else { o.name.clone() },
                description: o.description.clone(),
                current: o.current_value.clone(),
                choices: o.options.iter().map(|c| (c.value.clone(), c.name.clone(), c.description.clone())).collect(),
            })
            .collect()
    }

    /// One of the selectors beside the model was set.
    pub(super) fn set_chat_option(&mut self, id: String, value: String) {
        let Some(entry) = self.assistant() else { return };
        if entry.is_endpoint() {
            if id == "reasoning_effort"
                && let Some(e) = self.config.ai.endpoint.as_mut()
            {
                e.reasoning_effort = value.trim().to_owned();
                self.save_config();
            }
            return;
        }
        let Some(agent) = self.chat.agent.as_mut().filter(|a| a.entry.id == entry.id) else { return };
        agent.backend.set_config_option(&id, &value);
        self.config.ai.agent_options.entry(entry.id).or_default().insert(id, value);
        self.save_config();
    }

    /// The model picked in the chat: the endpoint's from the next message on; an agent's at once
    /// (it may refuse one it did not list).
    pub(super) fn set_chat_model(&mut self, picked: Picked) {
        let Some(entry) = self.assistant() else { return };
        let value = picked.value().trim().to_owned();
        if value.is_empty() {
            return;
        }
        if entry.is_endpoint() {
            if let Some(e) = self.config.ai.endpoint.as_mut() {
                e.model = value;
                self.save_config();
            }
            return;
        }
        let Some(agent) = self.chat.agent.as_mut().filter(|a| a.entry.id == entry.id) else { return };
        match &agent.model_option {
            Some(option) => agent.backend.set_config_option(option, &value),
            None => {
                agent.backend.set_model(&value);
                agent.models.current = value.clone();
            }
        }
        self.config.ai.agent_models.insert(entry.id, value);
        self.save_config();
    }

    /// The agent's ways to sign in, as the pane shows them.
    pub(super) fn auth_rows(&self) -> Vec<AuthRow> {
        let Some(agent) = &self.chat.agent else { return Vec::new() };
        agent
            .auth
            .iter()
            .map(|m| {
                let (command, by_agent) = match &m.kind {
                    AuthKind::Agent => (None, true),
                    AuthKind::Terminal { args, command } => {
                        let words: Vec<String> = match command {
                            Some((program, args)) => {
                                std::iter::once(program.clone()).chain(args.iter().cloned()).collect()
                            }
                            None => agent.entry.argv.iter().cloned().chain(args.iter().cloned()).collect(),
                        };
                        (Some(words.join(" ")), false)
                    }
                    AuthKind::EnvVar { vars } => {
                        (Some(t!("assistant.auth_env", vars = vars.join(", ")).into_owned()), false)
                    }
                };
                AuthRow {
                    id: m.id.clone(),
                    name: m.name.clone(),
                    description: m.description.clone(),
                    command,
                    by_agent,
                }
            })
            .collect()
    }

    pub(super) fn authenticate(&mut self, method: String) {
        if let Some(agent) = self.chat.agent.as_mut() {
            agent.auth.clear();
            agent.status = Some(t!("assistant.signing_in").into_owned());
            agent.backend.authenticate(&method);
        }
    }

    pub(super) fn answer_permission(&mut self, key: u64, option: Option<String>) {
        if let Some(agent) = self.chat.agent.as_mut() {
            agent.questions.retain(|q| q.key != key);
            agent.backend.answer_permission(key, option.as_deref());
        }
    }

    /// Sends the message in the field to the agent: as one prompt, with the thumbnails of its
    /// references where the agent takes pictures and the library allows it. The conversation
    /// is stored like the endpoint's.
    pub(super) fn send_to_agent(&mut self, entry: Entry, confirmed: bool) {
        let (text, pills) = self.chat.input.take_message();
        let pictures = if self.chat.agent.as_ref().is_some_and(|a| a.backend.takes_images()) {
            self.pictures_for(&pills).len()
        } else {
            0
        };
        if !entry.local && !self.chat.cloud_ok && !confirmed {
            let text = t!("chat.confirm_text", host = entry.name.clone(), refs = pills.len(), pictures = pictures)
                .into_owned();
            self.chat.confirm = Some((text, Pending::Message));
            return;
        }
        if confirmed {
            self.chat.cloud_ok = true;
        }
        self.ensure_agent(&entry, true);
        if self.chat.agent.is_none() {
            // It did not start: the message stays in the field.
            self.chat.input.restore(&text, pills);
            return;
        }
        let Some(mut s) = super::chat::store() else { return };
        let chat = match self.chat.current {
            Some(c) => c,
            None => {
                let mut plain = text.clone();
                for (n, p) in pills.iter().enumerate() {
                    plain = plain.replace(&format!("[[ref:{n}]]"), &format!("@{}", p.label));
                }
                let plain: String = plain.chars().take(40).collect();
                match s.create_chat(plain.trim()) {
                    Ok(id) => id,
                    Err(e) => {
                        self.toast(e.to_string());
                        return;
                    }
                }
            }
        };
        let vision = pictures > 0;
        let (llm, images) = self.llm_message(&text, &pills, vision);
        let mut msg = ChatMsg { user: true, text, pills, llm: llm.clone(), ..Default::default() };
        msg.id = s.add_message(chat, "user", &serde_json::to_value(&msg).unwrap_or_default()).unwrap_or(0);
        let mut parts = vec![PromptPart::Text(llm)];
        for png in images {
            parts.push(PromptPart::Image {
                mime: "image/png".into(),
                base64: base64::engine::general_purpose::STANDARD.encode(png),
            });
        }
        self.chat.input.clear();
        self.chat.offered = None;
        self.dispatch_to_agent(chat, msg, parts);
    }

    /// Sends `parts` to the running agent as the turn answering `msg` in conversation `chat`,
    /// and shows the answer coming. A session opened for another conversation (or none yet) is
    /// replaced by a fresh one, told what was said in this one before.
    fn dispatch_to_agent(&mut self, chat: i64, msg: ChatMsg, mut parts: Vec<PromptPart>) {
        let Some(agent) = self.chat.agent.as_mut() else { return };
        if agent.session_chat != Some(chat) {
            let earlier: Vec<String> = self
                .chat
                .messages
                .iter()
                .rev()
                .take(CATCH_UP)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .filter(|m| !m.text.trim().is_empty() || !m.llm.is_empty())
                .map(|m| {
                    format!("{}: {}", if m.user { "User" } else { "Assistant" }, if m.user { &m.llm } else { &m.text })
                })
                .collect();
            if !earlier.is_empty()
                && let Some(PromptPart::Text(text)) = parts.first_mut()
            {
                *text = format!("Earlier in this conversation:\n{}\n\nNow the user says:\n{text}", earlier.join("\n"));
            }
            if agent.session_chat.is_some() {
                agent.backend.new_session();
                agent.ready = false;
            }
            agent.session_chat = Some(chat);
        }
        agent.tool_rows.clear();
        agent.turn_started = now_ns();
        if agent.ready && agent.backend.ready() {
            agent.backend.prompt(&parts);
        } else {
            agent.queued = Some(parts);
        }
        self.chat.answering = msg.id;
        self.chat.messages.push(msg);
        self.chat.streaming = Some(ChatMsg::default());
        self.chat.current = Some(chat);
        if let Some(s) = super::chat::store() {
            self.chat.chats = s.chats(50).unwrap_or_default();
        }
    }

    /// A one-click action done by the agent: it gets the task as a message in a conversation of
    /// its own and makes the suggestions through Meshgrove's tools, like in any chat.
    pub(super) fn start_action_with_agent(&mut self, entry: Entry, title: String, user_text: String, action: &Action) {
        let Some(mut s) = super::chat::store() else { return };
        let chat = match s.create_chat(&title) {
            Ok(id) => id,
            Err(e) => {
                self.toast(e.to_string());
                return;
            }
        };
        let llm = self.action_instruction(action);
        let mut msg = ChatMsg { user: true, text: user_text, llm: llm.clone(), ..Default::default() };
        msg.id = s.add_message(chat, "user", &serde_json::to_value(&msg).unwrap_or_default()).unwrap_or(0);
        self.ensure_agent(&entry, true);
        if self.chat.agent.is_none() {
            return;
        }
        // A conversation of its own: whatever session there was belongs to another one.
        self.chat.messages.clear();
        self.chat.current = Some(chat);
        self.dispatch_to_agent(chat, msg, vec![PromptPart::Text(llm)]);
        self.chat.changes_stale = true;
        self.layout.focus_chat();
    }

    /// What the agent is told to do for a one-click action, with the files by their ids.
    fn action_instruction(&self, action: &Action) -> String {
        let scope = meshgrove_core::ai::Scope::new(&self.config, self.assistant_class());
        match action {
            Action::Tags { groups, inbox_done } => {
                let mut out = String::from(
                    "One-click action \"suggest tags\": call list_tags first, then propose_tags once per group below (its \
                     title as the call's title, every id of the group as an item with 1 to 5 tags, prefer tags in use",
                );
                if *inbox_done {
                    out.push_str(", mark_inbox_done: true");
                }
                out.push_str("). Then say in a sentence what you suggested.");
                for g in groups {
                    let ids: Vec<String> = g.paths.iter().filter_map(|p| scope.id_of(p)).collect();
                    if ids.is_empty() {
                        continue;
                    }
                    out.push_str(&format!("\n\nGroup \"{}\"", g.title));
                    if let Some(folder) = g.folder.as_ref().and_then(|f| scope.id_of(f)) {
                        out.push_str(&format!(" (they came in folder {folder}; folder_summary shows its readme)"));
                    }
                    out.push_str(":\n");
                    out.push_str(&ids.join("\n"));
                }
                out
            }
            Action::List { description, print } => format!(
                "One-click action: make a {} from this description: \"{}\". Look the files up with search_models (list_tags \
                 and list_printers help with the words), then call propose_list once, kind \"{}\", with fitting \
                 quantities for a print list. Then say in a sentence what is on it.",
                if *print { "print list" } else { "collection" },
                description.trim(),
                if *print { "print" } else { "collection" }
            ),
        }
    }

    /// Takes what the agent reports; runs every frame.
    pub(super) fn poll_agent(&mut self) {
        let Some(agent) = self.chat.agent.as_mut() else { return };
        let events = agent.backend.poll();
        for e in events {
            self.agent_event(e);
        }
    }

    fn agent_event(&mut self, e: Event) {
        if let Event::Permission { key, name, kind, title, options } = e {
            // Meshgrove's own tools only read, or suggest for the review: they run without
            // asking, once each. Everything else (a shell command, a file, a tool of another
            // server, whatever the agent brought itself) is the user's to allow: the title is
            // the agent's text.
            let plugins = self.plugin_tools();
            let own =
                |tool: &str| dispatch::is_fixed(tool) || plugins.as_ref().is_some_and(|p| p.writes(tool).is_some());
            let allow = options
                .iter()
                .find(|o| o.kind == "allow_once")
                .or_else(|| options.iter().find(|o| o.kind == "allow_always"))
                .map(|o| o.id.clone());
            let Some(agent) = self.chat.agent.as_mut() else { return };
            if runs_unasked(name.as_deref(), kind.as_deref(), &title, own)
                && let Some(allow) = allow
            {
                agent.backend.answer_permission(key, Some(&allow));
            } else {
                agent.questions.push(Question { key, title, options: question_options(&options) });
            }
            return;
        }
        let Some(agent) = self.chat.agent.as_mut() else { return };
        match e {
            Event::Ready { agent: name, .. } => {
                agent.ready = true;
                agent.auth.clear();
                agent.status = Some(if name.is_empty() { agent.entry.name.clone() } else { name });
                if let Some(wanted) = self.config.ai.agent_options.get(&agent.entry.id)
                    && let Some((id, value)) = agent.options.iter().find_map(|o| {
                        let v = wanted.get(&o.id)?;
                        (v != &o.current_value && o.options.iter().any(|c| &c.value == v))
                            .then(|| (o.id.clone(), v.clone()))
                    })
                {
                    agent.backend.set_config_option(&id, &value);
                }
                // The model picked before, where the agent lists it.
                if let Some(wanted) = self.config.ai.agent_models.get(&agent.entry.id).cloned()
                    && wanted != agent.models.current
                    && agent.models.choices.iter().any(|c| c.value == wanted)
                {
                    match &agent.model_option {
                        Some(option) => agent.backend.set_config_option(option, &wanted),
                        None => {
                            agent.backend.set_model(&wanted);
                            agent.models.current = wanted;
                        }
                    }
                }
                if let Some(parts) = agent.queued.take() {
                    agent.backend.prompt(&parts);
                }
            }
            Event::Models { current, available } => {
                if !available.is_empty() {
                    agent.models.choices = available
                        .into_iter()
                        .map(|m| ModelChoice { value: m.id, label: m.name, tip: m.description, group: None })
                        .collect();
                    agent.models.note = None;
                }
                if !current.is_empty() {
                    agent.models.current = current;
                }
            }
            Event::ConfigOptions(options) => {
                let model_id = acp::model_option(&options).map(|o| o.id.clone());
                // Not the agent's permission mode (Manual, Accept edits, Plan, Auto): Meshgrove
                // decides permissions itself (its own tools pass, others ask in the chat), and
                // its Review | Automatic switch says what becomes of the suggestions.
                agent.options = options
                    .iter()
                    .filter(|o| Some(&o.id) != model_id.as_ref() && o.category != "mode" && o.id != "mode")
                    .cloned()
                    .collect();
                // The options picked before, where the agent lists their values: one request at
                // a time, so each listing sends the next one that differs.
                if agent.ready
                    && let Some(wanted) = self.config.ai.agent_options.get(&agent.entry.id)
                    && let Some((id, value)) = agent.options.iter().find_map(|o| {
                        let v = wanted.get(&o.id)?;
                        (v != &o.current_value && o.options.iter().any(|c| &c.value == v))
                            .then(|| (o.id.clone(), v.clone()))
                    })
                {
                    agent.backend.set_config_option(&id, &value);
                }
                match acp::model_option(&options) {
                    Some(option) => {
                        agent.model_option = Some(option.id.clone());
                        agent.models.current = option.current_value.clone();
                        agent.models.choices = option
                            .options
                            .iter()
                            .map(|c| ModelChoice {
                                value: c.value.clone(),
                                label: c.name.clone(),
                                tip: c.description.clone(),
                                group: c.group.clone(),
                            })
                            .collect();
                        agent.models.note = None;
                    }
                    None => agent.models.note = Some(t!("assistant.no_model_choice").into_owned()),
                }
            }
            Event::ModelRejected(why) => {
                self.config.ai.agent_models.remove(&agent.entry.id);
                self.toast(t!("assistant.model_refused", error = why).into_owned());
            }
            Event::Text { thought, text } => {
                if !thought && let Some(msg) = self.chat.streaming.as_mut() {
                    msg.text.push_str(&text);
                }
            }
            Event::Tool { id, title, status, output: _, kind } => {
                let Some(msg) = self.chat.streaming.as_mut() else { return };
                let summary = match status {
                    Some(ToolStatus::Completed) => t!("chat.tool_ok").into_owned(),
                    Some(ToolStatus::Failed) => t!("assistant.tool_failed").into_owned(),
                    _ => "…".into(),
                };
                match agent.tool_rows.get(&id) {
                    Some(&row) => {
                        if let Some(entry) = msg.tools.get_mut(row) {
                            if let Some(t) = title {
                                entry.0 = t;
                            }
                            entry.1 = summary;
                        }
                    }
                    None => {
                        agent.tool_rows.insert(id, msg.tools.len());
                        msg.tools.push((title.or(kind).unwrap_or_else(|| "tool".into()), summary));
                    }
                }
            }
            Event::Permission { .. } => unreachable!("handled above"),
            Event::AuthRequired { methods } => {
                agent.ready = false;
                agent.auth = methods;
                agent.status = Some(t!("assistant.sign_in_needed").into_owned());
                if self.chat.streaming.is_some() {
                    self.finish_agent_turn(Some(t!("assistant.sign_in_needed").into_owned()));
                }
            }
            Event::AuthStatus(label) => agent.status = Some(label),
            Event::TurnEnded { reason } => {
                let notice = match reason.as_str() {
                    "" | "cancelled" | "auth" => None,
                    "error" => None,
                    other => Some(t!("assistant.stopped", reason = other.to_owned()).into_owned()),
                };
                self.finish_agent_turn(notice);
            }
            Event::Error(message) => {
                if self.chat.streaming.is_some() {
                    if let Some(msg) = self.chat.streaming.as_mut() {
                        msg.failed = true;
                    }
                    self.finish_agent_turn(Some(message));
                } else {
                    self.toast(message);
                }
            }
            Event::Exited { tail } => {
                let why = tail.last().cloned().unwrap_or_else(|| t!("assistant.exited").into_owned());
                let name = agent.entry.name.clone();
                let id = agent.entry.id.clone();
                if self.chat.streaming.is_some() {
                    if let Some(msg) = self.chat.streaming.as_mut() {
                        msg.failed = true;
                    }
                    self.finish_agent_turn(Some(why));
                } else {
                    self.toast(t!("assistant.start_failed", name = name, error = why).into_owned());
                }
                // Not started again by itself: the next message tries once more.
                self.chat.agent_failed = Some(id);
                self.chat.agent = None;
            }
        }
    }

    /// The answer is complete: the suggestions the agent made through the MCP server since the
    /// message went out become cards (accepted at once in automatic mode, moves excepted), and
    /// the answer is stored.
    fn finish_agent_turn(&mut self, notice: Option<String>) {
        self.cancel_questions();
        let Some(mut msg) = self.chat.streaming.take() else { return };
        if let Some(n) = notice {
            msg.notices.push(n);
        }
        if let Some(agent) = self.chat.agent.as_mut() {
            agent.questions.clear();
            agent.tool_rows.clear();
            let since = agent.turn_started;
            let client = agent.entry.id.clone();
            if let Some(store) = super::chat::store() {
                let batches = store.batches(Some("chat"), Some(&client), &[], 50).unwrap_or_default();
                for b in batches.into_iter().filter(|b| b.created_ns >= since).rev() {
                    if self.config.ai.chat_mode == ChatMode::Auto && b.state == "pending" {
                        let items = store.proposals(b.id).unwrap_or_default();
                        let movable = |p: &proposals::Proposal| match p.kind {
                            ProposalKind::Delete | ProposalKind::Archive => false,
                            ProposalKind::Move => p.target.as_ref().is_some_and(|t| self.config.in_landing(&t.path)),
                            _ => true,
                        };
                        if !items.is_empty() && items.iter().all(movable) {
                            let mut ops = Vec::new();
                            let mut decisions = Vec::new();
                            for p in &items {
                                ops.extend(proposals::to_ops(p, &PickedAll::all(p), b.inbox_done));
                                decisions.push((p.id, Decision::Accepted));
                            }
                            self.library.accept(b.id, b.uid.clone(), b.title.clone(), ops, decisions);
                        }
                    }
                    msg.cards.push(Card::Proposed { batch: b.uid, title: b.title, items: b.items });
                }
            }
        }
        if let (Some(mut s), Some(chat)) = (super::chat::store(), self.chat.current) {
            msg.id = s.add_message(chat, "assistant", &serde_json::to_value(&msg).unwrap_or_default()).unwrap_or(0);
        }
        self.chat.messages.push(msg);
        self.chat.changes_stale = true;
    }

    /// Stops the agent's answer.
    pub(super) fn stop_agent_answer(&mut self) {
        if let Some(agent) = self.chat.agent.as_mut() {
            agent.queued = None;
            agent.questions.clear();
            agent.backend.cancel();
        }
        self.cancel_questions();
    }

    /// The access class of the chat's assistant, for what it may see.
    pub(super) fn assistant_class(&self) -> AccessClass {
        match self.assistant() {
            Some(e) if e.local => AccessClass::Local,
            _ => AccessClass::Cloud,
        }
    }
}

/// The endpoint's model ids as choices: ids of the form `vendor/model` (OpenRouter) fold under
/// their vendors when there are several.
pub fn endpoint_choices(ids: &[String]) -> Vec<ModelChoice> {
    let vendors: std::collections::HashSet<&str> =
        ids.iter().filter_map(|id| id.split_once('/').map(|(v, _)| v)).collect();
    let grouped = vendors.len() >= 2 && ids.iter().filter(|id| id.contains('/')).count() * 2 >= ids.len();
    ids.iter()
        .map(|id| ModelChoice {
            value: id.clone(),
            label: id.clone(),
            tip: String::new(),
            group: grouped
                .then(|| id.split_once('/').map(|(v, _)| v.to_owned()).unwrap_or_default())
                .filter(|g| !g.is_empty()),
        })
        .collect()
}

/// Whether a tool call may run without asking the user: only one of Meshgrove's own tools
/// (`own` says which names those are) served by Meshgrove's own MCP server, and only when the
/// agent does not class the call as running, editing, deleting or moving something. The name
/// is taken from the call's `name` (`mcp__meshgrove-chat__search_models`,
/// `meshgrove-chat/search_models`, `meshgrove-chat:search_models`) or, failing that, from a
/// title that is nothing but a tool name. Where the name has a server part, it has to be
/// [`acp::SERVER_NAME`]: another server's tool that borrows one of our names is asked about.
/// A title with more words in it (a shell command, say) never passes, whatever it mentions.
fn runs_unasked(name: Option<&str>, kind: Option<&str>, title: &str, own: impl Fn(&str) -> bool) -> bool {
    if matches!(kind, Some("execute" | "edit" | "delete" | "move" | "switch_mode")) {
        return false;
    }
    let raw = name.unwrap_or(title).trim();
    if raw.is_empty() || raw.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((server, tool)) = split_tool_name(raw) else { return false };
    server.is_none_or(|s| s == acp::SERVER_NAME) && own(tool)
}

/// A tool call's name taken apart: the server it names (none for a bare tool name) and the
/// tool. `None` when the tool part is not a plain name.
fn split_tool_name(raw: &str) -> Option<(Option<&str>, &str)> {
    let (server, tool) = match raw.strip_prefix("mcp__") {
        Some(rest) => match rest.rsplit_once("__") {
            Some((server, tool)) => (Some(server), tool),
            None => (None, rest),
        },
        None => match raw.rsplit_once(['/', ':']) {
            Some((server, tool)) => (Some(server), tool),
            None => (None, raw),
        },
    };
    let plain = !tool.is_empty() && tool.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    plain.then_some((server, tool))
}

fn question_options(options: &[PermissionOption]) -> Vec<(String, String)> {
    options.iter().map(|o| (o.id.clone(), if o.name.is_empty() { o.kind.clone() } else { o.name.clone() })).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only Meshgrove's own tools run without asking, and only when the agent does not class
    /// the call as running, editing, deleting or moving something; the title alone counts
    /// only when it is nothing but a tool name.
    #[test]
    fn only_meshgroves_own_reading_or_suggesting_tools_run_unasked() {
        let own = |t: &str| dispatch::is_fixed(t) || t == "bingrove_plan";
        let yes = |name: Option<&str>, kind: Option<&str>, title: &str| runs_unasked(name, kind, title, own);
        assert!(yes(Some("mcp__meshgrove-chat__search_models"), Some("other"), "search_models"));
        assert!(yes(Some("meshgrove-chat/propose_tags"), None, "propose_tags"));
        assert!(yes(Some("meshgrove-chat:bingrove_plan"), Some("read"), "whatever the agent writes"));
        assert!(yes(None, None, "search_models"), "a bare tool name as the title");
        assert!(yes(None, Some("search"), "mcp__meshgrove-chat__list_tags"));
        assert!(yes(Some("search_models"), None, "Search models"), "a bare tool name");
        // Another server's tool with one of our names: the agent takes that server's word for
        // the kind, Meshgrove does not.
        assert!(!yes(Some("mcp__other-server__search_models"), Some("other"), "search_models"));
        assert!(!yes(Some("other/search_models"), None, "search_models"));
        assert!(!yes(None, None, "mcp__other__list_tags"));
        // Not Meshgrove's: the shell, files, anything the agent brought.
        assert!(!yes(Some("Bash"), Some("execute"), "rm -rf ~/.local/share/meshgrove"));
        assert!(!yes(None, None, "rm -rf ~/.local/share/meshgrove"), "a title that mentions meshgrove");
        assert!(!yes(None, None, "echo search_models"), "a title with more than a name in it");
        assert!(!yes(Some("Write"), Some("edit"), "search_models"), "the name decides when given");
        assert!(!yes(Some("mcp__other__search_models"), Some("execute"), "search_models"), "a running kind");
        assert!(!yes(Some("mcp__meshgrove-chat__propose_moves"), Some("move"), "propose_moves"));
        assert!(!yes(None, None, ""));
    }

    #[test]
    fn vendor_ids_fold_under_their_vendors_only_when_most_have_one() {
        let ids: Vec<String> =
            ["openai/gpt-5", "anthropic/claude", "google/gemini", "mistral"].iter().map(|s| s.to_string()).collect();
        let choices = endpoint_choices(&ids);
        assert_eq!(choices[0].group.as_deref(), Some("openai"));
        assert_eq!(choices[3].group, None);
        let plain: Vec<String> = ["qwen3:8b", "gemma3", "llama3"].iter().map(|s| s.to_string()).collect();
        assert!(endpoint_choices(&plain).iter().all(|c| c.group.is_none()));
    }
}
