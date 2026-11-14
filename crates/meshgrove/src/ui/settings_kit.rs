//! The parts the settings pages are built from, on SolidCraft's UI kit: a row keeps its label in a
//! column of its own with the control (and its note) beside it, a choice among a few is a row of
//! joined buttons, a note is a quiet line under what it explains. Sections and page headings are
//! the kit's own (`settings_section`, `settings_heading`).

use egui::{Color32, Response, RichText, Ui};
use solidcraft_theme::Tokens;
use solidcraft_ui_kit as sc;

/// The width of the label column of a page's rows.
pub const LABELS: f32 = 190.0;

/// Green of a state that is fine ("connected"): the theme's.
pub fn good() -> Color32 {
    super::appearance::tokens().form_success
}

/// A labelled row: the label in the label column, the control (and anything under it, such as a
/// note) beside it.
pub fn row<R>(ui: &mut Ui, label: &str, control: impl FnOnce(&mut Ui) -> R) -> R {
    let t = Tokens::get();
    // Under its label's name: the kit's controls name themselves after the Ui they are in, and two
    // rows of a page are otherwise the same Ui to them (two rows of joined buttons clashed).
    let named = |ui: &mut Ui| ui.push_id(label, control).inner;
    let font = egui::FontId::proportional(t.font_label);
    let width = ui.painter().layout_no_wrap(label.to_owned(), font, t.form_ink).size().x;
    if width <= LABELS.min(ui.available_width() * 0.45) {
        return sc::settings_block(ui, label, LABELS, named);
    }
    // A label too long for one line of its column would be cut off by the kit's block: here it
    // wraps in the column, the control beside it as in every other row.
    let width = ui.available_width();
    let column = LABELS.min(width * 0.45);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = t.space_14;
        ui.allocate_ui_with_layout(
            egui::vec2(column, t.control_height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(column);
                ui.add_space(t.form_gap_y);
                let text = RichText::new(label).font(egui::FontId::proportional(t.font_label)).color(t.form_ink);
                ui.add(egui::Label::new(text).wrap());
            },
        );
        let control = (width - column - t.space_14).max(0.0);
        let place = egui::Rect::from_min_size(ui.next_widget_position(), egui::vec2(control, 0.0));
        let builder = egui::UiBuilder::new().max_rect(place).layout(egui::Layout::top_down(egui::Align::Min));
        ui.scope_builder(builder, |ui| {
            ui.set_width(control);
            named(ui)
        })
        .inner
    })
    .inner
}

/// A quiet explanation under the control it belongs to, closer to it than the page's rhythm puts
/// the next row.
pub fn hint(ui: &mut Ui, text: impl Into<String>) {
    let text = text.into();
    ui.add_space(-Tokens::get().space_6);
    sc::text(ui, &text, sc::Text::Note(&text));
}

/// A checkbox with its explanation under it. True when the user switched it.
pub fn check(ui: &mut Ui, value: &mut bool, label: impl Into<String>, hint_text: impl Into<String>) -> bool {
    let before = *value;
    sc::checkbox(ui, &label.into(), value);
    hint(ui, hint_text);
    *value != before
}

/// A choice among a few as joined buttons, the chosen one lit. True when the user picked another.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, String)]) -> bool {
    segmented_where(ui, value, options, |_| None)
}

