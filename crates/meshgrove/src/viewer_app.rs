//! A start with model files alone (the launcher "Meshgrove Viewer", or the settings' choice for
//! models opened from a file manager): each model in a window of its own, the first of them in
//! the program's, and no main window. Nothing of the library runs here (no index, no scan, no
//! thumbnails, no plugins, no AI, no update check) and no settings are written: a main window
//! that is open beside it is left alone.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context as _;
use egui::{Ui, ViewportCommand};
use meshgrove_core::config::Config;
use meshgrove_core::listing::{self, Entry};
use meshgrove_core::printer::Printer;
use meshgrove_render::Renderer;

use crate::meshes::{HeadStart, MeshStore};
use crate::screenshot::ScreenshotHook;
use crate::spacemouse::SpaceMouse;
use crate::startup::{self, Programs};
use crate::ui::model_window::{self, ModelWindow, WindowAction, WindowContext};
use crate::ui::viewer::SpaceInput;
use crate::ui::{EditorList, SlicerList, appearance};

/// Shows `files` (models, at least one) and returns once the last of their windows is closed.
pub fn run(config: Config, files: Vec<PathBuf>) -> eframe::Result {
    // The window and the graphics take a moment to start: the first model is read meanwhile.
    let head = files.first().map(|f| HeadStart::of(f));
    let name = files.first().map(|f| listing::file_name(f)).unwrap_or_default();
    let own_title_bar = appearance::own_title_bar(config.title_bar);
    let options = eframe::NativeOptions {
        viewport: model_window::viewport(&name, model_window::icon(crate::app_icon), own_title_bar),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "meshgrove",
        options,
        Box::new(move |cc| {
            Ok(Box::new(ViewerApp::new(cc, config, &files, head).map_err(|e| e.into_boxed_dyn_error())?))
        }),
    )
}

struct ViewerApp {
    config: Config,
    /// The first is shown in the program's own window, the others beside it.
    windows: Vec<ModelWindow>,
    /// The program's window is titled for this model.
    title: String,
    meshes: MeshStore,
    renderer: Renderer,
    printers: Vec<Printer>,
    programs: Programs,
    spacemouse: SpaceMouse,
    brand: egui::TextureHandle,
    screenshot: Option<ScreenshotHook>,
}

impl ViewerApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
        mut config: Config,
        files: &[PathBuf],
        head: Option<HeadStart>,
    ) -> anyhow::Result<ViewerApp> {
        let rs = cc.wgpu_render_state.as_ref().context("eframe was not started with the wgpu renderer")?;
        startup::mark("window and graphics");
        startup::look(&cc.egui_ctx, &config);
        let mut windows: Vec<ModelWindow> =
            files.iter().filter_map(|f| Entry::from_path(f).ok()).map(ModelWindow::new).collect();
        let first = windows.first_mut().context("none of the files can be read")?;
        let mut screenshot = ScreenshotHook::from_env();
        if let Some(hook) = &mut screenshot {
            if !hook.measure.is_empty() {
                first.script_measure(std::mem::take(&mut hook.measure));
            }
            if let Some(view) = hook.window_view.take() {
                first.script_view(&view);
            }
        }
        let app = ViewerApp {
            title: first.title(),
            windows,
            meshes: MeshStore::with_head_start(rs.device.clone(), cc.egui_ctx.clone(), head),
            renderer: Renderer::new(&rs.device, &rs.adapter),
            // Found here if the settings never named any; only the main window writes them down.
            printers: startup::printers(&mut config),
            programs: Programs::of(&mut config),
            spacemouse: SpaceMouse::start(cc.egui_ctx.clone()),
            brand: startup::brand(&cc.egui_ctx),
            screenshot,
            config,
        };
        startup::mark("viewer set up");
        Ok(app)
    }

    /// Does what a window asked for; what to tell the user in that window, if anything.
    fn act(&self, action: WindowAction) -> Option<String> {
        match action {
            WindowAction::OpenInSlicer(path, index) => {
                self.programs.open_in_slicer(vec![path], index).map(|(message, _)| message)
            }
            WindowAction::OpenInEditor(path, index) => self.programs.open_in_editor(vec![path], index),
            WindowAction::Message(message) => Some(message),
            WindowAction::ShowInGallery(path) => show_in_gallery(&path)
                .err()
                .map(|e| t!("toast.slicer_failed", name = "Meshgrove", error = e.to_string()).into_owned()),
            // Prints are logged in the library's index, which is not open here (`can_log_prints`).
            WindowAction::Print(..) => None,
        }
    }
}

