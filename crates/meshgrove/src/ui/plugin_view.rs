//! A plugin panel's widget tree ([`View`]) drawn with Meshgrove's widgets; what the user does
//! comes back as [`Event`]s for the plugin, which answers with a new tree. What is being typed
//! or dragged stays on screen until the plugin rendered anew; then the plugin's values count.
//!
//! The grid editor and the canvas are host widgets of their own ([`super::grid_editor`],
//! [`super::canvas`]).

use std::collections::HashMap;

use base64::Engine as _;
use egui::{Align, Color32, Layout, Rect, RichText, Sense, Ui, UiBuilder, pos2, vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use meshgrove_core::config::{DisplayUnits, display_units};
use meshgrove_plugin_api::ui::{
    Choice, Event, FormRow, Level, Node, Tab, TableColumn, TableRow, TextStyle, Unit, View,
};
use serde_json::{Value, json};

/// What a panel keeps between frames.
#[derive(Default)]
pub struct PanelUiState {
    /// Values edited here and not rendered by the plugin yet, by input id.
    pending: HashMap<String, Value>,
    /// The text input with the keyboard focus (its text survives a new view).
    typing: Option<String>,
    /// The number field whose typing Escape just dropped.
    escaped: Option<String>,
    /// Decoded pictures, by a hash of their data.
    textures: HashMap<u64, egui::TextureHandle>,
    markdown: CommonMarkCache,
}

impl PanelUiState {
    /// The plugin rendered anew: its values count again, except for the text being typed.
    pub fn new_view(&mut self) {
        let typing = self.typing.clone();
        self.pending.retain(|id, _| Some(id) == typing.as_ref());
    }
}

/// A 3D view the panel wants: drawn by the caller, who has the meshes and the renderer.
pub struct MeshViewArgs<'a> {
    pub key: &'a str,
    pub height: f32,
    /// A printer bed (x, y in mm) to show under the model.
    pub bed: Option<[f32; 2]>,
    /// The space the model is meant to fit (minimum and maximum corner, in the coordinates of
    /// the plugin's triangles).
    pub frame: Option<[[f32; 3]; 2]>,
}

/// Draws `view`; `meshes` draws the panel's 3D views. Returns what the user did.
pub fn show(
    ui: &mut Ui,
    view: &View,
    state: &mut PanelUiState,
    meshes: &mut dyn FnMut(&mut Ui, MeshViewArgs),
) -> Vec<Event> {
    let mut draw = Draw { state, meshes, events: Vec::new(), units: display_units() };
    draw.node(ui, &view.root);
    draw.events
}

struct Draw<'a> {
    state: &'a mut PanelUiState,
    meshes: &'a mut dyn FnMut(&mut Ui, MeshViewArgs),
    events: Vec<Event>,
    units: DisplayUnits,
}

/// Nodes holding others (they keep a scope of their own in a row, for stable ids).
fn is_container(node: &Node) -> bool {
    matches!(
        node,
        Node::Column { .. }
            | Node::Row { .. }
            | Node::Group { .. }
            | Node::Tabs { .. }
            | Node::Split { .. }
            | Node::Scroll { .. }
            | Node::Form { .. }
            | Node::Table { .. }
            | Node::GridEditor(_)
            | Node::Canvas(_)
            | Node::MeshView { .. }
    )
}

fn changed(id: &str, value: Value) -> Event {
    Event::Changed { id: id.to_owned(), value }
}

