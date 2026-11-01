//! The grid editor of plugin panels ([`GridEditor`]): cells, zones around or on the grid, items
//! on it. Meshgrove handles the mouse and keys itself — select (Shift/Ctrl adds), drag to move,
//! drag an edge or corner to resize, drag on free cells to draw a new item, arrows to nudge,
//! Delete, a context menu — snapping to `1/snap_div` of a cell, with a red ghost where the item
//! would leave the allowed area (the grid and the zones that accept items) or lie on another.
//! Every change goes to the plugin as a [`GridChange`]; its next view is what counts (it may
//! put the item somewhere else, or back).
//!
//! Coordinates are cells with y running down the widget (row 0 at the top), as the API defines.

use egui::{Align2, Color32, CornerRadius, CursorIcon, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use meshgrove_plugin_api::ui::{GridChange, GridEditor, GridItem, GridZone};

use super::plugin_view::parse_color;

/// Below this, two edges count as the same.
const EPS: f32 = 1e-4;
/// How far from an item's edge (in points) the pointer grabs it for resizing.
const HANDLE_REACH: f32 = 5.0;

/// A rectangle in cells.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cells {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Cells {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Cells {
        Cells { x, y, w, h }
    }

    fn of_item(i: &GridItem) -> Cells {
        Cells::new(i.x, i.y, i.w, i.h)
    }

    fn of_zone(z: &GridZone) -> Cells {
        Cells::new(z.x, z.y, z.w, z.h)
    }

    fn right(&self) -> f32 {
        self.x + self.w
    }

    fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// They share more than an edge.
    pub fn overlaps(&self, o: &Cells) -> bool {
        self.x < o.right() - EPS && o.x < self.right() - EPS && self.y < o.bottom() - EPS && o.y < self.bottom() - EPS
    }

    pub fn union(&self, o: &Cells) -> Cells {
        let (x, y) = (self.x.min(o.x), self.y.min(o.y));
        Cells::new(x, y, self.right().max(o.right()) - x, self.bottom().max(o.bottom()) - y)
    }

    fn moved_to(self, x: f32, y: f32) -> Cells {
        Cells { x, y, ..self }
    }
}

/// `v` on the nearest `1/div` of a cell.
pub fn snap(v: f32, div: u32) -> f32 {
    let d = div.max(1) as f32;
    (v * d).round() / d
}

fn snap_down(v: f32, div: u32) -> f32 {
    let d = div.max(1) as f32;
    (v * d + EPS).floor() / d
}

fn snap_up(v: f32, div: u32) -> f32 {
    let d = div.max(1) as f32;
    (v * d - EPS).ceil() / d
}

/// The grid and the zones items may reach into.
fn areas(editor: &GridEditor) -> Vec<Cells> {
    let grid = Cells::new(0.0, 0.0, editor.cols as f32, editor.rows as f32);
    std::iter::once(grid).chain(editor.zones.iter().filter(|z| z.accepts_items).map(Cells::of_zone)).collect()
}

/// Whether all of `rect` lies in `areas` (which may overlap and leave gaps between them).
pub fn covered(rect: Cells, areas: &[Cells]) -> bool {
    if rect.w <= EPS || rect.h <= EPS {
        return false;
    }
    // Cut `rect` at every edge of an area inside it; each piece lies wholly in or out of each area.
    let cuts = |lo: f32, hi: f32, edges: &mut dyn Iterator<Item = f32>| {
        let mut v: Vec<f32> = std::iter::once(lo).chain(edges.filter(|e| *e > lo + EPS && *e < hi - EPS)).collect();
        v.push(hi);
        v.sort_by(f32::total_cmp);
        v.dedup_by(|a, b| (*a - *b).abs() < EPS);
        v
    };
    let xs = cuts(rect.x, rect.right(), &mut areas.iter().flat_map(|a| [a.x, a.right()]));
    let ys = cuts(rect.y, rect.bottom(), &mut areas.iter().flat_map(|a| [a.y, a.bottom()]));
    xs.windows(2).all(|x| {
        ys.windows(2).all(|y| {
            let (cx, cy) = ((x[0] + x[1]) / 2.0, (y[0] + y[1]) / 2.0);
            areas.iter().any(|a| a.contains(cx, cy))
        })
    })
}

/// Whether item `id` (or a new one: `None`) fits at `rect`: in the allowed area, on no other item.
pub fn fits(editor: &GridEditor, id: Option<&str>, rect: Cells) -> bool {
    covered(rect, &areas(editor))
        && !editor.items.iter().any(|i| Some(i.id.as_str()) != id && Cells::of_item(i).overlaps(&rect))
}

/// The item at a point: the last one drawn, which lies on top.
pub fn item_at(items: &[GridItem], x: f32, y: f32) -> Option<usize> {
    items.iter().rposition(|i| Cells::of_item(i).contains(x, y))
}

