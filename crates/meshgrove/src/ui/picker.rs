//! A dropdown that opens on a search box: the choices under it, filtered as you type, with a row
//! that takes the typed text as a value of its own, and the choices of several providers folded
//! under their names. Enter picks the exact or first match. Only the rows in view are laid out,
//! so a long list (OpenRouter's) stays cheap. For picking a model, in the chat and the settings.
//!
//! After SolidCraft's `ui_kit::search_picker` (`crates/ui-egui/src/ui_kit/fields.rs`, MIT license;
//! see `meshgrove_core::ai::agent_setup` for the notice).

use egui::{Response, RichText, Ui, vec2};

/// Longest search text and custom value the picker takes.
const TEXT_CAP: usize = 256;
/// Characters of a label shown on a row, and of a tip or note shown with it.
const LABEL_CAP: usize = 64;
const NOTE_CAP: usize = 1024;
/// Rows the popup makes room for when it opens with none.
const ROWS_AWAITED: usize = 6;
const ROW_HEIGHT: f32 = 22.0;

/// One choice. The value, label and tip are searched; `tip` also shows on hover, and a change of
/// `group` between choices draws a heading (or, with several groups, a fold).
pub struct Choice<'a> {
    pub value: &'a str,
    pub label: &'a str,
    pub tip: &'a str,
    pub group: Option<&'a str>,
}

/// What the picker shows.
pub struct Picker<'a> {
    /// Keys the popup's state: one per picker in the window.
    pub id: &'a str,
    /// The text on the closed control, and the value of the row to highlight.
    pub current: &'a str,
    pub current_value: &'a str,
    /// The search box's empty-text hint.
    pub hint: &'a str,
    pub choices: &'a [Choice<'a>],
    /// Offer what was typed as a value of its own when it is not a listed one.
    pub custom: bool,
    /// A dim line under the list: still listing, or why it could not.
    pub note: Option<&'a str>,
    /// The control's width (`None`: what is available).
    pub width: Option<f32>,
    pub enabled: bool,
}

/// What was picked: a listed value, or one typed into the box.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Picked {
    Listed(String),
    Custom(String),
}

impl Picked {
    pub fn value(&self) -> &str {
        match self {
            Picked::Listed(v) | Picked::Custom(v) => v,
        }
    }
}

/// The dropdown; the pick, if one was made this frame, and the control's response.
pub fn show(ui: &mut Ui, p: Picker<'_>) -> egui::InnerResponse<Option<Picked>> {
    let state = PICKER.with(p.id);
    let mut picked = None;
    let response = ui
        .add_enabled_ui(p.enabled, |ui| {
            let width = p.width.unwrap_or_else(|| ui.available_width());
            let response = egui::ComboBox::from_id_salt(state)
                .width(width)
                .height(ROW_HEIGHT * 16.0)
                .selected_text(compact(p.current, LABEL_CAP))
                .truncate()
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show_ui(ui, |ui| picked = popup(ui, &p, state))
                .response;
            // Closed: the next opening starts with an empty box that has the focus.
            if !egui::ComboBox::is_open(ui.ctx(), response.id) {
                ui.data_mut(|data| {
                    data.remove::<String>(state.with("filter"));
                    data.remove::<bool>(state.with("focused"));
                    data.remove::<f32>(state.with("height"));
                    data.remove::<Folds>(state.with("folds"));
                });
            }
            response
        })
        .inner;
    egui::InnerResponse::new(picked, response)
}

/// The pickers' state lives under this id, one salt per picker.
static PICKER: std::sync::LazyLock<egui::Id> = std::sync::LazyLock::new(|| egui::Id::unique("picker"));

/// The folds made by hand in one opening: group name → open.
type Folds = std::collections::HashMap<String, bool>;

