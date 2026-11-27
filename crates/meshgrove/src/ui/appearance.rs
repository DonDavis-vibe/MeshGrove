//! The interface's look: SolidCraft's theme files on its token contract (`vendor/`, MIT OR
//! Apache-2.0). A theme sets egui's visuals, so every stock control takes its colours, and the
//! kit's own controls draw from the same tokens. With no theme chosen Meshgrove wears its own:
//! neutral greys and a Breeze blue, dark or light as the system is. Theme files the user
//! imports are kept as files beside the settings and offered with the others.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

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

/// The kit's own words (tooltips of its controls, its menus) in Meshgrove's two languages: the
/// ids are SolidCraft's, the texts as LightGrove carries them. Meshgrove's own texts stay in its
/// locale files and reach the kit already translated.
static KIT_ENGLISH: solidcraft_ui_text::Catalog =
    solidcraft_ui_text::Catalog::new("en", "English", include_str!("../../kit-texts/en.json"));
static KIT_GERMAN: solidcraft_ui_text::Catalog =
    solidcraft_ui_text::Catalog::new("de", "Deutsch", include_str!("../../kit-texts/de.json"));
static KIT_TEXTS: [&solidcraft_ui_text::Catalog; 2] = [&KIT_ENGLISH, &KIT_GERMAN];

/// The kit speaks the interface's language for as long as the guard lives: hold it for a frame.
pub fn language() -> solidcraft_ui_text::LanguageScope {
    solidcraft_ui_text::scope(&rust_i18n::locale())
}

/// Before the first frame and before any theme is read.
pub fn install() {
    solidcraft_ui_text::install(&KIT_TEXTS);
    // Before any theme file is read: they are validated against the icon set.
    icons::install(&ICONS);
    icons::glyphs::install(&super::glyphs::GLYPHS);
    theme_files::install_builtin(builtin::BUILTIN);
    if let Some(dir) = imported_dir() {
        IMPORTED.set(Imported::read(&dir));
    }
}

/// Where imported theme files are kept, one file each.
fn imported_dir() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("meshgrove").join("themes"))
}

/// The theme files the user imported, each with the file it is kept in.
#[derive(Default)]
struct Imported {
    themes: Vec<ThemeFile>,
    files: Vec<PathBuf>,
}

impl Imported {
    /// The theme files in `dir`, by their files' names; one that is no theme file is left out
    /// (logged).
    fn read(dir: &Path) -> Imported {
        let mut imported = Imported::default();
        let Ok(entries) = std::fs::read_dir(dir) else { return imported };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")))
            .collect();
        files.sort();
        for file in files {
            if let Err(e) = read_theme(&file).and_then(|theme| imported.keep(theme, file.clone())) {
                log::warn!("theme {}: {e}", file.display());
            }
        }
        imported
    }

    /// Adds `theme`, kept in `file`, or replaces the one of its id. Gives its id (one that a
    /// built-in theme has already gets a prefix) and the file it replaced, if another.
    fn keep(&mut self, theme: ThemeFile, file: PathBuf) -> Result<(String, Option<PathBuf>), String> {
        let id = theme_files::add(&mut self.themes, theme)?;
        let at = self.themes.iter().position(|theme| theme.id == id).unwrap_or(self.themes.len() - 1);
        let old = if let Some(slot) = self.files.get_mut(at) {
            let old = std::mem::replace(slot, file);
            (old != *slot).then_some(old)
        } else {
            self.files.push(file);
            None
        };
        Ok((id, old))
    }

    /// Copies the theme file at `path` into `dir` and adds it. Gives its id.
    fn import(&mut self, dir: &Path, path: &Path) -> Result<String, String> {
        let text = read_text(path)?;
        let theme = theme_files::parse(&text)?;
        // Its id as it will be kept, for the file's name: a trial on a copy.
        let id = theme_files::add(&mut self.themes.clone(), theme.clone())?;
        let file = dir.join(format!("{}.json", theme_files::slug(&id)));
        std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&file, text)).map_err(|e| e.to_string())?;
        let (id, old) = self.keep(theme, file)?;
        if let Some(old) = old {
            let _ = std::fs::remove_file(old);
        }
        Ok(id)
    }

    /// Takes the imported theme `id` away, and its file.
    fn remove(&mut self, id: &str) -> Result<(), String> {
        let at =
            self.themes.iter().position(|theme| theme.id == id).ok_or_else(|| format!("no imported theme `{id}`"))?;
        if let Some(file) = self.files.get(at) {
            match std::fs::remove_file(file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
                _ => {}
            }
            self.files.remove(at);
        }
        self.themes.remove(at);
        Ok(())
    }
}