/// The edges of an item a drag takes along.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Handle {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl Handle {
    fn cursor(self) -> CursorIcon {
        match (self.left || self.right, self.top || self.bottom) {
            (true, true) if self.left == self.top => CursorIcon::ResizeNwSe,
            (true, true) => CursorIcon::ResizeNeSw,
            (true, false) => CursorIcon::ResizeHorizontal,
            _ => CursorIcon::ResizeVertical,
        }
    }
}

/// The edges of `rect` (on screen) within `reach` of `p`, if any.
pub fn handle_at(rect: Rect, p: Pos2, reach: f32) -> Option<Handle> {
    if !rect.expand(reach).contains(p) {
        return None;
    }
    // Small items keep a middle to grab them by.
    let reach = reach.min(rect.width() / 4.0).min(rect.height() / 4.0);
    let h = Handle {
        left: (p.x - rect.left()).abs() <= reach,
        right: (p.x - rect.right()).abs() <= reach,
        top: (p.y - rect.top()).abs() <= reach,
        bottom: (p.y - rect.bottom()).abs() <= reach,
    };
    (h != Handle::default()).then_some(h)
}

/// `start` with the edges of `handle` moved by (dx, dy) cells, on the snap grid, at least one
/// snap step large.
pub fn resized(start: Cells, handle: Handle, dx: f32, dy: f32, div: u32) -> Cells {
    let step = 1.0 / div.max(1) as f32;
    let (mut x0, mut y0, mut x1, mut y1) = (start.x, start.y, start.right(), start.bottom());
    if handle.left {
        x0 = snap(x0 + dx, div).min(x1 - step);
    }
    if handle.right {
        x1 = snap(x1 + dx, div).max(x0 + step);
    }
    if handle.top {
        y0 = snap(y0 + dy, div).min(y1 - step);
    }
    if handle.bottom {
        y1 = snap(y1 + dy, div).max(y0 + step);
    }
    Cells::new(x0, y0, x1 - x0, y1 - y0)
}

/// The snapped rectangle a drag from `a` to `b` draws: every snap cell it touches.
pub fn band(a: (f32, f32), b: (f32, f32), div: u32) -> Cells {
    let (x0, x1) = (snap_down(a.0.min(b.0), div), snap_up(a.0.max(b.0), div));
    let (y0, y1) = (snap_down(a.1.min(b.1), div), snap_up(a.1.max(b.1), div));
    let step = 1.0 / div.max(1) as f32;
    Cells::new(x0, y0, (x1 - x0).max(step), (y1 - y0).max(step))
}

/// What the selected items would become when merged: the box around them.
pub fn merged(rects: impl IntoIterator<Item = Cells>) -> Option<Cells> {
    rects.into_iter().reduce(|a, b| a.union(&b))
}

/// `rect` moved by whole snap steps so it lies within `bounds` (as far as it fits).
fn clamp_into(rect: Cells, bounds: Cells, div: u32) -> Cells {
    let fit = |v: f32, size: f32, lo: f32, hi: f32| {
        if v + size > hi + EPS {
            snap_down(hi - size, div).max(snap_up(lo, div))
        } else if v < lo - EPS {
            snap_up(lo, div)
        } else {
            v
        }
    };
    rect.moved_to(fit(rect.x, rect.w, bounds.x, bounds.right()), fit(rect.y, rect.h, bounds.y, bounds.bottom()))
}

/// Cells to screen and back.
#[derive(Debug, Clone, Copy)]
struct Frame {
    /// Where cell (0, 0) is on screen, and points per cell.
    origin: Pos2,
    cell: f32,
}

impl Frame {
    /// `bounds` (cells) centred across the top of `rect`.
    fn of(rect: Rect, bounds: Cells, cell: f32) -> Frame {
        let origin = pos2(rect.center().x - bounds.w * cell / 2.0 - bounds.x * cell, rect.top() - bounds.y * cell);
        Frame { origin, cell }
    }

    fn rect(&self, c: Cells) -> Rect {
        Rect::from_min_size(self.origin + vec2(c.x, c.y) * self.cell, vec2(c.w, c.h) * self.cell)
    }

    fn cells(&self, p: Pos2) -> (f32, f32) {
        ((p.x - self.origin.x) / self.cell, (p.y - self.origin.y) / self.cell)
    }
}

/// The grid with its zones, in cells.
fn bounds(editor: &GridEditor) -> Cells {
    let grid = Cells::new(0.0, 0.0, editor.cols.max(1) as f32, editor.rows.max(1) as f32);
    editor.zones.iter().map(Cells::of_zone).fold(grid, |b, z| b.union(&z))
}

