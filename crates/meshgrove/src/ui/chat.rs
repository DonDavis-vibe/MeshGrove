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

use solidcraft_theme::Tokens;
use solidcraft_ui_kit as sc;

use super::chat_input::{self, ChatInput, Pill};
use super::picker::{self, Choice, Picked, Picker};
use super::settings_kit;
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
    /// Who can answer: (id, name), and who does (empty: nobody picked yet).
    pub assistants: &'a [(String, String)],
    pub assistant: &'a str,
    /// The assistant can answer: the endpoint always, an agent once it was connected.
    pub connected: bool,
    /// The search for agents on this computer is still on.
    pub looking: bool,
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
    /// Go to what a reference in a message stands for: the file in its folder, the folder,
    /// the list, the tag's or the search's results, the printer's settings.
    Follow(chat_input::Reference),
    /// Another assistant answers from now on (its id).
    Assistant(String),
    /// Start the picked agent.
    Connect,
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
    // The pane is one of the kit's forms (its controls' height, fills and fonts), built from its
    // parts for an assistant's chat: the header band, the bubble of a message, the tools' list,
    // the cards, and the composer with its round button.
    sc::scope(ui, |ui| {
        // The kit's forms cut their texts short; a conversation wraps.
        ui.style_mut().wrap_mode = None;
        let mut action = header(ui, &view);
        let t = Tokens::get();
        let busy = view.streaming.is_some();
        // The field at the bottom, the conversation above it.
        egui::Panel::bottom("chat_input").resizable(false).frame(egui::Frame::NONE).show(ui, |ui| {
            if let Some(a) = composer(ui, &view, input, busy, candidates) {
                action = Some(a);
            }
        });
        let frame = egui::Frame::NONE.inner_margin(egui::Margin::symmetric(t.space_10 as i8, t.space_8 as i8));
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink(false).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = t.space_10;
                if view.messages.is_empty() && view.streaming.is_none() {
                    ui.add_space(t.space_24);
                    sc::assistant_empty(ui, &t!("pane.chat"), &t!("chat.empty"), |_| {});
                }
                for m in view.messages.iter().chain(view.streaming) {
                    if let Some(a) = message(ui, m, &view, md) {
                        action = Some(a);
                    }
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
                    crate::ui::loading::mark(ui);
                }
            });
        });
        action
    })
}

/// The band over the conversation: the conversations, a new one, deleting this one, and at
/// its right end whether the assistant's changes wait in the review.
fn header(ui: &mut Ui, view: &ChatView) -> Option<ChatAction> {
    let mut action = None;
    let t = Tokens::get();
    sc::assistant_header(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = t.space_4;
            let modes = [
                (ChatMode::Review, t!("chat.mode_review").into_owned()),
                (ChatMode::Auto, t!("chat.mode_auto").into_owned()),
            ];
            // The list takes what the buttons and the mode leave of the band.
            let widest = modes.iter().map(|(_, label)| sc::ContentWidth::name(ui, label)).fold(0.0, f32::max);
            let button = t.control_height + t.space_10 + t.space_4;
            let beside = (widest + t.space_16) * 2.0 + 2.0 * button + 2.0 * t.space_8;
            let width = (ui.available_width() - beside).clamp(70.0, 240.0);
            let title = view
                .current
                .and_then(|c| view.chats.iter().find(|i| i.id == c))
                .map_or_else(|| t!("chat.new_title").into_owned(), |c| c.title.clone());
            settings_kit::dropdown(ui, "chats", &title, width, |ui| {
                for c in view.chats {
                    if settings_kit::choice_row(ui, view.current == Some(c.id), &short(&c.title, 60)) {
                        action = Some(ChatAction::Open(c.id));
                    }
                }
            });
            if sc::icon_action(ui, &t!("chat.new"), "plus", true, false).clicked() {
                action = Some(ChatAction::NewChat);
            }
            if let Some(c) = view.current
                && sc::icon_action(ui, &t!("chat.delete"), "delete", true, false).clicked()
            {
                action = Some(ChatAction::Delete(c));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut mode = view.mode;
                let picked = ui.scope(|ui| settings_kit::segmented(ui, &mut mode, &modes));
                let hints = format!(
                    "{}: {}\n\n{}: {}",
                    modes[0].1,
                    t!("chat.mode_review_hint"),
                    modes[1].1,
                    t!("chat.mode_auto_hint")
                );
                picked.response.on_hover_text(hints);
                if picked.inner {
                    action = Some(ChatAction::Mode(mode));
                }
            });
        });
    });
    action
}

