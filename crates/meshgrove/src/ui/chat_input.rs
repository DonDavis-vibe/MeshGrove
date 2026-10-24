//! The chat's message field: text with references ("pills") in between, started with `@`. A
//! pill is a piece of the text the field keeps whole: editing into it, or Backspace at its end,
//! removes all of it. The field grows upwards with its text, up to a limit, then scrolls.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{Color32, FontId, Key, RichText, Ui};
use serde::{Deserialize, Serialize};

use super::suggest::{self, Candidate};

/// What a pill stands for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reference {
    /// One file, the selection, a drop in the inbox …
    Files { paths: Vec<PathBuf> },
    Folder { path: PathBuf },
    List { name: String },
    Tag { name: String },
    Printer { name: String },
    /// A search in Meshgrove's syntax.
    Search { query: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pill {
    /// "Deckel.stl · 2 Teile"
    pub label: String,
    pub reference: Reference,
}

/// The text in the field stands for this pill: "@label ×".
fn pill_text(pill: &Pill) -> String {
    format!("@{} ×", pill.label)
}

#[derive(Debug, Clone, Default)]
pub struct ChatInput {
    pub text: String,
    /// Byte ranges of the pills in `text`, in order, not overlapping.
    pills: Vec<(Range<usize>, Pill)>,
    /// `text` as last synced, to see what an edit changed.
    seen: String,
}

impl ChatInput {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    pub fn pills(&self) -> impl Iterator<Item = (&Range<usize>, &Pill)> {
        self.pills.iter().map(|(r, p)| (r, p))
    }

    pub fn clear(&mut self) {
        *self = ChatInput::default();
    }

    /// Puts `pill` in place of `replace` (the typed "@query"), followed by a space; returns the
    /// byte offset after it.
    pub fn insert_pill(&mut self, replace: Range<usize>, pill: Pill) -> usize {
        self.sync();
        let insert = pill_text(&pill);
        let range = replace.start..replace.start + insert.len();
        self.text.replace_range(replace.clone(), &format!("{insert} "));
        let delta = insert.len() as isize + 1 - replace.len() as isize;
        for (r, _) in &mut self.pills {
            if r.start >= replace.end {
                *r = shift(r, delta);
            }
        }
        let at = self.pills.iter().position(|(r, _)| r.start > range.start).unwrap_or(self.pills.len());
        self.pills.insert(at, (range.clone(), pill));
        self.seen = self.text.clone();
        range.end + 1
    }

    /// Takes a pill out (its ✕); returns where its text was.
    pub fn remove_pill(&mut self, index: usize) -> usize {
        self.sync();
        let (range, _) = self.pills.remove(index);
        self.text.replace_range(range.clone(), "");
        for (r, _) in &mut self.pills {
            if r.start >= range.end {
                *r = shift(r, -(range.len() as isize));
            }
        }
        self.seen = self.text.clone();
        range.start
    }

    /// After `text` was edited: moves the pills with it, and removes every pill the edit touched
    /// as a whole. Returns where the cursor belongs when a pill was removed.
    pub fn sync(&mut self) -> Option<usize> {
        if self.text == self.seen {
            return None;
        }
        let (old, new) = (std::mem::take(&mut self.seen), self.text.clone());
        let p = common_prefix(&old, &new);
        let q = common_suffix(&old[p..], &new[p..]);
        let (old_end, delta) = (old.len() - q, new.len() as isize - old.len() as isize);
        let mut kept = Vec::new();
        // Pieces of broken pills still in the new text, in new coordinates.
        let mut leftovers: Vec<Range<usize>> = Vec::new();
        for (r, pill) in std::mem::take(&mut self.pills) {
            if r.end <= p {
                kept.push((r, pill));
            } else if r.start >= old_end {
                kept.push((shift(&r, delta), pill));
            } else {
                if r.start < p {
                    leftovers.push(r.start..p);
                }
                if r.end > old_end {
                    leftovers.push(shift(&(old_end..r.end), delta));
                }
            }
        }
        let mut cursor = None;
        let mut text = new;
        for piece in leftovers.iter().rev() {
            text.replace_range(piece.clone(), "");
            for (r, _) in &mut kept {
                if r.start >= piece.end {
                    *r = shift(r, -(piece.len() as isize));
                }
            }
            cursor = Some(piece.start);
        }
        self.text = text;
        self.pills = kept;
        self.seen = self.text.clone();
        cursor
    }

    /// The "@query" being typed just before byte `cursor`: after a space or at the start, not in
    /// a pill, at most 80 characters, no line break.
    pub fn mention(&self, cursor: usize) -> Option<(Range<usize>, String)> {
        let before = self.text.get(..cursor)?;
        let at = before.rfind('@')?;
        if self.pills.iter().any(|(r, _)| r.contains(&at)) {
            return None;
        }
        let starts_word = before[..at].chars().next_back().is_none_or(char::is_whitespace);
        let query = &before[at + 1..];
        (starts_word && !query.contains('\n') && query.chars().count() <= 80).then(|| (at..cursor, query.to_owned()))
    }

    /// The message: the text with each pill as `[[ref:n]]`, and the pills in that order.
    pub fn take_message(&mut self) -> (String, Vec<Pill>) {
        self.sync();
        let mut out = String::new();
        let mut last = 0;
        let mut pills = Vec::new();
        for (n, (r, pill)) in self.pills.iter().enumerate() {
            out.push_str(&self.text[last..r.start]);
            out.push_str(&format!("[[ref:{n}]]"));
            pills.push(pill.clone());
            last = r.end;
        }
        out.push_str(&self.text[last..]);
        (out.trim().to_owned(), pills)
    }
}

fn shift(r: &Range<usize>, delta: isize) -> Range<usize> {
    (r.start as isize + delta) as usize..(r.end as isize + delta) as usize
}

fn common_prefix(a: &str, b: &str) -> usize {
    a.char_indices().zip(b.chars()).find(|((_, x), y)| x != y).map_or(a.len().min(b.len()), |((i, _), _)| i)
}

/// Bytes at the end both share, on a character boundary.
fn common_suffix(a: &str, b: &str) -> usize {
    let mut n = 0;
    for (x, y) in a.chars().rev().zip(b.chars().rev()) {
        if x != y {
            break;
        }
        n += x.len_utf8();
    }
    n
}

fn char_index(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())].chars().count()
}

