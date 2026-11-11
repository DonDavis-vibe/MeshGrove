//! The "KI-Chat" pane: the conversation with the user's own model (the endpoint, or an AI agent
//! of this computer), its answers with the files they mention, what its tools did, the message
//! field with references, and under it who answers and with which model.

use std::collections::HashMap;

use egui::{RichText, Ui};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use meshgrove_core::ai::proposals::{AskQuestion, ChatInfo};
use meshgrove_core::config::ChatMode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::chat_input::{self, ChatInput, Pill};
use super::picker::{self, Choice, Picked, Picker};
use super::suggest::Candidate;
use crate::app::ModelList;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatMsg {
    #[serde(skip)]
    pub id: i64,
    pub user: bool,
    /// What the user wrote (pills as `[[ref:n]]`), or the answer (files as `[[id]]`).
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pills: Vec<Pill>,
    /// What went to the model for a user message: the text with the references' data.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub llm: String,
    /// Tools the model used: name and what came of it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cards: Vec<Card>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<String>,
    #[serde(default)]
    pub failed: bool,
    /// Pictures a plugin's tool returned; shown while the chat is open, not kept.
    #[serde(skip)]
    pub pictures: Vec<Picture>,
}

/// A picture in the chat.
#[derive(Clone, PartialEq)]
pub struct Picture(pub egui::TextureHandle);

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Picture").field(&self.0.size()).finish()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "card", rename_all = "snake_case")]
pub enum Card {
    /// Suggestions waiting in the review.
    Proposed { batch: String, title: String, items: usize },
    /// Changes made at once (automatic mode).
    Applied { change: i64, title: String, applied: usize },
    /// A question of the assistant and what the user answered.
    Answered { question: String, answer: String },
}

/// Questions the assistant asked, with what the user picked so far.
#[derive(Clone, Debug, PartialEq)]
pub struct AskCard {
    pub uid: String,
    /// Who asks, when not the chat's own assistant: the card then gets a window of its own.
    pub from: Option<String>,
    pub questions: Vec<AskQuestion>,
    /// Per question, per option: picked.
    pub chosen: Vec<Vec<bool>>,
    /// Per question: the answer in the user's own words.
    pub other: Vec<String>,
}

impl AskCard {
    pub fn new(uid: String, questions: Vec<AskQuestion>) -> AskCard {
        let chosen = questions.iter().map(|q| vec![false; q.options.len()]).collect();
        let other = vec![String::new(); questions.len()];
        AskCard { uid, from: None, questions, chosen, other }
    }

    /// Every question has an answer.
    pub fn complete(&self) -> bool {
        self.questions
            .iter()
            .enumerate()
            .all(|(i, q)| self.chosen[i].iter().any(|c| *c) || (q.allow_other && !self.other[i].trim().is_empty()))
    }

    /// The answers as the assistant gets them.
    pub fn answer(&self) -> Value {
        let answers: Vec<Value> = self
            .questions
            .iter()
            .enumerate()
            .map(|(i, q)| {
                let selected: Vec<&str> =
                    q.options.iter().enumerate().filter(|(j, _)| self.chosen[i][*j]).map(|(_, o)| o.as_str()).collect();
                let mut a = json!({"question": q.question, "selected": selected});
                let other = self.other[i].trim();
                if !other.is_empty() {
                    a["other"] = json!(other);
                }
                a
            })
            .collect();
        json!({"answers": answers})
    }

