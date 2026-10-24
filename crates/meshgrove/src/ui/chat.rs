//! The "KI-Chat" pane: the conversation with the user's own model, its answers with the files
//! they mention, what its tools did, and the message field with references.

use std::collections::HashMap;

use egui::{RichText, Ui};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use meshgrove_core::ai::proposals::ChatInfo;
use meshgrove_core::config::ChatMode;
use serde::{Deserialize, Serialize};

use super::chat_input::{self, ChatInput, Pill};
use super::suggest::Candidate;

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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "card", rename_all = "snake_case")]
pub enum Card {
    /// Suggestions waiting in the review.
    Proposed { batch: String, title: String, items: usize },
    /// Changes made at once (automatic mode).
    Applied { change: i64, title: String, applied: usize },
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
    /// The model, for the header.
    pub model: &'a str,
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
}

pub fn show(
    ui: &mut Ui,
    view: ChatView,
    input: &mut ChatInput,
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
        if ui.button("＋").on_hover_text(t!("chat.new")).clicked() {
            action = Some(ChatAction::NewChat);
        }
        if let Some(c) = view.current
            && ui.button("🗑").on_hover_text(t!("chat.delete")).clicked()
        {
            action = Some(ChatAction::Delete(c));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut mode = view.mode;
            ui.selectable_value(&mut mode, ChatMode::Auto, t!("chat.mode_auto")).on_hover_text(t!("chat.mode_auto_hint"));
            ui.selectable_value(&mut mode, ChatMode::Review, t!("chat.mode_review")).on_hover_text(t!("chat.mode_review_hint"));
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
                } else if ui.add_enabled(!input.is_empty(), egui::Button::new("➤")).on_hover_text(t!("chat.send")).clicked() {
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
        ui.weak(t!("chat.model", model = view.model));
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
            if busy {
                ui.spinner();
            }
        });
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
    for card in &m.cards {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            match card {
                Card::Proposed { batch, title, items } => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("✧ {title}")).strong());
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
                    ui.label(format!("✔ {}", t!("chat.card_applied", title = title, count = applied)));
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
                ui.add(egui::Button::new(text).fill(fill).stroke(egui::Stroke::NONE).corner_radius(8.0).sense(egui::Sense::hover()));
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
        let text = with_references("Nimm [[Garten/rose.stl]] und [[ Garten/rose.stl ]], nicht [[ref:0]].", &names, &mut ids);
        assert_eq!(text, "Nimm **rose.stl** und **rose.stl**, nicht [[ref:0]].");
        assert_eq!(ids, ["Garten/rose.stl"]);
    }
}
