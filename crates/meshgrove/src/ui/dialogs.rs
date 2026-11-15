//! Dialogs on SolidCraft's UI kit: a question over the window that waits for its answer. They
//! are the kit's confirmation card (an icon in a disc, the title, what is asked, and the kit's
//! footer with its two buttons), also where the card holds more than a sentence (a field, a
//! picture).

use egui::{RichText, Ui};
use solidcraft_theme::Tokens;
use solidcraft_ui_kit as sc;

pub use sc::Tone;

/// A question to answer with one of two buttons.
pub struct Dialog<'a> {
    /// Tells the dialogs apart.
    pub id: &'a str,
    /// A name from the kit's icon set, drawn in the disc beside the title.
    pub icon: &'a str,
    pub title: &'a str,
    /// What kind of question it is: the disc's colour, and a red primary button for
    /// [`Tone::Danger`].
    pub tone: Tone,
    /// The button that goes ahead; empty for a card that can only be closed.
    pub yes: &'a str,
    /// The button that does not.
    pub no: &'a str,
    /// Enter answers with `no`: for questions whose safe answer it is.
    pub enter_declines: bool,
    /// The card's width in the kit's card widths (1.5 is a confirmation's).
    pub cards: f32,
}

impl Dialog<'_> {
    /// Shows the dialog over the window. `Some(true)`: `yes` was chosen (never while `valid` is
    /// false); `Some(false)`: `no`, the ✕ or Esc; `None`: still open.
    pub fn show(self, ctx: &egui::Context, valid: bool, message: impl FnOnce(&mut Ui)) -> Option<bool> {
        let id = egui::Id::unique(("dialog", self.id));
        // The key press that led here (Enter in a field, in a list) must not answer the dialog
        // in the pass it first shows.
        let shown = id.with("shown");
        let pass = ctx.cumulative_pass_nr();
        let before: Option<u64> = ctx.data(|d| d.get_temp(shown));
        ctx.data_mut(|d| d.insert_temp(shown, pass));
        if before.is_none_or(|last| last + 1 < pass) {
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        }
        let mut card = sc::Confirm::new(id, self.icon, self.title).tone(self.tone).actions(self.yes, self.no, None);
        card.window = card.window.cards(self.cards);
        if self.enter_declines {
            card = card.selected(sc::Action::Cancel);
        }
        let (closed, action) = card.show(ctx, valid, |ui| {
            // The kit cuts texts to one line; what a dialog says wraps.
            ui.style_mut().wrap_mode = None;
            message(ui);
        });
        match action {
            sc::Action::Ok if !self.yes.is_empty() && valid => Some(true),
            sc::Action::Cancel => Some(false),
            _ if closed => Some(false),
            _ => None,
        }
    }
}

/// A sentence of the dialog.
pub fn say(ui: &mut Ui, text: &str) {
    let t = Tokens::get();
    ui.add(egui::Label::new(RichText::new(text).size(t.font_label).color(t.form_ink)).wrap());
}

/// A sentence that carries the weight: the question itself.
pub fn stress(ui: &mut Ui, text: &str) {
    let t = Tokens::get();
    let text = RichText::new(text).font(sc::type_face::semibold(t.font_label)).color(t.form_ink);
    ui.add(egui::Label::new(text).wrap());
}

/// A quieter sentence: what the answer leads to.
pub fn quiet(ui: &mut Ui, text: &str) {
    let t = Tokens::get();
    ui.add(egui::Label::new(RichText::new(text).size(t.font_small).color(t.form_dim)).wrap());
}

/// A name in a list of names, on one line, cut short where it is too long.
pub fn entry(ui: &mut Ui, text: &str) {
    let t = Tokens::get();
    ui.add(egui::Label::new(RichText::new(format!("•  {text}")).size(t.font_label).color(t.form_ink)).truncate());
}