/// A theme file's text: at most as long as a theme file may be, and text.
fn read_text(path: &Path) -> Result<String, String> {
    let mut text = String::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(theme_files::MAX_BYTES as u64 + 1).read_to_string(&mut text))
        .map_err(|e| e.to_string())?;
    Ok(text)
}

fn read_theme(path: &Path) -> Result<ThemeFile, String> {
    theme_files::parse(&read_text(path)?)
}

/// Imports the theme file at `path`: from now on it is offered with the others. Importing a file
/// of the same theme again replaces it. Gives its id and its name.
pub fn import(path: &Path) -> Result<(String, String), String> {
    let dir = imported_dir().ok_or("no folder for Meshgrove's settings")?;
    IMPORTED.with_borrow_mut(|imported| {
        let id = imported.import(&dir, path)?;
        let name = imported.themes.iter().find(|theme| theme.id == id).map_or_else(|| id.clone(), |t| t.name.clone());
        Ok((id, name))
    })
}

/// Takes the imported theme `id` away.
pub fn remove(id: &str) -> Result<(), String> {
    IMPORTED.with_borrow_mut(|imported| imported.remove(id))
}

/// The theme files in display order; empty when one of them fails validation (logged).
pub fn themes() -> &'static [ThemeFile] {
    theme_files::builtin()
}

/// Whether theme `id` is a dark one, and its file (`None`: one of the kit's own palettes).
/// `None` for an unknown id.
fn theme(id: &str) -> Option<(bool, Option<ThemeFile>)> {
    if let Some((own, _)) = OWN.iter().find(|(own, _)| *own == id) {
        return Some((*own == "dark", None));
    }
    let file = IMPORTED.with_borrow(|imported| theme_files::find(id, &imported.themes).cloned())?;
    Some((file.base.is_dark(), Some(file)))
}

/// Meshgrove's own look, a theme like the others (assets/themes-meshgrove): the dark or the light
/// one, as the system is.
const OWN_LOOK: [&str; 2] = ["meshgrove", "meshgrove-light"];

thread_local! {
    /// The theme chosen (`None`: Meshgrove's own look), as the picker marks it.
    static CURRENT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    /// Whether Meshgrove's own look was last applied dark.
    static OWN_DARK: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
    /// The theme files the user imported.
    static IMPORTED: RefCell<Imported> = RefCell::default();
}

/// The palette the interface draws with now.
pub fn tokens() -> Tokens {
    Tokens::get()
}

/// Whether one of SolidCraft's themes is on, not Meshgrove's own look (dark or light, chosen or
/// following the system).
pub fn themed() -> bool {
    CURRENT.with_borrow(|current| current.as_deref().is_some_and(|id| !OWN_LOOK.contains(&id)))
}

fn system_dark(ctx: &Context) -> bool {
    ctx.system_theme().is_none_or(|theme| theme == egui::Theme::Dark)
}

/// Give the interface theme `id`, or Meshgrove's own look (`None`, and for an id no longer
/// known), dark or light as the system is. True when `id` was applied.
pub fn apply(ctx: &Context, id: Option<&str>) -> bool {
    let chosen = id.and_then(theme);
    let known = chosen.is_some();
    let (dark, file) = chosen.unwrap_or_else(|| {
        let dark = system_dark(ctx);
        OWN_DARK.set(dark);
        (dark, theme_files::find(OWN_LOOK[usize::from(!dark)], &[]).cloned())
    });
    let values = BTreeMap::new();
    solidcraft_theme::set_dark(dark);
    solidcraft_theme::set_custom(
        file.as_ref().map(|file| Custom { dark, tokens: theme_files::resolve_with(file, &values) }),
    );
    icons::set_theme_palette(
        file.as_ref().and_then(|file| theme_files::resolve_icon_palette(file, &values)).map(|p| (dark, p)),
    );
    solidcraft_theme::apply(ctx);
    CURRENT.set(id.filter(|_| known).map(str::to_owned));
    known
}

