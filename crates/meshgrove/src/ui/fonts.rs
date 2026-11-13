//! The interface font, as LightGrove's font module does it: the theme's own face (IBM Plex Sans,
//! SolidCraft's, at the kit's four weights), the desktop's font, or any family installed on the
//! system put in front. Whatever is chosen, the fonts behind it stay as fallbacks for the signs
//! it lacks.
//!
//! The settings' font list shows each family in itself. Only a window of rows is loaded as
//! preview fonts ([`preview_family`]): the rows on screen and two pages above and below them
//! ([`PreviewWindow`]). Faces that leave the window are dropped, and all of them go when the list
//! closes, so scrolling from A to Z never holds every installed font in memory.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use egui::{FontData, FontDefinitions, FontFamily};
use solidcraft_ui_kit::type_face::{self, BOLD_FAMILY, MEDIUM_FAMILY, SEMIBOLD_FAMILY, sans_families};

/// The setting's value for the desktop's font (a family is named as it is, the theme's font is
/// no value).
pub const SYSTEM: &str = "system";
/// The most families previewed at once, whatever the window asks for.
const MAX_PREVIEWS: usize = 120;
/// Pages of rows loaded above and below the rows on screen.
const PRELOAD_PAGES: usize = 2;
/// Faces bigger than this are not loaded for a preview (some CJK families are 20 MB and more).
const MAX_PREVIEW_BYTES: usize = 12 << 20;

const PLEX_SANS: &[u8] = include_bytes!("../../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Variable.ttf");
/// The kit's sans families in `sans_families()` order (regular, medium, semibold, bold), as
/// (font data name, weight).
const WEIGHTS: [(&str, u16); 4] =
    [("plex-sans", 400), (MEDIUM_FAMILY, 500), (SEMIBOLD_FAMILY, 600), (BOLD_FAMILY, 700)];
/// The desktop's font in the definitions, when there is one.
const SYSTEM_FACE: &str = "system-ui";

/// Which font the interface is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice<'a> {
    /// The theme's: Plex Sans under a theme, the desktop's font in Meshgrove's own look.
    Theme,
    /// The desktop's font, under any theme.
    System,
    /// An installed family.
    Family(&'a str),
}

impl<'a> Choice<'a> {
    /// The choice a `ui_font` setting stands for.
    pub fn of(setting: Option<&'a str>) -> Choice<'a> {
        match setting {
            None | Some("") => Choice::Theme,
            Some(SYSTEM) => Choice::System,
            Some(family) => Choice::Family(family),
        }
    }
}

/// The egui family that draws in Plex Sans, whichever font the interface uses.
pub fn theme_family() -> FontFamily {
    FontFamily::Name("mg-theme-font".into())
}

/// The egui family that draws in the desktop's font, whichever font the interface uses.
pub fn system_family() -> FontFamily {
    FontFamily::Name("mg-system-font".into())
}

/// The egui family that draws in installed `family`, once [`definitions`] was given it as a
/// preview. Check [`is_loaded`] before drawing with it: egui refuses unknown families.
pub fn preview_family(family: &str) -> FontFamily {
    FontFamily::Name(format!("preview:{family}").into())
}

/// Whether `ctx` can draw with `family` now.
pub fn is_loaded(ctx: &egui::Context, family: &FontFamily) -> bool {
    ctx.fonts(|f| f.definitions().families.contains_key(family))
}

/// The font definitions of the interface. `base` holds the fallbacks (egui's bundled fonts),
/// `system` the desktop's font; `themed` says whether a theme is on. A family that is not
/// installed gives the theme's font. Each of `previews` gets a preview family.
pub fn definitions(
    mut fonts: FontDefinitions,
    system: Option<Arc<FontData>>,
    choice: Choice<'_>,
    themed: bool,
    previews: &BTreeSet<String>,
) -> FontDefinitions {
    let fallbacks = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let system_first: Vec<String> = match system {
        Some(face) => {
            fonts.font_data.insert(SYSTEM_FACE.to_owned(), face);
            std::iter::once(SYSTEM_FACE.to_owned()).chain(fallbacks.iter().cloned()).collect()
        }
        None => fallbacks.clone(),
    };
    for (name, weight) in WEIGHTS {
        let mut data = FontData::from_static(PLEX_SANS);
        data.tweak.coords = egui::epaint::text::VariationCoords::new([(b"wght", f32::from(weight))]);
        fonts.font_data.insert(name.to_owned(), Arc::new(data));
    }
    let chosen = match choice {
        Choice::Family(family) => system_faces(family, database()),
        _ => Vec::new(),
    };
    // Plex in front for the theme's font under a theme, and behind a chosen family there.
    let plex = themed && choice != Choice::System;
    for (i, ((name, weight), family)) in WEIGHTS.into_iter().zip(sans_families()).enumerate() {
        let mut list: Vec<String> = Vec::new();
        if let Some(face) = chosen.get(i) {
            let key = format!("chosen-{weight}");
            fonts.font_data.insert(key.clone(), Arc::new(face.clone()));
            list.push(key);
        }
        if plex {
            list.push(name.to_owned());
        }
        list.extend(system_first.iter().cloned());
        fonts.families.insert(family, list);
    }
    fonts
        .families
        .insert(theme_family(), std::iter::once("plex-sans".to_owned()).chain(fallbacks.iter().cloned()).collect());
    fonts.families.insert(system_family(), system_first);
    let mut cache = PREVIEWS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // Faces that left the window go; the ones still in it are reused, not read again.
    cache.retain(|name, _| previews.contains(name));
    for name in previews.iter().take(MAX_PREVIEWS) {
        let face =
            cache.entry(name.clone()).or_insert_with(|| face(name, 400, database(), MAX_PREVIEW_BYTES).map(Arc::new));
        let Some(face) = face else { continue };
        let key = format!("preview:{name}");
        fonts.font_data.insert(key.clone(), face.clone());
        fonts.families.insert(preview_family(name), std::iter::once(key).chain(fallbacks.iter().cloned()).collect());
    }
    type_face::with_weight_families(fonts)
}

/// The preview faces loaded now, by family (`None`: not loadable, too big or gone).
static PREVIEWS: Mutex<BTreeMap<String, Option<Arc<FontData>>>> = Mutex::new(BTreeMap::new());

/// The rows of a long list whose fonts are loaded: the rows on screen and [`PRELOAD_PAGES`] pages
/// above and below. It moves only when the rows on screen come within a page of its edge, so
/// scrolling reloads about once per page, not on every step.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreviewWindow {
    window: Option<Range<usize>>,
}

