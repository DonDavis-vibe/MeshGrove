pub mod details;
pub mod duplicates;
pub mod gallery;
pub mod icons;
pub mod layout;
pub mod model_window;
pub mod offscreen;
pub mod settings;
pub mod sidebar;
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
