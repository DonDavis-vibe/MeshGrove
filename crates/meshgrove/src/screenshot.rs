//! Test hook: `MESHGROVE_SCREENSHOT=out.png [MESHGROVE_SCREENSHOT_DELAY=8]` saves a picture of the
//! window once the delay has passed (thumbnails had time to load) and then quits.
//! Lets us check the UI from scripts and CI without capturing the user's desktop.
//! `MESHGROVE_POINTER=x,y` additionally pins the mouse pointer (logical points) to test hover effects,
//! `MESHGROVE_SEARCH=query` starts with a library search, `MESHGROVE_HOLD=1` holds the left button down
//! at the pointer from 0.7 s on and `MESHGROVE_WHEEL=dy` turns the mouse wheel there for half a second after 1 s.
//! `MESHGROVE_CLICKS=x,y;x,y;…` clicks those points one after the other, from 3 s on, every 0.8 s
//! (`x,y*2` double-clicks, `x,y*r` right-clicks), and `MESHGROVE_TYPE=text` types the text after the last click; steps
//! separated by `|` follow half a second apart, `{enter}`, `{down}` and `{esc}` press those keys,
//! `{click x,y}` clicks there (to click after typing), `{rclick x,y}` right-clicks, `{wheel dy}` turns
//! the wheel at the pointer, `{copy}` and `{paste}` are Ctrl+C and Ctrl+V.
//! `MESHGROVE_MEASURE=fx,fy;fx,fy` measures between those points (fractions of the view) in the
//! first model window that opens, which the hook's input cannot reach; `MESHGROVE_WINDOW_VIEW=top`
//! (or `front`, `left` …) starts that window in a standard view.

use std::time::{Duration, Instant};

pub struct ScreenshotHook {
    pub pointer: Option<egui::Pos2>,
    hold: bool,
    wheel: f32,
    /// Point and how: `1` click, `2` double click, `0` right click.
    clicks: Vec<(egui::Pos2, u8)>,
    next_click: usize,
    /// Steps to type after the clicks, half a second apart: text, `{enter}`, `{down}`, `{esc}`.
    typing: std::collections::VecDeque<String>,
    typed_at: f32,
    release: Option<(egui::Pos2, egui::PointerButton)>,
    started: Instant,
    pressed: bool,
    pub search: Option<String>,
    /// Measuring points for the first model window.
    pub measure: Vec<egui::Vec2>,
    /// Standard view for the first model window.
    pub window_view: Option<String>,
    /// `MESHGROVE_FLATTEN=1`: start with "include subfolders" on.
    pub flatten: bool,
    /// `MESHGROVE_VIEW`: what to open once it can be shown. `duplicates` (the duplicates view),
    /// `inbox`, `import`, `review`, `history`, `settings` or `settings:<category>` (the settings window,
    /// e.g. `settings:printer`, `settings:ai`).
    pub view: Option<String>,
    path: String,
    at: Instant,
    requested: bool,
}

impl ScreenshotHook {
    pub fn from_env() -> Option<ScreenshotHook> {
        let path = std::env::var("MESHGROVE_SCREENSHOT").ok()?;
        let delay = std::env::var("MESHGROVE_SCREENSHOT_DELAY").ok().and_then(|s| s.parse().ok()).unwrap_or(8.0);
        let point = |s: &str| {
            let (x, y) = s.split_once(',')?;
            Some(egui::pos2(x.trim().parse().ok()?, y.trim().parse().ok()?))
        };
        let pointer = std::env::var("MESHGROVE_POINTER").ok().and_then(|s| point(&s));
        let click = |s: &str| match (s.strip_suffix("*2"), s.strip_suffix("*r")) {
            (Some(s), _) => point(s).map(|p| (p, 2)),
            (_, Some(s)) => point(s).map(|p| (p, 0)),
            _ => point(s).map(|p| (p, 1)),
        };
        let clicks =
            std::env::var("MESHGROVE_CLICKS").map(|s| s.split(';').filter_map(click).collect()).unwrap_or_default();
        let search = std::env::var("MESHGROVE_SEARCH").ok();
        let measure = std::env::var("MESHGROVE_MEASURE")
            .map(|s| s.split(';').filter_map(point).map(|p| p.to_vec2()).collect())
            .unwrap_or_default();
        let hold = std::env::var_os("MESHGROVE_HOLD").is_some();
        let flatten = std::env::var_os("MESHGROVE_FLATTEN").is_some();
        let wheel = std::env::var("MESHGROVE_WHEEL").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        Some(ScreenshotHook {
            pointer,
            hold,
            wheel,
            clicks,
            next_click: 0,
            release: None,
            typing: std::env::var("MESHGROVE_TYPE")
                .map(|s| s.split('|').map(str::to_owned).collect())
                .unwrap_or_default(),
            typed_at: 0.0,
            started: Instant::now(),
            pressed: false,
            search,
            measure,
            window_view: std::env::var("MESHGROVE_WINDOW_VIEW").ok(),
            flatten,
            view: std::env::var("MESHGROVE_VIEW").ok(),
            path,
            at: Instant::now() + Duration::from_secs_f32(delay),
            requested: false,
        })
    }

