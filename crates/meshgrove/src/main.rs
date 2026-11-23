// Release builds on Windows are GUI programs without a console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[macro_use]
extern crate rust_i18n;

i18n!("locales", fallback = "en");

mod actions;
mod app;
mod assist;
mod checker;
mod folder_scan;
mod i18n;
mod library;
mod logging;
mod meshes;
mod nav;
#[cfg(feature = "plugins")]
mod plugin_mesh;
#[cfg(feature = "plugins")]
mod plugins;
mod print_export;
mod proposals;
mod screenshot;
mod spacemouse;
mod startup;
mod text_preview;
mod thumbs;
mod ui;
mod update;
mod viewer_app;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use meshgrove_core::config::{Config, OpenFiles};

/// What this start shows.
#[derive(Debug, PartialEq)]
enum Launch {
    /// The main window: at this folder, or at this file's folder with the file selected.
    Gallery(Option<PathBuf>),
    /// Only model windows, one for each of these files.
    Viewer(Vec<PathBuf>),
}

/// Whether the settings send model files to the model window. They do where Meshgrove is one
/// launcher (Windows, macOS, the AppImage). Beside the launcher "Meshgrove Viewer" each of the
/// two has its meaning: this one shows the main window, whatever the settings held before.
fn settings_open_the_viewer(open_files: OpenFiles, kind: &update::InstallKind) -> bool {
    open_files == OpenFiles::Viewer && !kind.viewer_launcher()
}

/// Reads the command line (without the program's name): `--view FILES…` shows the models among
/// the files in windows of their own, `--gallery PATH` the main window; with neither, `viewer`
/// says what model files open in ([`settings_open_the_viewer`]). A start without a model
/// (`is_model`) among its files shows the main window, at the first of them.
fn launch(args: impl IntoIterator<Item = OsString>, viewer: bool, is_model: impl Fn(&Path) -> bool) -> Launch {
    let mut paths: Vec<PathBuf> = args.into_iter().filter(|a| !a.is_empty()).map(PathBuf::from).collect();
    let viewer = match paths.first().and_then(|p| p.to_str()) {
        Some("--view") => {
            paths.remove(0);
            true
        }
        Some("--gallery") => {
            paths.remove(0);
            false
        }
        _ => viewer,
    };
    let models: Vec<PathBuf> = paths.iter().filter(|p| is_model(p)).cloned().collect();
    if viewer && !models.is_empty() { Launch::Viewer(models) } else { Launch::Gallery(paths.into_iter().next()) }
}

/// A file the model windows show.
fn is_model_file(path: &Path) -> bool {
    let kind = meshgrove_core::Kind::from_file_name(&meshgrove_core::listing::file_name(path));
    meshes::viewable(kind) && path.is_file()
}