fn popup(ui: &mut Ui, p: &Picker<'_>, state: egui::Id) -> Option<Picked> {
    let (filter_id, focused_id, folds_id) = (state.with("filter"), state.with("focused"), state.with("folds"));
    // Esc closes this popup only.
    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        ui.close();
    }
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
    ui.set_min_width(popup_width(ui, p));
    let mut filter: String = ui.data(|data| data.get_temp(filter_id)).unwrap_or_default();
    let search = ui.add(
        egui::TextEdit::singleline(&mut filter)
            .id(state.with("search"))
            .char_limit(TEXT_CAP)
            .hint_text(p.hint)
            .desired_width(f32::INFINITY),
    );
    // Asked until it has it: egui can drop the request of the opening's first frame.
    if !ui.data(|data| data.get_temp::<bool>(focused_id)).unwrap_or(false) {
        if search.has_focus() {
            ui.data_mut(|data| data.insert_temp(focused_id, true));
        } else {
            search.request_focus();
            ui.ctx().request_repaint();
        }
    }
    if search.changed() {
        ui.data_mut(|data| data.insert_temp(filter_id, filter.clone()));
    }
    let typed = filter.trim();
    let query: String = typed.chars().flat_map(char::to_lowercase).collect();
    let mut searched = String::new();
    let mut hit = |choice: &Choice<'_>| {
        searched.clear();
        for text in [choice.value, choice.label, choice.tip] {
            searched.extend(text.chars().take(TEXT_CAP).flat_map(char::to_lowercase));
            searched.push('\n');
        }
        searched.contains(&query)
    };
    let visible: Vec<&Choice<'_>> = p.choices.iter().filter(|choice| query.is_empty() || hit(choice)).collect();
    // Two or more groups fold under their headers: open by default are the current value's
    // group (every group when it has none), and every group once something is typed; a fold
    // made by hand stays until the popup closes.
    let grouped = groups(p.choices).iter().filter(|(group, _)| group.is_some()).count() >= 2;
    let folds: Folds = ui.data(|data| data.get_temp(folds_id)).unwrap_or_default();
    let current_group =
        p.choices.iter().find(|c| c.value == p.current_value).and_then(|c| c.group).filter(|g| !g.is_empty());
    let is_open = |group: &str| {
        folds.get(group).copied().unwrap_or(!query.is_empty() || current_group.is_none() || Some(group) == current_group)
    };
    let rows = rows(&visible, grouped, &is_open);
    let shown: Vec<&Choice<'_>> =
        rows.iter().filter_map(|row| if let Row::Choice(c) = row { Some(*c) } else { None }).collect();
    let custom = (p.custom && !typed.is_empty() && !p.choices.iter().any(|c| c.value == typed)).then_some(typed);
    let mut picked = None;
    let mut toggled: Option<(String, bool)> = None;
    if search.lost_focus() && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
        let found = visible.iter().find(|c| c.value == typed).or_else(|| shown.first());
        picked = match (typed.is_empty(), found, custom) {
            (true, ..) => {
                ui.close();
                None
            }
            (_, Some(choice), _) => Some(Picked::Listed(choice.value.to_owned())),
            (_, None, Some(model)) => Some(Picked::Custom(model.to_owned())),
            (_, None, None) => {
                search.request_focus();
                None
            }
        };
    }
    let width = ui.available_width();
    let gap = ui.spacing().item_spacing.y;
    let weak = ui.visuals().weak_text_color();
    let note = p.note.map(|note| {
        ui.painter().layout(compact(note, 200), egui::FontId::proportional(12.0), weak, width)
    });
    let note_height = note.as_ref().map_or(0.0, |note| note.size().y + gap);
    let most = ui.ctx().content_rect().height() - 96.0;
    // A popup's Ui stays within its size of the last frame, so it can't grow once it is open:
    // all that can change below the box shares a height fixed when it opens.
    let height = ui.data_mut(|data| {
        *data.get_temp_mut_or_insert_with(state.with("height"), || {
            let count = p.choices.len() + groups(p.choices).iter().filter(|(group, _)| group.is_some()).count();
            let count = if count == 0 { ROWS_AWAITED } else { count };
            let heading = 16.0 + gap;
            let custom = if p.custom { heading + ROW_HEIGHT + gap } else { 0.0 };
            let list = (count as f32 * (ROW_HEIGHT + gap)).clamp(custom + ROW_HEIGHT + gap, ROW_HEIGHT * 10.0);
            (heading + list + note_height).min(most).max(heading + custom + ROW_HEIGHT + gap)
        })
    });
    ui.allocate_ui(vec2(width, height), |ui| {
        ui.set_min_height(height);
        if let Some(model) = custom {
            heading(ui, &t!("picker.custom_heading"));
            let text = t!("picker.use_custom", value = compact(model, LABEL_CAP));
            if row(ui, &text, false).clicked() {
                picked = Some(Picked::Custom(model.to_owned()));
            }
        }
        if !visible.is_empty() {
            heading(ui, &t!("picker.models_heading"));
            let list = (ui.available_height() - note_height).max(ROW_HEIGHT);
            egui::ScrollArea::vertical()
                .id_salt(state.with("list"))
                .max_height(list)
                .min_scrolled_height(list)
                .auto_shrink([false, false])
                .show_rows(ui, ROW_HEIGHT, rows.len(), |ui, range| {
                    for r in rows.get(range).unwrap_or_default() {
                        match r {
                            Row::Heading(group) => heading(ui, group),
                            Row::Group { name, count, open } => {
                                let response = group_row(ui, name, *count, *open);
                                if response.clicked() {
                                    toggled = Some(((*name).to_owned(), !*open));
                                }
                            }
                            Row::Choice(choice) => {
                                let response = row(ui, &compact(choice.label, LABEL_CAP), choice.value == p.current_value);
                                let response = if choice.tip.is_empty() {
                                    response
                                } else {
                                    response.on_hover_text(compact(choice.tip, NOTE_CAP))
                                };
                                if response.clicked() {
                                    picked = Some(Picked::Listed(choice.value.to_owned()));
                                }
                            }
                        }
                    }
                });
        }
        if let Some((group, open)) = toggled {
            let mut folds = folds;
            folds.insert(group, open);
            ui.data_mut(|data| data.insert_temp(folds_id, folds));
            // The click took the focus from the box; typing goes on filtering.
            search.request_focus();
        }
        if let Some(note) = note {
            let response = ui.add(egui::Label::new(note));
            if let Some(whole) = p.note.filter(|whole| whole.chars().count() > 200) {
                response.on_hover_text(compact(whole, NOTE_CAP));
            }
        }
    });
    if picked.is_some() {
        ui.close();
    }
    picked
}