impl PreviewWindow {
    /// The rows to load, given the rows on screen out of `count`. Returns whether that changed.
    pub fn follow(&mut self, visible: Range<usize>, count: usize) -> bool {
        let page = visible.len().max(1);
        let fits = self.window.as_ref().is_some_and(|w| {
            let above = w.start == 0 || visible.start >= w.start + page;
            let below = w.end >= count || visible.end + page <= w.end;
            w.end <= count && above && below
        });
        if fits {
            return false;
        }
        let start = visible.start.saturating_sub(PRELOAD_PAGES * page);
        let end = (visible.end + PRELOAD_PAGES * page).min(count);
        let window = start..end.max(start);
        let changed = self.window.as_ref() != Some(&window);
        self.window = Some(window);
        changed
    }

    pub fn rows(&self) -> Range<usize> {
        self.window.clone().unwrap_or(0..0)
    }

    /// Forget the window (the list closed or its rows changed).
    pub fn reset(&mut self) {
        self.window = None;
    }
}

/// The faces of installed `family` nearest to the kit's four weights, or none when it is not
/// installed.
fn system_faces(family: &str, db: &fontdb::Database) -> Vec<FontData> {
    WEIGHTS
        .iter()
        .map(|(_, weight)| face(family, *weight, db, usize::MAX))
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default()
}

/// The upright face of installed `family` nearest to `weight`, up to `max_bytes` big. A variable
/// face gets its weight axis set, like Plex.
fn face(family: &str, weight: u16, db: &fontdb::Database, max_bytes: usize) -> Option<FontData> {
    let query = fontdb::Query {
        families: &[fontdb::Family::Name(family)],
        weight: fontdb::Weight(weight),
        stretch: fontdb::Stretch::Normal,
        style: fontdb::Style::Normal,
    };
    let id = db.query(&query)?;
    // `query` falls back to any family; only an exact family match counts.
    if !db.face(id).is_some_and(|f| f.families.iter().any(|(name, _)| name.eq_ignore_ascii_case(family))) {
        return None;
    }
    let mut data = db
        .with_face_data(id, |bytes, index| {
            (bytes.len() <= max_bytes).then(|| {
                let mut data = FontData::from_owned(bytes.to_vec());
                data.index = index;
                data
            })
        })
        .flatten()?;
    data.tweak.coords = egui::epaint::text::VariationCoords::new([(b"wght", f32::from(weight))]);
    Some(data)
}