/// [`segmented`] with options that cannot be picked now: `unavailable` gives the reason, shown when
/// the pointer rests on them.
pub fn segmented_where<T: PartialEq + Copy>(
    ui: &mut Ui,
    value: &mut T,
    options: &[(T, String)],
    unavailable: impl Fn(T) -> Option<String>,
) -> bool {
    let t = Tokens::get();
    let labels: Vec<&str> = options.iter().map(|(_, label)| label.as_str()).collect();
    let reasons: Vec<Option<String>> = options.iter().map(|(option, _)| unavailable(*option)).collect();
    let enabled: Vec<bool> = reasons.iter().map(Option::is_none).collect();
    let disabled: Vec<Option<&str>> = reasons.iter().map(Option::as_deref).collect();
    let mut index = options.iter().position(|(option, _)| option == value).unwrap_or(0);
    // As wide as its words need, as the kit's own settings segments are.
    let widest = labels.iter().map(|label| sc::ContentWidth::name(ui, label)).fold(0.0, f32::max);
    let width = ((widest + t.space_16) * labels.len() as f32).min(ui.available_width());
    let config = sc::Segments { enabled: &enabled, disabled: &disabled, compact: true, ..Default::default() };
    let layout = egui::Layout::left_to_right(egui::Align::Center);
    // Named after its first choice, so two of them in one row stay apart as well.
    let picked = ui
        .push_id(labels.first().copied().unwrap_or_default(), |ui| {
            ui.allocate_ui_with_layout(egui::vec2(width, t.control_height), layout, |ui| {
                sc::segmented(ui, &mut index, &labels, config)
            })
            .inner
        })
        .inner;
    match options.get(index) {
        Some((option, _)) if picked && option != value => {
            *value = *option;
            true
        }
        _ => false,
    }
}

/// A small coloured label for a state or a count.
pub fn pill(ui: &mut Ui, text: impl Into<String>, color: Color32) -> Response {
    // A widget of its own size: as a frame it took the whole height of the row it stood in,
    // and stood as a block beside what it belongs to instead of level with it.
    let text = egui::WidgetText::from(RichText::new(text.into()).color(color)).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Body,
    );
    let size = egui::vec2(text.size().x + 16.0, text.size().y + 6.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, rect.height() / 2.0, color.gamma_multiply(0.16));
        ui.painter().galley(rect.center() - text.size() / 2.0, text, color);
    }
    response
}

/// A control exactly `width` wide (less only where the page itself is narrower). The kit's
/// controls take the room they are given, and a cell of a table gives them almost none.
pub fn fixed_width<R>(ui: &mut Ui, width: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let size = egui::vec2(width, Tokens::get().control_height);
    ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_width(width);
        add(ui)
    })
    .inner
}

/// A one-line text field `width` wide, with a quiet hint while it is empty.
pub fn text_field(ui: &mut Ui, value: &mut String, hint: &str, width: f32) -> Response {
    // The kit's plain text field: no completion of expressions, as its value fields have.
    fixed_width(ui, width, |ui| sc::text_field_reserving(ui, value, hint, 0.0).1)
}

/// A list that opens, `width` wide, showing `current`; `choices` draws its rows ([`choice`],
/// [`choice_row`]).
pub fn dropdown(ui: &mut Ui, id: impl egui::AsIdSalt, current: &str, width: f32, choices: impl FnOnce(&mut Ui)) {
    fixed_width(ui, width, |ui| {
        // One line, also where the texts around it wrap: a long choice is cut short.
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        sc::dropdown_menu(ui, id, current, choices)
    });
}

/// A pane's content as one of the kit's forms: the height of its controls, its fields' fills,
/// its label font. The kit's forms cut their texts short; a pane's texts wrap, as they did.
pub fn pane<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    sc::scope(ui, |ui| {
        ui.style_mut().wrap_mode = None;
        add(ui)
    })
}

/// The frame of a card in a pane: the theme's fine border around what belongs together.
pub fn card_frame() -> egui::Frame {
    let t = Tokens::get();
    egui::Frame::NONE
        .fill(t.dialog_body)
        .stroke(t.tertiary_border(t.dialog_rule))
        .corner_radius(t.radius_lg)
        .inner_margin(t.space_10)
}

/// Text that stands out: the kit's heavier face. (The kit's forms give every text one ink, so
/// egui's "strong", another ink, shows nothing there.)
pub fn strong(text: impl Into<String>) -> RichText {
    let t = Tokens::get();
    RichText::new(text).font(sc::type_face::semibold(t.font_label)).color(t.form_ink)
}

/// A button of a pane that does something, as high as the kit's fields.
pub fn action(ui: &mut Ui, label: impl AsRef<str>, enabled: bool) -> Response {
    kit_button(ui, label.as_ref(), enabled, sc::Button::Normal)
}

/// The button a card or a pane is about, in the accent colour.
pub fn primary(ui: &mut Ui, label: impl AsRef<str>, enabled: bool) -> Response {
    kit_button(ui, label.as_ref(), enabled, sc::Button::Primary)
}