/// The field with what stands under it: who answers, the model, and the round button that
/// sends (or stops the answer). One box, with the accent's border while it is typed into.
fn composer(
    ui: &mut Ui,
    view: &ChatView,
    input: &mut ChatInput,
    busy: bool,
    candidates: &mut dyn FnMut(&str) -> Vec<Candidate<Pill>>,
) -> Option<ChatAction> {
    let mut action = None;
    let t = Tokens::get();
    let field = egui::Id::unique("chat_field");
    let focused = ui.memory(|m| m.has_focus(field));
    // The kit's composer is made for a window of its own and rounds only its lower corners;
    // in a pane it is a box with a rule over it.
    let rule = ui.max_rect();
    t.secondary_hline(ui.painter(), rule.x_range(), rule.top(), t.dialog_rule);
    egui::Frame::NONE.inner_margin(t.space_8).show(ui, |ui| {
        let border =
            if focused { egui::Stroke::new(1.0, t.form_primary) } else { t.control_border_stroke(t.control_border) };
        egui::Frame::NONE
            .fill(t.control_fill)
            .stroke(border)
            .corner_radius(t.radius_lg)
            .inner_margin(egui::Margin::symmetric(t.space_10 as i8, t.space_8 as i8))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width().max(0.0));
                ui.spacing_mut().item_spacing.y = t.space_6;
                let out = chat_input::show(ui, input, field, &t!("chat.hint"), candidates);
                if out.send && !busy {
                    action = Some(ChatAction::Send);
                }
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Max), |ui| {
                        let label = if busy { t!("chat.stop") } else { t!("chat.send") };
                        if sc::assistant_send(ui, busy || !input.is_empty(), busy, &label).clicked() {
                            action = Some(if busy { ChatAction::Stop } else { ChatAction::Send });
                        }
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            if let Some(a) = assistant_row(ui, view) {
                                action = Some(a);
                            }
                        });
                    });
                });
            });
    });
    action
}

/// A quiet line of what is going on.
fn quiet(ui: &mut Ui, text: &str) {
    let t = Tokens::get();
    ui.label(RichText::new(text).size(t.font_small).color(t.form_dim));
}

/// Under the field: who answers (a choice when there are several), the model picker, and what
/// an agent is up to.
fn assistant_row(ui: &mut Ui, view: &ChatView) -> Option<ChatAction> {
    let mut action = None;
    let t = Tokens::get();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = t.space_6;
        let picked = view.assistants.iter().find(|(id, _)| id == view.assistant);
        let pick = t!("chat.pick_assistant");
        let name = picked.map_or(pick.as_ref(), |(_, name)| name.as_str());
        if view.assistants.len() > 1 || picked.is_none() {
            settings_kit::dropdown(ui, "chat_assistant", name, 150.0, |ui| {
                for (id, name) in view.assistants {
                    if settings_kit::choice_row(ui, id == view.assistant, name) {
                        action = Some(ChatAction::Assistant(id.clone()));
                    }
                }
            });
        } else {
            quiet(ui, name);
        }
        if view.looking {
            crate::ui::loading::mark(ui);
            quiet(ui, &t!("assistant.looking"));
        }
        let Some((_, picked_name)) = picked else { return };
        if !view.connected {
            // Nothing runs until the user says so; the models can be picked once it does.
            if settings_kit::bar_button(ui, &t!("chat.connect"))
                .on_hover_text(t!("chat.connect_hint", name = picked_name.clone()))
                .clicked()
            {
                action = Some(ChatAction::Connect);
            }
            if let Some(status) = view.status {
                quiet(ui, status);
            }
            return;
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
            let shown = picker::compact(selected, 24);
            settings_kit::fixed_width(ui, 120.0, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                sc::dropdown_menu(ui, ("chat_option", &o.id), &shown, |ui| {
                    quiet(ui, &o.name);
                    for (value, name, description) in &o.choices {
                        let r = sc::list_picker(ui, value, name, *value == o.current);
                        let r = if description.is_empty() { r } else { r.on_hover_text(description) };
                        if r.clicked() && *value != o.current {
                            action = Some(ChatAction::Option { id: o.id.clone(), value: value.clone() });
                        }
                    }
                })
                .on_hover_text(tip);
            });
        }
        if let Some(status) = view.status {
            quiet(ui, status);
        }
    });
    action
}

