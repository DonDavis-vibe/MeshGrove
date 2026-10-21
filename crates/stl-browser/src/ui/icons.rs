//! Simple painted icons, so tiles without a picture still read at a glance.

use egui::{Align2, Color32, FontId, Painter, Rect, Stroke, StrokeKind, pos2, vec2};

/// KDE Breeze folder blue.
pub const FOLDER_BLUE: Color32 = Color32::from_rgb(61, 174, 233);

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

/// Short label for the file icon: upper-case extension, at most five characters.
pub fn extension_label(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => {
            ext.chars().take(5).collect::<String>().to_uppercase()
        }
        _ => String::new(),
    }
}