/// The kit's button with its room taken in the row itself: a row that wraps moves it to the
/// next line whole, and only a label longer than a whole line is cut short. Named by its place
/// among the widgets, as egui names its own: two may carry the same label.
fn kit_button(ui: &mut Ui, label: &str, enabled: bool, look: sc::Button) -> Response {
    let t = Tokens::get();
    let line = ui.max_rect().width();
    let room = if line.is_finite() && line > 0.0 { line } else { f32::INFINITY };
    let pad = if matches!(look, sc::Button::Primary) { t.space_16 } else { t.space_12 };
    let width = bar_button_width(ui, label) - 2.0 * t.space_8 + 2.0 * pad;
    let key = format!("{label}#{}", ui.next_auto_id().value());
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width.min(room), t.control_height), egui::Sense::hover());
    ui.add_enabled_ui(enabled, |ui| sc::button_at(ui, &key, rect, label, look)).inner
}

/// A checkbox without a text: the tick of a row or of a whole card. `mixed` draws it as
/// "some of them"; a click then ticks all.
pub fn check_mark(ui: &mut Ui, value: &mut bool, mixed: bool, enabled: bool) -> Response {
    ui.add_enabled_ui(enabled, |ui| {
        // As high as a line of text, not as a field: beside a title at the top of a card it
        // stands level with the title then.
        let t = Tokens::get();
        ui.spacing_mut().interact_size.y = t.space_16 + t.space_4;
        sc::checkbox_indeterminate(ui, "", value, mixed)
    })
    .inner
}

/// The mark of something under way: the logo's tree, in one of its three animations.
pub fn busy(ui: &mut Ui) {
    super::loading::mark(ui);
}

