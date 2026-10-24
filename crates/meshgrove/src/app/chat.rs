//! The chat with the user's own model: conversations kept in the suggestion store, references
//! offered after `@`, what they stand for sent along, the answer streamed in, and the changes
//! of a message undone or redone through the arrow next to it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base64::Engine;
use egui_commonmark::CommonMarkCache;
use meshgrove_core::ai::proposals::{ChatInfo, ProposalStore};
use meshgrove_core::ai::types::ModelFilter;
use meshgrove_core::ai::{Context, read};
use meshgrove_core::cache::ThumbCache;
use meshgrove_core::config::AccessClass;
use meshgrove_core::listing;
use serde_json::{Value, json};

use super::{App, View};
use crate::assist::actions::{Action, ActionJob};
use crate::assist::chat::{ChatEvent, ChatService, Turn, system_prompt};
use crate::assist::secret;
use crate::ui::chat::{self as chat_view, Card, ChatAction, ChatMsg, ChatView};
use crate::ui::chat_input::{ChatInput, Pill, Reference};
use crate::ui::suggest::Candidate;

/// Earlier messages sent along with a new one.
const HISTORY: usize = 20;
/// Most thumbnails sent with one message.
const MAX_PICTURES: usize = 4;

#[derive(Default)]
pub struct ChatState {
    service: Option<ChatService>,
    chats: Vec<ChatInfo>,
    current: Option<i64>,
    messages: Vec<ChatMsg>,
    /// The answer being written, and the user message it answers.
    streaming: Option<ChatMsg>,
    answering: i64,
    pub input: ChatInput,
    md: CommonMarkCache,
    loaded: bool,
    /// Per user message: changes in effect and taken back; read again when marked stale.
    changes: HashMap<i64, (usize, usize)>,
    /// What became of the suggestions on the cards: batch uid → state.
    card_states: HashMap<String, String>,
    changes_stale: bool,
    /// The references for the query last typed after `@`.
    offered: Option<(String, Vec<Candidate<Pill>>)>,
    /// The cloud endpoint may be sent to without asking again (this session).
    pub(super) cloud_ok: bool,
    /// Waiting for the OK to go to a cloud endpoint: what goes out, and what does.
    pub confirm: Option<(String, Pending)>,
}

/// What goes to a cloud endpoint once the user said yes.
pub enum Pending {
    /// The message in the field.
    Message,
    Action { title: String, user_text: String, llm: String, files: usize, action: Action },
    /// The tag editor's ✧ for the selection.
    TagCandidates,
}

/// The host of an endpoint's address, for saying where things go.
pub(super) fn host_of(url: &str) -> String {
    url.split("://").nth(1).unwrap_or(url).split('/').next().unwrap_or_default().to_owned()
}

fn store() -> Option<ProposalStore> {
    ProposalStore::open(&ProposalStore::default_location()?).ok()
}

impl App {
    /// The chat exists while AI is on and an endpoint is set up.
    pub(super) fn chat_available(&self) -> bool {
        self.config.ai.enabled && self.config.ai.endpoint.as_ref().is_some_and(|e| !e.base_url.trim().is_empty())
    }

    pub(super) fn chat_class(&self) -> AccessClass {
        match &self.config.ai.endpoint {
            Some(e) if e.local => AccessClass::Local,
            _ => AccessClass::Cloud,
        }
    }

