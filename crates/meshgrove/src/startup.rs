//! What the main window and a start with model files alone (`viewer_app`) set up alike: the
//! look, and the programs and printers of the settings.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use meshgrove_core::config::{Config, Program, Slicer};
use meshgrove_core::listing;
use meshgrove_core::printer::{self, Printer};
use meshgrove_core::{Kind, programs};

use crate::actions;

static BEGAN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Starts the clock of [`mark`]: first thing in `main`.
pub fn begin() {
    BEGAN.get_or_init(std::time::Instant::now);
}

/// Notes in the log how long after its start the program got to `what` ("start: first frame
/// after 212 ms"): where a slow start spends its time.
pub fn mark(what: &str) {
    if let Some(began) = BEGAN.get() {
        log::info!("start: {what} after {} ms", began.elapsed().as_millis());
    }
}

static FRAME_DRAWN: AtomicBool = AtomicBool::new(false);
static MODEL_DRAWN: AtomicBool = AtomicBool::new(false);

/// A frame begins, so what the frame before drew has been handed to the screen: notes once
/// each when that was the first frame and the first frame with the model. Call it first thing
/// in every frame.
pub fn frame_begins() {
    static FRAME: std::sync::Once = std::sync::Once::new();
    static MODEL: std::sync::Once = std::sync::Once::new();
    if FRAME_DRAWN.load(Ordering::Relaxed) {
        FRAME.call_once(|| mark("first frame handed to the screen"));
    }
    if MODEL_DRAWN.load(Ordering::Relaxed) {
        MODEL.call_once(|| mark("first model handed to the screen"));
    }
}

/// The first frame is drawn (the window shows once it is handed to the screen): noted once,
/// call it at the end of every frame.
pub fn first_frame() {
    static NOTED: std::sync::Once = std::sync::Once::new();
    NOTED.call_once(|| {
        mark("first frame drawn");
        FRAME_DRAWN.store(true, Ordering::Relaxed);
    });
}

/// The first model is drawn into a frame: noted once. Asks for one more frame, whose
/// beginning says when this one reached the screen ([`frame_begins`]).
pub fn first_model(ctx: &egui::Context) {
    static NOTED: std::sync::Once = std::sync::Once::new();
    NOTED.call_once(|| {
        mark("first model drawn");
        MODEL_DRAWN.store(true, Ordering::Relaxed);
        ctx.request_repaint();
    });
}

/// How a window of the program is started: drawn with wgpu, on a device that has what the
/// 3D views need of a software renderer ([`meshgrove_render::device_features`]: nothing
/// where a graphics card draws).
pub fn native_options(viewport: egui::ViewportBuilder) -> eframe::NativeOptions {
    let mut options = eframe::NativeOptions { viewport, renderer: eframe::Renderer::Wgpu, ..Default::default() };
    if let egui_wgpu::WgpuSetup::CreateNew(new) = &mut options.wgpu_options.wgpu_setup {
        new.instance_descriptor.flags.remove(meshgrove_render::UNUSED_INSTANCE_FLAGS);
        let usual = new.device_descriptor.clone();
        new.device_descriptor = std::sync::Arc::new(move |adapter| {
            let mut descriptor = usual(adapter);
            descriptor.required_features |= meshgrove_render::device_features(adapter);
            descriptor
        });
    }
    options
}

static SOFTWARE: AtomicBool = AtomicBool::new(false);

/// The renderer of the 3D views of this program's windows.
pub fn renderer(rs: &egui_wgpu::RenderState) -> meshgrove_render::Renderer {
    let renderer = meshgrove_render::Renderer::for_window(&rs.device, &rs.adapter);
    SOFTWARE.store(renderer.software(), Ordering::Relaxed);
    mark("renderer");
    renderer
}

/// Whether the processor draws the 3D views instead of a graphics card (the settings say so).
pub fn software_renderer() -> bool {
    SOFTWARE.load(Ordering::Relaxed)
}

/// The theme and the fonts of the settings; `MESHGROVE_THEME` overrides the theme for one
/// start (screenshots).
pub fn look(ctx: &egui::Context, config: &Config) {
    // Ctrl +/- and Ctrl+wheel resize tiles instead of zooming the whole UI.
    ctx.options_mut(|o| o.zoom_with_keyboard = false);
    crate::ui::appearance::install();
    let theme = std::env::var("MESHGROVE_THEME").ok().or_else(|| config.ui_theme.clone());
    crate::ui::appearance::apply(ctx, theme.as_deref().filter(|t| !t.is_empty()));
    crate::app::apply_fonts(ctx, config.ui_font.as_deref(), &BTreeSet::new());
    mark("look and fonts");
}