/// The kit's thin bar of something under way: filled to `fraction`, or with a piece that
/// travels along it when nobody knows how far it is.
pub fn progress(ui: &mut Ui, fraction: Option<f32>) {
    let t = Tokens::get();
    match fraction {
        Some(fraction) => {
            sc::progress(ui, "pane", fraction);
        }
        None => {
            let size = egui::vec2(ui.available_width().max(0.0), t.space_6);
            let (bar, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            ui.painter().rect_filled(bar, t.space_3, t.dialog_rule);
            const PIECE: f32 = 0.3;
            let at = (ui.input(|i| i.time) * 0.7).fract() as f32 * (1.0 + PIECE) - PIECE;
            let piece = egui::Rect::from_min_max(
                egui::pos2(bar.left() + bar.width() * at.max(0.0), bar.top()),
                egui::pos2(bar.left() + bar.width() * (at + PIECE).min(1.0), bar.bottom()),
            );
            ui.painter().rect_filled(piece, t.space_3, t.form_primary);
            ui.ctx().request_repaint();
        }
    }
}

/// The ink for text and signs on a fill of the form's primary colour (a badge, a button of our
/// own, the round mark's dot): the theme's `on_accent`, unless that can hardly be read there
/// (below 3:1). Then black or white, whichever reads better. That is so in Meshgrove Dark
/// (white on a light blue) and in the two 1-bit themes (whose `on_accent` pairs with `accent`,
/// the opposite of their primary colour); every other theme keeps its own ink.
pub fn ink_on_accent() -> Color32 {
    let t = Tokens::get();
    let fill = t.form_primary;
    if solidcraft_theme::contrast_ratio(t.on_accent, fill) >= 3.0 {
        return t.on_accent;
    }
    let (black, white) = (Color32::BLACK, Color32::WHITE);
    let better = solidcraft_theme::contrast_ratio(black, fill) >= solidcraft_theme::contrast_ratio(white, fill);
    if better { black } else { white }
}

/// [`text_field`] that keeps its identity (and with it the keyboard's focus) while what
/// stands before it in its row comes and goes: named by `id`, which has to be one of a kind in
/// the whole window (a row's own number in it), not by its place among the widgets.
pub fn text_field_named(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut String,
    hint: &str,
    width: f32,
) -> Response {
    text_field_named_reserving(ui, id, value, hint, width, 0.0)
}

/// [`text_field_named`] whose text stops `reserve` points short of its right edge, for a
/// button drawn inside the field there ([`field_button`]).
pub fn text_field_named_reserving(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut String,
    hint: &str,
    width: f32,
    reserve: f32,
) -> Response {
    let size = egui::vec2(width.min(ui.available_width().max(60.0)), Tokens::get().control_height);
    // Its room first, in the row itself, so a row that wraps moves it as one piece.
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    // A scope with a name of its own: one salted with a name still counts the widgets before
    // it in its parent, and a chip more in the row would make this another field to egui.
    let place = egui::UiBuilder::new()
        .scope_id(egui::Id::unique(("named-field", id)))
        .max_rect(rect)
        .layout(egui::Layout::left_to_right(egui::Align::Center));
    ui.scope_builder(place, |ui| {
        ui.set_width(rect.width());
        sc::text_field_reserving(ui, value, hint, reserve).1
    })
    .inner
}

/// How much room [`field_button`] takes at a field's right end.
pub fn field_button_room() -> f32 {
    Tokens::get().control_height
}

/// A small button inside a field, at its right end, in the accent colour with one of the kit's
/// icons: what the field can also be filled with (a folder from the system's dialog). The
/// field keeps [`field_button_room`] free for it. It takes clicks but never the keyboard.
pub fn field_button(ui: &mut Ui, field: &Response, icon: &str, tooltip: &str) -> Response {
    let t = Tokens::get();
    let room = field_button_room();
    let slot = egui::Rect::from_min_max(egui::pos2(field.rect.right() - room, field.rect.top()), field.rect.max)
        .shrink(t.space_3);
    let response = ui.interact(slot, field.id.with("field-button"), egui::Sense::CLICK);
    if ui.is_rect_visible(slot) {
        let fill = if response.hovered() { t.form_primary.gamma_multiply(0.85) } else { t.form_primary };
        ui.painter().rect_filled(slot, t.radius_sm, fill);
        let side = (slot.height() - 2.0 * t.space_4).max(8.0);
        let sign = egui::Rect::from_center_size(slot.center(), egui::vec2(side, side));
        sc::icons::paint_ink(ui.painter(), sign, icon, ink_on_accent());
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tooltip)
}

/// The kit's checkbox with a label that wraps where it is longer than its line, and is never
/// cut short. (The kit lays a label that may not wrap out a second time at the width it
/// measured, and rounding then cuts some labels with "…"; one that may wrap is painted as
/// measured.)
pub fn checkbox(ui: &mut Ui, label: &str, value: &mut bool) -> Response {
    let before = ui.style().wrap_mode;
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
    let response = sc::checkbox(ui, label, value);
    ui.style_mut().wrap_mode = before;
    response
}

/// One of several of which exactly one is chosen, as a round mark with its label (which wraps):
/// the kit's checkbox in the round. Clicked when the user picks it.
pub fn radio(ui: &mut Ui, label: &str, chosen: bool) -> Response {
    let t = Tokens::get();
    let mark = t.space_14;
    let font = egui::FontId::proportional(t.font_label);
    let room = (ui.available_width() - mark - t.space_6).max(0.0);
    let galley = ui.painter().layout(label.to_owned(), font, t.form_ink, room);
    let size = egui::vec2(
        (mark + t.space_6 + galley.size().x).min(ui.available_width().max(0.0)),
        galley.size().y.max(t.control_height - t.space_4),
    );
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let center = egui::pos2(rect.left() + mark / 2.0, rect.center().y);
        let lit = chosen || response.hovered() || response.has_focus();
        let (fill, border) = if chosen { (t.form_primary, t.form_primary) } else { (t.control_fill, t.control_border) };
        ui.painter().circle_filled(center, mark / 2.0, fill);
        let stroke = if lit { egui::Stroke::new(1.0, t.form_primary) } else { t.control_border_stroke(border) };
        ui.painter().circle_stroke(center, mark / 2.0 - stroke.width / 2.0, stroke);
        if chosen {
            ui.painter().circle_filled(center, mark / 5.0, ink_on_accent());
        }
        let at = egui::pos2(rect.left() + mark + t.space_6, rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(at, galley, t.form_ink);
    }
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, ui.is_enabled(), chosen, label));
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// One row of an open list. True when it was clicked.
pub fn choice_row(ui: &mut Ui, chosen: bool, text: &str) -> bool {
    sc::list_picker(ui, text, text, chosen).clicked()
}