    /// The answers in one line, for the card that stays in the conversation.
    pub fn summary(&self) -> String {
        self.questions
            .iter()
            .enumerate()
            .map(|(i, q)| {
                let mut parts: Vec<String> =
                    q.options.iter().enumerate().filter(|(j, _)| self.chosen[i][*j]).map(|(_, o)| o.clone()).collect();
                let other = self.other[i].trim();
                if !other.is_empty() {
                    parts.push(other.to_owned());
                }
                parts.join(", ")
            })
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// A permission question an agent asked: what it wants to run, and its answers (id, name).
#[derive(Clone, Debug, PartialEq)]
pub struct Question {
    pub key: u64,
    pub title: String,
    pub options: Vec<(String, String)>,
}

/// One way an agent offers to sign in.
#[derive(Clone, Debug, PartialEq)]
pub struct AuthRow {
    pub id: String,
    pub name: String,
    pub description: String,
    /// What to run in a terminal (or which variables to set); `None` when the agent does it.
    pub command: Option<String>,
    /// The agent signs in by itself when asked (it opens the browser).
    pub by_agent: bool,
}

/// A selector beside the model: an endpoint's reasoning effort, or one of an agent's session
/// options; its choices as (value, name, description).
#[derive(Clone, Debug, PartialEq)]
pub struct OptionRow {
    pub id: String,
    pub name: String,
    pub description: String,
    pub current: String,
    pub choices: Vec<(String, String, String)>,
}

pub struct ChatView<'a> {
    pub chats: &'a [ChatInfo],
    pub current: Option<i64>,
    pub messages: &'a [ChatMsg],
    /// The answer being written.
    pub streaming: Option<&'a ChatMsg>,
    pub mode: ChatMode,
    /// Per user message: changes still in effect, and taken back.
    pub changes: &'a HashMap<i64, (usize, usize)>,
    /// What became of the cards' suggestions: batch uid → state.
    pub card_states: &'a HashMap<String, String>,
    /// A file id's name for the answer's references.
    pub names: &'a dyn Fn(&str) -> String,
    /// Who can answer: (id, name), and who does.
    pub assistants: &'a [(String, String)],
    pub assistant: &'a str,
    /// The assistant's models, for the picker.
    pub models: &'a ModelList,
    /// The selectors beside it (reasoning effort, an agent's mode …).
    pub options: &'a [OptionRow],
    /// What an agent is up to (connecting, its name, a sign-in needed); `None` for the endpoint.
    pub status: Option<&'a str>,
    /// Permission questions the agent asked.
    pub questions: &'a [Question],
    /// The agent needs a sign-in, in these ways.
    pub auth: &'a [AuthRow],
}

pub enum ChatAction {
    NewChat,
    Open(i64),
    Delete(i64),
    Mode(ChatMode),
    Send,
    Stop,
    Undo(i64),
    Redo(i64),
    Review,
    /// Show this file (an id) in the gallery.
    Reveal(String),
    /// Another assistant answers from now on (its id).
    Assistant(String),
    /// Another model, listed or typed.
    Model(Picked),
    /// One of the selectors beside the model: the option and its new value.
    Option {
        id: String,
        value: String,
    },
    /// The agent's permission question `key`, answered with an option (`None`: cancelled).
    Permission {
        key: u64,
        option: Option<String>,
    },
    /// The agent signs in by itself with this method.
    Authenticate(String),
    /// The assistant's question `uid` is answered (what the card holds), or not at all.
    Answer {
        uid: String,
        cancel: bool,
    },
}

pub fn show(
    ui: &mut Ui,
    view: ChatView,
    input: &mut ChatInput,
    asks: &mut [AskCard],
    md: &mut CommonMarkCache,
    candidates: &mut dyn FnMut(&str) -> Vec<Candidate<Pill>>,
) -> Option<ChatAction> {
    let mut action = None;
    // Header: conversations, a new one, the mode.
    ui.horizontal(|ui| {
        let title = view
            .current
            .and_then(|c| view.chats.iter().find(|i| i.id == c))
            .map_or_else(|| t!("chat.new_title").into_owned(), |c| short(&c.title, 26));
        egui::ComboBox::from_id_salt("chats").selected_text(title).width(160.0).truncate().show_ui(ui, |ui| {
            for c in view.chats {
                if ui.selectable_label(view.current == Some(c.id), &c.title).clicked() {
                    action = Some(ChatAction::Open(c.id));
                }
            }
        });
        if ui.button("+").on_hover_text(t!("chat.new")).clicked() {
            action = Some(ChatAction::NewChat);
        }
        if let Some(c) = view.current
            && ui.button("🗑").on_hover_text(t!("chat.delete")).clicked()
        {
            action = Some(ChatAction::Delete(c));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut mode = view.mode;
            ui.selectable_value(&mut mode, ChatMode::Auto, t!("chat.mode_auto"))
                .on_hover_text(t!("chat.mode_auto_hint"));
            ui.selectable_value(&mut mode, ChatMode::Review, t!("chat.mode_review"))
                .on_hover_text(t!("chat.mode_review_hint"));
            if mode != view.mode {
                action = Some(ChatAction::Mode(mode));
            }
        });
    });
    ui.separator();

