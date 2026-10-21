#[macro_use]
extern crate rust_i18n;

i18n!("locales", fallback = "en");

mod app;
mod i18n;
mod library;
mod meshes;
mod nav;
mod screenshot;
mod thumbs;
mod ui;

use std::path::PathBuf;

use stlb_core::config::Config;

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,stl_browser=info")).init();
    let (config, config_error) = match Config::load() {
        Ok(config) => (config, None),
        Err(e) => {
            log::warn!("ignoring broken settings: {e}");
            (Config::default(), Some(e))
        }
    };
    i18n::init(config.language.as_deref());

    // Optional argument: a folder to open, or a file to open its folder with it selected.
    let start = std::env::args_os()
        .nth(1)
        .filter(|a| !a.is_empty())
        .map(PathBuf::from)
        .map(|p| std::path::absolute(&p).unwrap_or(p));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(t!("app.title"))
            // Matches the .desktop file name so KDE shows the right icon on Wayland.
            .with_app_id("stl-browser")
            .with_inner_size([1400.0, 860.0])
            .with_min_inner_size([720.0, 480.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "stl-browser",
        options,
        Box::new(|cc| {
            Ok(Box::new(app::App::new(cc, config, config_error, start).map_err(|e| e.into_boxed_dyn_error())?))
        }),
    )
}
