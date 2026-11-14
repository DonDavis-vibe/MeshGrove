//! The programs' own icons (slicers and editors) as pictures to paint beside their names: on
//! the settings' Programs page, on the info pane's buttons and in the open-with menus.
//!
//! The app says each frame which programs there are and whether their icons are wanted
//! ([`publish`]); the places that show a program ask by its name ([`paint`]). A picture is
//! looked up and read once, the first time it is asked for
//! ([`meshgrove_core::program_icons`] finds the file). An AppImage's icon is inside the
//! AppImage: it is unpacked on a thread of its own and shows when that is done.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use egui::{Rect, TextureHandle, Ui};
use meshgrove_core::config::Program;
use meshgrove_core::program_icons::Finder;

/// The pictures are kept at this edge length: sharp at the 16 to 28 points they are drawn at,
/// also on a dense screen.
const EDGE: u32 = 64;

#[derive(Default)]
struct State {
    enabled: bool,
    /// The programs of the settings, by the name they are shown under.
    programs: HashMap<String, Program>,
    /// What was found for a program's file: its picture, or that it has none.
    icons: HashMap<PathBuf, Option<TextureHandle>>,
    /// Reads the desktop entries once, at the first question.
    finder: Option<Finder>,
    /// AppImages whose icon is being unpacked.
    unpacking: std::collections::HashSet<PathBuf>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// AppImages whose unpacking is done: the program, and the picture file it gave (or none).
static UNPACKED: Mutex<Vec<(PathBuf, Option<PathBuf>)>> = Mutex::new(Vec::new());

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut state = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    f(state.get_or_insert_with(State::default))
}

/// The programs there are now, and whether their icons are shown at all (the settings' switch).
pub fn publish<'a>(programs: impl IntoIterator<Item = &'a Program>, enabled: bool) {
    with(|state| {
        state.enabled = enabled;
        state.programs.clear();
        state.programs.extend(programs.into_iter().map(|p| (p.name.clone(), p.clone())));
    });
}

/// Whether the icons are switched on: rows then keep room for one, so names stay in line.
pub fn enabled() -> bool {
    with(|state| state.enabled)
}

/// The icon of the program shown as `name`; none when icons are off, the name is not a
/// program's, or the program brings no icon that can be read.
pub fn of(ctx: &egui::Context, name: &str) -> Option<TextureHandle> {
    with(|state| {
        if !state.enabled {
            return None;
        }
        // What the threads unpacked since the last question.
        let done = std::mem::take(&mut *UNPACKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for (program, file) in done {
            state.unpacking.remove(&program);
            state.icons.insert(program, file.and_then(|file| load(ctx, &file)));
        }
        let program = state.programs.get(name)?.clone();
        if let Some(known) = state.icons.get(&program.program) {
            return known.clone();
        }
        if state.unpacking.contains(&program.program) {
            return None;
        }
        let finder = state.finder.get_or_insert_with(Finder::real);
        let found = finder.icon_file(&program);
        // Nothing on the desktop for it, and it is an AppImage: its icon is inside. Unpacking
        // starts the AppImage's runtime, which takes a moment: not on this thread.
        if found.is_none()
            && let Some(cache) = super::settings::cache_dir().map(|dir| dir.join("program-icons"))
            && meshgrove_core::program_icons::is_appimage(&program.program)
        {
            state.unpacking.insert(program.program.clone());
            let (file, ctx) = (program.program.clone(), ctx.clone());
            let spawned = std::thread::Builder::new().name("program-icon".into()).spawn(move || {
                let icon = meshgrove_core::program_icons::appimage_icon(&file, &cache);
                UNPACKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((file, icon));
                ctx.request_repaint();
            });
            if spawned.is_ok() {
                return None;
            }
            state.unpacking.remove(&program.program);
        }
        let texture = found.and_then(|file| load(ctx, &file));
        state.icons.insert(program.program.clone(), texture.clone());
        texture
    })
}

/// Paints the icon of the program `name` into `rect` (kept square, in its middle). False when
/// there is none: the caller draws its own sign then.
pub fn paint(ui: &Ui, rect: Rect, name: &str) -> bool {
    let Some(texture) = of(ui.ctx(), name) else { return false };
    let side = rect.width().min(rect.height());
    let at = Rect::from_center_size(rect.center(), egui::vec2(side, side));
    let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    ui.painter().image(texture.id(), at, uv, egui::Color32::WHITE);
    true
}

/// Reads a PNG or an SVG into a texture of [`EDGE`] pixels; none when it cannot be read.
fn load(ctx: &egui::Context, file: &Path) -> Option<TextureHandle> {
    let image = match file.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("svg") => svg(file),
        _ => raster(file),
    };
    let Some(image) = image else {
        log::debug!("program icon {} could not be read", file.display());
        return None;
    };
    Some(ctx.load_texture(format!("program-icon-{}", file.display()), image, egui::TextureOptions::LINEAR))
}

fn raster(file: &Path) -> Option<egui::ColorImage> {
    // Icons are small; a file of many megabytes under an icon's name is not read.
    if file.metadata().ok()?.len() > 8 << 20 {
        return None;
    }
    let image = image::open(file).ok()?;
    let image = image.resize(EDGE, EDGE, image::imageops::FilterType::Lanczos3).to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}

fn svg(file: &Path) -> Option<egui::ColorImage> {
    use resvg::{tiny_skia, usvg};
    let data = std::fs::read(file).ok().filter(|d| d.len() <= 4 << 20)?;
    let tree = usvg::Tree::from_data(&data, &usvg::Options::default()).ok()?;
    let size = tree.size();
    let longest = size.width().max(size.height());
    if longest <= 0.0 {
        return None;
    }
    let scale = EDGE as f32 / longest;
    let (w, h) = ((size.width() * scale).ceil().max(1.0) as u32, (size.height() * scale).ceil().max(1.0) as u32);
    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    Some(egui::ColorImage::from_rgba_premultiplied([w as usize, h as usize], pixmap.data()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG and an SVG are both read into a picture of the kept size; a file that is neither
    /// gives none.
    #[test]
    fn pictures_are_read_from_png_and_svg() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("icon.png");
        image::RgbaImage::from_pixel(128, 128, image::Rgba([200, 60, 20, 255])).save(&png).unwrap();
        let read = raster(&png).unwrap();
        assert_eq!(read.size, [EDGE as usize, EDGE as usize]);
        let vector = dir.path().join("icon.svg");
        let drawing = r##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="12"><rect width="24" height="12" fill="#f0a94a"/></svg>"##;
        std::fs::write(&vector, drawing).unwrap();
        let drawn = svg(&vector).unwrap();
        assert_eq!(drawn.size, [EDGE as usize, EDGE as usize / 2], "the longer side is the kept size");
        assert_eq!(drawn.pixels[0], egui::Color32::from_rgb(0xf0, 0xa9, 0x4a));
        std::fs::write(dir.path().join("broken.png"), "not a picture").unwrap();
        assert!(raster(&dir.path().join("broken.png")).is_none());
    }
}