impl eframe::App for ViewerApp {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let rs = frame.wgpu_render_state().expect("wgpu renderer").clone();
        appearance::follow_system(&ctx);
        let _language = appearance::language();
        let programs = &self.programs;
        crate::ui::program_icons::publish(programs.slicers.iter().chain(&programs.editors), self.config.program_icons);
        self.meshes.begin_frame();
        let (raw, presses) = (self.spacemouse.snapshot().raw, self.spacemouse.take_presses());
        let knob = &self.config.spacemouse;
        let space = if knob.enabled { SpaceInput::read(&ctx, knob, raw, presses) } else { None };
        if let Some(hook) = &mut self.screenshot {
            hook.tick(&ctx);
        }

        let mut cx = WindowContext {
            meshes: &mut self.meshes,
            render_state: &rs,
            renderer: &self.renderer,
            printers: &self.printers,
            space: space.as_ref(),
            free_orbit: self.config.free_orbit,
            slicers: SlicerList { names: &programs.slicer_names, default: programs.default_slicer },
            editors: EditorList { programs: &programs.editors, default: programs.default_editor },
            quality: self.config.preview_quality,
            can_log_prints: false,
            icon: model_window::icon(crate::app_icon),
            own_title_bar: appearance::own_title_bar(self.config.title_bar),
            brand: self.brand.id(),
            standalone: true,
        };
        let (mut asked, mut closed) = (Vec::new(), Vec::new());
        for (i, window) in self.windows.iter_mut().enumerate() {
            let mut actions = Vec::new();
            let open = match i {
                0 => window.show_root(ui, &mut cx, &mut actions),
                _ => window.show(&ctx, &mut cx, &mut actions),
            };
            asked.extend(actions.into_iter().map(|action| (i, action)));
            if !open {
                closed.push(i);
            }
        }
        for (i, action) in asked {
            if let Some(message) = self.act(action) {
                self.windows[i].notify(message);
            }
        }
        startup::first_frame();
        if self.windows.first().is_some_and(ModelWindow::loaded) {
            startup::first_model();
        }

        // A test run's picture is taken: its close ends the program, whatever is open.
        let finished = self.screenshot.as_ref().is_some_and(ScreenshotHook::finished);
        let own_closed = closed.first() == Some(&0);
        for i in closed.into_iter().rev() {
            self.windows.remove(i);
        }
        match self.windows.first() {
            Some(first) if !finished => {
                if own_closed {
                    // The program's window stays for the next model, whose own window goes
                    // instead: the program ends with the last model.
                    ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                    ctx.request_repaint();
                }
                let title = first.title();
                if title != self.title {
                    ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
                    self.title = title;
                }
            }
            _ => ctx.send_viewport_cmd(ViewportCommand::Close),
        }
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        // As in the main window: the press the desktop took from Meshgrove's own title bar.
        solidcraft_ui_kit::window_chrome::release_press_taken_by_os(ctx, raw);
        if let Some(hook) = &mut self.screenshot {
            hook.inject(raw);
        }
    }
}

/// Starts the main window as a program of its own, at the folder of `path` with it selected:
/// the AppImage this runs from, else this program.
fn show_in_gallery(path: &Path) -> std::io::Result<()> {
    let program = match std::env::var_os("APPIMAGE").filter(|p| !p.is_empty()) {
        Some(appimage) => PathBuf::from(appimage),
        None => std::env::current_exe()?,
    };
    let mut command = Command::new(program);
    // An AppImage's runtime variables of this process are not handed on.
    for var in ["APPDIR", "APPIMAGE", "ARGV0", "OWD"] {
        command.env_remove(var);
    }
    command.arg("--gallery").arg(path).spawn().map(drop)
}
