//! Test hook: `STLB_SCREENSHOT=out.png [STLB_SCREENSHOT_DELAY=8]` saves a picture of the
//! window once the delay has passed (thumbnails had time to load) and then quits.
//! Lets us check the UI from scripts and CI without capturing the user's desktop.
//! `STLB_POINTER=x,y` additionally pins the mouse pointer (logical points) to test hover effects.

use std::time::{Duration, Instant};

pub struct ScreenshotHook {
    pub pointer: Option<egui::Pos2>,
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
        Some(ScreenshotHook { pointer, path, at: Instant::now() + Duration::from_secs_f32(delay), requested: false })
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