    pub(super) fn chat_pane(&mut self, ui: &mut egui::Ui) {
        if !self.chat.loaded {
            self.chat.loaded = true;
            self.load_chats(None);
        }
        if std::mem::take(&mut self.chat.changes_stale) {
            self.refresh_chat_changes();
        }
        let index = self.index.as_ref();
        let config = &self.config;
        let class = self.chat_class();
        let scope = meshgrove_core::ai::Scope::new(config, class);
        let names = |id: &str| scope.resolve(id).map_or_else(|| id.rsplit('/').next().unwrap_or(id).to_owned(), |p| listing::file_name(&p));
        let model = config.ai.endpoint.as_ref().map(|e| e.model.clone()).unwrap_or_default();
        let view = ChatView {
            chats: &self.chat.chats,
            current: self.chat.current,
            messages: &self.chat.messages,
            streaming: self.chat.streaming.as_ref(),
            mode: config.ai.chat_mode,
            changes: &self.chat.changes,
            card_states: &self.chat.card_states,
            names: &names,
            model: &model,
        };
        // References: computed once per typed query.
        let selection = self.menu_for.clone();
        let current = self.nav.current().to_path_buf();
        let inbox = self.inbox_count;
        let offered = &mut self.chat.offered;
        let mut candidates = |query: &str| -> Vec<Candidate<Pill>> {
            if let Some((q, list)) = offered.as_ref()
                && q == query
            {
                return list.clone();
            }
            let list = index
                .and_then(|lib| Context::new(lib, config, class, None).ok())
                .map(|cx| references(&cx, query, &selection, &current, inbox))
                .unwrap_or_default();
            *offered = Some((query.to_owned(), list.clone()));
            list
        };
        let action = chat_view::show(ui, view, &mut self.chat.input, &mut self.chat.md, &mut candidates);
        match action {
            Some(ChatAction::NewChat) => {
                self.chat.current = None;
                self.chat.messages.clear();
            }
            Some(ChatAction::Open(id)) => self.load_chats(Some(id)),
            Some(ChatAction::Delete(id)) => {
                if let Some(mut s) = store() {
                    let _ = s.delete_chat(id);
                }
                self.chat.current = None;
                self.chat.messages.clear();
                self.load_chats(None);
            }
            Some(ChatAction::Mode(mode)) => {
                self.config.ai.chat_mode = mode;
                self.save_config();
            }
            Some(ChatAction::Send) => self.send_chat(false),
            Some(ChatAction::Stop) => {
                if let Some(s) = &self.chat.service {
                    s.stop();
                }
            }
            Some(ChatAction::Undo(message)) => self.chat_changes(message, true),
            Some(ChatAction::Redo(message)) => self.chat_changes(message, false),
            Some(ChatAction::Review) => self.show_review(),
            Some(ChatAction::Reveal(id)) => {
                if let Some(path) = meshgrove_core::ai::Scope::new(&self.config, self.chat_class()).resolve(&id) {
                    self.reveal(&path);
                }
            }
            None => {}
        }
    }

