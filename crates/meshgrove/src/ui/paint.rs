//! Painting on a model in a 3D view, as a plugin's mesh view asks for it: which gesture a press
//! starts, and what it does until the button is let go. Kept apart from the view, as a plain
//! state machine, so the rules can be tested without a window:
//!
//! - The left button belongs to the tool: it paints (Shift held at the press erases) or picks.
//! - The right button turns the model, the middle one moves it, the wheel zooms.
//! - What a press starts it finishes: neither another button, nor Shift, nor leaving the view
//!   turns one gesture into another on the way.
//! - A press outside the view starts nothing, even when the pointer then comes in.
//! - A double click does not reset the camera (it is two dabs of paint).

use egui::{Pos2, Vec2};

/// What the left button does (the plugin's `PaintTool`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    /// Dabs along the stroke.
    Brush,
    /// The triangles under the pointer.
    Triangles,
    /// A click picks a point.
    Click,
    /// Where the pointer goes on the surface.
    Drag,
}

impl Tool {
    pub fn of(tool: meshgrove_plugin_api::ui::PaintTool) -> Tool {
        use meshgrove_plugin_api::ui::PaintTool;
        match tool {
            PaintTool::Brush => Tool::Brush,
            PaintTool::Triangles => Tool::Triangles,
            PaintTool::Click => Tool::Click,
            PaintTool::Drag => Tool::Drag,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Primary,
    Secondary,
    Middle,
}

/// What a press started.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gesture {
    Paint {
        erase: bool,
    },
    /// The click tool: picks where the button is let go, if it did not travel.
    Pick {
        shift: bool,
        from: Pos2,
    },
    Orbit,
    Pan,
}

/// What happened in one frame, as far as the view is concerned.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Frame {
    /// The pointer, if it is anywhere over the window.
    pub pos: Option<Pos2>,
    /// The pointer is over the view (and nothing lies on top of it there).
    pub inside: bool,
    pub shift: bool,
    pub pressed: Vec<Button>,
    pub released: Vec<Button>,
    /// How far the pointer moved since the last frame.
    pub delta: Vec2,
    /// Wheel, in points.
    pub scroll: f32,
    pub double_clicked: bool,
}

/// What the view is to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    StrokeStart {
        erase: bool,
    },
    /// Paint where the pointer is.
    Sample(Pos2),
    StrokeEnd,
    Pick {
        at: Pos2,
        shift: bool,
    },
    Orbit(Vec2),
    Pan(Vec2),
    Zoom(f32),
}

/// A click of the pick tool may travel this far (points) and still be a click.
const CLICK_SLOP: f32 = 4.0;

/// The gesture under way, if any.
#[derive(Debug, Default)]
pub struct Gestures {
    active: Option<(Button, Gesture)>,
}

impl Gestures {
    #[cfg(test)]
    pub fn active(&self) -> Option<Gesture> {
        self.active.map(|(_, g)| g)
    }

    /// Whether the user is painting now.
    #[cfg(test)]
    pub fn painting(&self) -> bool {
        matches!(self.active(), Some(Gesture::Paint { .. }))
    }