enum Row<'a> {
    /// A lone group's dim heading.
    Heading(&'a str),
    /// One of several groups: its header, with how many of its choices are listed, folded or open.
    Group { name: &'a str, count: usize, open: bool },
    Choice(&'a Choice<'a>),
}

/// The groups among `choices` in order of first appearance, each with its choices; `None` is
/// the choices with no group.
fn groups<'a, 'c, I>(choices: I) -> Vec<(Option<&'a str>, Vec<&'c Choice<'a>>)>
where
    'a: 'c,
    I: IntoIterator<Item = &'c Choice<'a>>,
{
    let mut groups: Vec<(Option<&'a str>, Vec<&'c Choice<'a>>)> = Vec::new();
    for choice in choices {
        let group = choice.group.filter(|g| !g.is_empty());
        match groups.iter_mut().find(|(name, _)| *name == group) {
            Some((_, members)) => members.push(choice),
            None => groups.push((group, vec![choice])),
        }
    }
    groups
}

/// The list's rows: each group's choices under its header, which folds them when there are
/// several groups and is a plain heading when there is one; choices with no group as they are.
fn rows<'a>(visible: &[&'a Choice<'a>], grouped: bool, is_open: &dyn Fn(&str) -> bool) -> Vec<Row<'a>> {
    let mut rows = Vec::with_capacity(visible.len());
    for (group, members) in groups(visible.iter().copied()) {
        match group {
            Some(name) if grouped => {
                let open = is_open(name);
                rows.push(Row::Group { name, count: members.len(), open });
                if open {
                    rows.extend(members.into_iter().map(Row::Choice));
                }
            }
            Some(name) => {
                rows.push(Row::Heading(name));
                rows.extend(members.into_iter().map(Row::Choice));
            }
            None => rows.extend(members.into_iter().map(Row::Choice)),
        }
    }
    rows
}

/// The header of one of several groups: a chevron, the name and how many of its choices are
/// listed. A click folds or opens the group.
fn group_row(ui: &mut Ui, name: &str, count: usize, open: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let visuals = ui.visuals();
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, 3.0, visuals.widgets.hovered.weak_bg_fill);
    }
    let chevron = if open { "⏷" } else { "⏵" };
    let text = format!("{chevron} {}  ·  {count}", compact(&name.to_uppercase(), LABEL_CAP));
    let galley = painter.layout_no_wrap(text, egui::FontId::proportional(12.0), visuals.weak_text_color());
    painter.galley(egui::pos2(rect.left() + 6.0, rect.center().y - galley.size().y / 2.0), galley, visuals.weak_text_color());
    response.on_hover_text(t!("picker.group_models", count = count))
}

/// A row: read from the left, as a list of ids is.
fn row(ui: &mut Ui, text: &str, selected: bool) -> Response {
    ui.add_sized(vec2(ui.available_width(), ROW_HEIGHT), egui::Button::selectable(selected, (text, egui::Atom::grow())))
}

/// A dim one-line heading in the popup.
fn heading(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(compact(text, LABEL_CAP)).small().weak()).truncate());
}

