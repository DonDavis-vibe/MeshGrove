//! A picture of the user's behind the file view (Settings → General → Appearance → Background
//! picture), as SolidCraft has one behind its 3D view. It is read off the window thread; until it
//! is there, and when it cannot be read, the view's own background stays.
//!
//! Ported from SolidCraft (`crates/ui-egui/src/gpu.rs`: `background_uv`, `decode_background`,
//! `paint_background_picture`), MIT OR Apache-2.0, Copyright (c) the SolidCraft authors.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

/// What a read gives: the picture's pixels, or why it could not be read.
type Decoded = Arc<Mutex<Option<Result<egui::ColorImage, String>>>>;

/// The background picture, once read.
#[derive(Default)]
pub struct Background {
    /// The file shown (or being read).
    path: Option<PathBuf>,
    decoded: Decoded,
    texture: Option<egui::TextureHandle>,
}

impl Background {
    /// Paints the part of picture `path` that `rect` shows, the picture covering the window from
    /// its centre and dimmed by `dim` (0 to 1) towards the panels' colour; nothing for `None`.
    /// Gives why the picture could not be read, once.
    pub fn paint(
        &mut self,
        painter: &egui::Painter,
        rect: egui::Rect,
        path: Option<&Path>,
        dim: f32,
    ) -> Option<String> {
        if self.path.as_deref() != path {
            self.read(painter.ctx(), path);
        }
        let done = self.decoded.lock().unwrap_or_else(PoisonError::into_inner).take();
        let error = match done {
            Some(Ok(image)) => {
                let options = egui::TextureOptions {
                    mipmap_mode: Some(egui::TextureFilter::Linear),
                    ..egui::TextureOptions::LINEAR
                };
                self.texture = Some(painter.ctx().load_texture("background-picture", image, options));
                None
            }
            Some(Err(error)) => Some(error),
            None => None,
        };
        if let Some(texture) = &self.texture {
            let window = painter.ctx().content_rect();
            painter.image(texture.id(), rect, cover_uv(texture.size_vec2(), rect, window), egui::Color32::WHITE);
            let panel = painter.ctx().global_style().visuals.panel_fill;
            painter.rect_filled(rect, 0.0, panel.gamma_multiply(dim.clamp(0.0, 1.0)));
        }
        error
    }

    /// Starts reading `path` on a thread of its own; the picture shown so far goes.
    fn read(&mut self, ctx: &egui::Context, path: Option<&Path>) {
        self.path = path.map(Path::to_path_buf);
        self.texture = None;
        // A read still under way for the former picture answers into a slot nobody looks at.
        self.decoded = Decoded::default();
        let Some(path) = path.map(Path::to_path_buf) else { return };
        let (slot, repaint) = (self.decoded.clone(), ctx.clone());
        let limit = ctx.input(|i| i.max_texture_side) as u32;
        std::thread::spawn(move || {
            let result = std::fs::read(&path).map_err(|e| e.to_string()).and_then(|bytes| decode(&bytes, limit));
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
            repaint.request_repaint();
        });
    }
}

/// The part of a picture of size `image` (as texture coordinates) that `rect` shows in window
/// `window`: the picture covers the window from its centre, so a narrower window keeps the middle
/// of it, and panes and the view's own size don't move it.
fn cover_uv(image: egui::Vec2, rect: egui::Rect, window: egui::Rect) -> egui::Rect {
    let scale = (window.width() / image.x).max(window.height() / image.y).max(f32::EPSILON);
    let uv = |p: egui::Pos2| ((p - window.center()) / scale + image / 2.0) / image;
    egui::Rect::from_min_max(uv(rect.min).to_pos2(), uv(rect.max).to_pos2())
}

/// A picture's pixels, at most `limit` on a side (what the graphics card takes).
fn decode(bytes: &[u8], limit: u32) -> Result<egui::ColorImage, String> {
    let mut image = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
    if image.width().max(image.height()) > limit {
        image = image.resize(limit, limit, image::imageops::FilterType::Triangle);
    }
    let rgba = image.into_rgba8();
    Ok(egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picture_covers_the_window_from_its_centre() {
        let window = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 100.0));
        // As wide as the window: all of it across, the middle half of its height.
        let uv = cover_uv(egui::vec2(100.0, 100.0), window, window);
        assert_eq!(uv, egui::Rect::from_min_max(egui::pos2(0.0, 0.25), egui::pos2(1.0, 0.75)));
        // A pane in the right half shows the right half of that.
        let pane = egui::Rect::from_min_max(egui::pos2(100.0, 0.0), egui::pos2(200.0, 100.0));
        let uv = cover_uv(egui::vec2(100.0, 100.0), pane, window);
        assert_eq!(uv, egui::Rect::from_min_max(egui::pos2(0.5, 0.25), egui::pos2(1.0, 0.75)));
    }
}
