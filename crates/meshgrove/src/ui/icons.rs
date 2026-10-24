//! Simple painted icons, so tiles without a picture still read at a glance.

use std::sync::Arc;

use egui::{Align2, Color32, FontId, Galley, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2, pos2, vec2};

/// KDE Breeze folder blue.
pub const FOLDER_BLUE: Color32 = Color32::from_rgb(61, 174, 233);
/// Archives get a warm tone so they stand apart from folders.
pub const ARCHIVE_AMBER: Color32 = Color32::from_rgb(222, 164, 66);

/// Folder in the middle of the square `r`.
pub fn folder(p: &Painter, r: Rect) {
    let w = r.width();
    let body =
        Rect::from_min_max(pos2(r.min.x + w * 0.08, r.min.y + w * 0.26), pos2(r.max.x - w * 0.08, r.max.y - w * 0.14));
    let tab = Rect::from_min_size(pos2(body.min.x, body.min.y - w * 0.08), vec2(w * 0.34, w * 0.14));
    let back = FOLDER_BLUE.linear_multiply(0.7);
    p.rect_filled(tab, w * 0.03, back);
    p.rect_filled(body, w * 0.05, back);
    let front = Rect::from_min_max(pos2(body.min.x, body.min.y + w * 0.08), body.max);
    p.rect_filled(front, w * 0.05, FOLDER_BLUE);
}

/// Folder whose contents peek out: up to four previews in the folder body, the front flap
/// drawn translucently over their lower part.
/// One picture in a folder mosaic, optionally fading into the next one (`.1` = how far, 0..1).
pub type Preview = (egui::TextureId, egui::Vec2);
pub type MosaicCell = (Preview, Option<(Preview, f32)>);

pub fn folder_mosaic(p: &Painter, r: Rect, previews: &[MosaicCell], paper: Color32, color: Color32) {
    let w = r.width();
    let body =
        Rect::from_min_max(pos2(r.min.x + w * 0.04, r.min.y + w * 0.12), pos2(r.max.x - w * 0.04, r.max.y - w * 0.04));
    let tab = Rect::from_min_size(pos2(body.min.x, body.min.y - w * 0.07), vec2(w * 0.32, w * 0.12));
    let back = color.linear_multiply(0.7);
    p.rect_filled(tab, w * 0.03, back);
    p.rect_filled(body, w * 0.05, back);

    let inner = body.shrink(w * 0.045);
    let cells: Vec<Rect> = match previews.len() {
        1 => vec![inner],
        2 => split_h(inner).to_vec(),
        _ => {
            let [top, bottom] = split_v(inner);
            let [a, b] = split_h(top);
            let [c, d] = split_h(bottom);
            vec![a, b, c, d]
        }
    };
    for (cell, (now, next)) in cells.iter().zip(previews) {
        let cell = cell.shrink(w * 0.01);
        p.rect_filled(cell, w * 0.025, paper);
        let draw = |(texture, size): Preview, opacity: f32| {
            let scale = (cell.width() / size.x).min(cell.height() / size.y);
            let img = Rect::from_center_size(cell.center(), size * scale);
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            p.image(texture, img, uv, Color32::WHITE.gamma_multiply(opacity));
        };
        match next {
            Some((next, fade)) => {
                draw(*now, 1.0 - fade);
                draw(*next, *fade);
            }
            None => draw(*now, 1.0),
        }
    }

    let flap = Rect::from_min_max(pos2(body.min.x, body.max.y - body.height() * 0.3), body.max);
    p.rect_filled(flap, w * 0.05, color.gamma_multiply(0.85));
}

fn split_h(r: Rect) -> [Rect; 2] {
    let mid = r.center().x;
    [Rect::from_min_max(r.min, pos2(mid, r.max.y)), Rect::from_min_max(pos2(mid, r.min.y), r.max)]
}

fn split_v(r: Rect) -> [Rect; 2] {
    let mid = r.center().y;
    [Rect::from_min_max(r.min, pos2(r.max.x, mid)), Rect::from_min_max(pos2(r.min.x, mid), r.max)]
}

/// Sheet of paper with a folded corner and a short label (file extension) on it.
pub fn file(p: &Painter, r: Rect, label: &str, fill: Color32, ink: Color32) {
    let w = r.width();
    let page = Rect::from_center_size(r.center(), vec2(w * 0.56, w * 0.72));
    let fold = w * 0.14;
    let pts = vec![
        page.left_top(),
        pos2(page.right() - fold, page.top()),
        pos2(page.right(), page.top() + fold),
        page.right_bottom(),
        page.left_bottom(),
    ];
    p.add(egui::Shape::convex_polygon(pts, fill, Stroke::new(1.0, ink.linear_multiply(0.4))));
    p.add(egui::Shape::convex_polygon(
        vec![
            pos2(page.right() - fold, page.top()),
            pos2(page.right(), page.top() + fold),
            pos2(page.right() - fold, page.top() + fold),
        ],
        ink.linear_multiply(0.25),
        Stroke::NONE,
    ));
    if !label.is_empty() {
        let size = (w * 0.13).clamp(9.0, 22.0);
        p.text(page.center(), Align2::CENTER_CENTER, label, FontId::monospace(size), ink);
    }
}

/// Padding around a badge's text.
const BADGE_PAD: Vec2 = vec2(5.0, 2.0);