    // The field at the bottom, the conversation above it.
    let busy = view.streaming.is_some();
    egui::Panel::bottom("chat_input").resizable(false).show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Max), |ui| {
                if busy {
                    if ui.button("⏹").on_hover_text(t!("chat.stop")).clicked() {
                        action = Some(ChatAction::Stop);
                    }
                } else if ui
                    .add_enabled(!input.is_empty(), egui::Button::new("➤"))
                    .on_hover_text(t!("chat.send"))
                    .clicked()
                {
                    action = Some(ChatAction::Send);
                }
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    let out = chat_input::show(ui, input, egui::Id::unique("chat_field"), &t!("chat.hint"), candidates);
                    if out.send && !busy {
                        action = Some(ChatAction::Send);
                    }
                });
            });
        });
        if let Some(a) = assistant_row(ui, &view) {
            action = Some(a);
        }
    });
    egui::CentralPanel::default().show(ui, |ui| {
        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink(false).show(ui, |ui| {
            if view.messages.is_empty() && view.streaming.is_none() {
                ui.weak(t!("chat.empty"));
            }
            for m in view.messages.iter().chain(view.streaming) {
                if let Some(a) = message(ui, m, &view, md) {
                    action = Some(a);
                }
                ui.add_space(6.0);
            }
            for q in view.questions {
                if let Some(a) = question(ui, q) {
                    action = Some(a);
                }
            }
            for card in asks.iter_mut().filter(|c| c.from.is_none()) {
                if let Some(a) = ask_card(ui, card) {
                    action = Some(a);
                }
            }
            if !view.auth.is_empty()
                && let Some(a) = sign_in(ui, view.auth)
            {
                action = Some(a);
            }
            if busy && view.questions.is_empty() && asks.is_empty() {
                ui.spinner();
            }
        });
    });
    action
}

/// Under the field: who answers (a choice when there are several), the model picker, and what
/// an agent is up to.
fn assistant_row(ui: &mut Ui, view: &ChatView) -> Option<ChatAction> {
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        let name = view
            .assistants
            .iter()
            .find(|(id, _)| id == view.assistant)
            .map(|(_, name)| name.as_str())
            .unwrap_or(view.assistant);
        if view.assistants.len() > 1 {
            egui::ComboBox::from_id_salt("chat_assistant").selected_text(name).width(150.0).truncate().show_ui(
                ui,
                |ui| {
                    for (id, name) in view.assistants {
                        if ui.selectable_label(id == view.assistant, name).clicked() {
                            action = Some(ChatAction::Assistant(id.clone()));
                        }
                    }
                },
            );
        } else {
            ui.weak(name);
        }
        let choices: Vec<Choice> = view
            .models
            .choices
            .iter()
            .map(|c| Choice { value: &c.value, label: &c.label, tip: &c.tip, group: c.group.as_deref() })
            .collect();
        let none = t!("chat.model_none");
        let current = if view.models.current.is_empty() { none.as_ref() } else { view.models.current.as_str() };
        let picked = picker::show(
            ui,
            Picker {
                id: "chat_model",
                current,
                current_value: &view.models.current,
                hint: &t!("picker.search_hint"),
                choices: &choices,
                custom: true,
                note: view.models.note.as_deref(),
                width: Some(200.0),
                enabled: true,
            },
        );
        if let Some(p) = picked.inner {
            action = Some(ChatAction::Model(p));
        }
        for o in view.options {
            let selected = o.choices.iter().find(|c| c.0 == o.current).map_or(o.current.as_str(), |c| c.1.as_str());
            let tip = [o.name.as_str(), o.description.as_str()]
                .into_iter()
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            egui::ComboBox::from_id_salt(("chat_option", &o.id))
                .selected_text(picker::compact(selected, 24))
                .width(110.0)
                .truncate()
                .show_ui(ui, |ui| {
                    ui.label(RichText::new(&o.name).small().weak());
                    for (value, name, description) in &o.choices {
                        let r = ui.selectable_label(*value == o.current, name);
                        let r = if description.is_empty() { r } else { r.on_hover_text(description) };
                        if r.clicked() && *value != o.current {
                            action = Some(ChatAction::Option { id: o.id.clone(), value: value.clone() });
                        }
                    }
                })
                .response
                .on_hover_text(tip);
        }
        if let Some(status) = view.status {
            ui.weak(status);
        }
    });
    action
}