/// Meshgrove's sign for the title bars it draws itself.
pub fn brand(ctx: &egui::Context) -> egui::TextureHandle {
    let png = include_bytes!("../../../packaging/icons/meshgrove-64.png");
    let icon = image::load_from_memory(png).expect("bundled icon is a valid PNG").into_rgba8();
    let size = [icon.width() as usize, icon.height() as usize];
    let icon = egui::ColorImage::from_rgba_unmultiplied(size, icon.as_raw());
    ctx.load_texture("brand", icon, egui::TextureOptions::LINEAR)
}

/// The slicers and mesh editors the "open in" entries offer.
pub struct Programs {
    pub slicers: Vec<Slicer>,
    pub slicer_names: Vec<String>,
    pub default_slicer: usize,
    pub editors: Vec<Program>,
    pub default_editor: usize,
}

impl Programs {
    /// Those of the settings. Where none were ever chosen there, all that are installed, which
    /// `config` holds from then on.
    pub fn of(config: &mut Config) -> Programs {
        if config.slicers.is_none() {
            let found: Vec<Slicer> = programs::detect_slicers().into_iter().map(|d| d.program).collect();
            if !found.is_empty() {
                config.default_slicer = found.first().map(|s| s.name.clone());
                config.slicers = Some(found);
            }
        }
        let slicers = config.slicers.clone().unwrap_or_default();
        for s in &slicers {
            log::info!("slicer: {} ({})", s.name, s.program.display());
        }
        if slicers.is_empty() {
            log::info!("no slicer found; models open with the default application");
        }
        let default_slicer = Config::default_slicer_index(&slicers, config.default_slicer.as_deref());
        let slicer_names = slicers.iter().map(|s| s.name.clone()).collect();
        if config.editors.is_none() {
            let found: Vec<Program> = programs::detect_editors().into_iter().map(|d| d.program).collect();
            if !found.is_empty() {
                config.default_editor = found.first().map(|e| e.name.clone());
                config.editors = Some(found);
            }
        }
        let editors = config.editors.clone().unwrap_or_default();
        for e in &editors {
            log::info!("mesh editor: {} ({})", e.name, e.program.display());
        }
        let default_editor = Config::default_slicer_index(&editors, config.default_editor.as_deref());
        Programs { slicers, slicer_names, default_slicer, editors, default_editor }
    }

    /// Hands the models among `files` that slicers open to the slicer number `index`: what to
    /// tell the user, and the models it took (none if it could not be started).
    pub fn open_in_slicer(&self, files: Vec<PathBuf>, index: usize) -> Option<(String, Vec<PathBuf>)> {
        let slicer = self.slicers.get(index)?;
        let models: Vec<PathBuf> = files.into_iter().filter(|p| kind_of(p).is_sliceable()).collect();
        if models.is_empty() {
            return None;
        }
        Some(match actions::open_in_slicer(slicer, &models) {
            Ok(actions::SlicerOpened::Started) => {
                (t!("toast.slicer_started", name = slicer.name, count = models.len()).into_owned(), models)
            }
            Ok(actions::SlicerOpened::HandedOver) => {
                (t!("toast.slicer_handed_over", name = slicer.name, count = models.len()).into_owned(), models)
            }
            Err(e) => (t!("toast.slicer_failed", name = slicer.name, error = e.to_string()).into_owned(), Vec::new()),
        })
    }

    /// Opens the models among `files` that the editor number `index` reads: what to tell the
    /// user.
    pub fn open_in_editor(&self, files: Vec<PathBuf>, index: usize) -> Option<String> {
        let editor = self.editors.get(index)?;
        let models: Vec<PathBuf> = files.into_iter().filter(|p| editor.opens(kind_of(p))).collect();
        if models.is_empty() {
            return None;
        }
        let message = match actions::open_in_editor(editor, &models) {
            Ok(()) => t!("toast.slicer_started", name = editor.name, count = models.len()),
            Err(e) => t!("toast.slicer_failed", name = editor.name, error = e.to_string()),
        };
        Some(message.into_owned())
    }
}

fn kind_of(path: &Path) -> Kind {
    Kind::from_file_name(&listing::file_name(path))
}

/// The printers of the settings, in their order. Where none were ever chosen there, all the
/// slicers know, which `config` holds from then on.
pub fn printers(config: &mut Config) -> Vec<Printer> {
    if config.printers.is_none() {
        let found = printer::detect_all();
        if !found.is_empty() {
            config.default_printer = found.first().map(|d| d.printer.name.clone());
            config.printers = Some(found.into_iter().map(|d| d.printer).collect());
        }
    }
    let printers = config.printers_in_order();
    for p in &printers {
        log::info!("printer: {} ({} × {} × {} mm)", p.name, p.x, p.y, p.z);
    }
    printers
}