fn byte_index(text: &str, chars: usize) -> usize {
    text.char_indices().nth(chars).map_or(text.len(), |(i, _)| i)
}

/// What happened in the field this frame.
#[derive(Default)]
pub struct InputOutput {
    /// Enter was pressed with something to send.
    pub send: bool,
}

/// Most lines the field grows to before it scrolls.
const MAX_LINES: usize = 10;

/// The field, and above it the list of references matching what follows an `@`.
/// `candidates` gives them for the typed query.
pub fn show(
    ui: &mut Ui,
    input: &mut ChatInput,
    id: egui::Id,
    hint: &str,
    candidates: &mut dyn FnMut(&str) -> Vec<Candidate<Pill>>,
) -> InputOutput {
    let mut out = InputOutput::default();
    let focused = ui.memory(|m| m.has_focus(id));
    // Where the cursor was last frame decides whether the reference list is open now.
    let state = egui::text_edit::TextEditState::load(ui.ctx(), id);
    let cursor = state
        .as_ref()
        .and_then(|s| s.cursor.char_range())
        .map(|r| byte_index(&input.text, r.primary.index.0))
        .unwrap_or(input.text.len());
    let dismissed: Option<usize> = ui.data(|d| d.get_temp(id.with("dismissed")));
    let mention = input.mention(cursor).filter(|(r, _)| focused && dismissed != Some(r.start));
    let mut list = mention.as_ref().map(|(_, q)| suggest::rank(q, candidates(q), 8)).unwrap_or_default();
    let highlighted = suggest::highlight(ui, id.with("pick"), mention.as_ref().map_or("", |m| &m.1), list.len(), focused);
    let mut chosen: Option<usize> = None;
    if focused {
        if mention.is_some() && !list.is_empty() {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter) || i.consume_key(egui::Modifiers::NONE, Key::Tab)) {
                chosen = Some(highlighted.unwrap_or(0));
            }
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape))
                && let Some((r, _)) = &mention
            {
                ui.data_mut(|d| d.insert_temp(id.with("dismissed"), r.start));
            }
        } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) && !input.is_empty() {
            out.send = true;
        }
    }

    // The reference list, above the field.
    if let Some((range, _)) = &mention
        && !list.is_empty()
    {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (i, c) in list.iter().enumerate() {
                let text = format!("@{}", c.label);
                let row = ui.horizontal(|ui| {
                    let r = ui.selectable_label(highlighted == Some(i), RichText::new(text).strong());
                    ui.weak(&c.detail);
                    r
                });
                if row.inner.clicked() {
                    chosen = Some(i);
                }
            }
        });
        if let Some(i) = chosen {
            let pill = list.swap_remove(i).value;
            let after = input.insert_pill(range.clone(), pill);
            set_cursor(ui, id, char_index(&input.text, after));
        }
    }

    // The field.
    let pills: Vec<Range<usize>> = input.pills().map(|(r, _)| r.clone()).collect();
    let visuals = ui.visuals().clone();
    let mut layouter = |ui: &Ui, text: &dyn egui::TextBuffer, wrap_width: f32| -> Arc<egui::Galley> {
        let text = text.as_str();
        let font = FontId::proportional(14.0);
        let plain = TextFormat { font_id: font.clone(), color: visuals.text_color(), ..Default::default() };
        let pill = TextFormat {
            font_id: FontId::proportional(13.0),
            color: visuals.strong_text_color(),
            background: visuals.selection.bg_fill.linear_multiply(0.6),
            ..Default::default()
        };
        let mut job = LayoutJob { wrap: egui::text::TextWrapping { max_width: wrap_width, ..Default::default() }, ..Default::default() };
        let mut last = 0;
        for r in pills.iter().filter(|r| r.end <= text.len() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end)) {
            job.append(&text[last..r.start], 0.0, plain.clone());
            job.append(&text[r.clone()], 0.0, pill.clone());
            last = r.end;
        }
        job.append(&text[last.min(text.len())..], 0.0, plain);
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let lines = input.text.lines().count().max(1) + usize::from(input.text.ends_with('\n'));
    let output = egui::TextEdit::multiline(&mut input.text)
        .id(id)
        .hint_text(hint)
        .desired_rows(lines.min(MAX_LINES))
        .desired_width(f32::INFINITY)
        .layouter(&mut layouter)
        .show(ui);
    if output.response.changed()
        && let Some(at) = input.sync()
    {
        set_cursor(ui, id, char_index(&input.text, at));
    }

    // ✕ at the end of each pill removes it.
    let mut remove = None;
    for (i, (r, _)) in input.pills().enumerate() {
        let end = char_index(&input.text, r.end);
        let cross = output.galley.pos_from_cursor(CCursor::new(end.saturating_sub(1)));
        let rect = cross.translate(output.galley_pos.to_vec2()).expand2(egui::vec2(4.0, 1.0));
        let hit = ui.interact(rect, id.with(("pill", i)), egui::Sense::click()).on_hover_text(t!("chat.remove_pill"));
        if hit.hovered() {
            ui.painter().rect_filled(rect, 3.0, Color32::from_black_alpha(60));
        }
        if hit.clicked() {
            remove = Some(i);
        }
    }
    if let Some(i) = remove {
        let at = input.remove_pill(i);
        set_cursor(ui, id, char_index(&input.text, at));
    }
    out
}