/// `#rrggbb` or `#rrggbbaa`.
pub fn parse_color(text: &str) -> Option<Color32> {
    let hex = text.strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    match hex.len() {
        6 => Some(Color32::from_rgb(byte(0)?, byte(2)?, byte(4)?)),
        8 => Some(Color32::from_rgba_unmultiplied(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
        _ => None,
    }
}

fn hex(c: Color32) -> String {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    if a == 255 { format!("#{r:02x}{g:02x}{b:02x}") } else { format!("#{r:02x}{g:02x}{b:02x}{a:02x}") }
}

fn level_color(ui: &Ui, level: Level) -> Color32 {
    match level {
        Level::Info => ui.visuals().hyperlink_color,
        Level::Success => Color32::from_rgb(60, 160, 80),
        Level::Warn => ui.visuals().warn_fg_color,
        Level::Error => ui.visuals().error_fg_color,
    }
}

impl Draw<'_> {
    fn children(&mut self, ui: &mut Ui, children: &[Node]) {
        for (i, child) in children.iter().enumerate() {
            ui.push_id(i, |ui| self.node(ui, child));
        }
    }

    fn node(&mut self, ui: &mut Ui, node: &Node) {
        match node {
            Node::Column { children } => {
                ui.vertical(|ui| self.children(ui, children));
            }
            Node::Row { children } => {
                ui.horizontal_wrapped(|ui| {
                    // A button too wide for the rest of the line goes to the next one whole. Only
                    // what lies in the row itself wraps: plain widgets go in without a scope of
                    // their own (containers keep theirs, for their ids).
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                    for (i, child) in children.iter().enumerate() {
                        if is_container(child) {
                            ui.push_id(i, |ui| self.node(ui, child));
                        } else {
                            self.node(ui, child);
                        }
                    }
                });
            }
            Node::Group { title, collapsible, children } => match title {
                Some(title) if *collapsible => {
                    egui::CollapsingHeader::new(RichText::new(title).strong())
                        .id_salt("group")
                        .default_open(true)
                        .show(ui, |ui| self.children(ui, children));
                }
                _ => {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if let Some(title) = title {
                            ui.label(RichText::new(title).strong());
                        }
                        self.children(ui, children);
                    });
                }
            },
            Node::Tabs { id, selected, tabs } => self.tabs(ui, id, *selected, tabs),
            Node::Split { vertical, ratio, first, second } => self.split(ui, *vertical, *ratio, first, second),
            Node::Scroll { max_height, children } => {
                let area = egui::ScrollArea::vertical().id_salt("scroll");
                let area = match max_height {
                    Some(h) => area.max_height(*h).auto_shrink([false, true]),
                    None => area.auto_shrink([false, false]),
                };
                area.show(ui, |ui| self.children(ui, children));
            }
            Node::Label { text, style } => {
                let text = RichText::new(text);
                ui.label(match style {
                    TextStyle::Body => text,
                    TextStyle::Heading => text.heading(),
                    TextStyle::Strong => text.strong(),
                    TextStyle::Small => text.small(),
                    TextStyle::Weak => text.weak(),
                    TextStyle::Mono => text.monospace(),
                });
            }
            Node::Markdown { text } => {
                CommonMarkViewer::new().show(ui, &mut self.state.markdown, text);
            }
            Node::Separator => {
                ui.separator();
            }
            Node::Image { data, width, alt, .. } => self.image(ui, data, *width, alt.as_deref()),
            Node::Notice { level, text } => {
                let color = level_color(ui, *level);
                egui::Frame::new()
                    .stroke(egui::Stroke::new(1.0, color))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let icon = match level {
                            Level::Info => "ℹ",
                            Level::Success => "✔",
                            Level::Warn => "⚠",
                            Level::Error => "✖",
                        };
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(color, icon);
                            ui.label(text);
                        });
                    });
            }
            Node::Progress { fraction, text } => {
                let bar = egui::ProgressBar::new(fraction.unwrap_or(0.0).clamp(0.0, 1.0)).animate(fraction.is_none());
                let bar = match text {
                    Some(t) => bar.text(t.as_str()),
                    None if fraction.is_some() => bar.show_percentage(),
                    None => bar,
                };
                ui.add(bar);
                if fraction.is_none() {
                    ui.ctx().request_repaint();
                }
            }
            Node::Button { id, label, primary, enabled } => {
                let mut button = egui::Button::new(label.as_str());
                if *primary {
                    button = button.fill(ui.visuals().selection.bg_fill);
                }
                if ui.add_enabled(*enabled, button).clicked() {
                    self.events.push(Event::Click { id: id.clone() });
                }
            }
            Node::TextInput { id, value, hint, multiline, enabled } => {
                self.text_input(ui, id, value, hint.as_deref(), *multiline, *enabled);
            }
            Node::NumberInput { id, value, min, max, step, unit, decimals, enabled } => {
                self.number(ui, id, *value, (*min, *max, *step), *unit, *decimals, *enabled);
            }
            Node::Slider { id, value, min, max, step, enabled } => {
                let mut v = self.pending_f64(id).unwrap_or(*value);
                let mut slider = egui::Slider::new(&mut v, *min..=*max);
                if let Some(step) = step {
                    slider = slider.step_by(*step);
                }
                let r = ui.add_enabled(*enabled, slider);
                self.after_drag(id, v, &r);
            }
            Node::Checkbox { id, label, checked, enabled } => {
                let mut on = self.state.pending.get(id).and_then(Value::as_bool).unwrap_or(*checked);
                if ui.add_enabled(*enabled, egui::Checkbox::new(&mut on, label.as_str())).changed() {
                    self.state.pending.insert(id.clone(), json!(on));
                    self.events.push(changed(id, json!(on)));
                }
            }
            Node::Select { id, options, selected, enabled } => self.select(ui, id, options, selected, *enabled),
            Node::Radio { id, options, selected } => {
                let current = self.pending_str(id).unwrap_or_else(|| selected.clone());
                ui.horizontal_wrapped(|ui| {
                    for o in options {
                        if ui.radio(current == o.value, o.label.as_str()).clicked() && current != o.value {
                            self.state.pending.insert(id.clone(), json!(o.value));
                            self.events.push(changed(id, json!(o.value)));
                        }
                    }
                });
            }
            Node::Color { id, value } => {
                let current = self.pending_str(id).unwrap_or_else(|| value.clone());
                let mut color = parse_color(&current).unwrap_or(Color32::GRAY);
                let r =
                    egui::color_picker::color_edit_button_srgba(ui, &mut color, egui::color_picker::Alpha::OnlyBlend);
                if r.changed() {
                    self.state.pending.insert(id.clone(), json!(hex(color)));
                    self.events.push(changed(id, json!(hex(color))));
                }
            }
            Node::Form { id, rows, submit } => self.form(ui, id, rows, submit.as_deref()),
            Node::Table { id, columns, rows, selected, max_height } => {
                self.table(ui, id, columns, rows, selected.as_deref(), *max_height);
            }
            Node::GridEditor(grid) => {
                // The selection made here, until the plugin renders it.
                let mut selection =
                    self.state.pending.get(&grid.id).and_then(|v| serde_json::from_value(v.clone()).ok());
                let changes = super::grid_editor::show(ui, grid, &mut selection);
                if let Some(selection) = selection {
                    self.state.pending.insert(grid.id.clone(), json!(selection));
                }
                self.events.extend(changes.into_iter().map(|change| Event::Grid { id: grid.id.clone(), change }));
            }
            Node::Canvas(canvas) => {
                let changes = super::canvas::show(ui, canvas);
                self.events.extend(changes.into_iter().map(|change| Event::Canvas { id: canvas.id.clone(), change }));
            }
            Node::MeshView { key, height, bed, frame, .. } => {
                let height = height.unwrap_or_else(|| ui.available_height().max(200.0));
                (self.meshes)(ui, MeshViewArgs { key, height, bed: *bed, frame: *frame });
            }
            Node::Unknown => {
                ui.label(RichText::new(t!("plugins.unknown_widget")).weak().italics());
            }
        }
    }

    fn pending_f64(&self, id: &str) -> Option<f64> {
        self.state.pending.get(id).and_then(Value::as_f64)
    }

    fn pending_str(&self, id: &str) -> Option<String> {
        self.state.pending.get(id).and_then(Value::as_str).map(str::to_owned)
    }

    /// A dragged value is kept here while dragging and sent when let go; a changed one at once.
    /// Nothing is kept while the value is being typed: a value that moves under a `DragValue`
    /// makes it drop the text typed so far.
    fn after_drag(&mut self, id: &str, value: f64, r: &egui::Response) {
        if r.dragged() || r.changed() {
            self.state.pending.insert(id.to_owned(), json!(value));
        }
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            self.events.push(changed(id, json!(value)));
        }
    }

    fn tabs(&mut self, ui: &mut Ui, id: &str, selected: usize, tabs: &[Tab]) {
        let current = self.state.pending.get(id).and_then(Value::as_u64).map_or(selected, |v| v as usize);
        ui.horizontal(|ui| {
            for (i, tab) in tabs.iter().enumerate() {
                if ui.selectable_label(i == current, tab.label.as_str()).clicked() && i != current {
                    self.state.pending.insert(id.to_owned(), json!(i));
                    self.events.push(changed(id, json!(i)));
                }
            }
        });
        ui.separator();
        if let Some(tab) = tabs.get(current) {
            ui.push_id(("tab", current), |ui| self.children(ui, &tab.children));
        }
    }

    /// Two parts with a border the user can drag; the share is kept by egui for this split.
    fn split(&mut self, ui: &mut Ui, vertical: bool, ratio: f32, first: &Node, second: &Node) {
        const GAP: f32 = 8.0;
        let rect = ui.available_rect_before_wrap();
        let id = ui.scope_id().with("split");
        let mut ratio = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(ratio).clamp(0.1, 0.9);
        let length = if vertical { rect.height() } else { rect.width() } - GAP;
        let first_len = (length * ratio).max(40.0);
        let (first_rect, handle, second_rect) = if vertical {
            let handle = Rect::from_min_size(pos2(rect.min.x, rect.min.y + first_len), vec2(rect.width(), GAP));
            (
                Rect::from_min_max(rect.min, pos2(rect.max.x, handle.min.y)),
                handle,
                Rect::from_min_max(pos2(rect.min.x, handle.max.y), rect.max),
            )
        } else {
            let handle = Rect::from_min_size(pos2(rect.min.x + first_len, rect.min.y), vec2(GAP, rect.height()));
            (
                Rect::from_min_max(rect.min, pos2(handle.min.x, rect.max.y)),
                handle,
                Rect::from_min_max(pos2(handle.max.x, rect.min.y), rect.max),
            )
        };
        let r = ui.interact(handle, id.with("handle"), Sense::drag());
        if r.dragged() && length > 0.0 {
            let delta = if vertical { r.drag_delta().y } else { r.drag_delta().x };
            ratio = ((first_len + delta) / length).clamp(0.1, 0.9);
            ui.data_mut(|d| d.insert_temp(id, ratio));
        }
        if r.hovered() || r.dragged() {
            let icon = if vertical { egui::CursorIcon::ResizeVertical } else { egui::CursorIcon::ResizeHorizontal };
            ui.ctx().set_cursor_icon(icon);
        }
        let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
        if vertical {
            ui.painter().hline(handle.x_range(), handle.center().y, stroke);
        } else {
            ui.painter().vline(handle.center().x, handle.y_range(), stroke);
        }
        for (i, (part, node)) in [(first_rect, first), (second_rect, second)].into_iter().enumerate() {
            let mut child =
                ui.new_child(UiBuilder::new().id_salt(("part", i)).max_rect(part).layout(Layout::top_down(Align::Min)));
            child.set_clip_rect(part.intersect(ui.clip_rect()));
            // What does not fit across (a wide form in a narrow part) can be scrolled to.
            egui::ScrollArea::horizontal()
                .id_salt(("part-scroll", i))
                .auto_shrink([false, false])
                .scroll_source(egui::containers::scroll_area::ScrollSource::SCROLL_BAR)
                .show(&mut child, |ui| self.node(ui, node));
        }
        ui.advance_cursor_after_rect(rect);
    }

    fn image(&mut self, ui: &mut Ui, data: &str, width: Option<f32>, alt: Option<&str>) {
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            data.hash(&mut h);
            h.finish()
        };
        let texture = match self.state.textures.entry(key) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(slot) => {
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(data.trim())
                    .ok()
                    .and_then(|bytes| image::load_from_memory(&bytes).ok())
                    .map(|img| {
                        let img = img.to_rgba8();
                        let size = [img.width() as usize, img.height() as usize];
                        egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw())
                    });
                let Some(img) = decoded else {
                    ui.label(RichText::new(alt.unwrap_or("(?)")).weak());
                    return;
                };
                slot.insert(ui.ctx().load_texture(format!("plugin-image-{key}"), img, egui::TextureOptions::LINEAR))
            }
        };
        let mut image = egui::Image::from_texture(egui::load::SizedTexture::from_handle(texture));
        image = image.max_width(width.unwrap_or_else(|| ui.available_width()));
        let r = ui.add(image);
        if let Some(alt) = alt {
            r.on_hover_text(alt);
        }
    }

    fn text_input(&mut self, ui: &mut Ui, id: &str, value: &str, hint: Option<&str>, multiline: bool, enabled: bool) {
        let mut text = self.pending_str(id).unwrap_or_else(|| value.to_owned());
        let edit = if multiline { egui::TextEdit::multiline(&mut text) } else { egui::TextEdit::singleline(&mut text) };
        let edit = edit.hint_text(hint.unwrap_or_default()).desired_width(f32::INFINITY);
        let r = ui.add_enabled(enabled, edit);
        if r.has_focus() {
            self.state.typing = Some(id.to_owned());
        } else if self.state.typing.as_deref() == Some(id) {
            self.state.typing = None;
        }
        if r.changed() {
            self.state.pending.insert(id.to_owned(), json!(text));
        }
        // Sent when the user is done: Enter or leaving the field.
        let done = r.lost_focus() || (!multiline && r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        if done && text != value {
            self.events.push(changed(id, json!(text)));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn number(
        &mut self,
        ui: &mut Ui,
        id: &str,
        value: f64,
        (min, max, step): (Option<f64>, Option<f64>, Option<f64>),
        unit: Unit,
        decimals: Option<u8>,
        enabled: bool,
    ) {
        // Lengths show in the user's unit and are sent in millimetres.
        let scale = if unit == Unit::Mm && self.units == DisplayUnits::Imperial { 25.4 } else { 1.0 };
        let suffix = match unit {
            Unit::Mm if scale != 1.0 => " in",
            Unit::Mm => " mm",
            Unit::Degrees => "°",
            Unit::Percent => " %",
            Unit::None => "",
        };
        let mut shown = self.pending_f64(id).unwrap_or(value) / scale;
        // A typed value counts when the user is done (Enter, Tab or leaving the field; Escape
        // drops it): not clamped after the first digit, and sent once instead of per keystroke.
        let mut drag = egui::DragValue::new(&mut shown)
            .suffix(suffix)
            .speed(step.unwrap_or(0.1) / scale)
            .update_while_editing(false);
        if min.is_some() || max.is_some() {
            drag = drag.range(min.unwrap_or(f64::MIN) / scale..=max.unwrap_or(f64::MAX) / scale);
        }
        let decimals = decimals.map(usize::from).map(|d| if scale != 1.0 { d + 2 } else { d });
        drag = drag.max_decimals_opt(decimals);
        let r = ui.add_enabled(enabled, drag);
        // Escape drops the text, but egui takes it a frame later all the same (the field has
        // lost its focus again then, without the key): that change is not the user's.
        if self.state.escaped.as_deref() == Some(id) {
            self.state.escaped = None;
            if r.changed() {
                return;
            }
        }
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.state.escaped = Some(id.to_owned());
        }
        self.after_drag(id, shown * scale, &r);
    }

    fn select(&mut self, ui: &mut Ui, id: &str, options: &[Choice], selected: &str, enabled: bool) {
        let current = self.pending_str(id).unwrap_or_else(|| selected.to_owned());
        let label = options.iter().find(|o| o.value == current).map_or(current.as_str(), |o| o.label.as_str());
        ui.add_enabled_ui(enabled, |ui| {
            // No wider than the row: long choices are cut short (the list shows them whole).
            let width = ui.available_width().clamp(80.0, 240.0);
            egui::ComboBox::from_id_salt(id).selected_text(label).width(width).truncate().show_ui(ui, |ui| {
                for o in options {
                    if ui.selectable_label(o.value == current, o.label.as_str()).clicked() && o.value != current {
                        self.state.pending.insert(id.to_owned(), json!(o.value));
                        self.events.push(changed(id, json!(o.value)));
                    }
                }
            });
        });
    }

    fn form(&mut self, ui: &mut Ui, id: &str, rows: &[FormRow], submit: Option<&str>) {
        // The values get what the longest label leaves of the width, and wrap in it: a long
        // checkbox label must not push the form (and what follows it) out of its part.
        const GAP: f32 = 12.0;
        let font = egui::TextStyle::Body.resolve(ui.style());
        let color = ui.visuals().text_color();
        let labels = rows
            .iter()
            .map(|r| ui.fonts_mut(|f| f.layout_no_wrap(r.label.clone(), font.clone(), color).size().x))
            .fold(0.0, f32::max);
        let values = ui.available_width() - labels - GAP;
        if values < 100.0 {
            // Too narrow beside each other: each label above its input.
            for (i, row) in rows.iter().enumerate() {
                ui.label(RichText::new(row.label.as_str()).small().weak());
                ui.push_id(i, |ui| self.node(ui, &row.node));
                ui.add_space(4.0);
            }
        } else {
            self.form_grid(ui, id, rows, values);
        }
        if let Some(label) = submit
            && ui.button(label).clicked()
        {
            let values = rows.iter().filter_map(|row| self.value_of(&row.node)).collect();
            self.events.push(Event::Submit { id: id.to_owned(), values });
        }
    }

    fn form_grid(&mut self, ui: &mut Ui, id: &str, rows: &[FormRow], values: f32) {
        const GAP: f32 = 12.0;
        egui::Grid::new(("form", id)).num_columns(2).spacing([GAP, 6.0]).show(ui, |ui| {
            for (i, row) in rows.iter().enumerate() {
                ui.label(row.label.as_str());
                ui.push_id(i, |ui| {
                    ui.set_max_width(values);
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                    self.node(ui, &row.node);
                });
                ui.end_row();
            }
        });
    }

    /// An input's id and value as shown (edited or rendered).
    fn value_of(&self, node: &Node) -> Option<(String, Value)> {
        let (id, rendered) = match node {
            Node::TextInput { id, value, .. } => (id, json!(value)),
            Node::NumberInput { id, value, .. } | Node::Slider { id, value, .. } => (id, json!(value)),
            Node::Checkbox { id, checked, .. } => (id, json!(checked)),
            Node::Select { id, selected, .. } | Node::Radio { id, selected, .. } => (id, json!(selected)),
            Node::Color { id, value } => (id, json!(value)),
            _ => return None,
        };
        Some((id.clone(), self.state.pending.get(id).cloned().unwrap_or(rendered)))
    }

    fn table(
        &mut self,
        ui: &mut Ui,
        id: &str,
        columns: &[TableColumn],
        rows: &[TableRow],
        selected: Option<&str>,
        max_height: Option<f32>,
    ) {
        let current = match self.state.pending.get(id) {
            Some(v) => v.as_str().map(str::to_owned),
            None => selected.map(str::to_owned),
        };
        let mut table = egui_extras::TableBuilder::new(ui)
            .id_salt(("table", id))
            .striped(true)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .max_scroll_height(max_height.unwrap_or(f32::INFINITY));
        // Columns without a width of the plugin's can be dragged wider at their right edge, so
        // a value cut short can be read whole (or on hover); the last one takes what is left.
        for (i, c) in columns.iter().enumerate() {
            table = table.column(match c.width {
                Some(w) => egui_extras::Column::exact(w),
                None if i + 1 == columns.len() => egui_extras::Column::remainder(),
                None => egui_extras::Column::auto().at_least(40.0).resizable(true),
            });
        }
        let mut clicked = None;
        table
            .header(20.0, |mut header| {
                for c in columns {
                    header.col(|ui| {
                        ui.strong(c.title.as_str());
                    });
                }
            })
            .body(|body| {
                body.rows(20.0, rows.len(), |mut row| {
                    let r = &rows[row.index()];
                    row.set_selected(current.as_deref() == Some(r.id.as_str()));
                    for (c, cell) in columns.iter().zip(&r.cells) {
                        // Not selectable: a click on the text picks the row.
                        let label = egui::Label::new(cell.as_str()).truncate().selectable(false);
                        row.col(|ui| {
                            if c.numeric {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.add(label));
                            } else {
                                ui.add(label);
                            }
                        });
                    }
                    if row.response().clicked() {
                        clicked = Some(r.id.clone());
                    }
                });
            });
        if let Some(row) = clicked {
            let row = (current.as_deref() != Some(row.as_str())).then_some(row);
            self.state.pending.insert(id.to_owned(), json!(row));
            self.events.push(Event::TableSelect { id: id.to_owned(), row });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_are_read_and_written_as_hex() {
        assert_eq!(parse_color("#ff8000"), Some(Color32::from_rgb(255, 128, 0)));
        assert_eq!(parse_color("#ff800080").map(|c| c.a()), Some(128));
        assert_eq!(parse_color("ff8000"), None);
        assert_eq!(hex(Color32::from_rgb(1, 2, 3)), "#010203");
    }

    #[test]
    fn a_new_view_drops_what_was_sent_but_keeps_what_is_typed() {
        let mut state = PanelUiState::default();
        state.pending.insert("width".into(), json!(33.5));
        state.pending.insert("name".into(), json!("Kis"));
        state.typing = Some("name".into());
        state.new_view();
        assert!(!state.pending.contains_key("width"));
        assert_eq!(state.pending["name"], "Kis");
    }

    /// Draws every widget once in a headless egui and clicks nothing: no panics, no events.
    #[test]
    fn every_widget_draws() {
        let json = serde_json::json!({"v": 1, "root": {"type": "split", "first": {"type": "scroll", "children": [
            {"type": "form", "id": "f", "submit": "Los", "rows": [
                {"label": "Breite", "node": {"type": "number_input", "id": "w", "value": 20.0, "unit": "mm"}},
                {"label": "Name", "node": {"type": "text_input", "id": "n", "value": "x"}},
                {"label": "Art", "node": {"type": "select", "id": "s", "selected": "a", "options": [{"value": "a", "label": "A"}]}},
                {"label": "An", "node": {"type": "checkbox", "id": "c", "label": "an", "checked": true}}]},
            {"type": "table", "id": "t", "columns": [{"title": "Teil"}, {"title": "Anzahl", "numeric": true}],
             "rows": [{"id": "r1", "cells": ["Bin", "3"]}]},
            {"type": "tabs", "id": "tabs", "tabs": [{"label": "Eins", "children": [{"type": "label", "text": "1"}]}]},
            {"type": "group", "title": "G", "collapsible": true, "children": [{"type": "markdown", "text": "**fett**"}]},
            {"type": "notice", "level": "warn", "text": "Achtung"},
            {"type": "progress", "fraction": 0.5},
            {"type": "radio", "id": "r", "selected": "a", "options": [{"value": "a", "label": "A"}, {"value": "b", "label": "B"}]},
            {"type": "slider", "id": "sl", "value": 1.0, "min": 0.0, "max": 2.0},
            {"type": "color", "id": "col", "value": "#ff0000"},
            {"type": "grid_editor", "id": "g", "cols": 3, "rows": 2, "pitch": 42.0,
             "zones": [{"id": "z", "x": -0.5, "y": 0.0, "w": 0.5, "h": 2.0, "label": "21 mm", "accepts_items": true}],
             "items": [{"id": "0", "x": 0.0, "y": 0.0, "w": 2.0, "h": 1.0, "label": "Bin", "badges": ["6u"]}]},
            {"type": "canvas", "id": "c", "width": 100.0, "height": 50.0, "interactive": true, "shapes": [
                {"shape": "polygon", "points": [[0, 0], [40, 0], [40, 10], [10, 10], [10, 40], [0, 40]], "fill": "#80808080"},
                {"shape": "rect", "x": 50, "y": 5, "w": 20, "h": 20, "id": "r"},
                {"shape": "circle", "x": 80, "y": 30, "r": 8},
                {"shape": "line", "points": [[0, 45], [100, 45]]},
                {"shape": "text", "x": 50, "y": 45, "text": "Silhouette"}]},
            {"type": "something_new"},
            {"type": "button", "id": "b", "label": "Go", "primary": true}]},
          "second": {"type": "mesh_view", "id": "m", "key": "preview", "revision": 1}}});
        let view: View = serde_json::from_value(json).unwrap();
        let ctx = egui::Context::default();
        let mut state = PanelUiState::default();
        let mut keys = Vec::new();
        let mut events = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            events = show(ui, &view, &mut state, &mut |_, args| keys.push(args.key.to_owned()));
        });
        // The font atlas was never uploaded anywhere.
        output.textures_delta.clear();
        // egui may run a first frame twice (to size things).
        assert!(!keys.is_empty() && keys.iter().all(|k| k == "preview"), "{keys:?}");
        assert!(events.is_empty(), "{events:?}");
    }

    type Shapes = Vec<egui::epaint::ClippedShape>;

    /// Runs `view` in a headless egui of `width` × 600 points: two frames to settle, then one
    /// for each input of `steps`, which sees what the last settling frame drew. The events the
    /// view sent, and what the last frame drew.
    fn drive(view: &View, width: f32, steps: impl FnOnce(&Shapes) -> Vec<Vec<egui::Event>>) -> (Vec<Event>, Shapes) {
        let ctx = egui::Context::default();
        let mut state = PanelUiState::default();
        let mut events = Vec::new();
        let mut time = 0.0;
        let mut frame = |input: Vec<egui::Event>, events: &mut Vec<Event>| {
            time += 0.05;
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(width, 600.0))),
                time: Some(time),
                events: input,
                ..Default::default()
            };
            let mut output = ctx.run_ui(raw, |ui| {
                // A 3D view takes its space and the pointer, as the real one does.
                let mut mesh = |ui: &mut Ui, args: MeshViewArgs| {
                    ui.allocate_exact_size(vec2(ui.available_width(), args.height), Sense::click_and_drag());
                };
                events.extend(show(ui, view, &mut state, &mut mesh));
            });
            output.textures_delta.clear();
            output.shapes
        };
        frame(Vec::new(), &mut events);
        let mut shapes = frame(Vec::new(), &mut events);
        for input in steps(&shapes) {
            shapes = frame(input, &mut events);
        }
        (events, shapes)
    }

    fn button(at: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }
    }

    /// Clicks through egui: a frame with the press, one with the release; the events the view sent.
    fn click_at(view: &View, p: egui::Pos2) -> Vec<Event> {
        drive(view, 400.0, |_| vec![vec![egui::Event::PointerMoved(p), button(p, true)], vec![button(p, false)]]).0
    }

    /// The texts a frame drew: each with its place and whether it was cut short.
    fn texts(shapes: &Shapes) -> Vec<(String, Rect, bool)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, Rect, bool)>) {
            match shape {
                egui::Shape::Text(t) => {
                    out.push((t.galley.text().to_owned(), t.visual_bounding_rect(), t.galley.elided))
                }
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        shapes.iter().for_each(|s| walk(&s.shape, &mut out));
        out
    }

    /// A number field with a minimum, typed into: nothing while typing (no clamping to 42 after
    /// the "2"), the whole number once on Enter.
    #[test]
    fn typed_numbers_are_sent_once_when_done() {
        let json = serde_json::json!({"v": 1, "root": {"type": "number_input", "id": "w", "value": 42.0,
            "min": 42.0, "max": 500.0, "unit": "mm"}});
        let view: View = serde_json::from_value(json).unwrap();
        let at = egui::pos2(20.0, 17.0);
        let typed = |last: egui::Event| {
            drive(&view, 400.0, |_| {
                vec![
                    vec![egui::Event::PointerMoved(at), button(at, true)],
                    vec![button(at, false)],
                    // The field takes the keyboard and selects its text.
                    Vec::new(),
                    vec![egui::Event::Text("2".into())],
                    vec![egui::Event::Text("50".into())],
                    vec![last],
                    Vec::new(),
                    Vec::new(),
                ]
            })
        };
        let (events, _) = typed(key(egui::Key::Enter));
        assert_eq!(events, [Event::Changed { id: "w".into(), value: json!(250.0) }]);
        let (events, shapes) = typed(key(egui::Key::Escape));
        assert_eq!(events, [], "Escape drops what was typed");
        assert!(texts(&shapes).iter().any(|(t, ..)| t.starts_with("42")), "{:?}", texts(&shapes));
    }

    /// The sizes column of a narrow parts table cuts its values short: hovering shows one whole,
    /// and dragging the column's right edge makes room for it.
    #[test]
    fn cut_short_cells_can_be_read() {
        const SIZE: &str = "250.0 × 305.0 × 7.0 mm";
        let json = serde_json::json!({"v": 1, "root": {"type": "table", "id": "parts",
            "columns": [{"title": "Teil"}, {"title": "mm"}, {"title": "Stk.", "width": 36.0, "numeric": true}],
            "rows": [{"id": "0", "cells": ["baseplate_1A_5x4.stl", SIZE, "1"]}]}});
        let view: View = serde_json::from_value(json).unwrap();
        let cell = |shapes: &Shapes| texts(shapes).into_iter().find(|(t, _, elided)| t == SIZE && *elided);
        let whole = |shapes: &Shapes| texts(shapes).into_iter().any(|(t, _, elided)| t == SIZE && !elided);

        let (_, shapes) = drive(&view, 220.0, |shapes| {
            let (_, rect, _) = cell(shapes).expect("cut short at first");
            let hover = vec![egui::Event::PointerMoved(rect.center())];
            // Still for a second: longer than egui waits before a tooltip.
            std::iter::once(hover).chain(std::iter::repeat_n(Vec::new(), 20)).collect()
        });
        assert!(whole(&shapes), "the whole value on hover: {:?}", texts(&shapes));

        let (_, shapes) = drive(&view, 220.0, |shapes| {
            // The resize line of the sizes column: the second vertical line from the left.
            let mut lines: Vec<f32> = shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::LineSegment { points: [a, b], .. } if a.x == b.x && (b.y - a.y).abs() > 15.0 => {
                        Some(a.x)
                    }
                    _ => None,
                })
                .collect();
            lines.sort_by(f32::total_cmp);
            let x = lines[1];
            let at = |dx: f32| egui::pos2(x + dx, 40.0);
            let mut steps = vec![vec![egui::Event::PointerMoved(at(0.0)), button(at(0.0), true)]];
            steps.extend([10.0, 40.0, 80.0, 120.0].map(|dx| vec![egui::Event::PointerMoved(at(dx))]));
            steps.push(vec![button(at(120.0), false), egui::Event::PointerMoved(egui::pos2(5.0, 300.0))]);
            steps.extend([Vec::new(), Vec::new()]);
            steps
        });
        assert!(whole(&shapes) && cell(&shapes).is_none(), "room for the whole value: {:?}", texts(&shapes));
    }

    #[test]
    fn table_rows_are_picked_by_a_click() {
        let json = serde_json::json!({"v": 1, "root": {"type": "table", "id": "parts",
            "columns": [{"title": "Teil"}, {"title": "Anzahl", "width": 60.0, "numeric": true}],
            "rows": [{"id": "0", "cells": ["a", "1"]}, {"id": "1", "cells": ["b", "2"]}, {"id": "2", "cells": ["c", "3"]}],
            "max_height": 260.0}});
        let view: View = serde_json::from_value(json).unwrap();
        // Header 20 points, rows 20 points: the middle of the second row.
        let events = click_at(&view, egui::pos2(40.0, 8.0 + 20.0 + 20.0 + 10.0));
        assert_eq!(events, [Event::TableSelect { id: "parts".into(), row: Some("1".into()) }]);
    }

    /// Three buttons in a row too narrow for them: the third one goes to the next line, where
    /// a click finds it.
    #[test]
    fn rows_wrap_buttons_to_the_next_line() {
        let json = serde_json::json!({"v": 1, "root": {"type": "split", "ratio": 0.4,
            "first": {"type": "row", "children": [
                {"type": "button", "id": "a", "label": "Automatisch belegen"},
                {"type": "button", "id": "g", "label": "Generieren + Druckliste"},
                {"type": "button", "id": "k", "label": "Kalibrierteile"}]},
            "second": {"type": "label", "text": "x"}}});
        let view: View = serde_json::from_value(json).unwrap();
        // The part is 0.4 × (400 − 8) ≈ 157 points wide: one button per line, about 22 apart.
        let events = click_at(&view, egui::pos2(30.0, 2.0 * 22.0 + 10.0));
        assert_eq!(events, [Event::Click { id: "k".into() }]);
    }

    /// The parts table of a panel like Bingrove's: below a 3D view in the second part of a split.
    #[test]
    fn tables_in_a_split_below_a_mesh_view_take_clicks() {
        let json = serde_json::json!({"v": 1, "root": {"type": "split", "ratio": 0.5,
            "first": {"type": "label", "text": "form"},
            "second": {"type": "column", "children": [
                {"type": "mesh_view", "id": "m", "key": "drawer", "revision": 1, "height": 100.0},
                {"type": "table", "id": "parts", "columns": [{"title": "Teil"}, {"title": "Anzahl", "width": 60.0}],
                 "rows": [{"id": "0", "cells": ["baseplate_1A_5x4.stl", "1"]}, {"id": "1", "cells": ["bin_2x1_6u.stl", "2"]}],
                 "max_height": 260.0}]}}});
        let view: View = serde_json::from_value(json).unwrap();
        // On the text of the second row's first cell.
        let events = click_at(&view, egui::pos2(215.0, 8.0 + 100.0 + 4.0 + 20.0 + 20.0 + 10.0));
        assert_eq!(events, [Event::TableSelect { id: "parts".into(), row: Some("1".into()) }]);
    }
}