/// An answer's text: unboxed, in the kit's label font and ink, as its `assistant_prose`. What
/// the answer stresses (a file's name) stands in the form's primary colour: the kit's forms give
/// every text one ink, and egui's "strong" is only another ink.
fn prose<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = Tokens::get();
    ui.scope(|ui| {
        ui.style_mut().text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(t.font_label));
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        let visuals = ui.visuals_mut();
        visuals.widgets.noninteractive.fg_stroke.color = t.form_ink;
        visuals.widgets.active.fg_stroke.color = t.form_primary;
        add(ui)
    })
    .inner
}

/// A card in the conversation: the theme's fine border around what belongs together.
fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = Tokens::get();
    egui::Frame::NONE
        .fill(t.dialog_body)
        .stroke(t.tertiary_border(t.dialog_rule))
        .corner_radius(t.radius_lg)
        .inner_margin(t.space_10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = t.space_6;
            add(ui)
        })
        .inner
}

/// A card's title: heavier, and whole.
fn card_title(ui: &mut Ui, text: &str) {
    let t = Tokens::get();
    let text = RichText::new(text).font(sc::type_face::semibold(t.font_label)).color(t.form_ink);
    ui.add(egui::Label::new(text).wrap());
}

/// The assistant's questions: round marks where one answer is wanted, checkboxes where
/// several may be picked, a field for an answer in the user's own words where allowed.
pub fn ask_card(ui: &mut Ui, card: &mut AskCard) -> Option<ChatAction> {
    let mut action = None;
    let t = Tokens::get();
    self::card(ui, |ui| {
        card_title(ui, &t!("chat.question_title"));
        for i in 0..card.questions.len() {
            let q = card.questions[i].clone();
            ui.add_space(t.space_2);
            ui.add(egui::Label::new(RichText::new(&q.question).color(t.form_ink)).wrap());
            for (j, option) in q.options.iter().enumerate() {
                if q.multi {
                    settings_kit::checkbox(ui, option, &mut card.chosen[i][j]);
                } else if settings_kit::radio(ui, option, card.chosen[i][j]).clicked() {
                    card.chosen[i].fill(false);
                    card.chosen[i][j] = true;
                }
            }
            if q.allow_other {
                if !q.options.is_empty() {
                    quiet(ui, &t!("chat.question_other"));
                }
                let changed = ui
                    .push_id(("ask_other", &card.uid, i), |ui| {
                        sc::text_field_reserving(ui, &mut card.other[i], "", 0.0).1.changed()
                    })
                    .inner;
                // Typing an own answer to a one-answer question drops the option picked.
                if changed && !q.multi && !card.other[i].trim().is_empty() {
                    card.chosen[i].fill(false);
                }
            }
        }
        ui.add_space(t.space_2);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = t.space_6;
            if settings_kit::primary(ui, &t!("chat.answer"), card.complete()).clicked() {
                action = Some(ChatAction::Answer { uid: card.uid.clone(), cancel: false });
            }
            if settings_kit::action(ui, &t!("chat.answer_cancel"), true).clicked() {
                action = Some(ChatAction::Answer { uid: card.uid.clone(), cancel: true });
            }
        });
    });
    action
}

/// An agent's permission question: what it wants to run, and the answers it offers.
fn question(ui: &mut Ui, q: &Question) -> Option<ChatAction> {
    let mut action = None;
    let t = Tokens::get();
    sc::assistant_approval(ui, &t!("assistant.asks", tool = q.title.clone()), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(t.space_6, t.space_6);
            for (id, name) in &q.options {
                if settings_kit::action(ui, name, true).clicked() {
                    action = Some(ChatAction::Permission { key: q.key, option: Some(id.clone()) });
                }
            }
            if settings_kit::action(ui, &t!("trash.cancel"), true).clicked() {
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
    let t = Tokens::get();
    card(ui, |ui| {
        card_title(ui, &t!("assistant.sign_in_title"));
        for m in methods {
            if m.by_agent {
                if settings_kit::action(ui, &m.name, true).on_hover_text(&m.description).clicked() {
                    action = Some(ChatAction::Authenticate(m.id.clone()));
                }
            } else {
                ui.add(egui::Label::new(RichText::new(&m.name).color(t.form_ink)).wrap());
            }
            if let Some(command) = &m.command {
                // The button first, from the right: the command takes what is left of the line.
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if settings_kit::bar_button(ui, &t!("settings.mcp_copy")).clicked() {
                            ui.ctx().copy_text(command.clone());
                        }
                        let mut shown = command.as_str();
                        let field = egui::TextEdit::singleline(&mut shown)
                            .desired_width(f32::INFINITY)
                            .background_color(t.input_fill())
                            .code_editor();
                        ui.add(field);
                    });
                });
            }
        }
        quiet(ui, &t!("assistant.sign_in_hint"));
    });
    action
}

