//! The interface's look: SolidCraft's theme files on its token contract (`vendor/`, MIT OR
//! Apache-2.0). A theme sets egui's visuals, so every stock control takes its colours, and the
//! kit's own controls draw from the same tokens. With no theme chosen Meshgrove looks as it
//! always did: egui's own dark or light, following the system.

use std::collections::BTreeMap;

use egui::{Context, Ui};
use solidcraft_theme::{Custom, Tokens};
use solidcraft_ui_kit::icons::{self, IconData, IconSet};
use solidcraft_ui_kit::theme_files::{self, ThemeFile};
use solidcraft_ui_kit::{self as kit, ThemeCard, ThemeSample};

mod builtin {
    include!(concat!(env!("OUT_DIR"), "/builtin_themes.rs"));
}

// Generated tables, typed as the kit's `IconData` expects.
#[allow(clippy::type_complexity)]
mod icon_data {
    include!(concat!(env!("OUT_DIR"), "/icon_data.rs"));
}

/// The kit's icons (assets/icons). Theme files name its roles and accents for their icon colours.
static ICONS: IconSet = IconSet::new(IconData {
    role_names: icon_data::ROLE_NAMES,
    role_source_hex: icon_data::ROLE_SOURCE_HEX,
    palette: icon_data::PALETTE,
    accent_names: icon_data::ACCENT_NAMES,
    accents: icon_data::ACCENTS,
    accent_variants: icon_data::ACCENT_VARIANTS,
    masters: icon_data::SVGS,
    aliases: icon_data::ALIASES,
    icon_accents: icon_data::ICON_ACCENTS,
    danger_accent: "constraint",
});

/// The kit's own two palettes, which are no files.
const OWN: [(&str, &str); 2] = [("dark", "SolidCraft Dark"), ("light", "SolidCraft Light")];

/// Before the first frame and before any theme is read.
pub fn install() {
    // Before any theme file is read: they are validated against the icon set.
    icons::install(&ICONS);
    icons::glyphs::install(&super::glyphs::GLYPHS);
    theme_files::install_builtin(builtin::BUILTIN);
}

/// The theme files in display order; empty when one of them fails validation (logged).
pub fn themes() -> &'static [ThemeFile] {
    theme_files::builtin()
}

/// Whether theme `id` is a dark one, and its file (`None`: one of the kit's own palettes).
/// `None` for an unknown id.
fn theme(id: &str) -> Option<(bool, Option<&'static ThemeFile>)> {
    if let Some((own, _)) = OWN.iter().find(|(own, _)| *own == id) {
        return Some((*own == "dark", None));
    }
    let file = theme_files::find(id, &[])?;
    Some((file.base.is_dark(), Some(file)))
}

