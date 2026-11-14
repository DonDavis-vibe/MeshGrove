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
            ui.painter().circle_filled(center, mark / 5.0, t.on_accent);
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