/// With no theme chosen, Meshgrove's own look follows the system from dark to light and back.
/// Call once a frame.
pub fn follow_system(ctx: &Context) {
    if CURRENT.with_borrow(Option::is_none) && system_dark(ctx) != OWN_DARK.get() {
        apply(ctx, None);
    }
}

/// Whether Meshgrove draws the main window's title bar itself. On macOS the system's stays: its
/// window buttons belong to the system's bar.
pub fn own_title_bar(setting: meshgrove_core::config::TitleBar) -> bool {
    cfg!(not(target_os = "macos")) && setting == meshgrove_core::config::TitleBar::App
}

/// Meshgrove's own title bar across the top of a window, in the theme's colours: the icon
/// (`brand`), `name` and, quieter, `about`, with the window's buttons at the right. Empty space
/// moves the window, a double click makes it fill the screen and back. Gives the button pressed;
/// the caller sends it on (or handles Close itself).
/// `trailing` draws into the room the title leaves free before the window's buttons (the
/// update pill).
pub fn title_bar(
    ui: &mut Ui,
    brand: egui::TextureId,
    name: &str,
    about: Option<&str>,
    trailing: impl FnOnce(&mut Ui, egui::Rect),
) -> Option<kit::window_chrome::WindowAction> {
    use kit::{type_face, window_chrome};
    let t = tokens();
    let (fill, ink, dim) = (t.app_bar, t.app_bar_text, t.app_bar_text.gamma_multiply(0.65));
    let bar =
        egui::Panel::top("title_bar").exact_size(t.control_height + t.space_8).frame(egui::Frame::NONE.fill(fill));
    bar.show(ui, |ui| {
        let chrome = window_chrome::TitleBar { traffic_lights: false, caption_buttons: true };
        chrome.show(ui, ui.max_rect(), |ui, content| {
            let painter = ui.painter().with_clip_rect(content);
            let mark = egui::Rect::from_center_size(
                egui::pos2(content.left() + t.space_10 + t.space_8, content.center().y),
                egui::vec2(t.space_16, t.space_16),
            );
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(brand, mark, uv, egui::Color32::WHITE);
            let at = egui::pos2(mark.right() + t.space_8, content.center().y);
            let name = painter.text(at, egui::Align2::LEFT_CENTER, name, type_face::semibold(t.font_label), ink);
            let mut title_end = name.right();
            if let Some(about) = about {
                let at = egui::pos2(name.right() + t.space_8, content.center().y);
                let font = egui::FontId::proportional(t.font_label);
                title_end = painter.text(at, egui::Align2::LEFT_CENTER, format!("·  {about}"), font, dim).right();
            }
            let room =
                egui::Rect::from_min_max(egui::pos2(title_end + t.space_16, content.top()), content.right_bottom());
            if room.width() > 0.0 {
                trailing(ui, room);
            }
        })
    })
    .inner
}

/// A glowing outline around `rect` in `accent`, over a breath of it: where something dragged
/// lands (LightGrove's drop outline).
pub fn glow(painter: &egui::Painter, rect: egui::Rect, accent: egui::Color32) {
    let radius = f32::from(tokens().radius_lg);
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
    id: String,
    name: String,
    sample: ThemeSample,
    dark: bool,
    /// One the user imported, which they can take away again.
    imported: bool,
}

impl Card {
    fn of(theme: &ThemeFile, imported: bool) -> Card {
        Card {
            id: theme.id.clone(),
            name: theme.name.clone(),
            sample: ThemeSample::from_tokens(&theme_files::resolve_with(theme, &BTreeMap::new())),
            dark: theme.base.is_dark(),
            imported,
        }
    }
}