/// The assistant's questions: radio buttons where one answer is wanted, checkboxes where
/// several may be picked, a field for an answer in the user's own words where allowed.
pub fn ask_card(ui: &mut Ui, card: &mut AskCard) -> Option<ChatAction> {
    let mut action = None;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(t!("chat.question_title")).strong());
        for i in 0..card.questions.len() {
            let q = card.questions[i].clone();
            ui.add_space(4.0);
            ui.label(&q.question);
            for (j, option) in q.options.iter().enumerate() {
                if q.multi {
                    ui.checkbox(&mut card.chosen[i][j], option);
                } else if ui.radio(card.chosen[i][j], option).clicked() {
                    card.chosen[i].fill(false);
                    card.chosen[i][j] = true;
                }
            }
            if q.allow_other {
                ui.horizontal(|ui| {
                    if !q.options.is_empty() {
                        ui.weak(t!("chat.question_other"));
                    }
                    let field =
                        ui.add(egui::TextEdit::singleline(&mut card.other[i]).desired_width(f32::INFINITY).id_salt((
                            "ask_other",
                            &card.uid,
                            i,
                        )));
                    // Typing an own answer to a one-answer question drops the option picked.
                    if field.changed() && !q.multi && !card.other[i].trim().is_empty() {
                        card.chosen[i].fill(false);
                    }
                });
            }
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(card.complete(), egui::Button::new(t!("chat.answer"))).clicked() {
                action = Some(ChatAction::Answer { uid: card.uid.clone(), cancel: false });
            }
            if ui.button(t!("chat.answer_cancel")).clicked() {
                action = Some(ChatAction::Answer { uid: card.uid.clone(), cancel: true });
            }
        });
    });
    action
}

/// An agent's permission question: what it wants to run, and the answers it offers.
fn question(ui: &mut Ui, q: &Question) -> Option<ChatAction> {
    let mut action = None;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(t!("assistant.asks", tool = q.title.clone())).strong());
        ui.horizontal_wrapped(|ui| {
            for (id, name) in &q.options {
                if ui.button(name).clicked() {
                    action = Some(ChatAction::Permission { key: q.key, option: Some(id.clone()) });
                }
            }
            if ui.button(t!("trash.cancel")).clicked() {
                action = Some(ChatAction::Permission { key: q.key, option: None });
            }
        });
    });
    action
}

/// The agent needs a sign-in: its ways to do so, a button where it does it itself, the command
/// to run where the user has to.
fn sign_in(ui: &mut Ui, methods: &[AuthRow]) -> Option<ChatAction> {
    let mut action = None;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(t!("assistant.sign_in_title")).strong());
        for m in methods {
            ui.horizontal_wrapped(|ui| {
                if m.by_agent {
                    if ui.button(&m.name).on_hover_text(&m.description).clicked() {
                        action = Some(ChatAction::Authenticate(m.id.clone()));
                    }
                } else {
                    ui.label(&m.name);
                }
            });
            if let Some(command) = &m.command {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut command.as_str()).desired_width(f32::INFINITY).code_editor(),
                    );
                    if ui.button(t!("settings.mcp_copy")).clicked() {
                        ui.ctx().copy_text(command.clone());
                    }
                });
            }
        }
        ui.weak(t!("assistant.sign_in_hint"));
    });
    action
}