/// A badge's text laid out, and the size of the badge around it.
pub fn badge_galley(p: &Painter, label: &str, fg: Color32) -> (Arc<Galley>, Vec2) {
    let galley = p.layout_no_wrap(label.to_owned(), FontId::proportional(10.5), fg);
    let size = galley.size() + 2.0 * BADGE_PAD;
    (galley, size)
}

/// Paints a badge laid out by [`badge_galley`] into `rect`.
pub fn paint_badge(p: &Painter, rect: Rect, galley: Arc<Galley>, bg: Color32, fg: Color32) {
    p.rect(rect, 4.0, bg, Stroke::NONE, StrokeKind::Inside);
    p.galley(rect.min + BADGE_PAD, galley, fg);
}

/// Small badge in the upper-right corner (e.g. "2×" for duplicates).
pub fn corner_badge(p: &Painter, r: Rect, label: &str, bg: Color32, fg: Color32) {
    let (galley, size) = badge_galley(p, label, fg);
    let rect = Rect::from_min_size(pos2(r.max.x - size.x - 4.0, r.min.y + 4.0), size);
    paint_badge(p, rect, galley, bg, fg);
}

/// Small badge in the lower-right corner (e.g. the print bed check). Returns its rectangle.
pub fn bottom_right_badge(p: &Painter, r: Rect, label: &str, bg: Color32, fg: Color32) -> Rect {
    badge_left_of(p, pos2(r.max.x - 4.0, r.max.y - 4.0), label, bg, fg)
}

/// Small badge whose lower-right corner is `corner` (badges in a row, right to left).
pub fn badge_left_of(p: &Painter, corner: egui::Pos2, label: &str, bg: Color32, fg: Color32) -> Rect {
    let (galley, size) = badge_galley(p, label, fg);
    let rect = Rect::from_min_size(corner - size, size);
    paint_badge(p, rect, galley, bg, fg);
    rect
}

/// Height of a badge (to stack things below it).
pub const BADGE_HEIGHT: f32 = 10.5 * 1.3 + 4.0;

/// Filament dots a tile shows at most (then "+N").
pub const FILAMENT_DOTS: usize = 8;

const DOT_RADIUS: f32 = 4.5;
const DOT_GAP: f32 = 3.0;
const DOTS_PAD: Vec2 = vec2(4.0, 3.0);

/// How many of `count` colours show as dots when at most `max` do, the "+N" for the rest, and
/// the size of the pill around them.
fn dots_layout(p: &Painter, count: usize, max: usize) -> (usize, Option<Arc<Galley>>, Vec2) {
    let shown = count.min(max);
    let more = (count > shown)
        .then(|| p.layout_no_wrap(format!("+{}", count - shown), FontId::proportional(9.5), Color32::from_gray(230)));
    let dots_w = shown as f32 * (2.0 * DOT_RADIUS + DOT_GAP) - DOT_GAP;
    let more_w = more.as_ref().map_or(0.0, |g| g.size().x + DOT_GAP);
    (shown, more, vec2(dots_w + more_w, 2.0 * DOT_RADIUS) + 2.0 * DOTS_PAD)
}

/// Size of [`filament_dots`] for `count` colours shown as at most `max` dots.
pub fn filament_dots_size(p: &Painter, count: usize, max: usize) -> Vec2 {
    dots_layout(p, count, max).2
}

/// Filament colours of a slicer project as a row of at most `max` dots (then "+N") on a dark
/// pill whose upper-left corner is `at`. Returns its rectangle.
pub fn filament_dots(p: &Painter, at: Pos2, colors: &[[u8; 3]], max: usize) -> Rect {
    let (shown, more, size) = dots_layout(p, colors.len(), max);
    let rect = Rect::from_min_size(at, size);
    p.rect(rect, size.y / 2.0, Color32::from_black_alpha(150), Stroke::NONE, StrokeKind::Inside);
    let ring = Stroke::new(1.0, Color32::from_white_alpha(90));
    for (i, &[red, green, blue]) in colors.iter().take(shown).enumerate() {
        let center = rect.min + DOTS_PAD + vec2(DOT_RADIUS + i as f32 * (2.0 * DOT_RADIUS + DOT_GAP), DOT_RADIUS);
        p.circle(center, DOT_RADIUS, Color32::from_rgb(red, green, blue), ring);
    }
    if let Some(galley) = more {
        let dots_w = shown as f32 * (2.0 * DOT_RADIUS + DOT_GAP) - DOT_GAP;
        let at = pos2(rect.min.x + DOTS_PAD.x + dots_w + DOT_GAP, rect.center().y - galley.size().y / 2.0);
        p.galley(at, galley, Color32::from_gray(230));
    }
    rect
}

/// Filament colours as a thin band filling `band`, an equal part for each colour.
pub fn filament_stripe(p: &Painter, band: Rect, colors: &[[u8; 3]]) {
    let part = band.width() / colors.len().max(1) as f32;
    for (i, &[red, green, blue]) in colors.iter().enumerate() {
        let x = band.min.x + i as f32 * part;
        let piece = Rect::from_min_max(pos2(x, band.min.y), pos2(x + part, band.max.y));
        p.rect_filled(piece, 0.0, Color32::from_rgb(red, green, blue));
    }
    // A dark edge keeps light colours apart from a light picture.
    p.rect_stroke(band, 0.0, Stroke::new(1.0, Color32::from_black_alpha(110)), StrokeKind::Inside);
}

/// Short label for the file icon: upper-case extension, at most five characters.
pub fn extension_label(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => {
            ext.chars().take(5).collect::<String>().to_uppercase()
        }
        _ => String::new(),
    }
}
