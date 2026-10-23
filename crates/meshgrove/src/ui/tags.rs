//! Tag colours and the suggested tags. Every tag has a slot in a calm 12-colour palette: the one
//! the user chose, a fitting one for the suggested tags, or one from a hash of its name, so a tag
//! looks the same everywhere and on every computer.

use std::collections::HashMap;

use egui::{Color32, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, Ui, pos2, vec2};

/// (background, text): pastels spread round the colour wheel (OKLCH L 0.78, C 0.10, every 30°
/// from 25°) with a very dark tone of the same hue as text, at least 7.5:1 contrast.
pub const PALETTE: [([u8; 3], [u8; 3]); 12] = [
    ([241, 158, 151], [57, 25, 23]),
    ([233, 166, 121], [54, 29, 10]),
    ([213, 179, 106], [47, 34, 3]),
    ([181, 191, 115], [35, 39, 7]),
    ([143, 201, 144], [19, 42, 20]),
    ([107, 204, 180], [0, 44, 35]),
    ([94, 202, 214], [0, 43, 47]),
    ([116, 194, 238], [5, 39, 57]),
    ([152, 183, 248], [24, 35, 60]),
    ([188, 171, 241], [38, 31, 58]),
    ([216, 161, 219], [48, 27, 49]),
    ([235, 157, 187], [55, 25, 37]),
];

/// Grey pills when colours are switched off in the settings.
const NEUTRAL: (Color32, Color32) = (Color32::from_rgb(74, 74, 74), Color32::from_rgb(230, 230, 230));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Themes,
    Properties,
    State,
    /// The user's own tags.
    Own,
}

impl Group {
    pub const SUGGESTED: [Group; 3] = [Group::Themes, Group::Properties, Group::State];

    pub fn title(self) -> String {
        match self {
            Group::Themes => t!("tags.group_themes"),
            Group::Properties => t!("tags.group_properties"),
            Group::State => t!("tags.group_state"),
            Group::Own => t!("tags.group_own"),
        }
        .into_owned()
    }
}

/// Suggested tags: (group, German, English, palette slot).
const SUGGESTIONS: &[(Group, &str, &str, u8)] = &[
    (Group::Themes, "Haushalt", "Household", 1),
    (Group::Themes, "Küche", "Kitchen", 2),
    (Group::Themes, "Bad", "Bathroom", 5),
    (Group::Themes, "Garten", "Garden", 4),
    (Group::Themes, "Balkon", "Balcony", 3),
    (Group::Themes, "Werkstatt", "Workshop", 1),
    (Group::Themes, "Werkzeug", "Tools", 8),
    (Group::Themes, "Keller", "Basement", 7),
    (Group::Themes, "Aufbewahrung", "Storage", 6),
    (Group::Themes, "Büro", "Office", 8),
    (Group::Themes, "Elektronik", "Electronics", 7),
    (Group::Themes, "Kinderzimmer", "Kids' room", 11),
    (Group::Themes, "Spielzeug", "Toys", 10),
    (Group::Themes, "Hobby", "Hobby", 9),
    (Group::Themes, "Tabletop", "Tabletop", 9),
    (Group::Themes, "Miniatur", "Miniature", 10),
    (Group::Themes, "Cosplay", "Cosplay", 11),
    (Group::Themes, "Airsoft", "Airsoft", 3),
    (Group::Themes, "Outdoor", "Outdoor", 4),
    (Group::Themes, "Haustier", "Pets", 1),
    (Group::Themes, "Auto", "Car", 8),
    (Group::Themes, "Fahrrad", "Bike", 6),
    (Group::Themes, "Deko", "Decor", 10),
    (Group::Themes, "Geschenk", "Gift", 11),
    (Group::Themes, "Weihnachten", "Christmas", 0),
    (Group::Themes, "Ostern", "Easter", 3),
    (Group::Themes, "Halloween", "Halloween", 1),
    (Group::Themes, "Ersatzteil", "Spare part", 7),
    (Group::Themes, "Kalibrierung", "Calibration", 5),
    (Group::Properties, "mehrfarbig", "multicolour", 10),
    (Group::Properties, "Print-in-Place", "print-in-place", 5),
    (Group::Properties, "ohne Stützen", "no supports", 4),
    (Group::Properties, "braucht Stützen", "needs supports", 0),
    (Group::Properties, "mehrteilig", "multi-part", 6),
    (Group::Properties, "Funktionsteil", "functional", 8),
    (Group::Properties, "beweglich", "articulated", 9),
    (Group::Properties, "Gewinde", "threads", 7),
    (Group::Properties, "Magnete", "magnets", 2),
    (Group::Properties, "Gewindeeinsätze", "heat-set inserts", 1),
    (Group::Properties, "Vasenmodus", "vase mode", 6),
    (Group::Properties, "PLA", "PLA", 4),
    (Group::Properties, "PETG", "PETG", 7),
    (Group::Properties, "TPU", "TPU", 10),
    (Group::Properties, "ASA", "ASA", 1),
    (Group::Properties, "Resin", "resin", 9),
    (Group::State, "eigenes Design", "own design", 4),
    (Group::State, "Remix", "remix", 9),
    (Group::State, "gekauft", "bought", 2),
    (Group::State, "kostenlos", "free", 5),
    (Group::State, "getestet", "tested", 5),
    (Group::State, "muss angepasst werden", "needs tweaking", 1),
    (Group::State, "fehlerhaft", "broken", 0),
];