static DATABASE: OnceLock<fontdb::Database> = OnceLock::new();
static LOADING: AtomicBool = AtomicBool::new(false);

/// The installed fonts (loaded on first use; a few hundred milliseconds).
fn database() -> &'static fontdb::Database {
    DATABASE.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        db
    })
}

static FAMILIES: OnceLock<Vec<String>> = OnceLock::new();

/// The installed font families, sorted, or `None` while they are still being listed (the first
/// call starts listing them in the background and asks `ctx` to repaint when done).
pub fn families(ctx: &egui::Context) -> Option<&'static [String]> {
    if let Some(db) = DATABASE.get() {
        let names = FAMILIES.get_or_init(|| {
            let mut names: Vec<String> =
                db.faces().filter_map(|f| f.families.first().map(|(n, _)| n.clone())).collect();
            names.sort_by_key(|n| n.to_lowercase());
            names.dedup();
            names
        });
        return Some(names);
    }
    if !LOADING.swap(true, Ordering::SeqCst) {
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("font-list".into())
            .spawn(move || {
                database();
                ctx.request_repaint();
            })
            .ok();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(fonts: &FontDefinitions, family: &FontFamily) -> String {
        fonts.families[family][0].clone()
    }

    #[test]
    fn the_choice_and_the_theme_decide_the_first_face() {
        let system = || Some(Arc::new(FontData::from_static(PLEX_SANS)));
        let base = FontDefinitions::default;
        let none = BTreeSet::new();
        // Under a theme its own font leads, at each of the kit's weights.
        let fonts = definitions(base(), system(), Choice::Theme, true, &none);
        for ((name, _), family) in WEIGHTS.into_iter().zip(sans_families()) {
            assert_eq!(first(&fonts, &family), name);
        }
        // Meshgrove's own look and "system font" both lead with the desktop's font.
        for (choice, themed) in [(Choice::Theme, false), (Choice::System, true), (Choice::System, false)] {
            let fonts = definitions(base(), system(), choice, themed, &none);
            for family in sans_families() {
                assert_eq!(first(&fonts, &family), SYSTEM_FACE, "{choice:?} {themed}");
            }
        }
        // Without a desktop font egui's own lead, and every family is still bound.
        let fonts = definitions(base(), None, Choice::System, false, &none);
        for family in sans_families() {
            assert!(!fonts.families[&family].is_empty());
        }
        assert!(fonts.families.contains_key(&theme_family()) && fonts.families.contains_key(&system_family()));
    }

    #[test]
    fn the_setting_names_the_choice() {
        assert_eq!(Choice::of(None), Choice::Theme);
        assert_eq!(Choice::of(Some("")), Choice::Theme);
        assert_eq!(Choice::of(Some(SYSTEM)), Choice::System);
        assert_eq!(Choice::of(Some("Inter")), Choice::Family("Inter"));
    }

    #[test]
    fn a_family_that_is_not_installed_gives_the_theme_font() {
        let db = fontdb::Database::new();
        assert!(system_faces("No Such Family 4711", &db).is_empty());
        let previews = BTreeSet::from(["No Such Family 4711".to_owned()]);
        let fonts =
            definitions(FontDefinitions::default(), None, Choice::Family("No Such Family 4711"), true, &previews);
        assert_eq!(first(&fonts, &FontFamily::Proportional), "plex-sans");
        assert!(!fonts.families.contains_key(&preview_family("No Such Family 4711")));
    }

    #[test]
    fn the_window_keeps_two_pages_around_the_screen_and_moves_by_pages() {
        let mut w = PreviewWindow::default();
        // 10 rows on screen at the top of 1000: nothing above, two pages below.
        assert!(w.follow(0..10, 1000));
        assert_eq!(w.rows(), 0..30);
        // Scrolling within the window does not reload ...
        assert!(!w.follow(5..15, 1000));
        assert!(!w.follow(10..20, 1000));
        // ... until the screen comes within a page of its end.
        assert!(w.follow(11..21, 1000));
        assert_eq!(w.rows(), 0..41);
        // A jump to the end (from A to Z) holds only the last pages, never everything.
        assert!(w.follow(990..1000, 1000));
        assert_eq!(w.rows(), 970..1000);
        // A shorter list (a search) resets it.
        w.reset();
        assert!(w.follow(0..10, 12));
        assert_eq!(w.rows(), 0..12);
    }
}