/// One row of an open list that sets `value` to `option`. True when that changed it.
pub fn choice<T: PartialEq>(ui: &mut Ui, value: &mut T, option: T, text: &str) -> bool {
    let chosen = *value == option;
    if choice_row(ui, chosen, text) && !chosen {
        *value = option;
        return true;
    }
    false
}

/// A button that can only be pressed while `enabled`.
pub fn button_if(ui: &mut Ui, enabled: bool, label: impl Into<egui::WidgetText>) -> Response {
    ui.add_enabled_ui(enabled, |ui| sc::button(ui, label, sc::Button::Normal)).inner
}

/// A button as high as the kit's fields and toggles, for a row of controls that should line up
/// (the toolbar, the bar over a 3D view, the rows of the info pane).
pub fn bar_button(ui: &mut Ui, label: &str) -> Response {
    bar_button_if(ui, label, true, false)
}

/// How wide [`bar_button`] is with this label: to see beforehand whether a row of them fits.
pub fn bar_button_width(ui: &Ui, label: &str) -> f32 {
    let t = Tokens::get();
    let text = egui::WidgetText::from(label).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Button,
    );
    (text.size().x + 2.0 * t.space_8).max(t.control_height)
}

/// [`bar_button`] at most `room` wide: a label too long for it is cut short with "…".
pub fn bar_button_within(ui: &mut Ui, label: &str, room: f32) -> Response {
    sized_bar_button(ui, label, room, true, false)
}

/// [`bar_button`] that can only be pressed while `enabled`, drawn as switched on while `on`.
pub fn bar_button_if(ui: &mut Ui, label: &str, enabled: bool, on: bool) -> Response {
    sized_bar_button(ui, label, f32::INFINITY, enabled, on)
}

fn sized_bar_button(ui: &mut Ui, label: &str, room: f32, enabled: bool, on: bool) -> Response {
    let t = Tokens::get();
    let size = egui::vec2(bar_button_width(ui, label).min(room), t.control_height);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let look = if on { sc::Button::SmallSelected } else { sc::Button::Normal };
    ui.add_enabled_ui(enabled, |ui| sc::button_at(ui, label, rect, label, look)).inner
}

/// A small square button with a sign on it (✖, ✏, ▲ …) and what it does as its tooltip.
pub fn small(ui: &mut Ui, sign: &str, tooltip: &str) -> Response {
    sc::compact_button(ui, sign, tooltip, true, false)
}

/// [`small`] that can only be pressed while `enabled`.
pub fn small_if(ui: &mut Ui, enabled: bool, sign: &str, tooltip: &str) -> Response {
    sc::compact_button(ui, sign, tooltip, enabled, false)
}

/// A slider `width` wide over `range`. With `format`, the value is written beside it and can be
/// typed; without, the slider stands alone (the caller says what it is at).
pub fn slider<T: egui::emath::Numeric>(
    ui: &mut Ui,
    value: &mut T,
    range: std::ops::RangeInclusive<T>,
    width: f32,
    format: Option<fn(f64) -> String>,
) -> Response {
    let options = sc::Slider {
        integer: T::INTEGRAL,
        show_value: format.is_some(),
        format: format.unwrap_or(|v| format!("{v:.0}")),
        ..Default::default()
    };
    fixed_width(ui, width, |ui| sc::slider(ui, value, range, options))
}

