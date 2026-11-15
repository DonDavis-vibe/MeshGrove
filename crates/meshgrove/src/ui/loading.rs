//! The mark of something under way, in place of a turning circle: the logo's low-poly tree, in
//! one of three little animations. Which one is picked at random whenever a wait begins, and
//! kept for as long as that wait is on screen.
//!
//! - the grove grows: three trees rise from the ground one after another, then fade;
//! - the tree unfolds: its facets open from the middle like folded paper, and fold away;
//! - the light wanders: the tree stands, and the logo's orange facet goes round its crown.
//!
//! The colours are the logo's own. A theme of one ink (the 1-bit ones) gets its ink instead.

use egui::{Color32, Id, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, vec2};
use solidcraft_theme::Tokens;

/// Which animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Grove,
    Unfold,
    Light,
}

const KINDS: [Kind; 3] = [Kind::Grove, Kind::Unfold, Kind::Light];

/// A wait that was not drawn for this long has ended: the next one picks anew.
const SAME_WAIT: f64 = 0.6;

/// The tree in a box of 48: its crown's corners round the middle, the trunk and the ground.
const BOX: f32 = 48.0;
const MIDDLE: (f32, f32) = (24.0, 19.0);
const CROWN: [(f32, f32); 7] =
    [(24.0, 6.0), (33.0, 12.0), (35.0, 24.0), (29.0, 31.0), (19.0, 31.0), (13.0, 24.0), (15.0, 12.0)];
/// The ground line's height in the box: where trees stand and grow from.
const GROUND: f32 = 40.0;

/// The logo's colours: the facets from light to dark, its one orange facet, the ground.
struct Palette {
    facets: [Color32; 7],
    dark: Color32,
    pale: Color32,
    glow: Color32,
    ground: Color32,
}

fn palette() -> Palette {
    let t = Tokens::get();
    // A theme whose accent is black or white draws in one ink: so does the tree.
    let [r, g, b, _] = t.form_primary.to_array();
    if (r == g && g == b) && (r == 0 || r == 255) {
        let ink = t.form_ink;
        let shade = |a: f32| ink.gamma_multiply(a);
        return Palette {
            facets: [shade(0.85), shade(1.0), shade(0.7), shade(0.5), shade(0.7), shade(0.5), shade(0.85)],
            dark: shade(0.45),
            pale: shade(0.8),
            glow: ink,
            ground: shade(0.6),
        };
    }
    let (dark, mid, light, pale) = (
        Color32::from_rgb(0x1f, 0x6f, 0x66),
        Color32::from_rgb(0x2e, 0x8b, 0x7a),
        Color32::from_rgb(0x3a, 0xa5, 0x8a),
        Color32::from_rgb(0x57, 0xc2, 0x9a),
    );
    Palette {
        facets: [light, pale, mid, dark, mid, dark, light],
        dark,
        pale,
        glow: Color32::from_rgb(0xf0, 0xa9, 0x4a),
        ground: Color32::from_rgb(0x2a, 0x7d, 0x6f),
    }
}

/// Set to `grove`, `unfold` or `light`, every wait shows that one (for pictures and checks).
fn forced() -> Option<Kind> {
    static FORCED: std::sync::OnceLock<Option<Kind>> = std::sync::OnceLock::new();
    *FORCED.get_or_init(|| match std::env::var("MESHGROVE_LOADING").ok()?.to_ascii_lowercase().as_str() {
        "grove" | "a" => Some(Kind::Grove),
        "unfold" | "b" => Some(Kind::Unfold),
        "light" | "c" => Some(Kind::Light),
        _ => None,
    })
}

/// The animation of the wait `key`: the one it has, or a new pick when it was not on screen
/// just now.
fn kind_of(ui: &Ui, key: Id) -> Kind {
    if let Some(kind) = forced() {
        return kind;
    }
    let now = ui.input(|i| i.time);
    let had: Option<(u8, f64)> = ui.data(|d| d.get_temp(key));
    let index = match had {
        Some((index, seen)) if now - seen <= SAME_WAIT => index,
        _ => pick(now, key),
    };
    ui.data_mut(|d| d.insert_temp(key, (index, now)));
    KINDS[usize::from(index) % KINDS.len()]
}