/// The widget's size and points per cell: as large as `width` allows, no taller than asked (or
/// than is sensible).
fn fit(editor: &GridEditor, width: f32) -> (egui::Vec2, f32) {
    let b = bounds(editor);
    let max_height = editor.height.unwrap_or_else(|| (width * b.h / b.w).min(560.0)).max(80.0);
    let cell = (width / b.w).min(max_height / b.h);
    (vec2(width, b.h * cell), cell)
}

/// What a drag on the editor does.
#[derive(Debug, Clone)]
enum Drag {
    Move { item: String, start: Cells, grab: (f32, f32) },
    Resize { item: String, start: Cells, handle: Handle, from: (f32, f32) },
    Band { from: (f32, f32) },
}

/// The editor's state between frames (kept by egui).
#[derive(Debug, Clone, Default)]
struct Memory {
    drag: Option<Drag>,
    /// The item the context menu is about.
    menu: Option<String>,
}

const ITEM_COLOR: Color32 = Color32::from_rgb(90, 130, 190);

/// Draws `editor`; `selection` is the selection made here since the plugin last rendered
/// (`None`: the plugin's). Returns the changes for the plugin.
pub fn show(ui: &mut Ui, editor: &GridEditor, selection: &mut Option<Vec<String>>) -> Vec<GridChange> {
    let mut changes = Vec::new();
    let div = editor.snap_div.max(1);
    let grid = Cells::new(0.0, 0.0, editor.cols.max(1) as f32, editor.rows.max(1) as f32);
    let bounds = bounds(editor);
    let allowed = merged(areas(editor)).unwrap_or(grid);

    let width = ui.available_width().max(60.0);
    let (size, cell) = fit(editor, width);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let frame = Frame::of(rect, bounds, cell);
    let id = response.id;
    let mut memory: Memory = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    let mut selected: Vec<String> = selection.clone().unwrap_or_else(|| editor.selected.clone());
    let select = |now: Vec<String>, selected: &mut Vec<String>, changes: &mut Vec<GridChange>| {
        if now != *selected {
            changes.push(GridChange::Selected { items: now.clone() });
            *selected = now;
        }
    };
    // A drag goes on (and ends) outside the widget too.
    let pointer = response.interact_pointer_pos().or_else(|| match memory.drag {
        Some(_) => ui.input(|i| i.pointer.latest_pos()),
        None => response.hover_pos(),
    });
    let item_by_id = |id: &str| editor.items.iter().find(|i| i.id == id);

    // ---- The mouse ----
    // Where the button went down: the drag starts a few points later.
    if response.drag_started_by(egui::PointerButton::Primary)
        && let Some(p) = ui.input(|i| i.pointer.press_origin()).or(response.interact_pointer_pos())
    {
        let (cx, cy) = frame.cells(p);
        let hit = item_at(&editor.items, cx, cy).map(|i| &editor.items[i]);
        let handle = hit.and_then(|i| handle_at(frame.rect(Cells::of_item(i)), p, HANDLE_REACH));
        memory.drag = match hit {
            Some(i) if i.locked => None,
            Some(i) => {
                let start = Cells::of_item(i);
                if !selected.contains(&i.id) {
                    select(vec![i.id.clone()], &mut selected, &mut changes);
                }
                Some(match handle {
                    Some(handle) => Drag::Resize { item: i.id.clone(), start, handle, from: (cx, cy) },
                    None => Drag::Move { item: i.id.clone(), start, grab: (cx - start.x, cy - start.y) },
                })
            }
            None if grid.contains(cx, cy) || covered(Cells::new(cx, cy, EPS * 2.0, EPS * 2.0), &areas(editor)) => {
                Some(Drag::Band { from: (cx, cy) })
            }
            None => None,
        };
        response.request_focus();
    }
    // Where the drag would put things now: (rect, fits).
    let ghost = match (&memory.drag, pointer) {
        (Some(drag), Some(p)) => {
            let (cx, cy) = frame.cells(p);
            Some(match drag {
                Drag::Move { item, start, grab } => {
                    let to = clamp_into(start.moved_to(snap(cx - grab.0, div), snap(cy - grab.1, div)), allowed, div);
                    (to, fits(editor, Some(item), to))
                }
                Drag::Resize { item, start, handle, from } => {
                    let to = resized(*start, *handle, cx - from.0, cy - from.1, div);
                    (to, fits(editor, Some(item), to))
                }
                Drag::Band { from } => {
                    let to = band(*from, (cx, cy), div);
                    (to, fits(editor, None, to))
                }
            })
        }
        _ => None,
    };
    if response.drag_stopped()
        && let (Some(drag), Some((to, ok))) = (memory.drag.take(), ghost)
    {
        match drag {
            // Sent even onto another item: the plugin decides (it may move that one).
            Drag::Move { item, start, .. } if to != start => changes.push(GridChange::Moved { item, x: to.x, y: to.y }),
            Drag::Resize { item, start, .. } if to != start => {
                changes.push(GridChange::Resized { item, x: to.x, y: to.y, w: to.w, h: to.h });
            }
            Drag::Band { .. } if ok => changes.push(GridChange::Created { x: to.x, y: to.y, w: to.w, h: to.h }),
            _ => {}
        }
    }
    if response.clicked() || response.secondary_clicked() {
        response.request_focus();
        if let Some(p) = response.interact_pointer_pos() {
            let (cx, cy) = frame.cells(p);
            let hit = item_at(&editor.items, cx, cy).map(|i| editor.items[i].id.clone());
            let adding = ui.input(|i| i.modifiers.shift || i.modifiers.command);
            let now = match (&hit, response.secondary_clicked()) {
                // The menu is about the item clicked; with it in a selection, about all of them.
                (Some(h), true) if selected.contains(h) => selected.clone(),
                (Some(h), _) if adding && selected.contains(h) => {
                    selected.iter().filter(|s| *s != h).cloned().collect()
                }
                (Some(h), _) if adding => selected.iter().cloned().chain([h.clone()]).collect(),
                (Some(h), _) => vec![h.clone()],
                (None, _) if adding => selected.clone(),
                (None, _) => Vec::new(),
            };
            if response.secondary_clicked() {
                memory.menu = hit;
            }
            select(now, &mut selected, &mut changes);
        }
    }

    // ---- The keys ----
    if response.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, ..Default::default() },
            );
        });
        let (delete, dx, dy) = ui.input(|i| {
            let key = |k| i.key_pressed(k);
            let delete = key(egui::Key::Delete) || key(egui::Key::Backspace);
            let dx = f32::from(i8::from(key(egui::Key::ArrowRight)) - i8::from(key(egui::Key::ArrowLeft)));
            let dy = f32::from(i8::from(key(egui::Key::ArrowDown)) - i8::from(key(egui::Key::ArrowUp)));
            (delete, dx, dy)
        });
        let movable: Vec<&GridItem> = selected.iter().filter_map(|s| item_by_id(s)).filter(|i| !i.locked).collect();
        if delete && !movable.is_empty() {
            changes.push(GridChange::Deleted { items: movable.iter().map(|i| i.id.clone()).collect() });
        } else if dx != 0.0 || dy != 0.0 {
            let step = 1.0 / div as f32;
            for i in movable {
                let to = Cells::of_item(i).moved_to(snap(i.x + dx * step, div), snap(i.y + dy * step, div));
                let to = clamp_into(to, allowed, div);
                if to.x != i.x || to.y != i.y {
                    changes.push(GridChange::Moved { item: i.id.clone(), x: to.x, y: to.y });
                }
            }
        }
    }

    // ---- The menu ----
    response.context_menu(|ui| {
        let Some(item) = memory.menu.as_deref().and_then(item_by_id) else {
            ui.close();
            return;
        };
        for a in &item.actions {
            if ui.button(a.label.as_str()).clicked() {
                changes.push(GridChange::Action { item: item.id.clone(), action: a.id.clone() });
                ui.close();
            }
        }
        if !item.actions.is_empty() {
            ui.separator();
        }
        let several: Vec<&GridItem> = selected.iter().filter_map(|s| item_by_id(s)).collect();
        if several.len() > 1 {
            let size = merged(several.iter().map(|i| Cells::of_item(i))).map(|m| format!(" ({} × {})", m.w, m.h));
            if ui.button(format!("{}{}", t!("grid.merge"), size.unwrap_or_default())).clicked() {
                changes.push(GridChange::Merge { items: several.iter().map(|i| i.id.clone()).collect() });
                ui.close();
            }
        }
        if (item.w > 1.0 || item.h > 1.0) && ui.button(t!("grid.split")).clicked() {
            changes.push(GridChange::Split { item: item.id.clone() });
            ui.close();
        }
        let gone: Vec<String> = if several.len() > 1 { selected.clone() } else { vec![item.id.clone()] };
        if !item.locked && ui.button(t!("grid.delete")).clicked() {
            changes.push(GridChange::Deleted { items: gone });
            ui.close();
        }
    });

    // ---- Drawing ----
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    painter.rect_filled(frame.rect(grid), 2.0, visuals.extreme_bg_color);
    let line = Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color);
    let fine = Stroke::new(0.5, line.color.gamma_multiply(0.5));
    let steps = |n: u32| (0..=n * div).map(move |k| k as f32 / div as f32);
    for x in steps(editor.cols) {
        let x0 = frame.origin.x + x * cell;
        let stroke = if x.fract() == 0.0 { line } else { fine };
        painter.vline(x0, frame.rect(grid).y_range(), stroke);
    }
    for y in steps(editor.rows) {
        let y0 = frame.origin.y + y * cell;
        let stroke = if y.fract() == 0.0 { line } else { fine };
        painter.hline(frame.rect(grid).x_range(), y0, stroke);
    }
    let small = FontId::proportional(11.0);
    for z in &editor.zones {
        let r = frame.rect(Cells::of_zone(z));
        let fill = z.color.as_deref().and_then(parse_color).unwrap_or(visuals.faint_bg_color);
        painter.rect_filled(r, 0.0, fill);
        if let Some(label) = &z.label
            && r.width().max(r.height()) > 40.0
        {
            painter.text(r.center(), Align2::CENTER_CENTER, label, small.clone(), visuals.weak_text_color());
        }
    }
    let moving = match &memory.drag {
        Some(Drag::Move { item, .. } | Drag::Resize { item, .. }) => Some(item.as_str()),
        _ => None,
    };
    for i in &editor.items {
        let r = frame.rect(Cells::of_item(i)).shrink(1.5);
        let base = i.color.as_deref().and_then(parse_color).unwrap_or(ITEM_COLOR);
        let fill = if moving == Some(i.id.as_str()) { base.gamma_multiply(0.4) } else { base };
        painter.rect_filled(r, CornerRadius::same(3), fill);
        let picked = selected.contains(&i.id);
        let stroke = if picked {
            Stroke::new(2.5, visuals.selection.stroke.color)
        } else {
            Stroke::new(1.0, Color32::from_black_alpha(90))
        };
        painter.rect_stroke(r, CornerRadius::same(3), stroke, StrokeKind::Inside);
        // Text stays on its item: badges along the top, the label below them, cut short.
        let text = text_on(fill);
        let clipped = painter.with_clip_rect(r.shrink(1.0));
        let mut top = r.top() + 2.0;
        if !i.badges.is_empty() || i.locked {
            let size = (cell * 0.2).clamp(8.0, 11.0);
            let badges = if i.locked { format!("🔒 {}", i.badges.join(" ")) } else { i.badges.join(" ") };
            let row = clipped.text(
                r.right_top() + vec2(-3.0, 2.0),
                Align2::RIGHT_TOP,
                badges,
                FontId::proportional(size),
                text,
            );
            top = row.bottom();
        }
        if let Some(label) = &i.label {
            let size = (cell * 0.2).clamp(8.0, 13.0);
            let area = Rect::from_min_max(pos2(r.left() + 3.0, top), r.max - vec2(3.0, 2.0));
            let mut job = egui::text::LayoutJob::simple(label.clone(), FontId::proportional(size), text, area.width());
            job.wrap.max_rows = ((area.height() / (size * 1.25)).floor() as usize).max(1);
            job.wrap.break_anywhere = true;
            job.wrap.overflow_character = Some('…');
            let galley = clipped.layout_job(job);
            clipped.galley(area.center() - galley.size() / 2.0, galley, text);
        }
    }
    if let Some((to, ok)) = ghost {
        let color = if ok { visuals.selection.stroke.color } else { visuals.error_fg_color };
        let r = frame.rect(to).shrink(1.0);
        painter.rect_filled(r, CornerRadius::same(3), color.gamma_multiply(0.25));
        painter.rect_stroke(r, CornerRadius::same(3), Stroke::new(2.0, color), StrokeKind::Inside);
        painter.text(r.center(), Align2::CENTER_CENTER, format!("{} × {}", to.w, to.h), small.clone(), color);
    }
    if response.has_focus() {
        painter.rect_stroke(
            rect,
            2.0,
            Stroke::new(1.0, visuals.selection.stroke.color.gamma_multiply(0.5)),
            StrokeKind::Inside,
        );
    }

    // The cursor says what a drag would do.
    if memory.drag.is_none()
        && let Some(p) = response.hover_pos()
    {
        let (cx, cy) = frame.cells(p);
        if let Some(i) = item_at(&editor.items, cx, cy).map(|i| &editor.items[i]).filter(|i| !i.locked) {
            let cursor =
                handle_at(frame.rect(Cells::of_item(i)), p, HANDLE_REACH).map_or(CursorIcon::Grab, Handle::cursor);
            ui.ctx().set_cursor_icon(cursor);
        }
        let mm = editor.pitch;
        response.clone().on_hover_text_at_pointer(t!(
            "grid.at",
            x = format!("{:.1}", cx.max(0.0) * mm),
            y = format!("{:.1}", cy.max(0.0) * mm)
        ));
    }

    ui.data_mut(|d| d.insert_temp(id, memory));
    if changes.iter().any(|c| matches!(c, GridChange::Selected { .. })) || selection.is_some() {
        *selection = Some(selected);
    }
    changes
}

