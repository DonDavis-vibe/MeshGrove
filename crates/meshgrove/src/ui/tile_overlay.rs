//! The marks over a tile's picture (file type, filaments, duplicates, tags, print bed check,
//! favourite, print state), placed by the user's tile layout: the top corners stack theirs
//! downwards, the bottom corners line theirs up from the corner inwards. The gallery and the
//! preview in the settings draw them alike.

use egui::{Color32, Painter, Pos2, Rect, Visuals, pos2, vec2};
use meshgrove_core::config::{CheckStyle, FilamentStyle, TileBadge, TileLayout, TileSlot};

use super::icons;
use super::tags::{self, TagStyle};

/// The favourite star on tiles.
pub const FAVORITE_GOLD: Color32 = Color32::from_rgb(245, 197, 66);
/// Printed marks on tiles.
pub const PRINTED_GREEN: Color32 = Color32::from_rgb(143, 201, 144);

/// Distance of the marks from the picture's edges.
const INSET: f32 = 4.0;
/// Room the top corners leave free for the marks at the bottom.
const BOTTOM_ROW: f32 = 26.0;
/// Between marks stacked in a top corner.
const STACK_GAP: f32 = 2.0;
/// Between marks in a row at the bottom.
const ROW_GAP: f32 = 3.0;
/// Thickness of the filament stripe.
const STRIPE: f32 = 5.0;
/// Rows of tag pills and of tag dots.
const PILL_ROW: f32 = 18.0;
const DOT_ROW: f32 = 13.0;
const DOT_WIDTH: f32 = 12.0;
/// The "+N" pill of tags left out.
const MORE: (Color32, Color32) = (Color32::from_black_alpha(170), Color32::from_gray(230));

/// What the print bed check says.
pub enum Check {
    /// Fits only when turned by this angle (as shown, "90°").
    Rotated(String),
    TooBig,
    /// G-code sliced for a printer that is none of the user's.
    OtherPrinter,
    /// The printability check found something, or the library flagged the file.
    Problem,
}

pub enum PrintMark {
    /// On a print list: copies done of those wanted.
    List {
        done: u32,
        wanted: u32,
    },
    Printed(u32),
    /// The newest print failed.
    Failed,
}

/// What one tile has to show; `None` or empty: nothing of that kind.
#[derive(Default)]
pub struct Overlays<'a> {
    pub file_type: Option<String>,
    /// Without repeats.
    pub filaments: Vec<[u8; 3]>,
    pub duplicates: Option<usize>,
    pub tags: &'a [String],
    pub check: Option<Check>,
    pub favorite: bool,
    pub print: Option<PrintMark>,
}

/// The settings the marks are drawn with.
pub struct Look<'a> {
    pub layout: &'a TileLayout,
    /// Tags as colour dots instead of names.
    pub tag_dots: bool,
    /// Tags shown at most.
    pub tag_count: usize,
    pub tag_style: &'a TagStyle,
}