/// The tools an answer used: a line that folds, and under it the kit's list of them.
fn tools(ui: &mut Ui, m: &ChatMsg) {
    let t = Tokens::get();
    let id = egui::Id::unique(("chat-tools", m.id, m.tools.len()));
    let mut open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let text = t!("chat.tools", count = m.tools.len());
    let galley = ui.painter().layout_no_wrap(text.into_owned(), egui::FontId::proportional(t.font_small), t.form_dim);
    let size = egui::vec2(t.space_16 + galley.size().x + t.space_6, t.control_height - t.space_6);
    let (rect, line) = ui.allocate_exact_size(size, egui::Sense::click());
    if line.hovered() {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
    }
    sc::disclosure(ui.painter(), egui::pos2(rect.left() + t.space_8, rect.center().y), open, t.form_dim);
    ui.painter().galley(
        egui::pos2(rect.left() + t.space_16, rect.center().y - galley.size().y / 2.0),
        galley,
        t.form_dim,
    );
    if line.clicked() {
        open = !open;
        ui.data_mut(|d| d.insert_temp(id, open));
    }
    if !open {
        return;
    }
    sc::assistant_tool_frame(ui, false, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for (name, summary) in &m.tools {
            sc::assistant_tool_row(ui, false, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = t.space_6;
                    ui.add_space(t.space_4);
                    sc::assistant_tool_status(ui, sc::AssistantToolState::Done);
                    ui.label(RichText::new(name).monospace().size(t.font_small).color(t.form_ink));
                    let summary = RichText::new(summary).size(t.font_small).color(t.form_dim);
                    ui.add(egui::Label::new(summary).truncate());
                });
            });
        }
    });
}

/// What became of a card's suggestions, as the kit's small coloured label.
fn card_state(ui: &mut Ui, state: &str, items: usize) {
    use sc::AssistantTone as Tone;
    let (text, tone) = match state {
        "pending" if items == 1 => (t!("chat.card_proposed_one"), Tone::Working),
        "pending" => (t!("chat.card_proposed", count = items), Tone::Working),
        "partly" => (t!("chat.card_partly"), Tone::Warning),
        "accepted" => (t!("chat.card_accepted"), Tone::Success),
        "rejected" => (t!("chat.card_rejected"), Tone::Neutral),
        "undone" => (t!("chat.card_undone"), Tone::Neutral),
        _ => (t!("chat.card_expired"), Tone::Neutral),
    };
    sc::assistant_badge(ui, &text, tone);
}

