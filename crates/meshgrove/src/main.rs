// Release builds on Windows are GUI programs without a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[macro_use]
extern crate rust_i18n;

i18n!("locales", fallback = "en");

mod actions;
mod app;
mod folder_scan;
mod i18n;
mod library;
mod meshes;
mod nav;
mod print_export;
mod screenshot;
mod spacemouse;
mod text_preview;
mod thumbs;
mod ui;

use std::path::PathBuf;

use meshgrove_core::config::Config;

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,meshgrove=info")).init();
    meshgrove_core::config::migrate_legacy_dirs();
    let (config, config_error) = match Config::load() {
        Ok(config) => (config, None),
        Err(e) => {
            log::warn!("ignoring broken settings: {e}");
            (Config::default(), Some(e))
        }
    };
    i18n::init(config.language.as_deref());
    meshgrove_core::config::set_gltf_unit(config.gltf_unit);

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
            .with_app_id("meshgrove")
            .with_icon(app_icon())
            .with_inner_size([1400.0, 860.0])
            .with_min_inner_size([720.0, 480.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "meshgrove",
        options,
        Box::new(|cc| {
            Ok(Box::new(app::App::new(cc, config, config_error, start).map_err(|e| e.into_boxed_dyn_error())?))
        }),
    )
}

/// Window and taskbar icon (Windows, X11, macOS Dock while running).
pub(crate) fn app_icon() -> egui::IconData {
    let png = include_bytes!("../../../packaging/icons/meshgrove-256.png");
    let image = image::load_from_memory(png).expect("bundled icon is a valid PNG").into_rgba8();
    egui::IconData { width: image.width(), height: image.height(), rgba: image.into_raw() }
}
