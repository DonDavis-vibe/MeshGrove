#[macro_use]
extern crate rust_i18n;

i18n!("locales", fallback = "en");

mod i18n;
mod ui;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use stlb_core::{Mesh, loaders::stl};
use stlb_render::Renderer;
use ui::viewer::ModelViewer;

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,stl_browser=info")).init();
    i18n::init(None);

    let model_path = std::env::args_os().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(t!("app.title"))
            // Matches the .desktop file name so KDE shows the right icon on Wayland.
            .with_app_id("stl-browser")
            .with_inner_size([1280.0, 800.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "stl-browser",
        options,
        Box::new(|cc| {
            Ok(Box::new(
                App::new(cc, model_path.as_deref()).map_err(|e| e.into_boxed_dyn_error())?,
            ))
        }),
    )
}

struct LoadedModel {
    name: String,
    file_size: u64,
    triangles: usize,
    size_mm: glam::Vec3,
    load_time: Duration,
}

struct App {
    renderer: Renderer,
    viewer: ModelViewer,
    model: Option<LoadedModel>,
    error: Option<String>,
    gpu_name: String,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, model_path: Option<&std::path::Path>) -> anyhow::Result<App> {
        let rs = cc
            .wgpu_render_state
            .as_ref()
            .context("eframe was not started with the wgpu renderer")?;
        let mut app = App {
            renderer: Renderer::new(&rs.device, &rs.adapter),
            viewer: ModelViewer::new(),
            model: None,
            error: None,
            gpu_name: rs.adapter.get_info().name,
        };
        let mesh = match model_path {
            Some(path) => app.load(path),
            None => Some(Mesh::cube(20.0)),
        };
        if let Some(mesh) = mesh {
            app.viewer.set_mesh(app.renderer.upload(&rs.device, &mesh));
        }
        Ok(app)
    }

    fn load(&mut self, path: &std::path::Path) -> Option<Mesh> {
        let started = Instant::now();
        let result = std::fs::read(path)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| Ok((bytes.len(), stl::parse(&bytes)?)));
        match result {
            Ok((file_size, mesh)) => {
                self.model = Some(LoadedModel {
                    name: path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                    file_size: file_size as u64,
                    triangles: mesh.triangle_count(),
                    size_mm: mesh.bounds().map(|b| b.size()).unwrap_or_default(),
                    load_time: started.elapsed(),
                });
                Some(mesh)
            }
            Err(e) => {
                self.error = Some(t!("error.load_failed", error = e.to_string()).into_owned());
                None
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        egui::Panel::left("details").default_size(300.0).show(ui, |ui| {
            ui.heading(t!("app.title"));
            ui.separator();
            if let Some(m) = &self.model {
                ui.strong(&m.name);
                egui::Grid::new("model_facts").num_columns(2).show(ui, |ui| {
                    ui.label(t!("details.dimensions"));
                    ui.label(i18n::dimensions(m.size_mm));
                    ui.end_row();
                    ui.label(t!("details.triangles"));
                    ui.label(i18n::count(m.triangles));
                    ui.end_row();
                    ui.label(t!("details.file_size"));
                    ui.label(i18n::file_size(m.file_size));
                    ui.end_row();
                    ui.label(t!("details.load_time"));
                    ui.label(format!("{} ms", m.load_time.as_millis()));
                    ui.end_row();
                });
            }
            if let Some(err) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, err);
            }
            ui.add_space(8.0);
            ui.weak(t!("viewer.hint"));
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                ui.weak(t!("status.gpu", name = self.gpu_name));
            });
        });
        egui::CentralPanel::default().show(ui, |ui| {
            let rs = frame.wgpu_render_state().expect("wgpu renderer");
            self.viewer.ui(ui, rs, &self.renderer);
        });
    }
}
