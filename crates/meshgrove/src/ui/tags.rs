//! Tag colours and the suggested tags. Every tag has a slot in a calm 12-colour palette: the one
//! the user chose, a fitting one for the suggested tags, or one from a hash of its name, so a tag
//! looks the same everywhere and on every computer.

use std::collections::HashMap;

use meshgrove_core::config::SuggestedTag;

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

/// The suggested tags of `group` in the UI language.
pub fn suggested(group: Group, style: &TagStyle) -> Vec<String> {
    let german = rust_i18n::locale().starts_with("de");
    style
        .list(group)
        .iter()
        .map(|t| if german { &t.de } else { &t.en })
        .filter(|name| !name.trim().is_empty())
        .cloned()
        .collect()
}

/// Palette slot from the tag name alone (FNV-1a of the lower-case name).
fn hashed_slot(tag: &str) -> u8 {
    let hash = tag.to_lowercase().bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
    (hash % PALETTE.len() as u32) as u8
}

/// The palette slot a suggested tag is drawn in: its own, else from its name.
pub fn theme_slot(theme: &SuggestedTag) -> u8 {
    theme.color.filter(|&s| usize::from(s) < PALETTE.len()).unwrap_or_else(|| hashed_slot(&theme.de))
}

/// How tags are drawn: coloured or grey, and the colours the user chose.
#[derive(Debug, Clone, Default)]
pub struct TagStyle {
    pub colored: bool,
    /// The suggested tags by group (`Config::themes`, `properties`, `states`).
    pub themes: Vec<SuggestedTag>,
    pub properties: Vec<SuggestedTag>,
    pub states: Vec<SuggestedTag>,
    /// Chosen palette slots by lower-case tag name.
    pub chosen: HashMap<String, u8>,
}

impl TagStyle {
    /// The suggested tags of a group (none for the user's own).
    pub fn list(&self, group: Group) -> &[SuggestedTag] {
        match group {
            Group::Themes => &self.themes,
            Group::Properties => &self.properties,
            Group::State => &self.states,
            Group::Own => &[],
        }
    }

    /// The group and suggestion a tag belongs to, if it is a suggested one.
    fn find(&self, tag: &str) -> Option<(Group, &SuggestedTag)> {
        Group::SUGGESTED.into_iter().find_map(|g| self.list(g).iter().find(|t| t.matches(tag)).map(|t| (g, t)))
    }

    /// The group a tag belongs to: one of the suggested lists, else the user's own.
    pub fn group_of(&self, tag: &str) -> Group {
        self.find(tag).map_or(Group::Own, |(g, _)| g)
    }

    /// The palette slot of a tag: chosen, else its suggestion's colour, else from its name.
    pub fn slot(&self, tag: &str) -> u8 {
        let valid = |s: &u8| usize::from(*s) < PALETTE.len();
        let chosen = self.chosen.get(&tag.to_lowercase()).copied().filter(valid);
        let suggested = || self.find(tag).and_then(|(_, t)| t.color).filter(valid);
        chosen.or_else(suggested).unwrap_or_else(|| hashed_slot(tag))
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
    use meshgrove_core::config::{default_properties, default_states, default_themes};

    use super::*;

    fn default_style(colored: bool) -> TagStyle {
        TagStyle {
            colored,
            themes: default_themes(),
            properties: default_properties(),
            states: default_states(),
            ..Default::default()
        }
    }

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
        let mut style = default_style(true);
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
    fn defaults_are_grouped() {
        let style = default_style(false);
        assert_eq!(style.group_of("PETG"), Group::Properties);
        assert_eq!(style.group_of("Remix"), Group::State);
        assert_eq!(style.group_of("garden"), Group::Themes);
        assert_eq!(style.group_of("Meine Sachen"), Group::Own);
        assert_eq!(suggested(Group::State, &style).len(), 7);
    }

    #[test]
    fn custom_themes_replace_the_built_in_ones() {
        let style = TagStyle {
            colored: true,
            themes: vec![
                SuggestedTag::named("Drachen"),
                SuggestedTag { de: "Burg".into(), en: "Castle".into(), color: Some(3) },
            ],
            ..Default::default()
        };
        assert_eq!(style.group_of("drachen"), Group::Themes);
        assert_eq!(style.group_of("Garten"), Group::Own, "no longer a theme");
        assert_eq!(style.slot("castle"), 3);
        assert_eq!(style.slot("Drachen"), hashed_slot("Drachen"), "no colour set: from the name");
        assert_eq!(suggested(Group::Themes, &style).len(), 2);
    }
}