/// What the user did in the theme picker.
pub enum Pick {
    /// Chose the theme of this id.
    Theme(String),
    /// Asked to import a theme file.
    Import,
    /// Asked to take the imported theme of this id away.
    Remove(String),
}

/// The words of the theme picker.
pub struct PickerTexts<'a> {
    pub dark: &'a str,
    pub light: &'a str,
    /// The note of an imported theme's card, after dark or light.
    pub imported: &'a str,
    /// The import button and what resting on it says; `None`: no button (no file dialog).
    pub import: Option<(&'a str, &'a str)>,
    pub remove: &'a str,
}

/// The theme picker's cards, made once: their thumbnails are painted from each theme's tokens.
#[derive(Default)]
pub struct Picker {
    cards: Vec<Card>,
}

impl Picker {
    /// The themes have changed (one was imported or taken away): the cards are made again.
    pub fn refresh(&mut self) {
        self.cards.clear();
    }

    fn cards(&mut self) -> &[Card] {
        if self.cards.is_empty() {
            // Meshgrove's own two first, then the imported ones, then SolidCraft's own two, then
            // its theme files.
            let files = |own: bool| themes().iter().filter(move |theme| OWN_LOOK.contains(&theme.id.as_str()) == own);
            for theme in files(true) {
                self.cards.push(Card::of(theme, false));
            }
            IMPORTED.with_borrow(|imported| {
                self.cards.extend(imported.themes.iter().map(|theme| Card::of(theme, true)));
            });
            for (id, name) in OWN {
                let dark = id == "dark";
                let tokens = if dark { Tokens::dark() } else { Tokens::light() };
                self.cards.push(Card {
                    id: id.to_owned(),
                    name: name.to_owned(),
                    sample: ThemeSample::from_tokens(&tokens),
                    dark,
                    imported: false,
                });
            }
            for theme in files(false) {
                self.cards.push(Card::of(theme, false));
            }
        }
        &self.cards
    }