/// One choice of a list of which exactly one is chosen, as the kit's round mark in a tile: the
/// name, and what belongs to it at the right. `beside` is the room kept at the right of the tile
/// for small buttons. True when it was clicked.
pub fn choice_tile(ui: &mut Ui, chosen: bool, name: &str, about: &str, tooltip: &str, beside: f32) -> bool {
    let width = (ui.available_width() - beside).max(160.0);
    let tile = sc::SettingsChoice { key: name, icon: None, title: name, summary: about, detail: tooltip };
    // The room is asked for at the tile's own height (the kit's choice row): asked for without
    // a height, the tile hung below the middle of its row, and what stood beside it (a state,
    // the small buttons) was not level with it.
    let t = Tokens::get();
    let height = t.space_24 + t.space_16;
    ui.allocate_ui_with_layout(egui::vec2(width, height), egui::Layout::top_down(egui::Align::Min), |ui| {
        ui.set_width(width);
        sc::settings_choice_row(ui, &tile, chosen).clicked()
    })
    .inner
}

/// [`choice_tile`] with a picture inside it, between the round mark and the name: `picture`
/// paints it into the square it is given (a program's own icon). The kit's row has no room
/// for a picture of the caller's, so this is that row painted here, token for token.
pub fn choice_tile_with(
    ui: &mut Ui,
    chosen: bool,
    (name, about, tooltip): (&str, &str, &str),
    beside: f32,
    picture: impl FnOnce(&Ui, egui::Rect),
) -> bool {
    let t = Tokens::get();
    let accent = solidcraft_theme::TabAccent::current(ui.ctx());
    let size = egui::vec2((ui.available_width() - beside).max(160.0), t.space_24 + t.space_16);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if chosen {
            accent.soft
        } else if response.hovered() {
            t.hover
        } else {
            t.control_fill
        };
        let border =
            if chosen { egui::Stroke::new(1.0, accent.color) } else { t.control_border_stroke(t.control_border) };
        ui.painter().rect(rect, t.radius_lg, fill, border, egui::StrokeKind::Inside);
        let radio = egui::pos2(rect.left() + t.space_16, rect.center().y);
        ui.painter().circle(radio, t.space_6, t.control_fill, border);
        if chosen {
            ui.painter().circle_filled(radio, t.space_3, accent.color);
        }
        // The picture, as high as the row's text line allows, then the name.
        let side = t.space_24;
        let slot = egui::Rect::from_center_size(
            egui::pos2(radio.x + t.space_12 + side / 2.0, rect.center().y),
            egui::vec2(side, side),
        );
        picture(ui, slot);
        let about_font = egui::FontId::proportional(t.font_small);
        let about_width = ui.painter().layout_no_wrap(about.to_owned(), about_font.clone(), t.form_dim).size().x;
        let about_width = about_width.min((rect.width() * 0.6).max(0.0));
        let title = egui::Rect::from_min_max(
            egui::pos2(slot.right() + t.space_8, rect.top()),
            egui::pos2(rect.right() - t.space_8 - about_width - t.space_8, rect.bottom()),
        );
        let ink = if chosen { accent.ink } else { t.form_ink };
        sc::paint_text(ui, title, name, sc::type_face::semibold(t.font_label), ink, false);
        let body = egui::Rect::from_min_max(
            egui::pos2(rect.right() - t.space_8 - about_width, rect.top()),
            rect.max - egui::vec2(t.space_8, 0.0),
        );
        sc::paint_text(ui, body, about, about_font, t.form_dim, false);
    }
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, ui.is_enabled(), chosen, name));
    response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tooltip).clicked()
}

/// A choice of a few as one row of joined icon buttons, the chosen one lit: "either this or
/// that" at a glance. `options` are (the kit icon's name, what it stands for as the tooltip).
/// True when the user picked another.
pub fn icon_toggle<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &'static str, String)]) -> bool {
    let segments: Vec<sc::IconSegment> =
        options.iter().map(|(_, icon, tip)| sc::IconSegment { key: icon, icon, tip: tip.clone() }).collect();
    let mut index = options.iter().position(|(option, _, _)| option == value).unwrap_or(0);
    let width = options.len() as f32 * (Tokens::get().control_height + 4.0);
    let picked = fixed_width(ui, width, |ui| sc::segmented_icons(ui, &mut index, &segments, &[]));
    match options.get(index) {
        Some((option, _, _)) if picked && option != value => {
            *value = *option;
            true
        }
        _ => false,
    }
}