fn suggestion(tag: &str) -> Option<&'static (Group, &'static str, &'static str, u8)> {
    SUGGESTIONS.iter().find(|(_, de, en, _)| de.eq_ignore_ascii_case(tag) || en.eq_ignore_ascii_case(tag))
}

/// The group a tag belongs to; tags that are not suggested are the user's own.
pub fn group_of(tag: &str) -> Group {
    suggestion(tag).map_or(Group::Own, |s| s.0)
}

/// The suggested tags of `group` in the UI language.
pub fn suggested(group: Group) -> Vec<&'static str> {
    let german = rust_i18n::locale().starts_with("de");
    SUGGESTIONS.iter().filter(|s| s.0 == group).map(|s| if german { s.1 } else { s.2 }).collect()
}

/// Palette slot from the tag name alone (FNV-1a of the lower-case name).
fn hashed_slot(tag: &str) -> u8 {
    let hash = tag.to_lowercase().bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
    (hash % PALETTE.len() as u32) as u8
}

/// How tags are drawn: coloured or grey, and the colours the user chose.
#[derive(Debug, Clone, Default)]
pub struct TagStyle {
    pub colored: bool,
    /// Chosen palette slots by lower-case tag name.
    pub chosen: HashMap<String, u8>,
}

impl TagStyle {
    /// The palette slot of a tag: chosen, else suggested, else from its name.
    pub fn slot(&self, tag: &str) -> u8 {
        let chosen = self.chosen.get(&tag.to_lowercase()).copied().filter(|&s| usize::from(s) < PALETTE.len());
        chosen.or_else(|| suggestion(tag).map(|s| s.3)).unwrap_or_else(|| hashed_slot(tag))
    }

    /// (background, text) of a tag's pill.
    pub fn colors(&self, tag: &str) -> (Color32, Color32) {
        if !self.colored {
            return NEUTRAL;
        }
        let (bg, fg) = PALETTE[usize::from(self.slot(tag))];
        (Color32::from_rgb(bg[0], bg[1], bg[2]), Color32::from_rgb(fg[0], fg[1], fg[2]))
    }
}

/// A tag chip as a button (info pane, settings); `label` may add to the name, e.g. " ✕".
pub fn chip(ui: &mut Ui, style: &TagStyle, tag: &str, label: &str) -> egui::Response {
    let (bg, fg) = style.colors(tag);
    let text = egui::RichText::new(label).small().color(fg);
    ui.add(egui::Button::new(text).fill(bg).stroke(Stroke::NONE).corner_radius(9.0))
}