/// Black or white, whichever reads on `fill`.
fn text_on(fill: Color32) -> Color32 {
    let [r, g, b, _] = fill.to_srgba_unmultiplied();
    let light = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
    if light > 140.0 { Color32::from_gray(20) } else { Color32::from_gray(240) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor() -> GridEditor {
        let item = |id: &str, x: f32, y: f32, w: f32, h: f32| GridItem {
            id: id.into(),
            x,
            y,
            w,
            h,
            color: None,
            label: None,
            badges: Vec::new(),
            actions: Vec::new(),
            locked: false,
        };
        let zone = |id: &str, x: f32, y: f32, w: f32, h: f32, accepts_items: bool| GridZone {
            id: id.into(),
            x,
            y,
            w,
            h,
            label: None,
            color: None,
            accepts_items,
        };
        GridEditor {
            id: "grid".into(),
            cols: 5,
            rows: 7,
            pitch: 42.0,
            snap_div: 2,
            zones: vec![zone("zone_L", -0.5, 0.0, 0.5, 7.0, true), zone("zone_V", 0.0, 7.0, 5.0, 0.25, false)],
            items: vec![item("0", 0.0, 0.0, 2.0, 2.0), item("1", 2.0, 0.0, 1.0, 3.0), item("2", 1.0, 1.0, 1.0, 1.0)],
            selected: Vec::new(),
            height: None,
        }
    }

    #[test]
    fn values_snap_to_parts_of_a_cell() {
        assert_eq!(snap(1.3, 1), 1.0);
        assert_eq!(snap(1.3, 2), 1.5);
        assert_eq!(snap(1.2, 2), 1.0);
        assert_eq!(snap(-0.4, 2), -0.5);
        assert_eq!(snap(2.6, 0), 3.0, "0 counts as whole cells");
    }

    #[test]
    fn the_top_item_is_hit_and_edges_belong_to_the_next_cell() {
        let e = editor();
        assert_eq!(item_at(&e.items, 1.5, 1.5), Some(2), "drawn last, lies on top");
        assert_eq!(item_at(&e.items, 0.5, 0.5), Some(0));
        assert_eq!(item_at(&e.items, 2.0, 0.5), Some(1), "the left edge is the item's");
        assert_eq!(item_at(&e.items, 4.5, 6.5), None);
    }

    #[test]
    fn handles_are_the_edges_near_the_pointer() {
        let r = Rect::from_min_size(pos2(100.0, 100.0), vec2(80.0, 40.0));
        assert_eq!(handle_at(r, pos2(140.0, 120.0), 5.0), None, "the middle moves");
        assert_eq!(handle_at(r, pos2(178.0, 120.0), 5.0), Some(Handle { right: true, ..Handle::default() }));
        assert_eq!(
            handle_at(r, pos2(101.0, 139.0), 5.0),
            Some(Handle { left: true, bottom: true, ..Handle::default() }),
            "a corner"
        );
        assert_eq!(handle_at(r, pos2(300.0, 120.0), 5.0), None);
        let tiny = Rect::from_min_size(pos2(0.0, 0.0), vec2(8.0, 8.0));
        assert_eq!(handle_at(tiny, pos2(4.0, 4.0), 5.0), None, "small items keep a middle");
    }

    #[test]
    fn resizing_moves_the_grabbed_edges_on_the_snap_grid() {
        let start = Cells::new(1.0, 1.0, 2.0, 1.0);
        let right = Handle { right: true, ..Handle::default() };
        assert_eq!(resized(start, right, 1.3, 0.0, 1), Cells::new(1.0, 1.0, 3.0, 1.0));
        assert_eq!(resized(start, right, -5.0, 0.0, 2), Cells::new(1.0, 1.0, 0.5, 1.0), "at least one step");
        let top_left = Handle { left: true, top: true, ..Handle::default() };
        assert_eq!(resized(start, top_left, -0.6, -0.4, 2), Cells::new(0.5, 0.5, 2.5, 1.5));
    }

    #[test]
    fn items_fit_on_free_cells_of_the_grid_and_in_accepting_zones() {
        let e = editor();
        assert!(fits(&e, Some("2"), Cells::new(3.0, 3.0, 1.0, 1.0)), "free cells");
        assert!(!fits(&e, Some("2"), Cells::new(2.0, 2.0, 1.0, 1.0)), "on item 1");
        assert!(fits(&e, Some("1"), Cells::new(2.0, 0.5, 1.0, 3.0)), "its own old place does not count");
        assert!(fits(&e, None, Cells::new(-0.5, 4.0, 1.5, 1.0)), "reaching into the left zone");
        assert!(!fits(&e, None, Cells::new(4.0, 6.5, 1.0, 1.0)), "the front zone takes no items");
        assert!(!fits(&e, None, Cells::new(4.5, 3.0, 1.0, 1.0)), "out on the right");
        assert!(!fits(&e, None, Cells::new(3.0, 3.0, 0.0, 1.0)), "nothing");
        let gap = [Cells::new(0.0, 0.0, 1.0, 1.0), Cells::new(2.0, 0.0, 1.0, 1.0)];
        assert!(!covered(Cells::new(0.5, 0.0, 2.0, 1.0), &gap), "a gap between areas");
        let touching = [Cells::new(0.0, 0.0, 1.0, 1.0), Cells::new(1.0, 0.0, 1.0, 1.0)];
        assert!(covered(Cells::new(0.5, 0.0, 1.0, 1.0), &touching));
    }

    #[test]
    fn a_band_takes_every_snap_cell_it_touches() {
        assert_eq!(band((1.2, 0.2), (2.6, 1.9), 1), Cells::new(1.0, 0.0, 2.0, 2.0));
        assert_eq!(band((2.6, 1.9), (1.2, 0.2), 1), Cells::new(1.0, 0.0, 2.0, 2.0), "either way");
        assert_eq!(band((1.2, 0.2), (1.2, 0.2), 2), Cells::new(1.0, 0.0, 0.5, 0.5), "a click is one step");
        assert_eq!(band((1.0, 1.0), (2.0, 2.0), 1), Cells::new(1.0, 1.0, 1.0, 1.0), "on the lines");
    }

    #[test]
    fn merging_makes_the_box_around_the_items() {
        let e = editor();
        let m = merged(e.items.iter().map(Cells::of_item)).unwrap();
        assert_eq!(m, Cells::new(0.0, 0.0, 3.0, 3.0));
        assert_eq!(merged(Vec::<Cells>::new()), None);
        assert_eq!(merged([Cells::new(1.0, 2.0, 1.0, 1.0)]), Some(Cells::new(1.0, 2.0, 1.0, 1.0)));
    }

    #[test]
    fn moves_stay_within_the_allowed_box_on_the_snap_grid() {
        let bounds = Cells::new(-0.5, 0.0, 5.5, 7.0);
        assert_eq!(clamp_into(Cells::new(5.0, 6.5, 1.0, 1.0), bounds, 2), Cells::new(4.0, 6.0, 1.0, 1.0));
        assert_eq!(clamp_into(Cells::new(-3.0, -1.0, 2.0, 1.0), bounds, 2), Cells::new(-0.5, 0.0, 2.0, 1.0));
        assert_eq!(clamp_into(Cells::new(2.0, 3.0, 1.0, 1.0), bounds, 2), Cells::new(2.0, 3.0, 1.0, 1.0), "inside");
        // A zone narrower than a snap step: the item stops at the last whole step.
        let narrow = Cells::new(-0.464, 0.0, 5.928, 7.0);
        assert_eq!(clamp_into(Cells::new(5.0, 0.0, 1.0, 2.0), narrow, 1), Cells::new(4.0, 0.0, 1.0, 2.0));
        assert_eq!(clamp_into(Cells::new(-1.0, 0.0, 1.0, 2.0), narrow, 1), Cells::new(0.0, 0.0, 1.0, 2.0));
    }

    /// Drives the editor through egui as the user would: a frame per step.
    struct Driver {
        ctx: egui::Context,
        editor: GridEditor,
        selection: Option<Vec<String>>,
        frame: Option<Frame>,
        time: f64,
    }

    impl Driver {
        fn new(editor: GridEditor) -> Driver {
            let mut d = Driver { ctx: egui::Context::default(), editor, selection: None, frame: None, time: 0.0 };
            d.step(Vec::new());
            d
        }

        fn step(&mut self, events: Vec<egui::Event>) -> Vec<GridChange> {
            self.step_with(events, egui::Modifiers::NONE)
        }

        fn step_with(&mut self, events: Vec<egui::Event>, modifiers: egui::Modifiers) -> Vec<GridChange> {
            self.time += 0.05;
            let events = std::iter::once(egui::Event::ModifiersChanged(modifiers)).chain(events).collect();
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(600.0, 600.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let mut changes = Vec::new();
            let mut frame = None;
            let mut output = self.ctx.run_ui(input, |ui| {
                // Narrower than the screen, so drags can leave it.
                ui.set_max_width(300.0);
                let (top_left, width) = (ui.next_widget_position(), ui.available_width().max(60.0));
                let (size, cell) = fit(&self.editor, width);
                frame = Some(Frame::of(Rect::from_min_size(top_left, size), bounds(&self.editor), cell));
                changes.extend(show(ui, &self.editor, &mut self.selection));
            });
            output.textures_delta.clear();
            self.frame = frame;
            changes
        }

        /// Where cell coordinates are on screen.
        fn at(&self, x: f32, y: f32) -> Pos2 {
            let f = self.frame.unwrap();
            f.origin + vec2(x, y) * f.cell
        }

        fn button(&self, p: Pos2, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
            egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers }
        }

        fn click(&mut self, x: f32, y: f32, modifiers: egui::Modifiers) -> Vec<GridChange> {
            let p = self.at(x, y);
            let mut out =
                self.step_with(vec![egui::Event::PointerMoved(p), self.button(p, true, modifiers)], modifiers);
            out.extend(self.step_with(vec![self.button(p, false, modifiers)], modifiers));
            out
        }

        fn drag(&mut self, from: (f32, f32), to: (f32, f32)) -> Vec<GridChange> {
            let (a, b) = (self.at(from.0, from.1), self.at(to.0, to.1));
            let none = egui::Modifiers::NONE;
            let mut out = self.step(vec![egui::Event::PointerMoved(a), self.button(a, true, none)]);
            for k in 1..=8 {
                out.extend(self.step(vec![egui::Event::PointerMoved(a + (b - a) * (k as f32 / 8.0))]));
            }
            out.extend(self.step(vec![self.button(b, false, none)]));
            out
        }

        fn key(&mut self, key: egui::Key) -> Vec<GridChange> {
            let event = egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            };
            self.step(vec![event])
        }
    }

    /// Two items on a 4 × 3 grid with an accepting zone on the right: A at (0, 0), B at (2, 0) 1 × 2.
    fn small() -> GridEditor {
        let mut e = editor();
        e.cols = 4;
        e.rows = 3;
        e.snap_div = 1;
        e.zones = vec![GridZone {
            id: "zone_R".into(),
            x: 4.0,
            y: 0.0,
            w: 0.46,
            h: 3.0,
            label: None,
            color: None,
            accepts_items: true,
        }];
        e.items = vec![e.items[2].clone(), e.items[1].clone()];
        e.items[0].id = "A".into();
        (e.items[0].x, e.items[0].y) = (0.0, 0.0);
        e.items[1].id = "B".into();
        (e.items[1].h, e.items[1].y) = (2.0, 0.0);
        e
    }

    fn moved(item: &str, x: f32, y: f32) -> GridChange {
        GridChange::Moved { item: item.into(), x, y }
    }

    #[test]
    fn dragging_moves_resizes_and_draws_items() {
        let mut d = Driver::new(small());
        let changes = d.drag((0.5, 0.5), (1.6, 0.4));
        assert_eq!(changes, [GridChange::Selected { items: vec!["A".into()] }, moved("A", 1.0, 0.0)]);
        // Onto B: sent all the same, the plugin decides.
        assert!(d.drag((0.5, 0.5), (2.5, 0.5)).contains(&moved("A", 2.0, 0.0)));
        // Flung far out to the right (the drag starts beyond A's edge): stops at the last whole
        // cell, the zone is too narrow for it.
        let changes = d.drag((0.5, 0.5), (5.5, 0.5));
        assert!(changes.contains(&moved("A", 3.0, 0.0)), "{changes:?}");
        // B's right edge one cell further.
        let changes = d.drag((2.97, 1.0), (3.9, 1.0));
        assert!(
            changes.contains(&GridChange::Resized { item: "B".into(), x: 2.0, y: 0.0, w: 2.0, h: 2.0 }),
            "{changes:?}"
        );
        // On free cells: a new item, over the cells the drag touched.
        let changes = d.drag((0.2, 2.2), (1.4, 2.6));
        assert!(changes.contains(&GridChange::Created { x: 0.0, y: 2.0, w: 2.0, h: 1.0 }), "{changes:?}");
        // Over an item it is not created.
        let changes = d.drag((1.2, 1.2), (2.4, 1.4));
        assert!(!changes.iter().any(|c| matches!(c, GridChange::Created { .. })), "{changes:?}");
    }

    #[test]
    fn clicks_select_and_keys_delete_and_nudge() {
        let mut d = Driver::new(small());
        assert_eq!(d.click(0.5, 0.5, egui::Modifiers::NONE), [GridChange::Selected { items: vec!["A".into()] }]);
        assert_eq!(d.key(egui::Key::ArrowRight), [moved("A", 1.0, 0.0)]);
        assert_eq!(d.key(egui::Key::ArrowUp), [], "already at the top");
        let both = d.click(2.5, 0.5, egui::Modifiers::SHIFT);
        assert_eq!(both, [GridChange::Selected { items: vec!["A".into(), "B".into()] }]);
        assert_eq!(d.key(egui::Key::Delete), [GridChange::Deleted { items: vec!["A".into(), "B".into()] }]);
        assert_eq!(d.click(3.5, 2.5, egui::Modifiers::NONE), [GridChange::Selected { items: Vec::new() }]);
        assert_eq!(d.selection, Some(Vec::new()), "kept until the plugin renders anew");
    }
}