/// The popup's width: the control's, wide enough for the longest label and to search in, and
/// never wider than the window.
fn popup_width(ui: &Ui, p: &Picker<'_>) -> f32 {
    let longest =
        p.choices.iter().map(|c| c.label).max_by_key(|label| label.chars().take(LABEL_CAP + 1).count()).unwrap_or("");
    let text = ui
        .painter()
        .layout_no_wrap(compact(longest, LABEL_CAP), egui::TextStyle::Button.resolve(ui.style()), egui::Color32::WHITE)
        .size()
        .x;
    let wanted = text + ui.spacing().button_padding.x * 2.0 + ui.spacing().scroll.bar_width + 12.0;
    let most = (ui.ctx().content_rect().width() - 48.0).max(0.0);
    wanted.max(ui.available_width()).max(220.0).min(most)
}

/// `text` cut to `max_chars`, with an ellipsis if anything was left out.
pub fn compact(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let short: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() { format!("{short}…") } else { short }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice<'a>(value: &'a str, group: Option<&'a str>) -> Choice<'a> {
        Choice { value, label: value, tip: "", group }
    }

    #[test]
    fn several_groups_fold_and_one_is_a_heading() {
        let choices = [choice("a/1", Some("A")), choice("a/2", Some("A")), choice("b/1", Some("B")), choice("x", None)];
        let visible: Vec<&Choice<'_>> = choices.iter().collect();
        let only_a = |g: &str| g == "A";
        let listed = rows(&visible, true, &only_a);
        let kinds: Vec<String> = listed
            .iter()
            .map(|r| match r {
                Row::Heading(h) => format!("h:{h}"),
                Row::Group { name, count, open } => format!("g:{name}:{count}:{open}"),
                Row::Choice(c) => format!("c:{}", c.value),
            })
            .collect();
        assert_eq!(kinds, ["g:A:2:true", "c:a/1", "c:a/2", "g:B:1:false", "c:x"]);
        let one = [choice("a/1", Some("A")), choice("x", None)];
        let visible: Vec<&Choice<'_>> = one.iter().collect();
        assert!(matches!(rows(&visible, false, &|_| true)[0], Row::Heading("A")));
        assert_eq!(compact("abcdef", 3), "abc…");
        assert_eq!(compact("abc", 3), "abc");
    }
}
