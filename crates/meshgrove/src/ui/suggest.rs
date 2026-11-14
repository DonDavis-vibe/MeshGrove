//! Picking from candidates while typing, shared by the tag field and the chat's @ references:
//! what matches the typed text, in which order, the highlight that Up/Down move, and the
//! floating list they are picked from.

use egui::{Key, Ui};

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate<T> {
    pub label: String,
    /// Shown after the label (what kind it is, how many files …).
    pub detail: String,
    /// Something the library has, as opposed to a suggestion: listed first among equals.
    pub existing: bool,
    pub value: T,
}

/// The candidates whose label contains `needle` (any case): those starting with it first, then
/// existing ones before suggestions, otherwise in the given order; at most `max`.
pub fn rank<T>(needle: &str, mut items: Vec<Candidate<T>>, max: usize) -> Vec<Candidate<T>> {
    let needle = needle.trim().to_lowercase();
    if !needle.is_empty() {
        items.retain(|c| c.label.to_lowercase().contains(&needle));
        items.sort_by_key(|c| (!c.label.to_lowercase().starts_with(&needle), !c.existing));
    }
    items.truncate(max);
    items
}

/// The highlighted candidate of `count`, moved by Up/Down while `active` (the keys are taken
/// from the text field then). It starts at none and resets when the typed text changes.
pub fn highlight(ui: &mut Ui, id: egui::Id, needle: &str, count: usize, active: bool) -> Option<usize> {
    let (seen, mut picked): (String, Option<usize>) = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    if seen != needle {
        picked = None;
    }
    if active && count > 0 {
        let down = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
        let up = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
        let last = count - 1;
        if down {
            picked = Some(picked.map_or(0, |i| (i + 1).min(last)));
        } else if up {
            picked = picked.and_then(|i| i.checked_sub(1));
        }
    }
    picked = picked.filter(|&i| i < count);
    ui.data_mut(|d| d.insert_temp(id, (needle.to_owned(), picked)));
    picked
}

/// One row of a [`floating_list`]: a name, what it is (quieter), and a dot of its colour in front
/// where it has one (a tag).
pub struct Offer {
    pub name: String,
    pub about: String,
    pub dot: Option<egui::Color32>,
}

/// A floating list of what can be taken while typing, with a corner (`pivot`) at `at`: over a
/// field (pivot at its lower left) or under one. `lit` is the row Enter takes. The list is a layer of its own, so nothing around the field moves when it comes
/// and goes, and its rows take clicks but never the keyboard's focus (the field keeps it).
///
/// Returns the row that was clicked, and whether the pointer is down on the list: a click there
/// is a click outside the field to egui, which takes the field's focus; the caller keeps the
/// list for that frame and gives the focus back.
pub fn floating_list(
    ui: &Ui,
    id: egui::Id,
    (at, pivot): (egui::Pos2, egui::Align2),
    width: f32,
    rows: &[Offer],
    lit: Option<usize>,
) -> (Option<usize>, bool) {
    let t = solidcraft_theme::Tokens::get();
    let (mut clicked, mut down) = (None, false);
    egui::Area::new(id).order(egui::Order::Foreground).pivot(pivot).fixed_pos(at).show(ui.ctx(), |ui| {
        solidcraft_ui_kit::Menu::frame().show(ui, |ui| {
            ui.set_width((width - t.space_6 * 2.0 - 2.0).max(0.0));
            ui.spacing_mut().item_spacing.y = 0.0;
            for (i, offer) in rows.iter().enumerate() {
                let row = floating_row(ui, offer, lit == Some(i));
                down |= row.is_pointer_button_down_on();
                if row.clicked() {
                    clicked = Some(i);
                }
            }
        });
    });
    (clicked, down)
}

/// One row of a [`floating_list`]: its dot, the name and, quieter, what it is. Lit while it is
/// the one Enter takes, or under the pointer.
fn floating_row(ui: &mut Ui, offer: &Offer, lit: bool) -> egui::Response {
    let (name, about) = (offer.name.as_str(), offer.about.as_str());
    use egui::text::LayoutJob;
    let t = solidcraft_theme::Tokens::get();
    let size = egui::vec2(ui.available_width(), t.control_height);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::CLICK);
    if ui.is_rect_visible(rect) {
        if lit || response.hovered() {
            ui.painter().rect_filled(rect, t.radius_sm, if lit { t.selected_bg } else { t.hover });
        }
        let ink = if lit { t.selected_ink } else { t.form_ink };
        let face = solidcraft_ui_kit::type_face::semibold(t.font_label);
        let name = ui.painter().layout_no_wrap(name.to_owned(), face, ink);
        let mut left = rect.left() + t.space_8;
        if let Some(color) = offer.dot {
            // As in the sidebar's tag list: a dot of the tag's colour in front of its name.
            let radius = t.space_4;
            ui.painter().circle_filled(egui::pos2(left + radius, rect.center().y), radius, color);
            left += 2.0 * radius + t.space_6;
        }
        let after = left + name.size().x + t.space_8;
        ui.painter().galley(egui::pos2(left, rect.center().y - name.size().y / 2.0), name, ink);
        // What it is, cut short where the row ends.
        let mut job =
            LayoutJob::simple_singleline(about.to_owned(), egui::FontId::proportional(t.font_small), t.form_dim);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn c(label: &str, existing: bool) -> Candidate<()> {
        Candidate { label: label.into(), detail: String::new(), existing, value: () }
    }

    #[test]
    fn matches_at_the_start_and_existing_ones_come_first() {
        let items = vec![c("Gartenzaun", false), c("Vorgarten", true), c("Garten", true), c("Werkstatt", true)];
        let labels: Vec<String> = rank("gart", items.clone(), 8).into_iter().map(|c| c.label).collect();
        assert_eq!(labels, ["Garten", "Gartenzaun", "Vorgarten"]);
        assert_eq!(rank("", items, 2).len(), 2, "nothing typed: the first ones");
    }
}
