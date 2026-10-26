//! The parts the settings pages are built from, arranged after SolidCraft's preferences: a section
//! is a small capitalised title over a rule that folds the section away (remembered in the config),
//! a row keeps its label in a column of its own with the control beside it, a choice among a few is
//! a row of joined buttons, and a hint is a quiet line under what it explains.

use egui::{Color32, CornerRadius, Label, Response, RichText, Sense, Stroke, TextStyle, Ui, pos2, vec2};

/// Green of a state that is fine ("connected").
pub const GOOD: Color32 = Color32::from_rgb(92, 178, 112);

/// The width of the label column: wide enough for most labels, never most of a narrow page.
fn label_width(ui: &Ui) -> f32 {
    (ui.available_width() * 0.34).clamp(120.0, 210.0)
}

/// A section: its title in small capitals over a rule, then its content unless the user folded
/// it (`key` in `folded`). A click on the title gives whether it is to be folded now.
pub fn section(ui: &mut Ui, key: &str, title: &str, folded: &[String], body: impl FnOnce(&mut Ui)) -> Option<bool> {
    let open = !folded.iter().any(|f| f == key);
    ui.add_space(12.0);
    // A little larger than the sidebar's captions: these carry the page.
    let font = egui::FontId::proportional(ui.text_style_height(&TextStyle::Small) + 1.0);
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::click());
    let visuals = ui.visuals();
    let color = if response.hovered() { visuals.text_color() } else { visuals.weak_text_color() };
    let painter = ui.painter();
    // A triangle pointing at the content, or to the right when it is folded away.
    let c = pos2(rect.left() + 4.0, rect.center().y);
    let points = if open {
        vec![c + vec2(-3.5, -2.0), c + vec2(3.5, -2.0), c + vec2(0.0, 2.5)]
    } else {
        vec![c + vec2(-2.0, -3.5), c + vec2(-2.0, 3.5), c + vec2(2.5, 0.0)]
    };
    painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
    let galley = painter.layout_no_wrap(title.to_uppercase(), font, color);
    let text = pos2(rect.left() + 13.0, rect.center().y - galley.size().y / 2.0);
    let end = text.x + galley.size().x + 8.0;
    painter.galley(text, galley, color);
    if end < rect.right() {
        let y = rect.center().y.round() + 0.5;
        painter.hline(end..=rect.right(), y, visuals.widgets.noninteractive.bg_stroke);
    }
    let toggled = response.on_hover_cursor(egui::CursorIcon::PointingHand).clicked().then_some(open);
    if open {
        ui.add_space(6.0);
        body(ui);
    }
    toggled
}

/// A labelled row: the label in the label column, the control (and anything under it, such as a
/// hint) beside it.
pub fn row<R>(ui: &mut Ui, label: &str, control: impl FnOnce(&mut Ui) -> R) -> R {
    let width = label_width(ui);
    // On the middle of a control's first line.
    let pad = ((ui.spacing().interact_size.y - ui.text_style_height(&TextStyle::Body)) / 2.0).max(0.0);
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(vec2(width, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_width(width);
            ui.add_space(pad);
            ui.add(Label::new(label).wrap());
        });
        ui.vertical(|ui| control(ui)).inner
    })
    .inner
}

/// A quiet explanation under the control it belongs to.
pub fn hint(ui: &mut Ui, text: impl Into<String>) {
    ui.add(Label::new(RichText::new(text.into()).weak()).wrap());
}

/// A checkbox with its explanation under its text, not under the box.
pub fn check(ui: &mut Ui, value: &mut bool, label: impl Into<String>, hint_text: impl Into<String>) -> bool {
    let changed = ui.checkbox(value, label.into()).changed();
    let indent = ui.spacing().icon_width + ui.spacing().icon_spacing;
    ui.horizontal_top(|ui| {
        ui.add_space(indent - ui.spacing().item_spacing.x);
        hint(ui, hint_text);
    });
    changed
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
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 1.0;
        let r = ui.visuals().widgets.inactive.corner_radius;
        let last = options.len().saturating_sub(1);
        for (i, (option, label)) in options.iter().enumerate() {
            let corners = CornerRadius {
                nw: if i == 0 { r.nw } else { 0 },
                sw: if i == 0 { r.sw } else { 0 },
                ne: if i == last { r.ne } else { 0 },
                se: if i == last { r.se } else { 0 },
            };
            let selected = *option == *value;
            let button = egui::Button::selectable(selected, label.as_str())
                .frame_when_inactive(true)
                .corner_radius(corners)
                .min_size(vec2(52.0, ui.spacing().interact_size.y));
            let why = unavailable(*option);
            let response = ui.add_enabled(why.is_none(), button);
            let response = match why {
                Some(why) => response.on_disabled_hover_text(why),
                None => response,
            };
            if response.clicked() && !selected {
                *value = *option;
                changed = true;
            }
        }
    });
    changed
}

/// A small coloured label for a state or a count.
pub fn pill(ui: &mut Ui, text: impl Into<String>, color: Color32) -> Response {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.16))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(8, 0))
        .show(ui, |ui| ui.label(RichText::new(text.into()).color(color)))
        .response
}