    pub fn tick(&mut self, ctx: &egui::Context) {
        if self.requested {
            return;
        }
        // eframe skips frames for hidden/occluded windows; a test run must not hang on that.
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
        let now = Instant::now();
        if now < self.at {
            ctx.request_repaint_after(self.at - now);
            return;
        }
        self.requested = true;
        let (path, ctx2) = (self.path.clone(), ctx.clone());
        ctx.request_screenshot(move |image| {
            let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
            let saved = image::RgbaImage::from_raw(image.size[0] as u32, image.size[1] as u32, rgba)
                .map(|img| img.save(&path))
                .map(|r| r.map_err(|e| e.to_string()));
            match saved {
                Some(Ok(())) => log::info!("screenshot saved to {path}"),
                Some(Err(e)) => log::error!("screenshot failed: {e}"),
                None => log::error!("screenshot failed: bad image size"),
            }
            ctx2.send_viewport_cmd(egui::ViewportCommand::Close);
        });
        ctx.request_repaint();
    }
}

impl ScreenshotHook {
    /// Replaces real pointer input with the scripted one.
    pub fn inject(&mut self, raw: &mut egui::RawInput) {
        if self.pointer.is_none() && self.clicks.is_empty() && self.typing.is_empty() && self.release.is_none() {
            return;
        }
        raw.events.retain(|e| {
            !matches!(
                e,
                egui::Event::PointerMoved(_)
                    | egui::Event::PointerGone
                    | egui::Event::PointerButton { .. }
                    | egui::Event::MouseWheel { .. }
            )
        });
        let t = self.started.elapsed().as_secs_f32();
        let press = |pos, button, pressed| egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let button = |pos, pressed| press(pos, egui::PointerButton::Primary, pressed);
        if let Some((pos, which)) = self.release.take() {
            raw.events.push(press(pos, which, false));
        } else if let Some(&(pos, how)) = self.clicks.get(self.next_click)
            && t >= 3.0 + 0.8 * self.next_click as f32
        {
            self.next_click += 1;
            self.pointer = Some(pos);
            raw.events.push(egui::Event::PointerMoved(pos));
            if how == 2 {
                // The first click completes now, the second one with the release next frame.
                raw.events.push(button(pos, true));
                raw.events.push(button(pos, false));
            }
            let which = if how == 0 { egui::PointerButton::Secondary } else { egui::PointerButton::Primary };
            raw.events.push(press(pos, which, true));
            self.release = Some((pos, which));
        }
        if self.next_click == self.clicks.len()
            && t >= 3.0 + 0.8 * self.clicks.len() as f32
            && t >= self.typed_at + 0.5
            && let Some(step) = self.typing.pop_front()
        {
            self.typed_at = t;
            let key = match step.as_str() {
                "{enter}" => Some(egui::Key::Enter),
                "{down}" => Some(egui::Key::ArrowDown),
                "{esc}" => Some(egui::Key::Escape),
                _ => None,
            };
            let point = |s: &str| {
                let (x, y) = s.strip_suffix('}')?.split_once(',')?;
                Some(egui::pos2(x.trim().parse().ok()?, y.trim().parse().ok()?))
            };
            let click = step.strip_prefix("{click ").and_then(point);
            let right_click = step.strip_prefix("{rclick ").and_then(point);
            let wheel =
                step.strip_prefix("{wheel ").and_then(|s| s.strip_suffix('}')).and_then(|s| s.trim().parse().ok());
            match key {
                _ if wheel.is_some() => raw.events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, wheel.unwrap_or_default()),
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Wheel,
                    modifiers: egui::Modifiers::NONE,
                }),
                _ if click.is_some() || right_click.is_some() => {
                    let pos = click.or(right_click).unwrap_or_default();
                    let which =
                        if click.is_some() { egui::PointerButton::Primary } else { egui::PointerButton::Secondary };
                    self.pointer = Some(pos);
                    raw.events.push(egui::Event::PointerMoved(pos));
                    raw.events.push(press(pos, which, true));
                    self.release = Some((pos, which));
                }
                _ if step == "{copy}" => raw.events.push(egui::Event::Copy),
                _ if step == "{paste}" => raw.events.push(egui::Event::Paste(String::new())),
                Some(key) => {
                    for pressed in [true, false] {
                        raw.events.push(egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        });
                    }
                }
                None => raw.events.push(egui::Event::Text(step)),
            }
        }
        let Some(pos) = self.pointer else { return };
        raw.events.push(egui::Event::PointerMoved(pos));
        // Press only once the gallery is on screen, so the press lands on a tile.
        if self.hold && !self.pressed && t >= 0.7 {
            self.pressed = true;
            raw.events.push(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            });
        }
        if self.wheel != 0.0 && (1.0..1.5).contains(&t) {
            raw.events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, self.wheel),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
                modifiers: egui::Modifiers::NONE,
            });
        }
    }
}