/// One of the three, from the moment the wait began and where it is: as good as dice here.
fn pick(now: f64, key: Id) -> u8 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    now.to_bits().hash(&mut hasher);
    key.hash(&mut hasher);
    (hasher.finish() % KINDS.len() as u64) as u8
}

/// The mark as a widget, as high as a line of controls (where a spinner stood).
pub fn mark(ui: &mut Ui) -> Response {
    let side = ui.spacing().interact_size.y;
    mark_sized(ui, side)
}

/// The mark `side` points wide and high.
pub fn mark_sized(ui: &mut Ui, side: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
    paint_at(ui, rect, response.id);
    response
}

/// Paints the mark into `rect`. `key` names the wait: marks that share it (the tiles of a
/// folder being read) show the same animation.
pub fn paint_at(ui: &Ui, rect: Rect, key: Id) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    let kind = kind_of(ui, key);
    let time = ui.input(|i| i.time);
    paint(ui.painter(), rect, kind, time, &palette());
    ui.ctx().request_repaint();
}

/// A point of the 48 box in `rect` (the box keeps its shape and sits in the middle).
fn place(rect: Rect) -> impl Fn(f32, f32) -> Pos2 {
    let side = rect.width().min(rect.height());
    let origin = rect.center() - vec2(side, side) / 2.0;
    move |x, y| origin + vec2(x, y) * (side / BOX)
}

fn paint(painter: &egui::Painter, rect: Rect, kind: Kind, time: f64, colors: &Palette) {
    let at = place(rect);
    let unit = rect.width().min(rect.height()) / BOX;
    // The ground the trees stand on.
    let ground = Rect::from_min_max(at(8.0, GROUND), at(40.0, GROUND + 1.6));
    painter.rect_filled(ground, 0.8 * unit, colors.ground);
    match kind {
        Kind::Grove => {
            const PERIOD: f64 = 2.6;
            // Left, right, then the large one in the middle in front of them.
            for (shift, size, delay) in [(-5.5, 0.62, 0.0), (12.5, 0.62, 0.56), (3.5, 0.86, 0.28)] {
                let phase = ((time - delay) / PERIOD).rem_euclid(1.0) as f32;
                let (grown, alpha) = match phase {
                    p if p < 0.22 => (ease_out_back(p / 0.22), (p / 0.22).min(1.0)),
                    p if p < 0.78 => (1.0, 1.0),
                    p => (1.0 - 0.1 * (p - 0.78) / 0.22, 1.0 - (p - 0.78) / 0.22),
                };
                // The tree's own box, scaled about the middle of its foot on the ground.
                let scale = size * grown;
                let tree = |x: f32, y: f32| at(shift + 24.0 * size + (x - 24.0) * scale, GROUND + (y - GROUND) * scale);
                paint_tree(painter, &tree, colors, alpha, |i| (colors.facets[i], 1.0, 0.0));
            }
        }
        Kind::Unfold => {
            const PERIOD: f64 = 2.6;
            paint_tree(painter, &at, colors, 1.0, |i| {
                let phase = ((time - i as f64 * 0.11) / PERIOD).rem_euclid(1.0) as f32;
                let (open, turn) = match phase {
                    p if p < 0.24 => (ease(p / 0.24), -1.0),
                    p if p < 0.66 => (1.0, 0.0),
                    p if p < 0.88 => (1.0 - ease((p - 0.66) / 0.22), 1.0),
                    _ => (0.0, 1.0),
                };
                (colors.facets[i].gamma_multiply(open), open, turn * (1.0 - open) * 70_f32.to_radians())
            });
        }
        Kind::Light => {
            const PERIOD: f64 = 1.75;
            paint_tree(painter, &at, colors, 1.0, |i| {
                let phase = (time / PERIOD - i as f64 / 7.0).rem_euclid(1.0) as f32;
                let color = match phase {
                    p if p < 0.16 => lerp(colors.glow, colors.pale, p / 0.16),
                    p if p < 0.34 => lerp(colors.pale, colors.dark, (p - 0.16) / 0.18),
                    _ => colors.dark,
                };
                (color, 1.0, 0.0)
            });
        }
    }
}

