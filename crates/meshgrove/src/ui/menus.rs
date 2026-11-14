//! Menus on SolidCraft's UI kit: a button that opens one, a context menu, and their rows (an
//! action with its icon, a switch with its tick, one of several, a row that opens a further menu,
//! headings and rules). The popups are the kit's (its frame, its flyouts beside a row), the rows
//! its `MenuItem`.
//!
//! Icons are names from the kit's icon set (`assets/icons/svg`); the theme decides whether they
//! come in colour or in one ink. A row with a tick has the tick where the icon would be.

use egui::{Color32, PopupCloseBehavior, Response, Ui};
use solidcraft_ui_kit as sc;
use solidcraft_ui_kit::{Menu, MenuItem};

/// A small button that opens a menu under it; `rows` draws the menu. A click on a row closes it.
pub fn button<R>(ui: &mut Ui, label: &str, rows: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<Option<R>> {
    opened_by(ui, label, PopupCloseBehavior::CloseOnClick, rows)
}

/// [`button`] whose menu stays open on clicks inside it, for menus with something to drag or to
/// type into (a slider, a name field). The rows that act close it themselves: [`item`] does,
/// a switch needs `ui.close()`.
pub fn button_staying<R>(ui: &mut Ui, label: &str, rows: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<Option<R>> {
    opened_by(ui, label, PopupCloseBehavior::CloseOnClickOutside, rows)
}

/// The button and its menu, one frame around rows as wide as the kit's list menus.
fn opened_by<R>(
    ui: &mut Ui,
    label: &str,
    closes: PopupCloseBehavior,
    rows: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    let response = super::settings_kit::bar_button(ui, label);
    let shown = sc::Container::new(ui.ctx(), sc::Surface::Popup)
        .menu(&response)
        .close_behavior(closes)
        .frame(Menu::frame())
        .show(|ui| {
            let t = solidcraft_theme::Tokens::get();
            ui.set_width((sc::LIST_W - t.space_6 * 2.0 - 2.0).max(0.0));
            ui.spacing_mut().item_spacing.y = 0.0;
            rows(ui)
        });
    egui::InnerResponse { response, inner: shown.map(|s| s.inner) }
}

/// The menu of a right click on `response`. It stays open on clicks inside (a name field, a
/// switch); the rows that act close it themselves ([`item`] does).
pub fn context(response: &Response, rows: impl FnOnce(&mut Ui)) {
    sc::Container::new(&response.ctx, sc::Surface::Popup)
        .context_menu(response)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .frame(Menu::frame())
        .show(|ui| {
            // As wide as its longest row, and never a narrow strip.
            ui.set_min_width(MIN_W);
            ui.spacing_mut().item_spacing.y = 0.0;
            rows(ui);
        });
}

/// The context menu of something whose widgets do not last (the titles of a table that is built
/// anew when its columns change, which would close a menu hanging on one of them): it hangs on
/// `anchor`, a response that stays, and opens at the pointer in the frame `open` is true.
pub fn context_from(anchor: &Response, open: bool, rows: impl FnOnce(&mut Ui)) {
    sc::Container::new(&anchor.ctx, sc::Surface::Popup)
        .context_menu(anchor)
        .open_memory(open.then_some(egui::SetOpenCommand::Bool(true)))
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .frame(Menu::frame())
        .show(|ui| {
            ui.set_min_width(MIN_W);
            ui.spacing_mut().item_spacing.y = 0.0;
            rows(ui);
        });
}

/// The least width of a context menu, which is otherwise as wide as its longest row.
const MIN_W: f32 = 170.0;

/// The width a full-width piece (a rule, a heading) asks for: the menu's, but nothing while the
/// menu is still being measured, or it would claim all the room there is and make a menu that
/// sizes itself to its rows as wide as the window.
fn across(ui: &Ui, least: f32) -> f32 {
    if ui.is_sizing_pass() { least } else { ui.available_width() }
}

/// A row that does something, with its icon (`""` for none). True when it was clicked; the menu
/// closes then.
pub fn item(ui: &mut Ui, icon: &str, label: &str) -> bool {
    item_if(ui, icon, label, true, "", false)
}

/// [`item`] that can only be chosen while `enabled`; resting on it says `why_not` otherwise.
/// `danger` draws it in the colour of what cannot be taken back lightly.
pub fn item_if(ui: &mut Ui, icon: &str, label: &str, enabled: bool, why_not: &str, danger: bool) -> bool {
    let response = step(ui, icon, label, enabled, danger);
    if !enabled {
        if !why_not.is_empty() {
            response.on_hover_text(why_not);
        }
        return false;
    }
    let clicked = response.clicked();
    if clicked {
        ui.close();
    }
    clicked
}

/// A row like [`item_if`] that leaves the menu open when clicked: the first of two clicks
/// ("Delete", then "Really delete?"), or a row whose effect shows in the menu itself.
pub fn step(ui: &mut Ui, icon: &str, label: &str, enabled: bool, danger: bool) -> Response {
    MenuItem { enabled, danger, ..MenuItem::new(label, label, icon) }.show(ui)
}

/// A row with a dot of `color` in place of an icon (a tag in its colour). True when it was
/// clicked; the menu closes then.
pub fn swatch(ui: &mut Ui, color: Color32, label: &str) -> bool {
    let clicked = MenuItem { swatch: Some(color), ..MenuItem::new(label, label, "") }.show(ui).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

/// A switch: the row has a tick while it is on. Its response is `changed` when the user
/// switched it. With an `icon` the tick stands at the row's end, without one in the icon's place.
pub fn check(ui: &mut Ui, icon: &str, label: &str, value: &mut bool) -> Response {
    let mut response = ticked(ui, icon, label, *value);
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    response
}

/// One of several: the row has a tick while `value` is `option`. True when the user chose it
/// and it was another one. A click on any of them closes the menu.
pub fn choice<T: PartialEq>(ui: &mut Ui, value: &mut T, option: T, icon: &str, label: &str) -> bool {
    let chosen = *value == option;
    if !ticked(ui, icon, label, chosen).clicked() {
        return false;
    }
    ui.close();
    if chosen {
        return false;
    }
    *value = option;
    true
}

/// A row that can have a tick. The kit's row draws the tick where the icon would be; a row with
/// an icon keeps it and gets the tick at its end.
fn ticked(ui: &mut Ui, icon: &str, label: &str, on: bool) -> Response {
    if icon.is_empty() {
        return MenuItem { checked: Some(on), ..MenuItem::new(label, label, "") }.show(ui);
    }
    let t = solidcraft_theme::Tokens::get();
    let row = MenuItem::new(label, label, icon);
    let mark = t.space_16;
    // As the kit's row measures itself, plus the room for the tick.
    let width = if ui.is_sizing_pass() {
        let text = ui.painter().layout_no_wrap(label.to_owned(), egui::FontId::proportional(t.font_label), t.form_ink);
        (t.control_height + t.space_2 + text.size().x + t.space_24 + mark + t.space_8).min(ui.available_width())
    } else {
        ui.available_width()
    };
    let (r, response) = ui.allocate_exact_size(egui::vec2(width, t.control_height + t.space_2), egui::Sense::click());
    row.paint(ui, r, &response, mark + t.space_4);
    if on {
        let at = egui::Rect::from_center_size(egui::pos2(r.right() - t.space_8 - mark / 2.0, r.center().y), egui::vec2(mark, mark));
        // The kit keeps its own tick to itself: the same three points.
        let (c, m) = (at.center(), mark);
        let points = vec![
            egui::pos2(c.x - 0.30 * m, c.y + 0.02 * m),
            egui::pos2(c.x - 0.08 * m, c.y + 0.24 * m),
            egui::pos2(c.x + 0.32 * m, c.y - 0.22 * m),
        ];
        ui.painter().add(egui::Shape::line(points, egui::Stroke::new(1.6, t.form_primary)));
    }
    response
}

/// A row that opens a further menu beside it; `rows` draws that one.
pub fn submenu<R>(ui: &mut Ui, icon: &str, label: &str, rows: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    let row = MenuItem { submenu: true, ..MenuItem::new(label, label, icon) };
    Menu::item_button(ui, &row, rows).inner
}

/// Something that is not a row (a slider, a name field), set in from the menu's edge like the
/// rows' texts are.
pub fn inset<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = solidcraft_theme::Tokens::get();
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(t.space_8 as i8, t.space_4 as i8))
        .show(ui, add)
        .inner
}

/// A sentence of explanation among the rows, wrapped at the menu's width.
pub fn note(ui: &mut Ui, text: &str) {
    inset(ui, |ui| {
        let t = solidcraft_theme::Tokens::get();
        let text = egui::RichText::new(text).size(t.font_small).color(t.form_dim);
        ui.add(egui::Label::new(text).wrap().selectable(false));
    });
}

/// A quiet caption over the rows that follow.
pub fn heading(ui: &mut Ui, label: &str) {
    // The kit's `Menu::heading`, with a width of its own while the menu is measured.
    let t = solidcraft_theme::Tokens::get();
    let mut job = egui::text::LayoutJob::simple_singleline(
        label.to_uppercase(),
        sc::type_face::semibold(t.font_small),
        t.form_dim,
    );
    for section in &mut job.sections {
        section.format.extra_letter_spacing = t.font_small * t.font_section_tracking;
    }
    let galley = ui.painter().layout_job(job);
    let width = across(ui, galley.size().x + t.space_8 * 2.0);
    let (r, _) = ui.allocate_exact_size(egui::vec2(width, t.space_24), egui::Sense::hover());
    let at = egui::pos2(r.left() + t.space_8, r.center().y - galley.size().y / 2.0);
    ui.painter().with_clip_rect(ui.clip_rect().intersect(r)).galley(at, galley, t.form_dim);
}

/// A rule between groups of rows.
pub fn separator(ui: &mut Ui) {
    // The kit's `Menu::separator`, with no width of its own while the menu is measured.
    let t = solidcraft_theme::Tokens::get();
    let (r, _) = ui.allocate_exact_size(egui::vec2(across(ui, 0.0), t.space_10 + 1.0), egui::Sense::hover());
    ui.painter().hline(r.x_range(), r.center().y, t.tertiary_border(t.dialog_rule));
}
