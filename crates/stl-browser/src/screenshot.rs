//! Test hook: `STLB_SCREENSHOT=out.png [STLB_SCREENSHOT_DELAY=8]` saves a picture of the
//! window once the delay has passed (thumbnails had time to load) and then quits.
//! Lets us check the UI from scripts and CI without capturing the user's desktop.
//! `STLB_POINTER=x,y` additionally pins the mouse pointer (logical points) to test hover effects,
//! `STLB_SEARCH=query` starts with a library search, `STLB_HOLD=1` holds the left button down
//! at the pointer from 0.7 s on and `STLB_WHEEL=dy` turns the mouse wheel there for half a second after 1 s.

use std::time::{Duration, Instant};

pub struct ScreenshotHook {
    pub pointer: Option<egui::Pos2>,
    hold: bool,
    wheel: f32,
    started: Instant,
    pressed: bool,
    pub search: Option<String>,
    /// `STLB_FLATTEN=1`: start with "include subfolders" on.
    pub flatten: bool,
    /// `STLB_VIEW=duplicates`: open the duplicates view once the index is ready.
    pub duplicates: bool,
    path: String,
    at: Instant,
    requested: bool,
}

impl ScreenshotHook {
    pub fn from_env() -> Option<ScreenshotHook> {
        let path = std::env::var("STLB_SCREENSHOT").ok()?;
        let delay = std::env::var("STLB_SCREENSHOT_DELAY").ok().and_then(|s| s.parse().ok()).unwrap_or(8.0);
        let pointer = std::env::var("STLB_POINTER").ok().and_then(|s| {
            let (x, y) = s.split_once(',')?;
            Some(egui::pos2(x.trim().parse().ok()?, y.trim().parse().ok()?))
        });
        let search = std::env::var("STLB_SEARCH").ok();
        let hold = std::env::var_os("STLB_HOLD").is_some();
        let flatten = std::env::var_os("STLB_FLATTEN").is_some();
        let duplicates = std::env::var("STLB_VIEW").is_ok_and(|v| v == "duplicates");
        let wheel = std::env::var("STLB_WHEEL").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        Some(ScreenshotHook {
            pointer,
            hold,
            wheel,
            started: Instant::now(),
            pressed: false,
            search,
            flatten,
            duplicates,
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
        let Some(pos) = self.pointer else { return };
        raw.events.retain(|e| {
            !matches!(
                e,
                egui::Event::PointerMoved(_)
                    | egui::Event::PointerGone
                    | egui::Event::PointerButton { .. }
                    | egui::Event::MouseWheel { .. }
            )
        });
        raw.events.push(egui::Event::PointerMoved(pos));
        let t = self.started.elapsed().as_secs_f32();
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
