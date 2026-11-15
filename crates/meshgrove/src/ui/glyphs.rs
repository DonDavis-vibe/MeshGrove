//! The small marks the kit asks its host to paint: window captions, badges, pin, cross, the
//! path separator. Plain artwork, taken over from LightGrove's `glyphs.rs`; the 3D view compass,
//! the agent pointer and a one-ink brand mark are not used by Meshgrove and draw nothing.
use egui::{Color32, Painter, Pos2, Rect, Stroke, Ui, pos2, vec2};
use solidcraft_theme::Tokens;
use solidcraft_ui_kit::icons::glyphs::Glyphs;
use solidcraft_ui_kit::window_chrome::Caption;

pub static GLYPHS: Glyphs =
    Glyphs { transport, caption, badge, pin, cross, breadcrumb, axis_tip, axis_radius: 0.0, pointer, brand_mark };

fn triangle(p: &Painter, c: Pos2, dir: f32, half: f32, col: Color32) {
    let points = vec![
        pos2(c.x - half * dir, c.y - half * 1.2),
        pos2(c.x + half * dir, c.y),
        pos2(c.x - half * dir, c.y + half * 1.2),
    ];
    p.add(egui::Shape::convex_polygon(points, col, Stroke::NONE));
}

fn transport(p: &Painter, c: Pos2, what: &str, col: Color32) {
    match what {
        "first" => {
            p.line_segment([pos2(c.x - 5.0, c.y - 5.0), pos2(c.x - 5.0, c.y + 5.0)], Stroke::new(1.6, col));
            triangle(p, pos2(c.x + 1.0, c.y), -1.0, 4.0, col);
        }
        "back" => triangle(p, c, -1.0, 4.0, col),
        "forward" => triangle(p, c, 1.0, 4.0, col),
        "play" => triangle(p, pos2(c.x + 1.0, c.y), 1.0, 5.5, col),
        "pause" => {
            for dx in [-3.0, 3.0] {
                p.rect_filled(Rect::from_center_size(pos2(c.x + dx, c.y), vec2(3.0, 12.0)), 0.5, col);
            }
        }
        _ => {
            triangle(p, pos2(c.x - 1.0, c.y), 1.0, 4.0, col);
            p.line_segment([pos2(c.x + 5.0, c.y - 5.0), pos2(c.x + 5.0, c.y + 5.0)], Stroke::new(1.6, col));
        }
    }
}

fn caption(_ui: &Ui, which: Caption, maximised: bool, c: Pos2, stroke: Stroke) {
    let p = _ui.painter();
    let r = 5.0;
    match which {
        Caption::Minimize => {
            p.line_segment([pos2(c.x - r, c.y), pos2(c.x + r, c.y)], stroke);
        }
        Caption::Maximize if maximised => {
            p.rect_stroke(
                Rect::from_center_size(c + vec2(-1.5, 1.5), vec2(7.0, 7.0)),
                0.0,
                stroke,
                egui::StrokeKind::Middle,
            );
            p.line_segment([pos2(c.x - 2.0, c.y - 5.0), pos2(c.x + 5.0, c.y - 5.0)], stroke);
            p.line_segment([pos2(c.x + 5.0, c.y - 5.0), pos2(c.x + 5.0, c.y + 2.0)], stroke);
        }
        Caption::Maximize => {
            p.rect_stroke(Rect::from_center_size(c, vec2(2.0 * r, 2.0 * r)), 0.0, stroke, egui::StrokeKind::Middle);
        }
        Caption::Close => cross(p, Rect::from_center_size(c, vec2(2.0 * r, 2.0 * r)), stroke),
    }
}

fn badge(p: &Painter, r: Rect, what: &str, col: Color32) {
    match what {
        "lock" => {
            let body = Rect::from_min_max(pos2(r.left(), r.center().y), r.max);
            p.rect_filled(body, 1.0, col);
            p.circle_stroke(pos2(r.center().x, r.center().y), r.width() * 0.3, Stroke::new(1.4, col));
        }
        _ => {
            p.circle_filled(r.center(), r.width().min(r.height()) * 0.35, col);
        }
    }
}

fn pin(p: &Painter, r: Rect, pinned: bool, col: Color32) {
    let head = Rect::from_center_size(
        pos2(r.center().x, r.top() + r.height() * 0.35),
        vec2(r.width() * 0.5, r.height() * 0.4),
    );
    if pinned {
        p.rect_filled(head, 1.0, col);
    } else {
        p.rect_stroke(head, 1.0, Stroke::new(1.3, col), egui::StrokeKind::Inside);
    }
    p.line_segment([pos2(r.center().x, head.bottom()), pos2(r.center().x, r.bottom())], Stroke::new(1.3, col));
}

fn cross(p: &Painter, r: Rect, stroke: Stroke) {
    p.line_segment([r.left_top(), r.right_bottom()], stroke);
    p.line_segment([r.right_top(), r.left_bottom()], stroke);
}

fn breadcrumb(p: &Painter, c: Pos2, col: Color32) {
    let s = Stroke::new(1.3, col);
    p.line_segment([pos2(c.x - 2.0, c.y - 4.0), pos2(c.x + 2.0, c.y)], s);
    p.line_segment([pos2(c.x + 2.0, c.y), pos2(c.x - 2.0, c.y + 4.0)], s);
}

// The kit's glyph table fixes this signature.
#[allow(clippy::too_many_arguments)]
fn axis_tip(_: &Painter, _: Pos2, _: &str, _: [f64; 3], _: Pos2, _: f64, _: bool, _: Tokens, _: f32) {}

fn pointer(_: &Painter, _: Pos2) {}

fn brand_mark(_: &Painter, _: Pos2, _: f32, _: Color32) {}