    /// Takes in one frame; what the view is to do, in order.
    pub fn frame(&mut self, input: &Frame, tool: Tool) -> Vec<Action> {
        let mut actions = Vec::new();
        for &button in &input.pressed {
            if self.active.is_some() || !input.inside {
                continue;
            }
            let Some(at) = input.pos else { continue };
            let gesture = match button {
                Button::Primary if tool == Tool::Click => Gesture::Pick { shift: input.shift, from: at },
                Button::Primary => {
                    actions.push(Action::StrokeStart { erase: input.shift });
                    actions.push(Action::Sample(at));
                    Gesture::Paint { erase: input.shift }
                }
                Button::Secondary => Gesture::Orbit,
                Button::Middle => Gesture::Pan,
            };
            self.active = Some((button, gesture));
        }
        match self.active {
            Some((_, Gesture::Paint { .. })) if input.delta != Vec2::ZERO => {
                if let Some(at) = input.pos {
                    actions.push(Action::Sample(at));
                }
            }
            Some((_, Gesture::Orbit)) if input.delta != Vec2::ZERO => actions.push(Action::Orbit(input.delta)),
            Some((_, Gesture::Pan)) if input.delta != Vec2::ZERO => actions.push(Action::Pan(input.delta)),
            _ => {}
        }
        if let Some((button, gesture)) = self.active
            && input.released.contains(&button)
        {
            self.active = None;
            match gesture {
                Gesture::Paint { .. } => actions.push(Action::StrokeEnd),
                Gesture::Pick { shift, from } => {
                    if let Some(at) = input.pos.filter(|at| at.distance(from) <= CLICK_SLOP) {
                        actions.push(Action::Pick { at, shift });
                    }
                }
                Gesture::Orbit | Gesture::Pan => {}
            }
        }
        if input.inside && input.scroll != 0.0 {
            actions.push(Action::Zoom(input.scroll));
        }
        // A double click is two presses, each handled above: it never resets the camera here.
        actions
    }
}

/// How often a stroke under way is passed on, at most.
pub const STROKE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// Where a stroke is when it is passed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Start,
    Move,
    End,
}

/// What goes to the plugin.
#[derive(Debug, Clone, PartialEq)]
pub enum Out<H> {
    Pick { hit: Option<H>, shift: bool },
    Stroke { stroke: u64, phase: Phase, erase: bool, hits: Vec<H> },
}

#[derive(Debug)]
struct Pending<H> {
    id: u64,
    erase: bool,
    /// Its start went out.
    started: bool,
    ended: bool,
    hits: Vec<H>,
}

/// Picks and strokes on their way to a plugin. The panel answers one event at a time, so
/// nothing goes out while it is busy: what comes meanwhile waits here, and a stroke under way
/// goes out at most every [`STROKE_INTERVAL`], with what was added since.
#[derive(Debug)]
pub struct Outbox<H> {
    picks: Vec<(Option<H>, bool)>,
    strokes: std::collections::VecDeque<Pending<H>>,
    next: u64,
    last_sent: Option<std::time::Instant>,
}

impl<H> Default for Outbox<H> {
    fn default() -> Outbox<H> {
        Outbox { picks: Vec::new(), strokes: Default::default(), next: 1, last_sent: None }
    }
}

impl<H> Outbox<H> {
    pub fn start(&mut self, erase: bool) {
        self.end();
        let id = self.next;
        self.next += 1;
        self.strokes.push_back(Pending { id, erase, started: false, ended: false, hits: Vec::new() });
    }

    /// A hit of the stroke under way (none: dropped).
    pub fn add(&mut self, hit: H) {
        if let Some(open) = self.strokes.back_mut().filter(|s| !s.ended) {
            open.hits.push(hit);
        }
    }

    pub fn end(&mut self) {
        if let Some(open) = self.strokes.back_mut() {
            open.ended = true;
        }
    }

    pub fn pick(&mut self, hit: Option<H>, shift: bool) {
        self.picks.push((hit, shift));
    }

    /// Whether anything waits to go out.
    pub fn is_empty(&self) -> bool {
        self.picks.is_empty() && self.strokes.is_empty()
    }

    /// What goes out now: nothing while the panel is `busy`; else the picks, the strokes that
    /// ended, and the one under way if it has something new and its last part went out at
    /// least [`STROKE_INTERVAL`] ago (its start goes at once).
    pub fn take(&mut self, busy: bool, now: std::time::Instant) -> Vec<Out<H>> {
        let mut out = Vec::new();
        if busy {
            return out;
        }
        out.extend(self.picks.drain(..).map(|(hit, shift)| Out::Pick { hit, shift }));
        let due = self.last_sent.is_none_or(|at| now.duration_since(at) >= STROKE_INTERVAL);
        while let Some(stroke) = self.strokes.front_mut() {
            let (stroke_id, erase) = (stroke.id, stroke.erase);
            let piece = |phase, hits| Out::Stroke { stroke: stroke_id, phase, erase, hits };
            if !stroke.started {
                stroke.started = true;
                out.push(piece(Phase::Start, std::mem::take(&mut stroke.hits)));
            } else if !stroke.ended && due && !stroke.hits.is_empty() {
                out.push(piece(Phase::Move, std::mem::take(&mut stroke.hits)));
            }
            if !stroke.ended {
                break;
            }
            out.push(piece(Phase::End, std::mem::take(&mut stroke.hits)));
            self.strokes.pop_front();
        }
        if !out.is_empty() {
            self.last_sent = Some(now);
        }
        out
    }
}