    /// Lists the conversations and opens `open` (or the newest).
    fn load_chats(&mut self, open: Option<i64>) {
        let Some(s) = store() else { return };
        self.chat.chats = s.chats(50).unwrap_or_default();
        let id = open.or(self.chat.current).or_else(|| self.chat.chats.first().map(|c| c.id));
        self.chat.current = id;
        self.chat.messages = id
            .and_then(|id| s.messages(id).ok())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|m| {
                let mut msg: ChatMsg = serde_json::from_value(m.content).ok()?;
                msg.id = m.id;
                Some(msg)
            })
            .collect();
        self.chat.changes_stale = true;
    }

    /// The journal origins of a user message's changes: made at once (`chat:<id>`), or
    /// suggested in an answer to it and accepted in the review (`proposal:<uid>`).
    fn origins_by_message(&self) -> HashMap<String, i64> {
        let mut out = HashMap::new();
        let mut asked = None;
        for m in &self.chat.messages {
            if m.user {
                asked = Some(m.id);
                out.insert(format!("chat:{}", m.id), m.id);
            } else if let Some(user) = asked {
                for card in &m.cards {
                    if let Card::Proposed { batch, .. } = card {
                        out.insert(format!("proposal:{batch}"), user);
                    }
                }
            }
        }
        out
    }

    /// Which user messages have changes, in effect or taken back.
    fn refresh_chat_changes(&mut self) {
        self.chat.changes.clear();
        self.chat.card_states.clear();
        if let Some(s) = store() {
            for m in &self.chat.messages {
                for card in &m.cards {
                    if let Card::Proposed { batch, .. } = card
                        && let Ok(Some(state)) = s.batch_state(batch)
                    {
                        self.chat.card_states.insert(batch.clone(), state);
                    }
                }
            }
        }
        let Some(index) = &self.index else { return };
        let origins = self.origins_by_message();
        for c in index.changes(1000).unwrap_or_default() {
            if let Some(&id) = origins.get(&c.origin) {
                let e = self.chat.changes.entry(id).or_default();
                if c.undone_ns.is_some() {
                    e.1 += 1;
                } else {
                    e.0 += 1;
                }
            }
        }
    }

    pub(super) fn chat_changed(&mut self) {
        self.chat.changes_stale = true;
    }

    fn chat_changes(&mut self, message: i64, undo: bool) {
        let Some(index) = &self.index else { return };
        let origins = self.origins_by_message();
        let changes = index.changes(1000).unwrap_or_default();
        for c in changes.iter().filter(|c| origins.get(&c.origin) == Some(&message)) {
            match (undo, c.undone_ns.is_some()) {
                (true, false) => self.library.undo(c.id),
                (false, true) => self.library.redo(c.id),
                _ => {}
            }
        }
    }

    /// Shows a file in the gallery: its folder, with it selected.
    pub(super) fn reveal(&mut self, path: &Path) {
        let Some(parent) = path.parent().map(Path::to_path_buf) else { return };
        self.flatten = false;
        self.leave_search();
        self.view = View::Folder;
        self.select_after_load = Some(path.to_path_buf());
        if parent == self.nav.current() {
            self.nav.reload();
        } else {
            self.nav.open(parent);
        }
    }

    /// Sends the message in the field; to a cloud endpoint only after the user said yes once.
    pub(super) fn send_chat(&mut self, confirmed: bool) {
        if self.chat.streaming.is_some() || self.chat.input.is_empty() {
            return;
        }
        let Some(endpoint) = self.config.ai.endpoint.clone() else { return };
        let (text, pills) = self.chat.input.take_message();
        let pictures = if endpoint.vision { self.pictures_for(&pills).len() } else { 0 };
        if !endpoint.local && !self.chat.cloud_ok && !confirmed {
            let host = host_of(&endpoint.base_url);
            let text = t!("chat.confirm_text", host = host, refs = pills.len(), pictures = pictures).into_owned();
            self.chat.confirm = Some((text, Pending::Message));
            return;
        }
        if confirmed {
            self.chat.cloud_ok = true;
        }
        let Some(mut s) = store() else { return };
        let chat = match self.chat.current {
            Some(c) => c,
            None => {
                let plain: String = text.replace("[[ref:", "@").chars().take(40).collect();
                match s.create_chat(plain.trim()) {
                    Ok(id) => id,
                    Err(e) => {
                        self.toast(e.to_string());
                        return;
                    }
                }
            }
        };
        let (llm, images) = self.llm_message(&text, &pills, endpoint.vision);
        let mut msg = ChatMsg { user: true, text, pills, llm: llm.clone(), ..Default::default() };
        msg.id = s.add_message(chat, "user", &serde_json::to_value(&msg).unwrap_or_default()).unwrap_or(0);
        // The conversation so far, then this message (with its pictures).
        let mut messages = vec![json!({"role": "system", "content": system_prompt(self.config.ai.chat_mode)})];
        let start = self.chat.messages.len().saturating_sub(HISTORY);
        for m in &self.chat.messages[start..] {
            if m.user {
                messages.push(json!({"role": "user", "content": m.llm}));
            } else if !m.text.is_empty() {
                messages.push(json!({"role": "assistant", "content": m.text}));
            }
        }
        let content = if images.is_empty() {
            json!(llm)
        } else {
            let mut parts = vec![json!({"type": "text", "text": llm})];
            for png in images {
                let data = base64::engine::general_purpose::STANDARD.encode(png);
                parts.push(json!({"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{data}")}}));
            }
            Value::Array(parts)
        };
        messages.push(json!({"role": "user", "content": content}));
        let key = secret::load(&endpoint.base_url, endpoint.key_in_keyring, &endpoint.key_env);
        let service = self.chat.service.get_or_insert_with(|| {
            ChatService::start(self.db_path.clone(), self.library.applier(), self.egui_ctx.clone())
        });
        service.send(Turn {
            user_message: msg.id,
            messages,
            config: self.config.clone(),
            key,
            mode: self.config.ai.chat_mode,
        });
        self.chat.answering = msg.id;
        self.chat.messages.push(msg);
        self.chat.streaming = Some(ChatMsg::default());
        self.chat.input.clear();
        self.chat.offered = None;
        self.chat.current = Some(chat);
        self.chat.chats = s.chats(50).unwrap_or_default();
    }

    /// Takes what the chat thread reports; runs every frame so an answer comes in while the
    /// pane is hidden too.
    pub(super) fn poll_chat(&mut self) {
        let Some(service) = &self.chat.service else { return };
        let mut events = Vec::new();
        while let Some(e) = service.try_event() {
            events.push(e);
        }
        for e in events {
            let Some(msg) = self.chat.streaming.as_mut() else { continue };
            match e {
                ChatEvent::Text(t) => msg.text.push_str(&t),
                ChatEvent::Tool { name, summary } => msg.tools.push((name, summary)),
                ChatEvent::Proposed { batch, title, items } => msg.cards.push(Card::Proposed { batch, title, items }),
                ChatEvent::Applied { change, title, applied } => {
                    msg.cards.push(Card::Applied { change, title, applied });
                    self.chat.changes_stale = true;
                }
                ChatEvent::Notice(n) => msg.notices.push(n),
                ChatEvent::Done(_) | ChatEvent::Failed(_) => {
                    let mut msg = self.chat.streaming.take().unwrap_or_default();
                    if let ChatEvent::Failed(error) = &e {
                        msg.notices.push(error.clone());
                        msg.failed = true;
                    }
                    if let (Some(mut s), Some(chat)) = (store(), self.chat.current) {
                        msg.id = s.add_message(chat, "assistant", &serde_json::to_value(&msg).unwrap_or_default()).unwrap_or(0);
                    }
                    self.chat.messages.push(msg);
                    self.chat.changes_stale = true;
                }
            }
        }
    }

    /// The thumbnails of the referenced files that may be sent (pictures allowed), at most four.
    fn pictures_for(&self, pills: &[Pill]) -> Vec<Vec<u8>> {
        let Some(index) = &self.index else { return Vec::new() };
        let Some(dir) = ThumbCache::default_location() else { return Vec::new() };
        let thumbs = ThumbCache::new(dir);
        let Ok(cx) = Context::new(index, &self.config, self.chat_class(), Some(&thumbs)) else { return Vec::new() };
        let ids: Vec<String> = pills
            .iter()
            .flat_map(|p| match &p.reference {
                Reference::Files { paths } => paths.clone(),
                _ => Vec::new(),
            })
            .filter_map(|p| cx.scope.id_of(&p))
            .take(MAX_PICTURES)
            .collect();
        read::get_thumbnails(&cx, &ids, 256).map(|(images, _)| images.into_iter().map(|(_, png)| png).collect()).unwrap_or_default()
    }

    /// The message for the model: the text with each reference as ⟦n⟧ and their data below.
    fn llm_message(&self, text: &str, pills: &[Pill], vision: bool) -> (String, Vec<Vec<u8>>) {
        let mut out = text.to_owned();
        for (n, p) in pills.iter().enumerate() {
            out = out.replace(&format!("[[ref:{n}]]"), &format!("⟦{n}: {}⟧", p.label));
        }
        if pills.is_empty() {
            return (out, Vec::new());
        }
        out.push_str("\n\nReferences:");
        if let Some(index) = &self.index
            && let Ok(cx) = Context::new(index, &self.config, self.chat_class(), None)
        {
            for (n, p) in pills.iter().enumerate() {
                out.push_str(&format!("\n⟦{n}⟧ {}: {}", p.label, reference_data(&cx, &p.reference)));
            }
        }
        let images = if vision { self.pictures_for(pills) } else { Vec::new() };
        (out, images)
    }

    /// Asks before a message goes to a cloud endpoint for the first time this session, and
    /// before every one-click action does.
    pub(super) fn chat_confirm_modal(&mut self, ctx: &egui::Context) {
        let Some((text, pending)) = &self.chat.confirm else { return };
        let text = text.clone();
        let hint = match pending {
            Pending::Action { .. } => t!("actions.confirm_hint"),
            _ => t!("chat.confirm_hint"),
        };
        let mut decision = None;
        egui::Modal::new(egui::Id::unique("chat_confirm")).show(ctx, |ui| {
            ui.set_max_width(460.0);
            ui.heading(t!("chat.confirm_title"));
            ui.label(text);
            ui.weak(hint);
            ui.horizontal(|ui| {
                if ui.button(t!("chat.confirm_send")).clicked() {
                    decision = Some(true);
                }
                if ui.button(t!("trash.cancel")).clicked() {
                    decision = Some(false);
                }
            });
        });
        match decision.and_then(|yes| Some((yes, self.chat.confirm.take()?.1))) {
            Some((true, Pending::Message)) => self.send_chat(true),
            Some((true, Pending::Action { title, user_text, llm, files, action })) => {
                self.start_action(title, user_text, llm, files, action, true);
            }
            Some((true, Pending::TagCandidates)) => self.ask_tag_candidates(true),
            // Declined: nothing is sent.
            _ => {}
        }
    }

    /// Starts a one-click action in a new conversation: `user_text` stands as the user's
    /// message (`llm` is what the model reads of it later), the result comes as the answer.
    /// Actions only suggest, whatever the chat's mode.
    pub(super) fn start_action(
        &mut self,
        title: String,
        user_text: String,
        llm: String,
        files: usize,
        action: Action,
        confirmed: bool,
    ) {
        if !self.chat_available() {
            return;
        }
        if self.chat.streaming.is_some() {
            self.toast(t!("actions.busy").into_owned());
            return;
        }
        let Some(endpoint) = self.config.ai.endpoint.clone() else { return };
        if !endpoint.local && !confirmed {
            let pictures = if endpoint.vision { files } else { 0 };
            let text = t!("actions.confirm_text", host = host_of(&endpoint.base_url), files = files, pictures = pictures);
            self.chat.confirm = Some((text.into_owned(), Pending::Action { title, user_text, llm, files, action }));
            return;
        }
        let Some(mut s) = store() else { return };
        let chat = match s.create_chat(&title) {
            Ok(id) => id,
            Err(e) => {
                self.toast(e.to_string());
                return;
            }
        };
        let mut msg = ChatMsg { user: true, text: user_text, llm, ..Default::default() };
        msg.id = s.add_message(chat, "user", &serde_json::to_value(&msg).unwrap_or_default()).unwrap_or(0);
        let key = secret::load(&endpoint.base_url, endpoint.key_in_keyring, &endpoint.key_env);
        let service = self.chat.service.get_or_insert_with(|| {
            ChatService::start(self.db_path.clone(), self.library.applier(), self.egui_ctx.clone())
        });
        service.act(ActionJob { action, config: self.config.clone(), key });
        self.chat.current = Some(chat);
        self.chat.answering = msg.id;
        self.chat.messages = vec![msg];
        self.chat.streaming = Some(ChatMsg::default());
        self.chat.chats = s.chats(50).unwrap_or_default();
        self.chat.changes_stale = true;
        self.layout.focus_chat();
    }

    /// Puts files into the message field as a reference, and shows the chat.
    pub(super) fn mention_in_chat(&mut self, paths: Vec<PathBuf>) {
        let scope = meshgrove_core::ai::Scope::new(&self.config, self.chat_class());
        let paths: Vec<PathBuf> = paths.into_iter().filter(|p| scope.allows(p)).collect();
        if paths.is_empty() {
            self.toast(t!("actions.nothing_visible").into_owned());
            return;
        }
        let label = if paths.len() == 1 { listing::file_name(&paths[0]) } else { t!("chat.ref_files", count = paths.len()).into_owned() };
        self.chat.input.append_pill(Pill { label, reference: Reference::Files { paths } });
        self.layout.focus_chat();
        self.egui_ctx.request_repaint();
    }
}

