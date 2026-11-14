//! The chat's message field: text with references ("pills") in between, started with `@`. A
//! pill is a piece of the text the field keeps whole: editing into it, or Backspace at its end,
//! removes all of it. The field grows upwards with its text, up to a limit, then scrolls.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use egui::text::{CCursor, CCursorRange, LayoutJob, TextFormat};
use egui::{Color32, FontId, Key, Ui};
use serde::{Deserialize, Serialize};

use super::suggest::{self, Candidate};

/// What a pill stands for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reference {
    /// One file, the selection, a drop in the inbox …
    Files {
        paths: Vec<PathBuf>,
    },
    Folder {
        path: PathBuf,
    },
    List {
        name: String,
    },
    Tag {
        name: String,
    },
    Printer {
        name: String,
    },
    /// A search in Meshgrove's syntax.
    Search {
        query: String,
    },
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

    /// Puts `pill` at the end of the text (after a space), as "Mention in chat" does.
    pub fn append_pill(&mut self, pill: Pill) {
        self.sync();
        if !self.text.is_empty() && !self.text.ends_with([' ', '\n']) {
            self.text.push(' ');
            self.seen = self.text.clone();
        }
        let end = self.text.len();
        self.insert_pill(end..end, pill);
    }

    /// Takes a pill out (its ✖); returns where its text was.
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

    /// Puts a message taken with [`ChatInput::take_message`] back into the field.
    pub fn restore(&mut self, message: &str, pills: Vec<Pill>) {
        self.clear();
        let mut rest = message;
        while let Some(start) = rest.find("[[ref:") {
            let Some(end) = rest[start..].find("]]") else { break };
            self.text.push_str(&rest[..start]);
            self.seen = self.text.clone();
            let pill = rest[start + 6..start + end].parse::<usize>().ok().and_then(|n| pills.get(n).cloned());
            rest = &rest[start + end + 2..];
            if let Some(pill) = pill {
                let at = self.text.len();
                self.insert_pill(at..at, pill);
                // The pill brings its own space after it.
                rest = rest.strip_prefix(' ').unwrap_or(rest);
            }
        }
        self.text.push_str(rest);
        self.seen = self.text.clone();
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

/// The field, and over it the list of references matching what follows an `@`.
/// `candidates` gives them for the typed query.
///
/// The list floats over what is above the field (it would push the conversation up and down
/// with every letter otherwise) and is drawn after the field: a click on one of its rows is a
/// click outside the field to egui, which takes the field's focus then; the row gives it back.
pub fn show(
    ui: &mut Ui,
    input: &mut ChatInput,
    id: egui::Id,
    hint: &str,
    candidates: &mut dyn FnMut(&str) -> Vec<Candidate<Pill>>,
) -> InputOutput {
    let mut out = InputOutput::default();
    let focused = ui.memory(|m| m.has_focus(id));
    // The pointer went down on the list last frame: it stays, whatever the focus did meanwhile.
    let held_id = id.with("list-held");
    let held = ui.data(|d| d.get_temp::<bool>(held_id)).unwrap_or(false);
    // Where the cursor was last frame decides whether the reference list is open now.
    let state = egui::text_edit::TextEditState::load(ui.ctx(), id);
    let cursor = state
        .as_ref()
        .and_then(|s| s.cursor.char_range())
        .map(|r| byte_index(&input.text, r.primary.index.0))
        .unwrap_or(input.text.len());
    let dismissed: Option<usize> = ui.data(|d| d.get_temp(id.with("dismissed")));
    let mention = input.mention(cursor).filter(|(r, _)| (focused || held) && dismissed != Some(r.start));
    let mut list = mention.as_ref().map(|(_, q)| suggest::rank(q, candidates(q), 8)).unwrap_or_default();
    let highlighted =
        suggest::highlight(ui, id.with("pick"), mention.as_ref().map_or("", |m| &m.1), list.len(), focused);
    let mut open = mention.is_some() && !list.is_empty();
    if focused {
        if open {
            let taken = ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, Key::Enter) || i.consume_key(egui::Modifiers::NONE, Key::Tab)
            });
            if taken && let Some((range, _)) = &mention {
                let pill = list.swap_remove(highlighted.unwrap_or(0)).value;
                let after = input.insert_pill(range.clone(), pill);
                set_cursor(ui, id, char_index(&input.text, after));
                open = false;
            }
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape))
                && let Some((r, _)) = &mention
            {
                ui.data_mut(|d| d.insert_temp(id.with("dismissed"), r.start));
                open = false;
            }
        } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) && !input.is_empty() {
            out.send = true;
        }
    }

    // The field.
    let pills: Vec<Range<usize>> = input.pills().map(|(r, _)| r.clone()).collect();
    let t = solidcraft_theme::Tokens::get();
    let mut layouter = |ui: &Ui, text: &dyn egui::TextBuffer, wrap_width: f32| -> Arc<egui::Galley> {
        let text = text.as_str();
        // The kit's label font; a reference in the selection's colours.
        let plain = TextFormat { font_id: FontId::proportional(t.font_label), color: t.form_ink, ..Default::default() };
        let pill = TextFormat {
            font_id: FontId::proportional(t.font_label - 1.0),
            color: t.selected_ink,
            background: t.selected_bg,
            ..Default::default()
        };
        let mut job = LayoutJob {
            wrap: egui::text::TextWrapping { max_width: wrap_width, ..Default::default() },
            ..Default::default()
        };
        let mut last = 0;
        for r in pills
            .iter()
            .filter(|r| r.end <= text.len() && text.is_char_boundary(r.start) && text.is_char_boundary(r.end))
        {
            job.append(&text[last..r.start], 0.0, plain.clone());
            job.append(&text[r.clone()], 0.0, pill.clone());
            last = r.end;
        }
        job.append(&text[last.min(text.len())..], 0.0, plain);
        ui.fonts_mut(|f| f.layout_job(job))
    };
    // The kit's editor for a composer: no frame of its own (the box around it is the
    // composer's), two rows to start with, growing with what is typed.
    let output = solidcraft_ui_kit::assistant_input_laid_out(ui, id, &mut input.text, hint, &mut layouter);
    if output.response.changed()
        && let Some(at) = input.sync()
    {
        set_cursor(ui, id, char_index(&input.text, at));
    }
    // While the list is open, Tab and Escape are its keys: egui would walk the focus on to the
    // next widget with Tab otherwise (and a space typed after it would press that one). Set
    // after the field, which sets its own filter every frame.
    if open && focused {
        let keys = egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true };
        ui.memory_mut(|m| m.set_focus_lock_filter(id, keys));
    }

    // The reference list, floating with its lower edge on the composer's upper one.
    let mut down = false;
    if open && let Some((range, _)) = &mention {
        let field = output.response.response.rect;
        let at = egui::pos2(field.left() - t.space_10, field.top() - t.space_8 - t.space_4);
        let width = field.width() + 2.0 * t.space_10;
        let mut clicked = None;
        egui::Area::new(id.with("mentions"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(at)
            .show(ui.ctx(), |ui| {
                solidcraft_ui_kit::Menu::frame().show(ui, |ui| {
                    ui.set_width((width - t.space_6 * 2.0 - 2.0).max(0.0));
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (i, c) in list.iter().enumerate() {
                        // Lit is the one Enter takes: the first, until an arrow key picks.
                        let row = mention_row(ui, &c.label, &c.detail, highlighted.unwrap_or(0) == i);
                        down |= row.is_pointer_button_down_on();
                        if row.clicked() {
                            clicked = Some(i);
                        }
                    }
                });
            });
        if let Some(i) = clicked {
            let pill = list.swap_remove(i).value;
            let after = input.insert_pill(range.clone(), pill);
            // Also gives the field its focus back, which the click took.
            set_cursor(ui, id, char_index(&input.text, after));
        }
    }
    ui.data_mut(|d| d.insert_temp(held_id, down));

    // ✖ at the end of each pill removes it. Only for the pointer: a place the keyboard's focus
    // could rest on would be pressed by the next space.
    let mut remove = None;
    for (i, (r, _)) in input.pills().enumerate() {
        let end = char_index(&input.text, r.end);
        let cross = output.galley.pos_from_cursor(CCursor::new(end.saturating_sub(1)));
        let rect = cross.translate(output.galley_pos.to_vec2()).expand2(egui::vec2(4.0, 1.0));
        let hit = ui.interact(rect, id.with(("pill", i)), egui::Sense::CLICK).on_hover_text(t!("chat.remove_pill"));
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

/// One row of the reference list: "@name" and, quieter, what it is. Lit while it is the one
/// Enter takes, or under the pointer. It takes clicks but never the keyboard's focus.
fn mention_row(ui: &mut Ui, label: &str, detail: &str, lit: bool) -> egui::Response {
    let t = solidcraft_theme::Tokens::get();
    let size = egui::vec2(ui.available_width(), t.control_height);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::CLICK);
    if ui.is_rect_visible(rect) {
        if lit || response.hovered() {
            ui.painter().rect_filled(rect, t.radius_sm, if lit { t.selected_bg } else { t.hover });
        }
        let ink = if lit { t.selected_ink } else { t.form_ink };
        let face = solidcraft_ui_kit::type_face::semibold(t.font_label);
        let name = ui.painter().layout_no_wrap(format!("@{label}"), face, ink);
        let left = rect.left() + t.space_8;
        let after = left + name.size().x + t.space_8;
        ui.painter().galley(egui::pos2(left, rect.center().y - name.size().y / 2.0), name, ink);
        // What it is, cut short where the row ends.
        let mut job = LayoutJob::simple_singleline(detail.to_owned(), FontId::proportional(t.font_small), t.form_dim);
        job.wrap = egui::text::TextWrapping {
            max_width: (rect.right() - t.space_8 - after).max(0.0),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        let about = ui.fonts_mut(|f| f.layout_job(job));
        ui.painter().galley(egui::pos2(after, rect.center().y - about.size().y / 2.0), about, t.form_dim);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
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

    /// The field in a headless egui, with a button after it as the pane has (somewhere for the
    /// keyboard's focus to walk to). `run` draws a frame with its events; `find` gives where
    /// the last frame drew a text.
    struct Field {
        ctx: egui::Context,
        input: ChatInput,
        time: f64,
        shapes: Vec<egui::epaint::ClippedShape>,
    }

    impl Field {
        fn new() -> Field {
            let mut field =
                Field { ctx: egui::Context::default(), input: ChatInput::default(), time: 0.0, shapes: Vec::new() };
            field.run(Vec::new());
            field.ctx.memory_mut(|m| m.request_focus(egui::Id::unique("field")));
            field.run(Vec::new());
            field
        }

        fn run(&mut self, events: Vec<egui::Event>) {
            self.time += 0.05;
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let input = &mut self.input;
            let mut output = self.ctx.run_ui(raw, |ui| {
                // Room over the field for the list, as the conversation leaves it.
                ui.add_space(200.0);
                let mut candidates = |_: &str| {
                    vec![Candidate { label: "garden".into(), detail: "tag".into(), existing: true, value: pill("garden") }]
                };
                show(ui, input, egui::Id::unique("field"), "hint", &mut candidates);
                let _ = ui.button("send");
            });
            output.textures_delta.clear();
            self.shapes = output.shapes;
        }

        /// Types `text`, and lets the list settle: it opens a frame after the typing, and
        /// egui measures a new floating area for a frame before it draws it.
        fn text(&mut self, text: &str) {
            self.run(vec![egui::Event::Text(text.into())]);
            self.run(Vec::new());
            self.run(Vec::new());
        }

        fn key(&mut self, key: Key) {
            let event = |pressed| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            };
            self.run(vec![event(true)]);
            self.run(vec![event(false)]);
        }

        fn find(&self, text: &str) -> Option<egui::Rect> {
            fn walk(shape: &egui::Shape, text: &str, out: &mut Option<egui::Rect>) {
                match shape {
                    // The line's box, not the ink's: an "@" stands higher than an "x".
                    egui::Shape::Text(t) if t.galley.text() == text => {
                        *out = Some(egui::Rect::from_min_size(t.pos, t.galley.size()));
                    }
                    egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, text, out)),
                    _ => {}
                }
            }
            let mut out = None;
            self.shapes.iter().for_each(|s| walk(&s.shape, text, &mut out));
            out
        }

        fn click(&mut self, at: egui::Pos2) {
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            self.run(vec![egui::Event::PointerMoved(at), button(true)]);
            self.run(vec![button(false)]);
            self.run(Vec::new());
        }
    }

    /// Tab takes the reference from the list, and what is typed after it goes into the field:
    /// the focus stays there (it walked on to the pill's ✖ once, which the next space pressed).
    #[test]
    fn tab_takes_a_reference_and_typing_goes_on() {
        let mut field = Field::new();
        field.text("Check @ga");
        assert!(field.find("@garden").is_some(), "the list is open");
        field.key(Key::Tab);
        assert_eq!(field.input.pills().count(), 1, "{:?}", field.input.text);
        field.text(" now");
        assert_eq!(field.input.pills().count(), 1, "the pill stays: {:?}", field.input.text);
        assert_eq!(field.input.text, "Check @garden ×  now");
    }

    /// A click on a row of the list takes it, and the field keeps the focus.
    #[test]
    fn a_click_takes_a_reference_and_typing_goes_on() {
        let mut field = Field::new();
        field.text("@ga");
        let row = field.find("@garden").expect("the list is open");
        field.click(row.center());
        assert_eq!(field.input.pills().count(), 1, "{:?}", field.input.text);
        field.text("now");
        assert_eq!(field.input.text, "@garden × now");
    }

    /// The list floats: the field stands where it stood without it.
    #[test]
    fn the_list_does_not_move_the_field() {
        let mut field = Field::new();
        field.text("x");
        let before = field.find("x").expect("the typed text");
        field.text(" @ga");
        let row = field.find("@garden").expect("the list is open");
        let after = field.find("x @ga").expect("the typed text");
        assert_eq!(before.top(), after.top(), "the field stays");
        assert!(row.bottom() <= after.top(), "the list is over the field: {row:?} over {after:?}");
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
    fn a_taken_message_can_be_put_back() {
        let mut input = ChatInput::default();
        typed(&mut input, "Verbinde @");
        let (range, _) = input.mention(input.text.len()).unwrap();
        input.insert_pill(range, pill("Deckel.stl"));
        typed(&mut input, "bitte.");
        let (message, pills) = input.take_message();
        input.restore(&message, pills.clone());
        assert_eq!(input.take_message(), (message, pills));
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
