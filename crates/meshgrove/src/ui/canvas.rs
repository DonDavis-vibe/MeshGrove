//! The canvas of plugin panels ([`Canvas`]): a drawing in its own units (y down), fitted to the
//! widget. The wheel zooms around the pointer, the right or middle button pans, a double click
//! fits it again — all here, the plugin is not asked. When `interactive`, a click is reported
//! with the named rectangle under it, and a drag with where it started and where it is now, at
//! most about 30 times a second (and once more when let go), in the canvas's own units.

use egui::{Align2, FontId, Pos2, Rect, Sense, Shape as EShape, Stroke, Ui, Vec2, vec2};
use meshgrove_plugin_api::ui::{Canvas, CanvasChange, Shape};

use super::plugin_view::parse_color;

/// Drags are reported this often at most.
const DRAG_EVERY: f64 = 1.0 / 30.0;

/// Canvas units to screen and back: `screen = origin + world · scale`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub origin: Pos2,
    pub scale: f32,
}

impl Transform {
    /// `width × height` fitted into `rect`, centred, then zoomed by `zoom` around the centre and
    /// moved by `pan` points.
    pub fn fit(rect: Rect, width: f32, height: f32, zoom: f32, pan: Vec2) -> Transform {
        let fit = (rect.width() / width.max(1e-3)).min(rect.height() / height.max(1e-3));
        let scale = fit * zoom;
        let origin = rect.center() - vec2(width, height) * scale / 2.0 + pan;
        Transform { origin, scale }
    }

    pub fn screen(&self, p: [f32; 2]) -> Pos2 {
        self.origin + vec2(p[0], p[1]) * self.scale
    }

    pub fn world(&self, p: Pos2) -> [f32; 2] {
        let v = (p - self.origin) / self.scale;
        [v.x, v.y]
    }
}

/// The topmost named rectangle at a point (canvas units).
pub fn shape_at(shapes: &[Shape], p: [f32; 2]) -> Option<&str> {
    shapes.iter().rev().find_map(|s| match s {
        Shape::Rect { x, y, w, h, id: Some(id), .. }
            if (x.min(x + w)..=x.max(x + w)).contains(&p[0]) && (y.min(y + h)..=y.max(y + h)).contains(&p[1]) =>
        {
            Some(id.as_str())
        }
        _ => None,
    })
}

fn area2(points: &[[f32; 2]]) -> f32 {
    let n = points.len();
    (0..n).map(|i| points[i][0] * points[(i + 1) % n][1] - points[(i + 1) % n][0] * points[i][1]).sum()
}