/// Paints the marks of a tile over its picture `img`; returns where each one went.
pub fn paint(p: &Painter, img: Rect, overlays: &Overlays, look: &Look, visuals: &Visuals) -> Vec<(TileBadge, Rect)> {
    let marks = Marks { p, img, overlays, look, visuals };
    let layout = look.layout;
    let mut placed = Vec::new();
    let (mut top, mut bottom) = (img.min.y + INSET, img.max.y - INSET);
    let stripe = layout.filaments == FilamentStyle::Stripe;
    if stripe && !overlays.filaments.is_empty() && layout.shows(TileBadge::Filaments) {
        // Along the edge of its corner, under the other marks there.
        let band = if layout.slot_of(TileBadge::Filaments).is_top() {
            top += STRIPE;
            Rect::from_min_size(img.min, vec2(img.width(), STRIPE))
        } else {
            bottom -= STRIPE;
            Rect::from_min_max(pos2(img.min.x, img.max.y - STRIPE), img.max)
        };
        icons::filament_stripe(p, band, &overlays.filaments);
        placed.push((TileBadge::Filaments, band));
    }
    let shown = |badge: TileBadge| match badge {
        TileBadge::Filaments => !stripe && !overlays.filaments.is_empty(),
        TileBadge::Tags => !overlays.tags.is_empty(),
        other => marks.badge(other).is_some(),
    };
    let filled = |slot: TileSlot| layout.slot(slot).iter().any(|&b| shown(b));
    let limit = if filled(TileSlot::BottomLeft) || filled(TileSlot::BottomRight) {
        bottom + INSET - BOTTOM_ROW
    } else {
        bottom
    };
    // Tags and filament dots shrink to fit; the other marks take the room they need.
    let shrinks = |slot: TileSlot| {
        layout.slot(slot).iter().any(|&b| shown(b) && matches!(b, TileBadge::Tags | TileBadge::Filaments))
    };
    let fixed = |slot: TileSlot| -> f32 {
        let labels = layout.slot(slot).iter().filter_map(|&b| marks.badge(b));
        labels.map(|(label, _, fg)| icons::badge_galley(p, &label, fg).1.x + ROW_GAP).sum()
    };
    for slot in TileSlot::CORNERS {
        let items: Vec<TileBadge> = layout.slot(slot).iter().copied().filter(|&b| shown(b)).collect();
        // Next to marks in the other corner of the same edge: half the width, or along the
        // bottom what the other corner's fixed marks leave.
        let other = across(slot);
        let full = img.width() - 2.0 * INSET;
        let width = if !filled(other) {
            full
        } else if slot.is_top() || shrinks(other) {
            img.width() / 2.0 - INSET - 2.0
        } else {
            full - fixed(other) - ROW_GAP
        };
        if slot.is_top() {
            marks.stack(slot, &items, top, limit, width, &mut placed);
        } else {
            marks.row(slot, &items, bottom, width, &mut placed);
        }
    }
    placed
}

/// The other corner of the same edge.
fn across(slot: TileSlot) -> TileSlot {
    match slot {
        TileSlot::TopLeft => TileSlot::TopRight,
        TileSlot::TopRight => TileSlot::TopLeft,
        TileSlot::BottomLeft => TileSlot::BottomRight,
        TileSlot::BottomRight => TileSlot::BottomLeft,
        TileSlot::Hidden => TileSlot::Hidden,
    }
}

struct Marks<'a> {
    p: &'a Painter,
    img: Rect,
    overlays: &'a Overlays<'a>,
    look: &'a Look<'a>,
    visuals: &'a Visuals,
}

