pub mod chat;
pub mod chat_input;
pub mod details;
pub mod duplicates;
pub mod gallery;
pub mod icons;
pub mod inbox;
pub mod layout;
pub mod model_window;
pub mod offscreen;
pub mod review;
pub mod settings;
pub mod settings_ai;
pub mod sidebar;
pub mod spacemouse;
pub mod suggest;
pub mod tags;
pub mod turntable;
pub mod viewer;

use egui::containers::menu::MenuConfig;
use egui::{Popup, PopupCloseBehavior, Response, Ui};

/// For menus holding a text field: egui closes menus on any click, inside too, so clicking
/// into the field would close it. These close on clicks outside; their items close them.
pub fn typing_menu() -> MenuConfig {
    MenuConfig::new().close_behavior(PopupCloseBehavior::CloseOnClickOutside)
}

/// A context menu that stays open on clicks inside it (see [`typing_menu`]); its submenus
/// behave the same.
pub fn context_menu_with_field(response: &Response, add_contents: impl FnOnce(&mut Ui)) {
    Popup::context_menu(response).close_behavior(PopupCloseBehavior::CloseOnClickOutside).show(add_contents);
}

/// The slicers the "open in" entries offer: their names and which one is the default.
#[derive(Clone, Copy, Default)]
pub struct SlicerList<'a> {
    pub names: &'a [String],
    pub default: usize,
}

impl SlicerList<'_> {
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// The mesh editors the "open in" entries offer, and which one is the default. Not every
/// editor opens every kind of file.
#[derive(Clone, Copy, Default)]
pub struct EditorList<'a> {
    pub programs: &'a [meshgrove_core::config::Program],
    pub default: usize,
}

impl EditorList<'_> {
    /// The editors that open this kind of file as (index, name), the default one first.
    pub fn usable(&self, kind: meshgrove_core::Kind) -> Vec<(usize, &str)> {
        self.usable_for_any(&[kind])
    }

    /// The editors that open at least one of these kinds, the default one first.
    pub fn usable_for_any(&self, kinds: &[meshgrove_core::Kind]) -> Vec<(usize, &str)> {
        let mut list: Vec<(usize, &str)> = self
            .programs
            .iter()
            .enumerate()
            .filter(|(_, e)| kinds.iter().any(|k| e.opens(*k)))
            .map(|(i, e)| (i, e.name.as_str()))
            .collect();
        if let Some(at) = list.iter().position(|(i, _)| *i == self.default) {
            let first = list.remove(at);
            list.insert(0, first);
        }
        list
    }
}

/// Menu contents that scroll when the window is too small to show them all, with an arrow above
/// and below that is greyed out at the end of the list (and scrolls a few rows when clicked).
pub fn scroll_menu(ui: &mut Ui, id_salt: impl std::hash::Hash + std::fmt::Debug, add_contents: impl FnOnce(&mut Ui)) {
    const STEP: f32 = 72.0;
    const ARROW_HEIGHT: f32 = 16.0;
    let id = ui.scope_id().with(id_salt);
    // Scroll position and the furthest it can go, as of the last frame: the arrows above the
    // list need them before the list is laid out.
    let (offset, furthest): (f32, f32) = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    let pending: Option<f32> = ui.data_mut(|d| d.remove_temp(id.with("jump")));
    let overflowing = furthest > 1.0;
    let arrow = |ui: &mut Ui, up: bool, enabled: bool| -> bool {
        let text = egui::RichText::new(if up { "⏶" } else { "⏷" });
        let text = if enabled { text } else { text.weak() };
        let size = egui::vec2(ui.available_width().max(80.0), ARROW_HEIGHT);
        ui.add_sized(size, egui::Button::new(text).frame(false)).clicked() && enabled
    };
    let mut jump = None;
    if overflowing && arrow(ui, true, offset > 1.0) {
        jump = Some((offset - STEP).max(0.0));
    }
    let arrows = if overflowing { 2.0 * (ARROW_HEIGHT + ui.spacing().item_spacing.y) } else { 0.0 };
    let max_height = (ui.ctx().content_rect().height() - 90.0 - arrows).max(120.0);
    let mut area = egui::ScrollArea::vertical().id_salt(id).max_height(max_height).auto_shrink([true, true]);
    if let Some(target) = pending {
        area = area.vertical_scroll_offset(target);
    }
    let shown = area.show(ui, add_contents);
    let furthest_now = (shown.content_size.y - shown.inner_rect.height()).max(0.0);
    let offset_now = shown.state.clamped_offset().y;
    if overflowing && arrow(ui, false, offset_now < furthest_now - 1.0) {
        jump = Some((offset_now + STEP).min(furthest_now));
    }
    if let Some(target) = jump {
        ui.data_mut(|d| d.insert_temp(id.with("jump"), target));
        ui.ctx().request_repaint();
    }
    ui.data_mut(|d| d.insert_temp(id, (offset_now, furthest_now)));
    if (furthest_now > 1.0) != overflowing || (offset_now - offset).abs() > 0.5 {
        ui.ctx().request_repaint();
    }
}