fn cross(o: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// Triangles filling a simple polygon (convex or not), as indices into `points` (ear clipping).
pub fn triangulate(points: &[[f32; 2]]) -> Vec<[usize; 3]> {
    let mut left: Vec<usize> = (0..points.len()).collect();
    if left.len() < 3 {
        return Vec::new();
    }
    // Ears are found walking counter-clockwise (in the y-down canvas: clockwise on screen).
    if area2(points) < 0.0 {
        left.reverse();
    }
    let mut out = Vec::new();
    let mut guard = 0;
    while left.len() > 3 && guard < points.len() * points.len() {
        guard += 1;
        let n = left.len();
        let ear = (0..n).find(|&i| {
            let (a, b, c) = (left[(i + n - 1) % n], left[i], left[(i + 1) % n]);
            cross(points[a], points[b], points[c]) > 0.0
                && !left.iter().any(|&j| {
                    j != a && j != b && j != c && {
                        let p = points[j];
                        cross(points[a], points[b], p) >= 0.0
                            && cross(points[b], points[c], p) >= 0.0
                            && cross(points[c], points[a], p) >= 0.0
                    }
                })
        });
        // A broken polygon (crossing itself): take any corner rather than loop.
        let i = ear.unwrap_or(0);
        out.push([left[(i + n - 1) % n], left[i], left[(i + 1) % n]]);
        left.remove(i);
    }
    if left.len() == 3 {
        out.push([left[0], left[1], left[2]]);
    }
    out
}

/// How the user looks at the canvas, and the drag going on (kept by egui).
#[derive(Debug, Clone, Copy)]
struct View {
    zoom: f32,
    pan: Vec2,
    drag_from: Option<[f32; 2]>,
    sent_at: f64,
}

impl Default for View {
    fn default() -> View {
        View { zoom: 1.0, pan: Vec2::ZERO, drag_from: None, sent_at: 0.0 }
    }
}

/// Draws `canvas`; returns what the user did (only when it is interactive).
pub fn show(ui: &mut Ui, canvas: &Canvas) -> Vec<CanvasChange> {
    let mut changes = Vec::new();
    let (cw, ch) = (canvas.width.max(1e-3), canvas.height.max(1e-3));
    let width = ui.available_width().max(60.0);
    let height = (width * ch / cw).clamp(80.0, 600.0);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
    let id = response.id;
    let mut view: View = ui.data(|d| d.get_temp(id)).unwrap_or_default();

    // Looking around.
    if response.hovered() {
        let (scroll, pointer) = ui.input(|i| (i.smooth_scroll_delta.y, i.pointer.hover_pos()));
        if scroll != 0.0
            && let Some(p) = pointer
        {
            let before = Transform::fit(rect, cw, ch, view.zoom, view.pan).world(p);
            view.zoom = (view.zoom * (scroll * 0.002).exp()).clamp(0.2, 50.0);
            let after = Transform::fit(rect, cw, ch, view.zoom, view.pan).screen(before);
            view.pan += p - after;
        }
    }
    if response.dragged_by(egui::PointerButton::Secondary) || response.dragged_by(egui::PointerButton::Middle) {
        view.pan += response.drag_delta();
    }
    if response.double_clicked() {
        view = View::default();
    }
    let t = Transform::fit(rect, cw, ch, view.zoom, view.pan);

    // What the plugin hears of.
    if canvas.interactive {
        let now = ui.input(|i| i.time);
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            let at = t.world(p);
            changes.push(CanvasChange::Click {
                x: at[0],
                y: at[1],
                shape: shape_at(&canvas.shapes, at).map(str::to_owned),
            });
        }
        if response.drag_started_by(egui::PointerButton::Primary) {
            let origin = ui.input(|i| i.pointer.press_origin()).or(response.interact_pointer_pos());
            view.drag_from = origin.map(|p| t.world(p));
        }
        // A drag goes on (and ends) outside the widget too.
        let pointer = response.interact_pointer_pos().or_else(|| ui.input(|i| i.pointer.latest_pos()));
        if let (Some(from), Some(p)) = (view.drag_from, pointer) {
            let stopped = response.drag_stopped();
            if stopped
                || (response.dragged() && now - view.sent_at >= DRAG_EVERY && response.drag_delta() != Vec2::ZERO)
            {
                changes.push(CanvasChange::Drag { from, to: t.world(p) });
                view.sent_at = now;
            }
            if stopped {
                view.drag_from = None;
            }
        }
    }

    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    let background = canvas.background.as_deref().and_then(parse_color).unwrap_or(visuals.extreme_bg_color);
    painter.rect_filled(rect, 2.0, background);
    let color = |c: &Option<String>| c.as_deref().and_then(parse_color);
    let stroke = |s: &meshgrove_plugin_api::ui::Stroke| {
        Stroke::new(s.width.max(0.0), parse_color(&s.color).unwrap_or(visuals.text_color()))
    };
    for shape in &canvas.shapes {
        match shape {
            Shape::Line { points, stroke: s } => {
                painter.add(EShape::line(points.iter().map(|p| t.screen(*p)).collect(), stroke(s)));
            }
            Shape::Polygon { points, fill, stroke: s } => {
                let screen: Vec<Pos2> = points.iter().map(|p| t.screen(*p)).collect();
                if let Some(fill) = color(fill) {
                    let mut mesh = egui::Mesh::default();
                    for p in &screen {
                        mesh.colored_vertex(*p, fill);
                    }
                    for [a, b, c] in triangulate(points) {
                        mesh.add_triangle(a as u32, b as u32, c as u32);
                    }
                    painter.add(EShape::mesh(mesh));
                }
                painter.add(EShape::closed_line(screen, stroke(s)));
            }
            Shape::Rect { x, y, w, h, fill, stroke: s, .. } => {
                let r = Rect::from_two_pos(t.screen([*x, *y]), t.screen([x + w, y + h]));
                if let Some(fill) = color(fill) {
                    painter.rect_filled(r, 0.0, fill);
                }
                painter.rect_stroke(r, 0.0, stroke(s), egui::StrokeKind::Middle);
            }
            Shape::Circle { x, y, r, fill, stroke: s } => {
                let (center, radius) = (t.screen([*x, *y]), r * t.scale);
                if let Some(fill) = color(fill) {
                    painter.circle_filled(center, radius, fill);
                }
                painter.circle_stroke(center, radius, stroke(s));
            }
            Shape::Text { x, y, text, size, color: c } => {
                // A size is in canvas units (it zooms along); without one, 12 points.
                let px = size.map_or(12.0, |s| s * t.scale).clamp(4.0, 200.0);
                let c = color(c).unwrap_or(visuals.text_color());
                painter.text(t.screen([*x, *y]), Align2::LEFT_BOTTOM, text, FontId::proportional(px), c);
            }
        }
    }
    if view.zoom != 1.0 || view.pan != Vec2::ZERO {
        let hint = format!("{:.0} %", view.zoom * 100.0);
        painter.text(
            rect.right_bottom() - vec2(4.0, 2.0),
            Align2::RIGHT_BOTTOM,
            hint,
            FontId::proportional(10.0),
            visuals.weak_text_color(),
        );
    }
    if !canvas.interactive {
        response.on_hover_text(t!("canvas.hint"));
    } else {
        response.on_hover_cursor(egui::CursorIcon::Crosshair);
    }
    ui.data_mut(|d| d.insert_temp(id, view));
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    #[test]
    fn the_drawing_is_fitted_and_points_go_back_and_forth() {
        let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(200.0, 100.0));
        let t = Transform::fit(rect, 100.0, 100.0, 1.0, Vec2::ZERO);
        assert_eq!(t.scale, 1.0, "the height decides");
        assert_eq!(t.screen([0.0, 0.0]), pos2(60.0, 20.0), "centred across");
        assert_eq!(t.screen([100.0, 100.0]), pos2(160.0, 120.0));
        let p = t.world(pos2(110.0, 70.0));
        assert_eq!(p, [50.0, 50.0]);
        let zoomed = Transform::fit(rect, 100.0, 100.0, 2.0, vec2(5.0, 0.0));
        assert_eq!(zoomed.screen([50.0, 50.0]), pos2(115.0, 70.0), "zoomed about the centre, then panned");
    }

    #[test]
    fn clicks_name_the_topmost_named_rectangle() {
        let rect = |id: Option<&str>, x: f32| Shape::Rect {
            x,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            fill: None,
            stroke: Default::default(),
            id: id.map(str::to_owned),
        };
        let shapes = vec![rect(Some("a"), 0.0), rect(Some("b"), 5.0), rect(None, 8.0)];
        assert_eq!(shape_at(&shapes, [2.0, 2.0]), Some("a"));
        assert_eq!(shape_at(&shapes, [6.0, 2.0]), Some("b"), "drawn later, on top");
        assert_eq!(shape_at(&shapes, [9.0, 2.0]), Some("b"), "unnamed ones are not reported");
        assert_eq!(shape_at(&shapes, [30.0, 2.0]), None);
        let flipped = vec![Shape::Rect {
            x: 10.0,
            y: 10.0,
            w: -10.0,
            h: -10.0,
            fill: None,
            stroke: Default::default(),
            id: Some("f".into()),
        }];
        assert_eq!(shape_at(&flipped, [5.0, 5.0]), Some("f"), "negative sizes count too");
    }

    /// Twice the area of the triangles must be the polygon's, whichever way round it goes.
    #[test]
    fn concave_polygons_are_filled_by_triangles() {
        let l_shape = [[0.0, 0.0], [4.0, 0.0], [4.0, 1.0], [1.0, 1.0], [1.0, 3.0], [0.0, 3.0]];
        for points in [l_shape.to_vec(), l_shape.iter().rev().copied().collect()] {
            let triangles = triangulate(&points);
            assert_eq!(triangles.len(), points.len() - 2);
            let sum: f32 = triangles.iter().map(|[a, b, c]| cross(points[*a], points[*b], points[*c]).abs()).sum();
            assert!((sum - area2(&points).abs()).abs() < 1e-4, "{sum}");
        }
        assert!(triangulate(&[[0.0, 0.0], [1.0, 0.0]]).is_empty());
    }

    /// A click on a named rectangle and a drag, sent to an interactive 100 × 50 canvas through egui.
    #[test]
    fn clicks_and_drags_are_reported_in_canvas_units() {
        let canvas = Canvas {
            id: "c".into(),
            width: 100.0,
            height: 50.0,
            shapes: vec![Shape::Rect {
                x: 10.0,
                y: 10.0,
                w: 20.0,
                h: 20.0,
                fill: None,
                stroke: Default::default(),
                id: Some("r".into()),
            }],
            interactive: true,
            background: None,
        };
        let ctx = egui::Context::default();
        let mut time = 0.0;
        let mut t = None;
        let mut frame = |events: Vec<egui::Event>| {
            time += 0.05;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 400.0))),
                time: Some(time),
                events,
                ..Default::default()
            };
            let mut changes = Vec::new();
            let mut output = ctx.run_ui(input, |ui| {
                ui.set_max_width(200.0);
                let rect = Rect::from_min_size(ui.next_widget_position(), vec2(200.0, 100.0));
                t = Some(Transform::fit(rect, 100.0, 50.0, 1.0, Vec2::ZERO));
                changes = show(ui, &canvas);
            });
            output.textures_delta.clear();
            (changes, t.unwrap())
        };
        let (_, t) = frame(Vec::new());
        let button = |p, pressed| egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let p = t.screen([20.0, 20.0]);
        frame(vec![egui::Event::PointerMoved(p), button(p, true)]);
        let (changes, _) = frame(vec![button(p, false)]);
        assert_eq!(changes, [CanvasChange::Click { x: 20.0, y: 20.0, shape: Some("r".into()) }]);

        let (a, b) = (t.screen([50.0, 25.0]), t.screen([90.0, 45.0]));
        frame(vec![egui::Event::PointerMoved(a), button(a, true)]);
        let mut drags = Vec::new();
        for k in 1..=6 {
            drags.extend(frame(vec![egui::Event::PointerMoved(a + (b - a) * (k as f32 / 6.0))]).0);
        }
        drags.extend(frame(vec![button(b, false)]).0);
        assert!(drags.len() >= 2, "{drags:?}");
        assert!(
            drags.iter().all(|d| matches!(d, CanvasChange::Drag { from, .. } if *from == [50.0, 25.0])),
            "{drags:?}"
        );
        assert_eq!(drags.last(), Some(&CanvasChange::Drag { from: [50.0, 25.0], to: [90.0, 45.0] }));
    }
}