thread_local! {
    /// The theme the interface has now (`None`: Meshgrove's own look), as the picker marks it.
    static CURRENT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// The palette of the theme that is on; `None` in Meshgrove's own look, whose colours are egui's
/// and the ones written into the code.
pub fn tokens() -> Option<Tokens> {
    CURRENT.with_borrow(Option::is_some).then(Tokens::get)
}

/// Give the interface theme `id`, or Meshgrove's own look (`None`, and for an id no longer
/// known). True when `id` was applied.
pub fn apply(ctx: &Context, id: Option<&str>) -> bool {
    let Some((dark, file)) = id.and_then(theme) else {
        // egui's own visuals and spacing again, dark or light as the system says.
        ctx.set_visuals_of(egui::Theme::Dark, egui::Visuals::dark());
        ctx.set_visuals_of(egui::Theme::Light, egui::Visuals::light());
        ctx.all_styles_mut(|style| style.spacing = egui::Spacing::default());
        ctx.set_theme(egui::ThemePreference::System);
        // The kit's own controls (the theme cards, the font list) as dark or light as egui.
        solidcraft_theme::set_dark(ctx.system_theme().is_none_or(|theme| theme == egui::Theme::Dark));
        solidcraft_theme::set_custom(None);
        icons::set_theme_palette(None);
        CURRENT.set(None);
        ctx.request_repaint();
        return false;
    };
    let values = BTreeMap::new();
    solidcraft_theme::set_dark(dark);
    solidcraft_theme::set_custom(file.map(|file| Custom { dark, tokens: theme_files::resolve_with(file, &values) }));
    icons::set_theme_palette(file.and_then(|file| theme_files::resolve_icon_palette(file, &values)).map(|p| (dark, p)));
    solidcraft_theme::apply(ctx);
    CURRENT.set(id.map(str::to_owned));
    true
}

/// Whether Meshgrove draws the main window's title bar itself. On macOS the system's stays: its
/// window buttons belong to the system's bar.
pub fn own_title_bar(setting: meshgrove_core::config::TitleBar) -> bool {
    cfg!(not(target_os = "macos")) && setting == meshgrove_core::config::TitleBar::App
}

/// A glowing outline around `rect` in `accent`, over a breath of it: where something dragged
/// lands (LightGrove's drop outline).
pub fn glow(painter: &egui::Painter, rect: egui::Rect, accent: egui::Color32) {
    let radius = tokens().map_or(6.0, |t| f32::from(t.radius_lg));
    painter.rect_filled(rect, radius, accent.gamma_multiply(0.08));
    for (grow, alpha) in [(9.0, 0.05), (6.0, 0.09), (3.0, 0.16)] {
        let stroke = egui::Stroke::new(3.0, accent.gamma_multiply(alpha));
        painter.rect_stroke(rect.expand(grow), radius + grow, stroke, egui::StrokeKind::Outside);
    }
    painter.rect_stroke(rect, radius, egui::Stroke::new(2.0, accent), egui::StrokeKind::Inside);
}

/// The width of a theme card.
const CARD_WIDTH: f32 = 132.0;

/// One card of the theme picker.
struct Card {
    id: Option<&'static str>,
    name: String,
    sample: ThemeSample,
    dark: bool,
}

/// The theme picker's cards, made once: their thumbnails are painted from each theme's tokens.
#[derive(Default)]
pub struct Picker {
    cards: Vec<Card>,
}

impl Picker {
    fn cards(&mut self, own_name: &str, system_dark: bool) -> &[Card] {
        if self.cards.is_empty() {
            // Meshgrove's own look, shown in egui's colours.
            let v = if system_dark { egui::Visuals::dark() } else { egui::Visuals::light() };
            let mut own = if system_dark { Tokens::dark() } else { Tokens::light() };
            (own.panel, own.dialog_bg, own.dialog_body, own.dialog_footer) =
                (v.panel_fill, v.window_fill, v.panel_fill, v.faint_bg_color);
            (own.control_fill, own.viewport_top, own.viewport_bottom) =
                (v.widgets.inactive.bg_fill, v.extreme_bg_color, v.extreme_bg_color);
            (own.form_primary, own.accent, own.form_ink, own.form_dim) =
                (v.selection.bg_fill, v.selection.bg_fill, v.text_color(), v.weak_text_color());
            self.cards.push(Card {
                id: None,
                name: own_name.to_owned(),
                sample: ThemeSample::from_tokens(&own),
                dark: system_dark,
            });
            for (id, name) in OWN {
                let dark = id == "dark";
                let tokens = if dark { Tokens::dark() } else { Tokens::light() };
                self.cards.push(Card {
                    id: Some(id),
                    name: name.to_owned(),
                    sample: ThemeSample::from_tokens(&tokens),
                    dark,
                });
            }
            for theme in themes() {
                let tokens = theme_files::resolve_with(theme, &BTreeMap::new());
                self.cards.push(Card {
                    id: Some(theme.id.as_str()),
                    name: theme.name.clone(),
                    sample: ThemeSample::from_tokens(&tokens),
                    dark: theme.base.is_dark(),
                });
            }
        }
        &self.cards
    }

    /// All themes as thumbnail cards, as many to a row as fit. Gives the theme the user picked
    /// (`Some(None)`: Meshgrove's own look).
    pub fn show(&mut self, ui: &mut Ui, labels: [&str; 3]) -> Option<Option<String>> {
        let [own_name, dark, light] = labels;
        let selected = CURRENT.with_borrow(Clone::clone);
        let selected = selected.as_deref();
        let system_dark = ui.ctx().system_theme().is_none_or(|theme| theme == egui::Theme::Dark);
        let cards = self.cards(own_name, system_dark);
        let gap = Tokens::get().space_8;
        let available = ui.available_width();
        let columns = (((available + gap) / (CARD_WIDTH + gap)).floor() as usize).max(1);
        let mut chosen = None;
        // Two and a half rows, as SolidCraft's theme browser shows: the half row says there are more.
        let row = (CARD_WIDTH - gap) * 76.0 / 124.0 + 52.0 + gap;
        egui::ScrollArea::vertical().id_salt("ui_themes").auto_shrink([false, true]).max_height(row * 2.5).show(
            ui,
            |ui| {
                for row in cards.chunks(columns) {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for card in row {
                            let shown = ThemeCard {
                                name: &card.name,
                                note: if card.dark { dark } else { light },
                                options: None,
                                sample: card.sample,
                                selected: card.id == selected,
                                dark: card.dark,
                                high: false,
                                day: false,
                                night: false,
                            };
                            if kit::theme_card(ui, &shown, CARD_WIDTH).clicked() {
                                chosen = Some(card.id.map(str::to_owned));
                            }
                        }
                    });
                    ui.add_space(gap);
                }
            },
        );
        chosen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedded_theme_is_valid_and_resolves() {
        install();
        let themes = theme_files::read_builtin(builtin::BUILTIN).expect("the embedded themes are valid");
        assert_eq!(themes.len(), 25);
        for file in &themes {
            assert!(theme(&file.id).is_some(), "{}", file.id);
        }
        assert!(theme("dark").is_some() && theme("light").is_some());
        assert!(theme("no-such-theme").is_none());
    }
}