/// What a reference stands for, as the model gets it.
fn reference_data(cx: &Context, r: &Reference) -> String {
    let value: Value = match r {
        Reference::Files { paths } => serde_json::to_value(read::summaries(cx, &paths[..paths.len().min(20)]).unwrap_or_default()),
        Reference::Folder { path } => match cx.scope.id_of(path) {
            Some(id) => serde_json::to_value(read::folder_summary(cx, &id).ok()),
            None => Ok(Value::Null),
        },
        Reference::List { name } => serde_json::to_value(read::get_list(cx, name).ok()),
        Reference::Tag { name } => serde_json::to_value(
            read::search_models(cx, &ModelFilter { tags_all: vec![name.clone()], limit: Some(30), ..Default::default() }).ok(),
        ),
        Reference::Printer { name } => {
            serde_json::to_value(read::list_printers(cx).into_iter().find(|p| p.name == *name))
        }
        Reference::Search { query } => {
            serde_json::to_value(read::search_models(cx, &ModelFilter { q: Some(query.clone()), limit: Some(30), ..Default::default() }).ok())
        }
    }
    .unwrap_or(Value::Null);
    let mut text = value.to_string();
    if text.len() > 8000 {
        let mut cut = 8000;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push('…');
    }
    text
}

fn pill(label: String, detail: String, existing: bool, reference: Reference) -> Candidate<Pill> {
    Candidate { label: label.clone(), detail, existing, value: Pill { label, reference } }
}