impl Marks<'_> {
    /// Text and colours (background, text) of a mark drawn as a plain badge: all but the
    /// filaments and the tags.
    fn badge(&self, badge: TileBadge) -> Option<(String, Color32, Color32)> {
        let o = self.overlays;
        let warn = self.visuals.warn_fg_color.linear_multiply(0.9);
        let dark = Color32::from_black_alpha(150);
        let light = Color32::from_gray(230);
        let symbol = self.look.layout.check == CheckStyle::Symbol;
        let text = |full: String, short: &str| if symbol { short.to_owned() } else { full };
        Some(match badge {
            TileBadge::FileType => (o.file_type.clone()?, dark, light),
            TileBadge::Duplicates => (format!("{}×", o.duplicates?), warn, Color32::BLACK),
            TileBadge::Check => match o.check.as_ref()? {
                Check::Rotated(angle) => (text(format!("↻ {angle}"), "↻"), warn, Color32::BLACK),
                Check::TooBig => {
                    (text(t!("bed.badge_too_big").to_string(), "✕"), self.visuals.error_fg_color, Color32::WHITE)
                }
                // Kept small: a folder of G-code for a second printer would otherwise shout on every tile.
                Check::OtherPrinter => (text("⚠ 🖨".to_owned(), "⚠"), warn, Color32::BLACK),
                Check::Problem => ("⚠".to_owned(), warn, Color32::BLACK),
            },
            TileBadge::Favorite => o.favorite.then(|| ("★".to_owned(), dark, FAVORITE_GOLD))?,
            TileBadge::PrintState => match *o.print.as_ref()? {
                PrintMark::List { done, wanted } if done >= wanted => {
                    (format!("✓ {done}/{wanted}"), PRINTED_GREEN, Color32::BLACK)
                }
                PrintMark::List { done, wanted } => (format!("{done}/{wanted}"), dark, light),
                PrintMark::Printed(n) => (format!("✓{n}"), dark, PRINTED_GREEN),
                PrintMark::Failed => ("✗".to_owned(), dark, self.visuals.error_fg_color),
            },
            TileBadge::Filaments | TileBadge::Tags => return None,
        })
    }

    /// The marks of a top corner, one below the other from `y`, the tags ending above `limit`.
    fn stack(
        &self,
        slot: TileSlot,
        items: &[TileBadge],
        y: f32,
        limit: f32,
        width: f32,
        placed: &mut Vec<(TileBadge, Rect)>,
    ) {
        let left = slot.is_left();
        let edge = if left { self.img.min.x + INSET } else { self.img.max.x - INSET };
        let at = |y: f32, w: f32| pos2(if left { edge } else { edge - w }, y);
        let mut y = y;
        for (i, &badge) in items.iter().enumerate() {
            let (rect, height) = match badge {
                TileBadge::Tags => {
                    // As many rows as fit, keeping room for the marks below them.
                    let below = (items.len() - i - 1) as f32 * (icons::BADGE_HEIGHT + STACK_GAP);
                    match self.tag_column(edge, left, y, limit - below) {
                        Some(column) => column,
                        None => continue,
                    }
                }
                TileBadge::Filaments => {
                    let colors = &self.overlays.filaments;
                    let max = self.dots_that_fit(width);
                    let size = icons::filament_dots_size(self.p, colors.len(), max);
                    let rect = icons::filament_dots(self.p, at(y, size.x), colors, max);
                    (rect, size.y)
                }
                _ => {
                    let Some((label, bg, fg)) = self.badge(badge) else { continue };
                    let (galley, size) = icons::badge_galley(self.p, &label, fg);
                    let rect = Rect::from_min_size(at(y, size.x), size);
                    icons::paint_badge(self.p, rect, galley, bg, fg);
                    (rect, icons::BADGE_HEIGHT)
                }
            };
            placed.push((badge, rect));
            y += height + STACK_GAP;
        }
    }

    /// The marks of a bottom corner side by side from the corner inwards within `width`, their
    /// lower edges at `bottom`.
    fn row(&self, slot: TileSlot, items: &[TileBadge], bottom: f32, width: f32, placed: &mut Vec<(TileBadge, Rect)>) {
        let left = slot.is_left();
        let start = if left { self.img.min.x + INSET } else { self.img.max.x - INSET };
        let mut x = start;
        for &badge in items {
            let room = width - (x - start).abs();
            let at = |w: f32, h: f32| pos2(if left { x } else { x - w }, bottom - h);
            let rect = match badge {
                TileBadge::Tags => match self.tag_row(x, left, bottom, room) {
                    Some(rect) => rect,
                    None => continue,
                },
                TileBadge::Filaments => {
                    let colors = &self.overlays.filaments;
                    let max = self.dots_that_fit(room);
                    let size = icons::filament_dots_size(self.p, colors.len(), max);
                    icons::filament_dots(self.p, at(size.x, size.y), colors, max)
                }
                _ => {
                    let Some((label, bg, fg)) = self.badge(badge) else { continue };
                    let (galley, size) = icons::badge_galley(self.p, &label, fg);
                    let rect = Rect::from_min_size(at(size.x, size.y), size);
                    icons::paint_badge(self.p, rect, galley, bg, fg);
                    rect
                }
            };
            placed.push((badge, rect));
            x = if left { rect.max.x + ROW_GAP } else { rect.min.x - ROW_GAP };
        }
    }

    /// Filament dots fitting `room` (at least one; the rest as "+N").
    fn dots_that_fit(&self, room: f32) -> usize {
        let count = self.overlays.filaments.len();
        (1..=icons::FILAMENT_DOTS)
            .rev()
            .find(|&max| icons::filament_dots_size(self.p, count, max).x <= room)
            .unwrap_or(1)
    }

    /// The pill's (or dot's) colours of a tag.
    fn tag_colors(&self, tag: &str) -> (Color32, Color32) {
        self.look.tag_style.colors(tag)
    }

    /// One tag per row from `y` down to `limit`, as many as set; the last row says "+N" when
    /// more are left. Its rectangle and height.
    fn tag_column(&self, edge: f32, left: bool, y: f32, limit: f32) -> Option<(Rect, f32)> {
        let tags = self.overlays.tags;
        let row = if self.look.tag_dots { DOT_ROW } else { PILL_ROW };
        let rows = (((limit - y) / row).floor().max(0.0) as usize).min(self.look.tag_count).min(tags.len());
        let max_width = self.img.width() * 0.45;
        let mut rect = Rect::NOTHING;
        for (i, tag) in tags.iter().enumerate().take(rows) {
            let top = y + i as f32 * row;
            let more = tags.len() - i;
            let r = if i + 1 == rows && more > 1 {
                self.pill(edge, left, top, &format!("+{more}"), MORE, max_width)
            } else if self.look.tag_dots {
                let right = if left { edge + DOT_WIDTH } else { edge };
                tags::paint_dot(self.p, pos2(right, top), self.tag_colors(tag).0)
            } else {
                self.pill(edge, left, top, tag, self.tag_colors(tag), max_width)
            };
            rect = rect.union(r);
        }
        (rows > 0).then_some((rect, rows as f32 * row))
    }

    /// Tags side by side from `x` inwards within `room`, as many as set and fit; the last place
    /// says "+N" when more are left.
    fn tag_row(&self, x: f32, left: bool, bottom: f32, room: f32) -> Option<Rect> {
        let tags = self.overlays.tags;
        let dots = self.look.tag_dots;
        let max_width = (self.img.width() * 0.45).min(room);
        let width = |label: &str| if dots { DOT_WIDTH } else { tags::pill_size(self.p, label, max_width).x };
        let more_width = |left_out: usize| tags::pill_size(self.p, &format!("+{left_out}"), max_width).x;
        let count = self.look.tag_count.min(tags.len());
        let widths: Vec<f32> = tags.iter().take(count).map(|t| width(t)).collect();
        let fits = |k: usize, extra: f32| widths[..k].iter().map(|w| w + ROW_GAP).sum::<f32>() + extra <= room;
        // All of them if they fit, else as many as fit next to the "+N".
        let shown = if tags.len() <= count && fits(tags.len(), 0.0) {
            tags.len()
        } else {
            (0..count).rev().find(|&k| fits(k, more_width(tags.len() - k)))?
        };
        let mut x = x;
        let mut rect = Rect::NOTHING;
        let mut place = |w: f32| {
            let anchor = if left { x } else { x - w };
            x = if left { x + w + ROW_GAP } else { x - w - ROW_GAP };
            anchor
        };
        for (tag, &w) in tags.iter().zip(&widths).take(shown) {
            let r = if dots {
                let anchor = place(w);
                tags::paint_dot(self.p, pos2(anchor + DOT_WIDTH, bottom - DOT_WIDTH - 0.5), self.tag_colors(tag).0)
            } else {
                let h = tags::pill_size(self.p, tag, max_width).y;
                tags::paint_pill_from_left(self.p, pos2(place(w), bottom - h), tag, self.tag_colors(tag), max_width)
            };
            rect = rect.union(r);
        }
        if shown < tags.len() {
            let label = format!("+{}", tags.len() - shown);
            let size = tags::pill_size(self.p, &label, max_width);
            let r = tags::paint_pill_from_left(self.p, pos2(place(size.x), bottom - size.y), &label, MORE, max_width);
            rect = rect.union(r);
        }
        Some(rect)
    }

    /// A pill with one edge at `edge` (left or right) and its top at `top`.
    fn pill(&self, edge: f32, left: bool, top: f32, label: &str, colors: (Color32, Color32), max_width: f32) -> Rect {
        if left {
            tags::paint_pill_from_left(self.p, Pos2::new(edge, top), label, colors, max_width)
        } else {
            tags::paint_pill(self.p, Pos2::new(edge, top), label, colors, max_width)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The picture of a 160 px tile.
    const IMG: Rect = Rect { min: Pos2::new(6.0, 6.0), max: Pos2::new(154.0, 154.0) };

    fn tags() -> Vec<String> {
        ["Garten", "Kalibrierung", "Werkstatt", "Zubehör"].map(String::from).to_vec()
    }

    fn sample(tags: &[String]) -> Overlays<'_> {
        Overlays {
            file_type: Some("3MF".to_owned()),
            filaments: vec![[200, 60, 50], [50, 160, 150], [230, 230, 230]],
            duplicates: Some(2),
            tags,
            check: Some(Check::Rotated("90°".to_owned())),
            favorite: true,
            print: Some(PrintMark::Printed(2)),
        }
    }

    /// Paints `overlays` over `img` with `layout` and three tags at most; where the marks went.
    fn draw(img: Rect, overlays: &Overlays, layout: &TileLayout, dots: bool) -> Vec<(TileBadge, Rect)> {
        let ctx = egui::Context::default();
        let mut placed = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let style = TagStyle::default();
            let look = Look { layout, tag_dots: dots, tag_count: 3, tag_style: &style };
            placed = paint(ui.painter(), img, overlays, &look, &egui::Visuals::dark());
        });
        // The font atlas was never uploaded anywhere.
        output.textures_delta.clear();
        placed
    }

    fn rect_of(placed: &[(TileBadge, Rect)], badge: TileBadge) -> Rect {
        placed.iter().find(|(b, _)| *b == badge).map(|(_, r)| *r).unwrap_or_else(|| panic!("{badge:?} not drawn"))
    }

    fn assert_apart(placed: &[(TileBadge, Rect)]) {
        for (i, (a, ra)) in placed.iter().enumerate() {
            for (b, rb) in &placed[i + 1..] {
                assert!(!ra.intersects(*rb), "{a:?} {ra:?} overlaps {b:?} {rb:?}");
            }
        }
    }

    #[test]
    fn the_standard_layout_puts_the_marks_where_they_always_were() {
        let tags = tags();
        let placed = draw(IMG, &sample(&tags), &TileLayout::default(), false);
        assert_eq!(placed.len(), 7);
        assert_apart(&placed);
        let filaments = rect_of(&placed, TileBadge::Filaments);
        assert_eq!(filaments.min, IMG.min + vec2(4.0, 4.0));
        let duplicates = rect_of(&placed, TileBadge::Duplicates);
        assert_eq!((duplicates.max.x, duplicates.min.y), (IMG.max.x - 4.0, IMG.min.y + 4.0));
        let tags = rect_of(&placed, TileBadge::Tags);
        assert_eq!((tags.max.x, tags.min.y), (IMG.max.x - 4.0, IMG.min.y + 4.0 + icons::BADGE_HEIGHT + 2.0));
        let file_type = rect_of(&placed, TileBadge::FileType);
        assert_eq!((file_type.min.x, file_type.max.y), (IMG.min.x + 4.0, IMG.max.y - 4.0));
        let check = rect_of(&placed, TileBadge::Check);
        assert_eq!(check.max, IMG.max - vec2(4.0, 4.0));
        let favorite = rect_of(&placed, TileBadge::Favorite);
        assert_eq!((favorite.max.x, favorite.max.y), (check.min.x - 3.0, check.max.y));
        let print = rect_of(&placed, TileBadge::PrintState);
        assert_eq!(print.max.x, favorite.min.x - 3.0);
    }

    #[test]
    fn small_tiles_fold_the_tags_into_a_count() {
        let tags = tags();
        let img = Rect::from_min_size(pos2(6.0, 6.0), vec2(84.0, 84.0));
        let placed = draw(img, &sample(&tags), &TileLayout::default(), false);
        let tags = rect_of(&placed, TileBadge::Tags);
        assert!(tags.height() <= PILL_ROW, "one row only: {tags:?}");
        let untagged = draw(img, &sample(&[]), &TileLayout::default(), false);
        assert!(untagged.iter().all(|(b, _)| *b != TileBadge::Tags), "no tags, no mark");
    }

    #[test]
    fn marks_go_where_the_layout_says() {
        let names = tags();
        let mut layout = TileLayout::default();
        layout.place(TileBadge::Tags, TileSlot::BottomLeft, usize::MAX);
        layout.place(TileBadge::Favorite, TileSlot::Hidden, 0);
        layout.place(TileBadge::PrintState, TileSlot::Hidden, 0);
        layout.place(TileBadge::FileType, TileSlot::TopRight, 0);
        let placed = draw(IMG, &sample(&names), &layout, false);
        assert!(placed.iter().all(|(b, _)| !matches!(b, TileBadge::Favorite | TileBadge::PrintState)), "hidden");
        assert_apart(&placed);
        // Tags side by side from the bottom-left corner, up to the check on the right.
        let row = rect_of(&placed, TileBadge::Tags);
        assert_eq!((row.min.x, row.max.y), (IMG.min.x + 4.0, IMG.max.y - 4.0));
        assert!(row.width() > PILL_ROW * 2.0 && row.height() < PILL_ROW, "side by side: {row:?}");
        assert!(row.max.x < rect_of(&placed, TileBadge::Check).min.x, "{row:?}");
        let file_type = rect_of(&placed, TileBadge::FileType);
        assert_eq!((file_type.max.x, file_type.min.y), (IMG.max.x - 4.0, IMG.min.y + 4.0));
        assert!(rect_of(&placed, TileBadge::Duplicates).min.y > file_type.max.y);
        // Dots in a row too, narrower than the names.
        let dots = rect_of(&draw(IMG, &sample(&names), &layout, true), TileBadge::Tags);
        assert!(dots.height() < PILL_ROW && dots.width() < row.width(), "{dots:?}");
    }

    #[test]
    fn stripes_and_symbols() {
        let tags = tags();
        let mut layout =
            TileLayout { filaments: FilamentStyle::Stripe, check: CheckStyle::Symbol, ..Default::default() };
        let too_big = Overlays { check: Some(Check::TooBig), ..sample(&tags) };
        let placed = draw(IMG, &too_big, &layout, false);
        let stripe = rect_of(&placed, TileBadge::Filaments);
        assert_eq!(stripe, Rect::from_min_size(IMG.min, vec2(IMG.width(), STRIPE)));
        assert_eq!(rect_of(&placed, TileBadge::Duplicates).min.y, IMG.min.y + 4.0 + STRIPE);
        let symbol = rect_of(&placed, TileBadge::Check).width();
        layout.check = CheckStyle::Text;
        let text = rect_of(&draw(IMG, &too_big, &layout, false), TileBadge::Check).width();
        assert!(symbol < text, "{symbol} < {text}");
        layout.place(TileBadge::Filaments, TileSlot::BottomRight, 0);
        let placed = draw(IMG, &too_big, &layout, false);
        assert_eq!(rect_of(&placed, TileBadge::Filaments).max, IMG.max);
        assert_eq!(rect_of(&placed, TileBadge::Check).max.y, IMG.max.y - 4.0 - STRIPE);
    }
}