fn set_cursor(ui: &Ui, id: egui::Id, chars: usize) {
    let mut state = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
    state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(chars))));
    state.store(ui.ctx(), id);
    ui.memory_mut(|m| m.request_focus(id));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pill(name: &str) -> Pill {
        Pill { label: name.into(), reference: Reference::Tag { name: name.into() } }
    }

    /// The field after typing `text` at the end.
    fn typed(input: &mut ChatInput, text: &str) {
        input.text.push_str(text);
        input.sync();
    }

    #[test]
    fn pills_stand_in_the_text_and_become_references() {
        let mut input = ChatInput::default();
        typed(&mut input, "Verbinde @dec");
        let (range, query) = input.mention(input.text.len()).unwrap();
        assert_eq!(query, "dec");
        let after = input.insert_pill(range, pill("Deckel.stl"));
        assert_eq!(&input.text[..after], "Verbinde @Deckel.stl × ");
        typed(&mut input, "mit @");
        let (range, _) = input.mention(input.text.len()).unwrap();
        input.insert_pill(range, pill("Boden.stl"));
        typed(&mut input, "bitte.");
        let (message, pills) = input.take_message();
        assert_eq!(message, "Verbinde [[ref:0]] mit [[ref:1]] bitte.");
        assert_eq!(pills, [pill("Deckel.stl"), pill("Boden.stl")]);
    }

    #[test]
    fn a_pill_goes_as_a_whole() {
        let mut input = ChatInput::default();
        typed(&mut input, "a @");
        let (range, _) = input.mention(input.text.len()).unwrap();
        let after = input.insert_pill(range, pill("Garten"));
        // Backspace twice: the space, then the pill's last character.
        input.text.remove(after - 1);
        input.sync();
        let end = input.text.len();
        input.text.remove(end - "×".len());
        let cursor = input.sync();
        assert_eq!(input.text, "a ");
        assert_eq!(cursor, Some(2));
        assert_eq!(input.pills().count(), 0);
    }

    #[test]
    fn typing_into_a_pill_keeps_the_typing_and_drops_the_pill() {
        let mut input = ChatInput::default();
        let after = input.insert_pill(0..0, pill("Werkstatt"));
        typed(&mut input, "ok");
        input.text.insert(3, 'X'); // inside "@Werkstatt ×"
        input.sync();
        assert_eq!(input.text, "X ok");
        assert_eq!(after, "@Werkstatt × ".len());
    }

    #[test]
    fn editing_around_pills_moves_them() {
        let mut input = ChatInput::default();
        typed(&mut input, "x ");
        let (r, _) = (2..2, ());
        input.insert_pill(r, pill("Rose"));
        input.text.insert_str(0, "Hallo ");
        input.sync();
        let ranges: Vec<Range<usize>> = input.pills().map(|(r, _)| r.clone()).collect();
        assert_eq!(&input.text[ranges[0].clone()], "@Rose ×");
        // Everything selected and deleted.
        input.text.clear();
        input.sync();
        assert_eq!(input.pills().count(), 0);
        let i = input.insert_pill(0..0, pill("Ü"));
        assert!(input.text.is_char_boundary(i));
        input.remove_pill(0);
        assert_eq!(input.text, " ");
    }

    #[test]
    fn mentions_start_after_a_space() {
        let mut input = ChatInput::default();
        typed(&mut input, "mail@host");
        assert_eq!(input.mention(input.text.len()), None, "not inside a word");
        typed(&mut input, " @tag:garten passt:mini");
        assert_eq!(input.mention(input.text.len()).unwrap().1, "tag:garten passt:mini", "searches keep their spaces");
        typed(&mut input, "\nweiter");
        assert_eq!(input.mention(input.text.len()), None, "a new line ends it");
    }
}