/// The references offered for `query` after `@`, from what this endpoint may see.
fn references(cx: &Context, query: &str, selection: &[PathBuf], current: &Path, inbox: usize) -> Vec<Candidate<Pill>> {
    let mut out = Vec::new();
    let visible: Vec<PathBuf> = selection.iter().filter(|p| cx.scope.allows(p)).cloned().collect();
    if !visible.is_empty() {
        let label = if visible.len() == 1 { listing::file_name(&visible[0]) } else { t!("chat.ref_selection").into_owned() };
        out.push(pill(label, t!("chat.ref_files", count = visible.len()).into_owned(), true, Reference::Files { paths: visible }));
    }
    if cx.scope.allows(current) {
        out.push(pill(listing::file_name(current), t!("chat.ref_current_folder").into_owned(), true, Reference::Folder { path: current.to_path_buf() }));
    }
    if inbox > 0 {
        out.push(pill(t!("chat.ref_inbox").into_owned(), t!("chat.ref_new", count = inbox).into_owned(), true, Reference::Search { query: "is:neu".into() }));
    }
    if query.contains(':') {
        let found = read::search_models(cx, &ModelFilter { q: Some(query.into()), limit: Some(200), ..Default::default() })
            .map(|p| p.items.len())
            .unwrap_or(0);
        out.insert(0, pill(format!("{} {query}", t!("chat.ref_search")), t!("chat.ref_hits", count = found).into_owned(), true, Reference::Search { query: query.into() }));
        return out;
    }
    if let Ok(tags) = read::list_tags(cx) {
        for t in tags.existing.into_iter().take(200) {
            out.push(pill(t.name.clone(), t!("chat.ref_tag", count = t.count).into_owned(), true, Reference::Tag { name: t.name }));
        }
    }
    if let Ok(lists) = read::list_lists(cx) {
        for l in lists {
            out.push(pill(l.name.clone(), t!("chat.ref_list", count = l.count).into_owned(), true, Reference::List { name: l.name }));
        }
    }
    for p in read::list_printers(cx) {
        out.push(pill(p.name.clone(), t!("chat.ref_printer").into_owned(), true, Reference::Printer { name: p.name }));
    }
    if query.chars().count() >= 2 {
        if let Ok(page) = read::search_models(cx, &ModelFilter { q: Some(query.into()), limit: Some(8), ..Default::default() }) {
            for m in page.items {
                if let Some(path) = cx.scope.resolve(&m.id) {
                    let detail = m.size_mm.map_or_else(|| m.kind.clone(), |s| format!("{} · {:.0}×{:.0}×{:.0} mm", m.kind, s[0], s[1], s[2]));
                    out.push(pill(m.name, detail, false, Reference::Files { paths: vec![path] }));
                }
            }
        }
        for path in read::find_folders(cx, query, 5).unwrap_or_default() {
            out.push(pill(listing::file_name(&path), t!("chat.ref_folder").into_owned(), false, Reference::Folder { path }));
        }
    }
    out
}