fn message(ui: &mut Ui, m: &ChatMsg, view: &ChatView, md: &mut CommonMarkCache) -> Option<ChatAction> {
    let mut action = None;
    let t = Tokens::get();
    if m.user {
        // The kit's bubble at the right; what the message changed can be taken back under it.
        action = sc::assistant_bubble(ui, |ui| user_text(ui, m)).inner;
        if let Some(&(active, undone)) = view.changes.get(&m.id) {
            let row = egui::vec2(ui.available_width(), t.control_height - t.space_4);
            ui.allocate_ui_with_layout(row, egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if active > 0 {
                    if sc::icon_action(ui, &t!("chat.undo", count = active), "undo", true, false).clicked() {
                        action = Some(ChatAction::Undo(m.id));
                    }
                } else if undone > 0
                    && sc::icon_action(ui, &t!("chat.redo", count = undone), "redo", true, false).clicked()
                {
                    action = Some(ChatAction::Redo(m.id));
                }
            });
        }
        return action;
    }
    let mut mentioned = Vec::new();
    let text = with_references(&m.text, view.names, &mut mentioned);
    if !text.trim().is_empty() {
        prose(ui, |ui| CommonMarkViewer::new().show(ui, md, &text));
    }
    if !mentioned.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(t.space_4, t.space_4);
            for id in mentioned {
                if sc::button(ui, (view.names)(&id), sc::Button::Small).on_hover_text(&id).clicked() {
                    action = Some(ChatAction::Reveal(id));
                }
            }
        });
    }
    if !m.tools.is_empty() {
        tools(ui, m);
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
    for shown in &m.cards {
        card(ui, |ui| match shown {
            Card::Proposed { batch, title, items } => {
                // The title on a line of its own, cut short in a narrow pane; the state and
                // the button wrap below it, so the button never gets cut off.
                let face = sc::type_face::semibold(t.font_label);
                ui.add(egui::Label::new(RichText::new(title).font(face).color(t.form_ink)).truncate());
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = t.space_8;
                    let state = view.card_states.get(batch).map_or("pending", String::as_str);
                    card_state(ui, state, *items);
                    if matches!(state, "pending" | "partly")
                        && settings_kit::primary(ui, &t!("chat.card_review"), true).clicked()
                    {
                        action = Some(ChatAction::Review);
                    }
                });
            }
            Card::Applied { title, applied, .. } => {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = t.space_6;
                    sc::assistant_tool_status(ui, sc::AssistantToolState::Done);
                    let text = t!("chat.card_applied", title = title, count = applied);
                    ui.add(egui::Label::new(RichText::new(text).color(t.form_ink)).wrap());
                });
            }
            Card::Answered { question, answer } => {
                let text = t!("chat.card_answered", question = question, answer = answer);
                ui.add(egui::Label::new(RichText::new(text).color(t.form_dim)).wrap());
            }
        });
    }
    for notice in &m.notices {
        sc::status_line(ui, notice, sc::StatusTone::Warning);
    }
    if m.failed {
        sc::status_line(ui, &t!("chat.failed"), sc::StatusTone::Error);
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

/// The user's words with their references as small chips; a click on one goes to what it
/// stands for.
fn user_text(ui: &mut Ui, m: &ChatMsg) -> Option<ChatAction> {
    let t = Tokens::get();
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = t.space_3;
        let mut rest = m.text.as_str();
        while let Some(start) = rest.find("[[ref:") {
            let Some(end) = rest[start..].find("]]") else { break };
            if start > 0 {
                ui.label(&rest[..start]);
            }
            let n: usize = rest[start + 6..start + end].parse().unwrap_or(usize::MAX);
            if let Some(p) = m.pills.get(n) {
                // On the bubble's soft accent: the fill of a field, so the chip stands out.
                let text = RichText::new(format!("@{}", p.label)).size(t.font_small).color(t.form_ink);
                let chip =
                    egui::Button::new(text).fill(t.control_fill).stroke(egui::Stroke::NONE).corner_radius(t.radius_lg);
                let chip = ui.add(chip).on_hover_cursor(egui::CursorIcon::PointingHand);
                if chip.on_hover_text(t!("chat.follow_pill")).clicked() {
                    action = Some(ChatAction::Follow(p.reference.clone()));
                }
            }
            rest = &rest[start + end + 2..];
        }
        if !rest.is_empty() {
            ui.label(rest);
        }
    });
    action
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

    /// The texts a frame drew, each with its place.
    fn texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Rect)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(t) => out.push((t.galley.text().to_owned(), t.visual_bounding_rect())),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        shapes.iter().for_each(|s| walk(&s.shape, &mut out));
        out
    }

    /// A conversation with every part the pane can show, in a headless egui 420 points wide
    /// and high enough for all of it. Two frames to settle, then a click on the text `on` (when
    /// given, the lowest of them). The action of the last frame, the texts it drew, and the
    /// question card.
    fn drive(on: Option<&str>) -> (Option<ChatAction>, Vec<String>, AskCard) {
        let chats = [ChatInfo { id: 1, title: "Garden".into(), updated_ns: 0 }];
        let pill = Pill { label: "Garden".into(), reference: chat_input::Reference::Tag { name: "garden".into() } };
        let messages = [
            ChatMsg {
                id: 1,
                user: true,
                text: "Which of [[ref:0]] fit?".into(),
                pills: vec![pill],
                ..Default::default()
            },
            ChatMsg {
                id: 2,
                text: "Take [[Garden/rose.stl]].".into(),
                tools: vec![("search_models".into(), "12 files".into())],
                cards: vec![
                    Card::Proposed { batch: "b".into(), title: "Tag the garden parts".into(), items: 3 },
                    Card::Applied { change: 1, title: "Notes".into(), applied: 2 },
                    Card::Answered { question: "Which printer?".into(), answer: "A1 mini".into() },
                ],
                notices: vec!["One file could not be read.".into()],
                failed: true,
                ..Default::default()
            },
        ];
        let changes = HashMap::from([(1, (2, 0))]);
        let states = HashMap::new();
        let names = |id: &str| id.rsplit('/').next().unwrap_or(id).to_owned();
        let assistants = [("claude".to_owned(), "Claude Code".to_owned()), ("endpoint".to_owned(), "Own".to_owned())];
        let models = ModelList { current: "large".into(), ..Default::default() };
        let options = [OptionRow {
            id: "effort".into(),
            name: "Reasoning".into(),
            description: String::new(),
            current: "high".into(),
            choices: vec![("high".into(), "high".into(), String::new())],
        }];
        let questions =
            [Question { key: 7, title: "rm -rf".into(), options: vec![("allow".into(), "Allow once".into())] }];
        let auth = [AuthRow {
            id: "login".into(),
            name: "Sign in with the browser".into(),
            description: String::new(),
            command: Some("claude /login".into()),
            by_agent: false,
        }];
        let ask = AskQuestion {
            question: "Which filament?".into(),
            options: vec!["PLA".into(), "PETG".into()],
            multi: false,
            allow_other: true,
        };
        let pick =
            AskQuestion { question: "Which extras?".into(), options: vec!["Brim".into()], multi: true, ..ask.clone() };
        let mut asks = [AskCard::new("q1".into(), vec![ask, pick])];
        let mut input = ChatInput::default();
        let mut md = CommonMarkCache::default();
        let ctx = egui::Context::default();
        let (mut action, mut time) = (None, 0.0);
        let mut frame = |events: Vec<egui::Event>, action: &mut Option<ChatAction>| {
            time += 0.05;
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 2400.0))),
                time: Some(time),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(raw, |ui| {
                let view = ChatView {
                    chats: &chats,
                    current: Some(1),
                    messages: &messages,
                    streaming: None,
                    mode: ChatMode::Review,
                    changes: &changes,
                    card_states: &states,
                    names: &names,
                    assistants: &assistants,
                    assistant: "claude",
                    connected: true,
                    looking: false,
                    models: &models,
                    options: &options,
                    status: Some("ready"),
                    questions: &questions,
                    auth: &auth,
                };
                *action = show(ui, view, &mut input, &mut asks, &mut md, &mut |_| Vec::new());
            });
            output.textures_delta.clear();
            output.shapes
        };
        frame(Vec::new(), &mut action);
        let mut shapes = frame(Vec::new(), &mut action);
        if let Some(on) = on {
            let drawn = texts(&shapes);
            // The lowest one: the header's mode switch says "Review" too.
            let found = drawn.iter().filter(|(t, _)| t == on).max_by(|a, b| a.1.top().total_cmp(&b.1.top()));
            let at = found.unwrap_or_else(|| panic!("{on}: {drawn:?}")).1.center();
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            frame(vec![egui::Event::PointerMoved(at), button(true)], &mut action);
            shapes = frame(vec![button(false)], &mut action);
        }
        let drawn = texts(&shapes).into_iter().map(|(t, _)| t).collect();
        (action, drawn, asks[0].clone())
    }

    /// Every part of the pane draws, and nothing happens by itself.
    #[test]
    fn every_part_of_a_conversation_draws() {
        let (action, drawn, _) = drive(None);
        assert!(action.is_none());
        for part in [
            "Tag the garden parts",
            "Which filament?",
            "PLA",
            "Brim",
            "Sign in with the browser",
            "Allow once",
            "One file could not be read.",
        ] {
            assert!(drawn.iter().any(|t| t.contains(part)), "{part} is drawn: {drawn:?}");
        }
    }

    /// The buttons of the cards do what they say: the review, an agent's permission, an answer.
    #[test]
    fn the_cards_buttons_act() {
        assert!(matches!(drive(Some(&t!("chat.card_review"))).0, Some(ChatAction::Review)));
        let allowed = drive(Some("Allow once")).0;
        assert!(matches!(&allowed, Some(ChatAction::Permission { key: 7, option: Some(o) }) if o == "allow"));
        let (_, _, card) = drive(Some("PETG"));
        assert_eq!(card.chosen[0], [false, true], "the round mark picks one");
        let (_, _, card) = drive(Some("Brim"));
        assert_eq!(card.chosen[1], [true], "the checkbox is ticked");
        let followed = drive(Some("@Garden")).0;
        let tag = chat_input::Reference::Tag { name: "garden".into() };
        assert!(matches!(&followed, Some(ChatAction::Follow(r)) if *r == tag), "a chip leads to what it names");
    }

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