    /// `hint` with the import button beside it, then all themes as thumbnail cards, as many to a
    /// row as fit; an imported one's context menu takes it away. With none chosen yet, Meshgrove's
    /// own dark or light is marked, whichever the system has on.
    pub fn show(&mut self, ui: &mut Ui, texts: &PickerTexts<'_>, hint: impl FnOnce(&mut Ui)) -> Option<Pick> {
        let mut chosen = None;
        let t = Tokens::get();
        match texts.import {
            Some((label, tip)) => {
                let width = kit::ContentWidth::name(ui, label) + t.space_14 + t.space_6 + t.space_10 * 2.0;
                kit::theme_toolbar(ui, "ui_themes", width, t.space_24 * 19.0, hint, |ui| {
                    if kit::theme_import_button(ui, label).on_hover_text(tip).clicked() {
                        chosen = Some(Pick::Import);
                    }
                });
                ui.add_space(t.space_8);
            }
            None => hint(ui),
        }
        let selected = CURRENT.with_borrow(Clone::clone);
        let selected = selected.as_deref().unwrap_or(OWN_LOOK[usize::from(!OWN_DARK.get())]);
        let cards = self.cards();
        let gap = t.space_8;
        let available = ui.available_width();
        let columns = (((available + gap) / (CARD_WIDTH + gap)).floor() as usize).max(1);
        // Two and a half rows, as SolidCraft's theme browser shows: the half row says there are more.
        let row = (CARD_WIDTH - gap) * 76.0 / 124.0 + 52.0 + gap;
        egui::ScrollArea::vertical().id_salt("ui_themes").auto_shrink([false, true]).max_height(row * 2.5).show(
            ui,
            |ui| {
                for row in cards.chunks(columns) {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for card in row {
                            let tone = if card.dark { texts.dark } else { texts.light };
                            let note =
                                if card.imported { format!("{tone} · {}", texts.imported) } else { tone.to_owned() };
                            let shown = ThemeCard {
                                name: &card.name,
                                note: &note,
                                options: None,
                                sample: card.sample,
                                selected: card.id == selected,
                                dark: card.dark,
                                high: false,
                                day: false,
                                night: false,
                            };
                            let response = kit::theme_card(ui, &shown, CARD_WIDTH);
                            if response.clicked() {
                                chosen = Some(Pick::Theme(card.id.clone()));
                            }
                            if card.imported {
                                super::menus::context(&response, |ui| {
                                    if super::menus::item_if(ui, "delete", texts.remove, true, "", true) {
                                        chosen = Some(Pick::Remove(card.id.clone()));
                                    }
                                });
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

    /// Writes SolidCraft's two built-in palettes as theme files into scratch/, the base
    /// `scripts/meshgrove-themes.py` makes Meshgrove's own themes from:
    /// `cargo test -p meshgrove dump_builtin_palettes -- --ignored`.
    #[test]
    #[ignore = "a tool, not a check"]
    fn dump_builtin_palettes() {
        install();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scratch");
        std::fs::create_dir_all(&dir).unwrap();
        for (name, base, tokens) in [
            ("builtin-dark", theme_files::Base::Dark, Tokens::dark()),
            ("builtin-light", theme_files::Base::Light, Tokens::light()),
        ] {
            let value = theme_files::export(name, base, &tokens, None);
            std::fs::write(dir.join(format!("{name}.json")), serde_json::to_string_pretty(&value).unwrap()).unwrap();
        }
    }

    #[test]
    fn imported_themes_are_kept_replaced_and_removed() {
        install();
        let source = tempfile::tempdir().unwrap();
        let kept = tempfile::tempdir().unwrap();
        let file = |name: &str, body: &str| {
            let path = source.path().join(name);
            let text = format!(
                r#"{{"format":"solidcraft-theme/1","name":"My Theme","base":"dark","colors":{{"body":"{body}"}}}}"#
            );
            std::fs::write(&path, text).unwrap();
            path
        };
        let mut imported = Imported::default();
        let id = imported.import(kept.path(), &file("a.json", "#abcdef")).unwrap();
        assert_eq!(imported.themes.len(), 1);
        // The same theme again replaces it, from a file of another name too.
        assert_eq!(imported.import(kept.path(), &file("b.json", "#fedcba")).unwrap(), id);
        assert_eq!(imported.themes.len(), 1);
        assert_eq!(std::fs::read_dir(kept.path()).unwrap().count(), 1);
        // What is kept is read again at the next start.
        let again = Imported::read(kept.path());
        assert_eq!(again.themes.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), [id.as_str()]);
        assert!(std::fs::read_to_string(&again.files[0]).unwrap().contains("#fedcba"));
        // A file that is no theme is refused and leaves nothing behind.
        let broken = source.path().join("broken.json");
        std::fs::write(&broken, "{ not a theme").unwrap();
        assert!(imported.import(kept.path(), &broken).is_err());
        assert_eq!(std::fs::read_dir(kept.path()).unwrap().count(), 1);
        // A theme taking a built-in one's id gets its own.
        let builtin = &themes()[0];
        let clash = source.path().join("clash.json");
        std::fs::write(
            &clash,
            format!(r#"{{"format":"solidcraft-theme/1","id":"{}","name":"Clash","base":"light"}}"#, builtin.id),
        )
        .unwrap();
        assert_ne!(imported.import(kept.path(), &clash).unwrap(), builtin.id);
        imported.remove(&id).unwrap();
        assert_eq!(imported.themes.len(), 1);
        assert!(imported.remove(&id).is_err());
        assert_eq!(std::fs::read_dir(kept.path()).unwrap().count(), 1);
    }

    #[test]
    fn every_embedded_theme_is_valid_and_resolves() {
        install();
        let themes = theme_files::read_builtin(builtin::BUILTIN).expect("the embedded themes are valid");
        // SolidCraft's 25 and Meshgrove's own two.
        assert_eq!(themes.len(), 27);
        assert!(OWN_LOOK.iter().all(|id| theme(id).is_some()));
        for file in &themes {
            assert!(theme(&file.id).is_some(), "{}", file.id);
        }
        assert!(theme("dark").is_some() && theme("light").is_some());
        assert!(theme("no-such-theme").is_none());
    }
}