/// One tree: the trunk, then each facet as `facet` says (its colour, how far it is open from
/// the middle, and how far it is turned about it).
fn paint_tree(
    painter: &egui::Painter,
    at: &dyn Fn(f32, f32) -> Pos2,
    colors: &Palette,
    alpha: f32,
    facet: impl Fn(usize) -> (Color32, f32, f32),
) {
    let trunk = Rect::from_min_max(at(22.6, 30.0), at(25.4, GROUND));
    painter.rect_filled(trunk, 0.0, colors.dark.gamma_multiply(alpha));
    for i in 0..CROWN.len() {
        let (color, open, turn) = facet(i);
        if open <= 0.01 {
            continue;
        }
        let (sin, cos) = turn.sin_cos();
        let corner = |(x, y): (f32, f32)| {
            let (dx, dy) = ((x - MIDDLE.0) * open, (y - MIDDLE.1) * open);
            at(MIDDLE.0 + dx * cos - dy * sin, MIDDLE.1 + dx * sin + dy * cos)
        };
        let points = vec![at(MIDDLE.0, MIDDLE.1), corner(CROWN[i]), corner(CROWN[(i + 1) % CROWN.len()])];
        painter.add(Shape::convex_polygon(points, color.gamma_multiply(alpha), Stroke::NONE));
    }
}

fn ease(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Grows a little past its size and settles: a tree popping out of the ground.
fn ease_out_back(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0) - 1.0;
    1.0 + 2.2 * x * x * x + 1.2 * x * x
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many facets (triangles) a frame of `kind` at `time` paints.
    fn facets(kind: Kind, time: f64) -> usize {
        let ctx = egui::Context::default();
        let mut count = 0;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let rect = Rect::from_min_size(egui::pos2(10.0, 10.0), vec2(48.0, 48.0));
            let painter = ui.painter().clone();
            let before = ui.ctx().graphics(|g| g.get(ui.layer_id()).map_or(0, |list| list.next_idx().0));
            paint(&painter, rect, kind, time, &palette());
            let after = ui.ctx().graphics(|g| g.get(ui.layer_id()).map_or(0, |list| list.next_idx().0));
            count = after - before;
        });
        // The font atlas was never uploaded anywhere.
        output.textures_delta.clear();
        count
    }

    /// Every animation draws something at every moment of its run, and each has a moment
    /// that shows the whole tree.
    #[test]
    fn each_animation_draws_through_its_whole_run() {
        for kind in KINDS {
            let mut most = 0;
            for step in 0..60 {
                let shapes = facets(kind, f64::from(step) * 0.05);
                assert!(shapes >= 1, "{kind:?} at step {step}: the ground at least");
                most = most.max(shapes);
            }
            // The ground, a trunk and seven facets.
            assert!(most >= 9, "{kind:?} shows a whole tree at some moment: {most}");
        }
    }

    /// A wait keeps its animation while it is on screen; waits that begin at other moments
    /// come to all three in time.
    #[test]
    fn a_wait_keeps_its_animation_and_all_three_come_up() {
        let key = Id::unique("wait");
        let mut seen = std::collections::BTreeSet::new();
        for start in 0..40 {
            seen.insert(pick(f64::from(start) * 1.37, key));
        }
        assert_eq!(seen.len(), 3, "all three are picked over many waits: {seen:?}");
        let ctx = egui::Context::default();
        let mut kinds = Vec::new();
        for frame in 0..5 {
            let raw = egui::RawInput { time: Some(10.0 + f64::from(frame) * 0.1), ..Default::default() };
            let mut output = ctx.run_ui(raw, |ui| kinds.push(kind_of(ui, key)));
            output.textures_delta.clear();
        }
        assert!(kinds.windows(2).all(|pair| pair[0] == pair[1]), "one wait, one animation: {kinds:?}");
    }
}
