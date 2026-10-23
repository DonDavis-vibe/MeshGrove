//! Simple painted icons, so tiles without a picture still read at a glance.

use egui::{Align2, Color32, FontId, Painter, Rect, Stroke, StrokeKind, pos2, vec2};

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

/// Small format badge ("STL", "3MF") in the lower-left corner of a thumbnail.
pub fn badge(p: &Painter, r: Rect, label: &str, bg: Color32, fg: Color32) {
    let font = FontId::proportional(10.5);
    let galley = p.layout_no_wrap(label.to_owned(), font, fg);
    let pad = vec2(5.0, 2.0);
    let rect = Rect::from_min_size(
        pos2(r.min.x + 4.0, r.max.y - galley.size().y - 2.0 * pad.y - 4.0),
        galley.size() + 2.0 * pad,
    );
    p.rect(rect, 4.0, bg, Stroke::NONE, StrokeKind::Inside);
    p.galley(rect.min + pad, galley, fg);
}

/// Small badge in the upper-right corner (e.g. "2×" for duplicates).
pub fn corner_badge(p: &Painter, r: Rect, label: &str, bg: Color32, fg: Color32) {
    let font = FontId::proportional(10.5);
    let galley = p.layout_no_wrap(label.to_owned(), font, fg);
    let pad = vec2(5.0, 2.0);
    let size = galley.size() + 2.0 * pad;
    let rect = Rect::from_min_size(pos2(r.max.x - size.x - 4.0, r.min.y + 4.0), size);
    p.rect(rect, 4.0, bg, Stroke::NONE, StrokeKind::Inside);
    p.galley(rect.min + pad, galley, fg);
}

/// Small badge in the lower-right corner (e.g. the print bed check). Returns its rectangle.
pub fn bottom_right_badge(p: &Painter, r: Rect, label: &str, bg: Color32, fg: Color32) -> Rect {
    badge_left_of(p, pos2(r.max.x - 4.0, r.max.y - 4.0), label, bg, fg)
}

/// Small badge whose lower-right corner is `corner` (badges in a row, right to left).
pub fn badge_left_of(p: &Painter, corner: egui::Pos2, label: &str, bg: Color32, fg: Color32) -> Rect {
    let font = FontId::proportional(10.5);
    let galley = p.layout_no_wrap(label.to_owned(), font, fg);
    let pad = vec2(5.0, 2.0);
    let size = galley.size() + 2.0 * pad;
    let rect = Rect::from_min_size(corner - size, size);
    p.rect(rect, 4.0, bg, Stroke::NONE, StrokeKind::Inside);
    p.galley(rect.min + pad, galley, fg);
    rect
}

/// Height of [`corner_badge`] (to stack things below it).
pub const BADGE_HEIGHT: f32 = 10.5 * 1.3 + 4.0;

/// Filament colours of a slicer project as a row of dots in the upper-left corner.
pub fn filament_dots(p: &Painter, r: Rect, colors: &[[u8; 3]]) {
    const MAX: usize = 8;
    let (radius, gap, pad) = (4.5, 3.0, vec2(4.0, 3.0));
    let shown = colors.len().min(MAX);
    let more = (colors.len() > MAX).then(|| {
        p.layout_no_wrap(format!("+{}", colors.len() - MAX), FontId::proportional(9.5), Color32::from_gray(230))
    });
    let dots_w = shown as f32 * (2.0 * radius + gap) - gap;
    let more_w = more.as_ref().map_or(0.0, |g| g.size().x + gap);
    let size = vec2(dots_w + more_w, 2.0 * radius) + 2.0 * pad;
    let rect = Rect::from_min_size(r.min + vec2(4.0, 4.0), size);
    p.rect(rect, size.y / 2.0, Color32::from_black_alpha(150), Stroke::NONE, StrokeKind::Inside);
    let ring = Stroke::new(1.0, Color32::from_white_alpha(90));
    for (i, &[red, green, blue]) in colors.iter().take(MAX).enumerate() {
        let center = rect.min + pad + vec2(radius + i as f32 * (2.0 * radius + gap), radius);
        p.circle(center, radius, Color32::from_rgb(red, green, blue), ring);
    }
    if let Some(galley) = more {
        let at = pos2(rect.min.x + pad.x + dots_w + gap, rect.center().y - galley.size().y / 2.0);
        p.galley(at, galley, Color32::from_gray(230));
    }
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