/// A small coloured dot in front of a tag's name (sidebar, menus).
pub fn dot(ui: &mut Ui, style: &TagStyle, tag: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, style.colors(tag).0);
}

/// Paints a tag's colour dot (with a thin dark ring, so it shows on any thumbnail) whose right
/// edge sits at `right_top`, on a 13 px row.
pub fn paint_dot(p: &Painter, right_top: Pos2, color: Color32) {
    let center = pos2(right_top.x - 6.0, right_top.y + 6.0);
    p.circle(center, 5.0, color, Stroke::new(1.5, Color32::from_black_alpha(170)));
}

/// Paints a pill whose right edge sits at `right_top`; long names are cut with "…" to fit
/// `max_width`. Returns its rectangle.
pub fn paint_pill(p: &Painter, right_top: Pos2, label: &str, colors: (Color32, Color32), max_width: f32) -> Rect {
    pill(p, right_top, true, label, colors, max_width)
}

/// Like [`paint_pill`], but its left edge sits at `left_top` (pills in a row, left to right).
pub fn paint_pill_from_left(
    p: &Painter,
    left_top: Pos2,
    label: &str,
    colors: (Color32, Color32),
    max_width: f32,
) -> Rect {
    pill(p, left_top, false, label, colors, max_width)
}

fn pill(p: &Painter, anchor: Pos2, from_right: bool, label: &str, colors: (Color32, Color32), max_width: f32) -> Rect {
    let font = FontId::proportional(10.5);
    let pad = vec2(6.0, 1.5);
    let mut text = label.to_owned();
    let mut galley = p.layout_no_wrap(text.clone(), font.clone(), colors.1);
    while galley.size().x + 2.0 * pad.x > max_width && text.chars().count() > 1 {
        text.pop();
        let cut = format!("{}…", text.trim_end());
        galley = p.layout_no_wrap(cut, font.clone(), colors.1);
    }
    let size = galley.size() + 2.0 * pad;
    let left = if from_right { anchor.x - size.x } else { anchor.x };
    let rect = Rect::from_min_size(pos2(left, anchor.y), size);
    p.rect(rect, size.y / 2.0, colors.0, Stroke::NONE, StrokeKind::Inside);
    p.galley(rect.min + pad, galley, colors.1);
    rect
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: [u8; 3]) -> f32 {
        let lin = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
    }

    #[test]
    fn palette_text_is_readable() {
        for (bg, fg) in PALETTE {
            let contrast = (luminance(bg) + 0.05) / (luminance(fg) + 0.05);
            assert!(contrast >= 7.0, "{bg:?} on {fg:?}: {contrast:.1}:1");
        }
    }

    #[test]
    fn slots_are_stable_and_chosen_ones_win() {
        let mut style = TagStyle { colored: true, ..Default::default() };
        assert_eq!(style.slot("Mein Projekt"), style.slot("mein projekt"), "case does not matter");
        assert_eq!(style.slot("Garten"), 4);
        assert_eq!(style.slot("garden"), 4, "both languages of a suggestion");
        style.chosen.insert("garten".into(), 9);
        assert_eq!(style.slot("Garten"), 9);
        style.chosen.insert("kaputt".into(), 40);
        assert_eq!(style.slot("kaputt"), hashed_slot("kaputt"), "out-of-range choices are ignored");
        assert_eq!(TagStyle::default().colors("Garten"), NEUTRAL, "colours switched off");
    }

    #[test]
    fn suggestions_are_unique_and_grouped() {
        let mut seen = std::collections::HashSet::new();
        for (_, de, en, slot) in SUGGESTIONS {
            assert!(usize::from(*slot) < PALETTE.len());
            assert!(seen.insert(de.to_lowercase()), "{de} twice");
            if !de.eq_ignore_ascii_case(en) {
                assert!(seen.insert(en.to_lowercase()), "{en} twice");
            }
        }
        assert_eq!(group_of("PETG"), Group::Properties);
        assert_eq!(group_of("Remix"), Group::State);
        assert_eq!(group_of("Meine Sachen"), Group::Own);
    }
}