#[cfg(test)]
mod outbox_tests {
    use std::time::{Duration, Instant};

    use super::*;

    fn stroke(out: &Out<u32>) -> (Phase, Vec<u32>) {
        match out {
            Out::Stroke { phase, hits, .. } => (*phase, hits.clone()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_stroke_goes_out_in_pieces_and_waits_while_the_panel_is_busy() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut outbox = Outbox::<u32>::default();
        outbox.start(false);
        outbox.add(1);
        let first = outbox.take(false, at(0));
        assert_eq!(first.iter().map(stroke).collect::<Vec<_>>(), [(Phase::Start, vec![1])], "the start at once");
        outbox.add(2);
        assert!(outbox.take(false, at(20)).is_empty(), "not before 50 ms");
        outbox.add(3);
        // The panel is busy: everything waits, however long.
        assert!(outbox.take(true, at(60)).is_empty() && outbox.take(true, at(500)).is_empty());
        let next = outbox.take(false, at(510));
        assert_eq!(next.iter().map(stroke).collect::<Vec<_>>(), [(Phase::Move, vec![2, 3])]);
        assert!(outbox.take(false, at(600)).is_empty(), "nothing new, nothing sent");
        // Let go while busy: the end waits too, and brings what came last.
        outbox.add(4);
        outbox.end();
        assert!(outbox.take(true, at(700)).is_empty());
        let end = outbox.take(false, at(701));
        assert_eq!(end.iter().map(stroke).collect::<Vec<_>>(), [(Phase::End, vec![4])], "no 50 ms wait for the end");
        assert!(outbox.is_empty());
    }

    #[test]
    fn quick_strokes_and_picks_all_arrive_in_order() {
        let now = Instant::now();
        let mut outbox = Outbox::<u32>::default();
        // Two strokes and a click while the panel was busy.
        outbox.start(true);
        outbox.add(1);
        outbox.end();
        outbox.start(false);
        outbox.add(2);
        outbox.pick(Some(9), true);
        assert!(outbox.take(true, now).is_empty());
        let out = outbox.take(false, now);
        assert_eq!(out[0], Out::Pick { hit: Some(9), shift: true });
        let strokes: Vec<(u64, Phase, bool)> = out[1..]
            .iter()
            .map(|o| match o {
                Out::Stroke { stroke, phase, erase, .. } => (*stroke, *phase, *erase),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(strokes, [(1, Phase::Start, true), (1, Phase::End, true), (2, Phase::Start, false)]);
        // A new stroke ends one left open; hits after the end are dropped.
        outbox.start(false);
        outbox.add(5);
        let out = outbox.take(false, now + Duration::from_secs(1));
        assert_eq!(out.iter().map(stroke).map(|(p, _)| p).collect::<Vec<_>>(), [Phase::End, Phase::Start]);
        outbox.end();
        outbox.add(6);
        let out = outbox.take(false, now + Duration::from_secs(2));
        assert_eq!(out.iter().map(stroke).collect::<Vec<_>>(), [(Phase::End, vec![])], "5 went with the start");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2};

    fn press(button: Button, at: Pos2) -> Frame {
        Frame { pos: Some(at), inside: true, pressed: vec![button], ..Frame::default() }
    }

    fn moved(to: Pos2, delta: Vec2) -> Frame {
        Frame { pos: Some(to), inside: true, delta, ..Frame::default() }
    }

    fn release(button: Button, at: Pos2) -> Frame {
        Frame { pos: Some(at), inside: true, released: vec![button], ..Frame::default() }
    }

    #[test]
    fn a_left_drag_paints_and_does_not_turn_the_model() {
        let mut g = Gestures::default();
        let start = g.frame(&press(Button::Primary, pos2(10.0, 10.0)), Tool::Brush);
        assert_eq!(start, [Action::StrokeStart { erase: false }, Action::Sample(pos2(10.0, 10.0))]);
        assert!(g.painting());
        let drag = g.frame(&moved(pos2(30.0, 12.0), vec2(20.0, 2.0)), Tool::Brush);
        assert_eq!(drag, [Action::Sample(pos2(30.0, 12.0))], "paint, no orbit");
        assert_eq!(g.frame(&moved(pos2(30.0, 12.0), Vec2::ZERO), Tool::Brush), [], "still: nothing new");
        assert_eq!(g.frame(&release(Button::Primary, pos2(30.0, 12.0)), Tool::Brush), [Action::StrokeEnd]);
        assert_eq!(g.active(), None);
        // The other buttons look around.
        g.frame(&press(Button::Secondary, pos2(0.0, 0.0)), Tool::Brush);
        assert_eq!(g.frame(&moved(pos2(5.0, 0.0), vec2(5.0, 0.0)), Tool::Brush), [Action::Orbit(vec2(5.0, 0.0))]);
        g.frame(&release(Button::Secondary, pos2(5.0, 0.0)), Tool::Brush);
        g.frame(&press(Button::Middle, pos2(0.0, 0.0)), Tool::Brush);
        assert_eq!(g.frame(&moved(pos2(0.0, 5.0), vec2(0.0, 5.0)), Tool::Brush), [Action::Pan(vec2(0.0, 5.0))]);
        g.frame(&release(Button::Middle, pos2(0.0, 5.0)), Tool::Brush);
        let wheel = Frame { pos: Some(pos2(1.0, 1.0)), inside: true, scroll: 3.0, ..Frame::default() };
        assert_eq!(g.frame(&wheel, Tool::Brush), [Action::Zoom(3.0)]);
    }

    #[test]
    fn shift_at_the_press_erases_for_the_whole_stroke() {
        let mut g = Gestures::default();
        let shifted = Frame { shift: true, ..press(Button::Primary, pos2(1.0, 1.0)) };
        assert_eq!(g.frame(&shifted, Tool::Triangles)[0], Action::StrokeStart { erase: true });
        // Shift let go on the way: still erasing.
        g.frame(&moved(pos2(4.0, 1.0), vec2(3.0, 0.0)), Tool::Triangles);
        assert_eq!(g.active(), Some(Gesture::Paint { erase: true }));
        g.frame(&release(Button::Primary, pos2(4.0, 1.0)), Tool::Triangles);
        // Shift pressed on the way: still painting.
        g.frame(&press(Button::Primary, pos2(1.0, 1.0)), Tool::Triangles);
        g.frame(&Frame { shift: true, ..moved(pos2(4.0, 1.0), vec2(3.0, 0.0)) }, Tool::Triangles);
        assert_eq!(g.active(), Some(Gesture::Paint { erase: false }));
    }

    #[test]
    fn a_press_outside_the_view_does_nothing() {
        let mut g = Gestures::default();
        let outside = Frame { inside: false, ..press(Button::Primary, pos2(-5.0, 10.0)) };
        assert_eq!(g.frame(&outside, Tool::Brush), []);
        // The pointer comes in with the button still down: no painting, no turning.
        assert_eq!(g.frame(&moved(pos2(20.0, 10.0), vec2(25.0, 0.0)), Tool::Brush), []);
        assert_eq!(g.frame(&release(Button::Primary, pos2(20.0, 10.0)), Tool::Brush), []);
        let outside = Frame { inside: false, ..press(Button::Secondary, pos2(-5.0, 10.0)) };
        g.frame(&outside, Tool::Brush);
        assert_eq!(g.frame(&moved(pos2(20.0, 10.0), vec2(25.0, 0.0)), Tool::Brush), []);
        // Nor does the wheel outside it.
        let wheel = Frame { pos: Some(pos2(-5.0, 1.0)), inside: false, scroll: 3.0, ..Frame::default() };
        assert_eq!(g.frame(&wheel, Tool::Brush), []);
    }

    #[test]
    fn a_double_click_paints_twice_and_keeps_the_camera() {
        let mut g = Gestures::default();
        let at = pos2(10.0, 10.0);
        let mut all = Vec::new();
        for frame in [press(Button::Primary, at), release(Button::Primary, at), press(Button::Primary, at)] {
            all.extend(g.frame(&frame, Tool::Brush));
        }
        all.extend(g.frame(&Frame { double_clicked: true, ..release(Button::Primary, at) }, Tool::Brush));
        let starts = all.iter().filter(|a| matches!(a, Action::StrokeStart { .. })).count();
        let ends = all.iter().filter(|a| **a == Action::StrokeEnd).count();
        assert_eq!((starts, ends), (2, 2));
        assert!(all.iter().all(|a| matches!(a, Action::StrokeStart { .. } | Action::Sample(_) | Action::StrokeEnd)));
    }

    #[test]
    fn a_drag_never_turns_into_another_gesture() {
        let mut g = Gestures::default();
        g.frame(&press(Button::Primary, pos2(0.0, 0.0)), Tool::Brush);
        // The right button joins in: still painting, no turning; letting it go ends nothing.
        let both = Frame { pressed: vec![Button::Secondary], ..moved(pos2(5.0, 0.0), vec2(5.0, 0.0)) };
        assert_eq!(g.frame(&both, Tool::Brush), [Action::Sample(pos2(5.0, 0.0))]);
        assert_eq!(g.frame(&release(Button::Secondary, pos2(5.0, 0.0)), Tool::Brush), []);
        assert!(g.painting());
        // Leaving the view keeps the stroke going; it ends where the button is let go.
        let out = Frame { inside: false, ..moved(pos2(-50.0, 0.0), vec2(-55.0, 0.0)) };
        assert_eq!(g.frame(&out, Tool::Brush), [Action::Sample(pos2(-50.0, 0.0))]);
        let end = Frame { inside: false, ..release(Button::Primary, pos2(-50.0, 0.0)) };
        assert_eq!(g.frame(&end, Tool::Brush), [Action::StrokeEnd]);
        // An orbit stays an orbit when the left button joins in.
        g.frame(&press(Button::Secondary, pos2(0.0, 0.0)), Tool::Brush);
        let both = Frame { pressed: vec![Button::Primary], ..moved(pos2(3.0, 0.0), vec2(3.0, 0.0)) };
        assert_eq!(g.frame(&both, Tool::Brush), [Action::Orbit(vec2(3.0, 0.0))]);
        assert_eq!(g.frame(&release(Button::Primary, pos2(3.0, 0.0)), Tool::Brush), []);
        assert_eq!(g.active(), Some(Gesture::Orbit));
    }

    #[test]
    fn the_click_tool_picks_on_a_click_and_not_on_a_drag() {
        let mut g = Gestures::default();
        let shifted = Frame { shift: true, ..press(Button::Primary, pos2(10.0, 10.0)) };
        assert_eq!(g.frame(&shifted, Tool::Click), [], "nothing until it is let go");
        assert_eq!(
            g.frame(&release(Button::Primary, pos2(12.0, 11.0)), Tool::Click),
            [Action::Pick { at: pos2(12.0, 11.0), shift: true }]
        );
        g.frame(&press(Button::Primary, pos2(10.0, 10.0)), Tool::Click);
        assert_eq!(
            g.frame(&moved(pos2(40.0, 10.0), vec2(30.0, 0.0)), Tool::Click),
            [],
            "a drag does not turn the model"
        );
        assert_eq!(g.frame(&release(Button::Primary, pos2(40.0, 10.0)), Tool::Click), [], "nor pick");
    }
}