fn message(ui: &mut Ui, m: &ChatMsg, view: &ChatView, md: &mut CommonMarkCache) -> Option<ChatAction> {
    let mut action = None;
    if m.user {
        let fill = ui.visuals().faint_bg_color;
        egui::Frame::new().fill(fill).corner_radius(6.0).inner_margin(6.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if let Some(&(active, undone)) = view.changes.get(&m.id) {
                        if active > 0 {
                            if ui.small_button("↶").on_hover_text(t!("chat.undo", count = active)).clicked() {
                                action = Some(ChatAction::Undo(m.id));
                            }
                        } else if undone > 0
                            && ui.small_button("↷").on_hover_text(t!("chat.redo", count = undone)).clicked()
                        {
                            action = Some(ChatAction::Redo(m.id));
                        }
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| user_text(ui, m));
                });
            });
        });
        return action;
    }
    let mut mentioned = Vec::new();
    let text = with_references(&m.text, view.names, &mut mentioned);
    if !text.trim().is_empty() {
        CommonMarkViewer::new().show(ui, md, &text);
    }
    if !mentioned.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for id in mentioned {
                if ui.small_button(format!("📄 {}", (view.names)(&id))).on_hover_text(&id).clicked() {
                    action = Some(ChatAction::Reveal(id));
                }
            }
        });
    }
    if !m.tools.is_empty() {
        egui::CollapsingHeader::new(RichText::new(t!("chat.tools", count = m.tools.len())).weak().small())
            .id_salt(("tools", m.id, m.tools.len()))
            .show(ui, |ui| {
                for (name, summary) in &m.tools {
                    ui.horizontal(|ui| {
                        ui.monospace(name);
                        ui.weak(summary);
                    });
                }
            });
    }
    if !m.pictures.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for p in &m.pictures {
                let size = p.0.size_vec2();
                let scale = (240.0 / size.x.max(size.y)).min(1.0);
                let image = egui::Image::from_texture(egui::load::SizedTexture::new(p.0.id(), size * scale));
                ui.add(image);
            }
        });
    }
    for card in &m.cards {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            match card {
                Card::Proposed { batch, title, items } => {
                    // The title on a line of its own, cut short in a narrow pane; the state and
                    // the button wrap below it, so the button never gets cut off.
                    ui.add(egui::Label::new(RichText::new(format!("✨ {title}")).strong()).truncate());
                    ui.horizontal_wrapped(|ui| {
                        let state = view.card_states.get(batch).map_or("pending", String::as_str);
                        let open = matches!(state, "pending" | "partly");
                        ui.weak(match state {
                            "pending" if *items == 1 => t!("chat.card_proposed_one"),
                            "pending" => t!("chat.card_proposed", count = items),
                            "partly" => t!("chat.card_partly"),
                            "accepted" => t!("chat.card_accepted"),
                            "rejected" => t!("chat.card_rejected"),
                            "undone" => t!("chat.card_undone"),
                            _ => t!("chat.card_expired"),
                        });
                        if open && ui.button(t!("chat.card_review")).clicked() {
                            action = Some(ChatAction::Review);
                        }
                    });
                }
                Card::Applied { title, applied, .. } => {
                    ui.add(
                        egui::Label::new(format!("✔ {}", t!("chat.card_applied", title = title, count = applied)))
                            .wrap(),
                    );
                }
                Card::Answered { question, answer } => {
                    ui.add(
                        egui::Label::new(
                            RichText::new(t!("chat.card_answered", question = question, answer = answer)).weak(),
                        )
                        .wrap(),
                    );
                }
            }
        });
    }
    for notice in &m.notices {
        ui.colored_label(ui.visuals().warn_fg_color, notice);
    }
    if m.failed {
        ui.colored_label(ui.visuals().error_fg_color, t!("chat.failed"));
    }
    action
}

/// `text` cut to `max` characters, with "…" when cut.
fn short(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// The user's words with their references as small chips.
fn user_text(ui: &mut Ui, m: &ChatMsg) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        let mut rest = m.text.as_str();
        while let Some(start) = rest.find("[[ref:") {
            let Some(end) = rest[start..].find("]]") else { break };
            if start > 0 {
                ui.label(&rest[..start]);
            }
            let n: usize = rest[start + 6..start + end].parse().unwrap_or(usize::MAX);
            if let Some(p) = m.pills.get(n) {
                let fill = ui.visuals().selection.bg_fill.linear_multiply(0.6);
                let text = RichText::new(format!("@{}", p.label)).small().color(ui.visuals().strong_text_color());
                ui.add(
                    egui::Button::new(text)
                        .fill(fill)
                        .stroke(egui::Stroke::NONE)
                        .corner_radius(8.0)
                        .sense(egui::Sense::hover()),
                );
            }
            rest = &rest[start + end + 2..];
        }
        if !rest.is_empty() {
            ui.label(rest);
        }
    });
}

/// The answer with `[[id]]` as the file's name in bold; the ids in order, each once.
pub fn with_references(text: &str, names: &dyn Fn(&str) -> String, ids: &mut Vec<String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        let Some(end) = rest[start + 2..].find("]]") else { break };
        let id = rest[start + 2..start + 2 + end].trim().trim_start_matches("meshgrove:");
        out.push_str(&rest[..start]);
        if id.is_empty() || id.contains('\n') || id.starts_with("ref:") {
            out.push_str(&rest[start..start + 4 + end]);
        } else {
            out.push_str(&format!("**{}**", names(id)));
            if !ids.iter().any(|i| i == id) {
                ids.push(id.to_owned());
            }
        }
        rest = &rest[start + 4 + end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_in_answers_become_names() {
        let names = |id: &str| id.rsplit('/').next().unwrap_or(id).to_owned();
        let mut ids = Vec::new();
        let text =
            with_references("Nimm [[Garten/rose.stl]] und [[ Garten/rose.stl ]], nicht [[ref:0]].", &names, &mut ids);
        assert_eq!(text, "Nimm **rose.stl** und **rose.stl**, nicht [[ref:0]].");
        assert_eq!(ids, ["Garten/rose.stl"]);
    }
}