fn main() -> eframe::Result {
    startup::begin();
    meshgrove_core::config::migrate_legacy_dirs();
    let loaded = Config::load();
    let viewer = matches!(&loaded, Ok(config) if settings_open_the_viewer(config.open_files, update::kind()));
    let launch = launch(std::env::args_os().skip(1), viewer, is_model_file);
    logging::init(if matches!(launch, Launch::Viewer(_)) { logging::VIEWER } else { logging::MAIN });
    startup::mark("settings read");
    let (config, config_error) = match loaded {
        Ok(config) => (config, None),
        Err(e) => {
            log::warn!("ignoring broken settings: {e}");
            (Config::default(), Some(e))
        }
    };
    // Read beside the start of the window and the graphics, which takes the longest.
    app::warm_fonts();
    ui::program_icons::warm(&config);
    i18n::init(config.language.as_deref());
    meshgrove_core::config::set_gltf_unit(config.gltf_unit);
    meshgrove_core::config::set_model_units(config.model_units);
    meshgrove_core::config::set_display_units(config.display_units);

    let spelled = |p: PathBuf| {
        let p = std::path::absolute(&p).unwrap_or(p);
        meshgrove_core::fsinfo::spelled_like_root(&p, &config.library_roots)
    };
    // A folder to open, or a file to open its folder with it selected.
    let start = match launch {
        Launch::Viewer(files) => {
            let files = files.into_iter().map(spelled).collect();
            return viewer_app::run(config, files);
        }
        Launch::Gallery(start) => start.map(spelled),
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(t!("app.title"))
            // Matches the .desktop file name so KDE shows the right icon on Wayland.
            .with_app_id("meshgrove")
            .with_icon(app_icon())
            // Meshgrove's own title bar (the settings' choice) takes the place of the desktop's.
            .with_decorations(!ui::appearance::own_title_bar(config.title_bar))
            .with_inner_size([1400.0, 860.0])
            .with_min_inner_size([720.0, 480.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "meshgrove",
        options,
        Box::new(|cc| {
            let app = app::App::new(cc, config, config_error, start).map_err(|e| e.into_boxed_dyn_error())?;
            startup::mark("main window set up");
            Ok(Box::new(app))
        }),
    )
}

/// Window and taskbar icon (Windows, X11, macOS Dock while running).
pub(crate) fn app_icon() -> egui::IconData {
    let png = include_bytes!("../../../packaging/icons/meshgrove-256.png");
    let image = image::load_from_memory(png).expect("bundled icon is a valid PNG").into_rgba8();
    egui::IconData { width: image.width(), height: image.height(), rgba: image.into_raw() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(args: &[&str], viewer: bool) -> Launch {
        let is_model = |p: &Path| p.extension().is_some_and(|e| e == "stl" || e == "3mf");
        launch(args.iter().map(OsString::from), viewer, is_model)
    }

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    /// The launcher "Meshgrove Viewer" shows the models among its files, each in its window,
    /// whatever the settings say; what is no model does not come along.
    #[test]
    fn view_shows_only_the_models_in_windows_of_their_own() {
        let files = ["--view", "/m/a.stl", "/m/parts", "/m/pack.zip", "/m/b.3mf"];
        assert_eq!(started(&files, false), Launch::Viewer(paths(&["/m/a.stl", "/m/b.3mf"])));
        // Nothing to show in a model window: the main window opens there, as it always did.
        assert_eq!(started(&["--view", "/m/parts"], false), Launch::Gallery(Some("/m/parts".into())));
        assert_eq!(started(&["--view"], true), Launch::Gallery(None));
    }

    /// Without a flag the settings decide, and only for models: a folder opens the main window.
    #[test]
    fn the_settings_say_what_a_model_opens_in() {
        assert_eq!(started(&["/m/a.stl"], false), Launch::Gallery(Some("/m/a.stl".into())));
        assert_eq!(started(&["/m/a.stl", "/m/b.3mf"], true), Launch::Viewer(paths(&["/m/a.stl", "/m/b.3mf"])));
        assert_eq!(started(&["/m/parts"], true), Launch::Gallery(Some("/m/parts".into())));
        assert_eq!(started(&[], true), Launch::Gallery(None));
        assert_eq!(started(&[""], false), Launch::Gallery(None));
    }

    /// The settings count where Meshgrove is the only launcher; with "Meshgrove Viewer" beside
    /// it (.deb, .rpm, install.sh), this launcher is the main window's.
    #[test]
    fn the_settings_do_not_count_beside_the_viewer_launcher() {
        use update::InstallKind;
        for one in [InstallKind::Dev, InstallKind::AppImage("/a".into()), InstallKind::WindowsPortable("/d".into())] {
            assert!(settings_open_the_viewer(OpenFiles::Viewer, &one), "{one:?}");
            assert!(!settings_open_the_viewer(OpenFiles::Gallery, &one), "{one:?}");
        }
        for two in [InstallKind::Deb("debian13".into()), InstallKind::Rpm("fc44".into()), InstallKind::Source] {
            assert!(!settings_open_the_viewer(OpenFiles::Viewer, &two), "{two:?}");
        }
    }

    /// "Show in Meshgrove" from a model window must reach the main window, also when the
    /// settings send models to the viewer.
    #[test]
    fn gallery_always_shows_the_main_window() {
        assert_eq!(started(&["--gallery", "/m/a.stl"], true), Launch::Gallery(Some("/m/a.stl".into())));
        assert_eq!(started(&["--gallery"], true), Launch::Gallery(None));
    }
}
