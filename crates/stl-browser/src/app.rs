use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use crossbeam_channel::{Receiver, bounded};
use egui::{Button, Key, KeyboardShortcut, Modifiers, RichText, Ui};
use stlb_core::cache::ThumbCache;
use stlb_core::config::Config;
use stlb_core::listing;
use stlb_core::{Kind, loaders::stl};
use stlb_render::{GpuMesh, Renderer, upload_mesh};

use crate::i18n;
use crate::nav::{Listing, Navigator};
use crate::thumbs::ThumbService;
use crate::ui::details::{self, DetailsContext, ModelStatus};
use crate::ui::gallery::{Action, Gallery};
use crate::ui::sidebar::{Sidebar, SidebarAction};
use crate::ui::viewer::ModelViewer;

/// Wait this long after the selection settles before loading a model into the viewer,
/// so arrowing through a folder of 200 MB miniatures doesn't load every one of them.
const VIEWER_DEBOUNCE: Duration = Duration::from_millis(250);
const TILE_RANGE: std::ops::RangeInclusive<f32> = 96.0..=384.0;

type ModelResult = (PathBuf, Result<(GpuMesh, usize, glam::Vec3), String>);

pub struct App {
    config: Config,
    config_error: Option<String>,
    nav: Navigator,
    sidebar: Sidebar,
    gallery: Gallery,
    thumbs: ThumbService,
    renderer: Renderer,
    viewer: ModelViewer,
    model: ModelStatus,
    model_path: Option<PathBuf>,
    model_rx: Option<Receiver<ModelResult>>,
    selection_changed: Instant,
    last_selected: Option<usize>,
    filter: String,
    focus_filter: bool,
    select_after_load: Option<PathBuf>,
    gpu_name: String,
    rate: RateMeter,
    screenshot: Option<crate::screenshot::ScreenshotHook>,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        config: Config,
        config_error: Option<String>,
        start: Option<PathBuf>,
    ) -> anyhow::Result<App> {
        let rs = cc.wgpu_render_state.as_ref().context("eframe was not started with the wgpu renderer")?;
        // Ctrl +/- and Ctrl+wheel resize tiles instead of zooming the whole UI.
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        use_system_ui_font(&cc.egui_ctx);

        let cache = ThumbCache::new(ThumbCache::default_location().context("no cache directory")?);
        let (start_dir, select) = match start {
            Some(p) if p.is_file() => (p.parent().map(Path::to_path_buf).unwrap_or_default(), Some(p)),
            Some(p) => (p, None),
            None => (
                config
                    .last_folder
                    .clone()
                    .filter(|p| p.is_dir())
                    .or_else(|| config.library_roots.first().cloned())
                    .or_else(dirs::home_dir)
                    .unwrap_or_else(|| PathBuf::from("/")),
                None,
            ),
        };
        Ok(App {
            nav: Navigator::new(cc.egui_ctx.clone(), start_dir),
            sidebar: Sidebar::new(),
            gallery: Gallery::default(),
            thumbs: ThumbService::new(cc.egui_ctx.clone(), cache),
            renderer: Renderer::new(&rs.device, &rs.adapter),
            viewer: ModelViewer::new(),
            model: ModelStatus::Idle,
            model_path: None,
            model_rx: None,
            selection_changed: Instant::now(),
            last_selected: None,
            filter: String::new(),
            focus_filter: false,
            select_after_load: select,
            gpu_name: rs.adapter.get_info().name,
            rate: RateMeter::default(),
            screenshot: crate::screenshot::ScreenshotHook::from_env(),
            config,
            config_error,
        })
    }

    fn save_config(&mut self) {
        self.config.last_folder = Some(self.nav.current().to_path_buf());
        if let Err(e) = self.config.save() {
            log::warn!("could not save settings: {e}");
        }
    }

    fn selected_entry(&self) -> Option<&listing::Entry> {
        self.nav.entries().get(self.gallery.selected?)
    }

    fn open_entry(&mut self, index: usize) {
        let Some(entry) = self.nav.entries().get(index).cloned() else {
            return;
        };
        if entry.is_dir() {
            self.nav.open(entry.path);
        } else if let Err(e) = std::process::Command::new("xdg-open").arg(&entry.path).spawn() {
            log::warn!("xdg-open failed: {e}");
        }
    }

    fn on_listing_arrived(&mut self) {
        let entries = self.nav.entries().to_vec();
        self.gallery.set_entries(&entries, &self.thumbs);
        self.gallery.apply_filter(&entries, &self.filter, self.config.only_models);
        if let Some(target) = self.select_after_load.take() {
            self.gallery.selected = entries.iter().position(|e| e.path == target);
        }
    }

    /// Keeps the 3D viewer in sync with the selection (debounced, one load at a time).
    fn update_viewer(&mut self, ctx: &egui::Context, device: &wgpu::Device) {
        if self.gallery.selected != self.last_selected {
            self.last_selected = self.gallery.selected;
            self.selection_changed = Instant::now();
        }
        if let Some(rx) = &self.model_rx
            && let Ok((path, result)) = rx.try_recv()
        {
            self.model_rx = None;
            if Some(&path) == self.model_path.as_ref() {
                match result {
                    Ok((mesh, triangles, size_mm)) => {
                        self.viewer.set_mesh(Some(mesh));
                        self.model = ModelStatus::Ready { triangles, size_mm };
                    }
                    Err(e) => self.model = ModelStatus::Failed(e),
                }
            }
        }

        let wanted = self.selected_entry().filter(|e| e.kind == Kind::Stl).map(|e| e.path.clone());
        if wanted == self.model_path || self.model_rx.is_some() {
            return;
        }
        let waited = self.selection_changed.elapsed();
        if waited < VIEWER_DEBOUNCE {
            ctx.request_repaint_after(VIEWER_DEBOUNCE - waited);
            return;
        }
        self.viewer.set_mesh(None);
        self.model_path = wanted.clone();
        let Some(path) = wanted else {
            self.model = ModelStatus::Idle;
            return;
        };
        self.model = ModelStatus::Loading;
        let (tx, rx) = bounded(1);
        let (device, ctx) = (device.clone(), ctx.clone());
        std::thread::Builder::new()
            .name("viewer-load".into())
            .spawn(move || {
                let result = std::fs::read(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| stl::parse(&bytes).map_err(|e| e.to_string()))
                    .and_then(|mesh| {
                        let size = mesh.bounds().map(|b| b.size()).unwrap_or_default();
                        let triangles = mesh.triangle_count();
                        upload_mesh(&device, &mesh)
                            .map(|gpu| (gpu, triangles, size))
                            .ok_or_else(|| t!("error.empty_model").into_owned())
                    });
                let _ = tx.send((path, result));
                ctx.request_repaint();
            })
            .expect("spawning viewer loader");
        self.model_rx = Some(rx);
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.egui_wants_keyboard_input();
        let pressed = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        if pressed(Modifiers::ALT, Key::ArrowLeft) {
            self.nav.back();
        }
        if pressed(Modifiers::ALT, Key::ArrowRight) {
            self.nav.forward();
        }
        if pressed(Modifiers::ALT, Key::ArrowUp) || (!typing && pressed(Modifiers::NONE, Key::Backspace)) {
            self.nav.up();
        }
        if pressed(Modifiers::NONE, Key::F5) {
            self.nav.reload();
            self.sidebar.refresh();
        }
        if pressed(Modifiers::COMMAND, Key::F) {
            self.focus_filter = true;
        }
        if pressed(Modifiers::COMMAND, Key::Plus) || pressed(Modifiers::COMMAND, Key::Equals) {
            self.config.tile_size = (self.config.tile_size + 32.0).min(*TILE_RANGE.end());
        }
        if pressed(Modifiers::COMMAND, Key::Minus) {
            self.config.tile_size = (self.config.tile_size - 32.0).max(*TILE_RANGE.start());
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let nav_button = |ui: &mut Ui, icon: &str, enabled: bool, tip: std::borrow::Cow<str>| {
                ui.add_enabled(enabled, Button::new(RichText::new(icon).size(16.0))).on_hover_text(tip).clicked()
            };
            if nav_button(ui, "⬅", self.nav.can_go_back(), t!("nav.back")) {
                self.nav.back();
            }
            if nav_button(ui, "➡", self.nav.can_go_forward(), t!("nav.forward")) {
                self.nav.forward();
            }
            if nav_button(ui, "⬆", self.nav.can_go_up(), t!("nav.up")) {
                self.nav.up();
            }
            if nav_button(ui, "🔄", true, t!("nav.reload")) {
                self.nav.reload();
                self.sidebar.refresh();
            }
            ui.separator();

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::Slider::new(&mut self.config.tile_size, TILE_RANGE).show_value(false))
                    .on_hover_text(t!("gallery.tile_size"));
                ui.label("🔍");
                let mut changed = ui.checkbox(&mut self.config.only_models, t!("gallery.only_models")).changed();
                let filter = ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text(t!("gallery.filter_hint"))
                        .desired_width(180.0),
                );
                if std::mem::take(&mut self.focus_filter) {
                    filter.request_focus();
                }
                if filter.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
                    self.filter.clear();
                    filter.surrender_focus();
                    changed = true;
                }
                if changed || filter.changed() {
                    let entries = self.nav.entries().to_vec();
                    self.gallery.apply_filter(&entries, &self.filter, self.config.only_models);
                }

                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    egui::ScrollArea::horizontal().id_salt("breadcrumb").show(ui, |ui| self.breadcrumb(ui));
                });
            });
        });
    }

    fn breadcrumb(&mut self, ui: &mut Ui) {
        let current = self.nav.current().to_path_buf();
        let parts: Vec<PathBuf> =
            current.ancestors().map(Path::to_path_buf).collect::<Vec<_>>().into_iter().rev().collect();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                ui.weak("›");
            }
            let name = if part.parent().is_none() { "/".to_owned() } else { listing::file_name(part) };
            let last = i + 1 == parts.len();
            if ui
                .add(Button::new(if last { RichText::new(name).strong() } else { RichText::new(name) }).frame(false))
                .clicked()
                && !last
            {
                self.nav.open(part.clone());
            }
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let entries = self.nav.entries();
            let models = entries.iter().filter(|e| e.kind.is_model()).count();
            ui.label(t!(
                "status.items",
                shown = i18n::count(self.gallery.shown_count()),
                total = i18n::count(entries.len())
            ));
            ui.separator();
            ui.label(t!("status.models", count = i18n::count(models)));
            let (done, total) = self.thumbs.progress();
            let rate = self.rate.sample(done);
            if total > 0 && done < total {
                ui.separator();
                ui.add(egui::ProgressBar::new(done as f32 / total as f32).desired_width(120.0).desired_height(10.0));
                ui.label(t!("status.thumbs", done = i18n::count(done), total = i18n::count(total)));
                if rate > 0.0 {
                    ui.weak(t!("status.rate", rate = format!("{rate:.0}")));
                }
            }
            if let Some(err) = &self.config_error {
                ui.separator();
                ui.colored_label(ui.visuals().warn_fg_color, t!("error.config", error = err));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.weak(t!("status.gpu", name = self.gpu_name));
            });
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let rs = frame.wgpu_render_state().expect("wgpu renderer").clone();
        if self.nav.poll() {
            self.on_listing_arrived();
        }
        self.gallery.receive(&ctx, &self.thumbs);
        self.shortcuts(&ctx);
        self.update_viewer(&ctx, &rs.device);
        if let Some(hook) = &mut self.screenshot {
            hook.tick(&ctx);
        }

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(3.0);
            self.toolbar(ui);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));

        let sidebar_action = egui::Panel::left("sidebar")
            .default_size(250.0)
            .max_size(420.0)
            .show(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                self.sidebar.show(ui, &self.config, self.nav.current())
            })
            .inner;
        match sidebar_action {
            Some(SidebarAction::Navigate(path)) => self.nav.open(path),
            Some(SidebarAction::AddRoot(path)) => {
                if !self.config.library_roots.contains(&path) {
                    self.config.library_roots.push(path);
                    self.save_config();
                }
            }
            Some(SidebarAction::RemoveRoot(path)) => {
                self.config.library_roots.retain(|r| r != &path);
                self.save_config();
            }
            None => {}
        }

        egui::Panel::right("details").default_size(340.0).show(ui, |ui| {
            let entry = self.gallery.selected.and_then(|i| self.nav.entries().get(i));
            let thumb = self.gallery.selected.and_then(|i| self.gallery.thumb(i));
            details::show(
                ui,
                DetailsContext {
                    entry,
                    thumb,
                    model: &self.model,
                    viewer: &mut self.viewer,
                    render_state: &rs,
                    renderer: &self.renderer,
                },
            );
        });

        egui::CentralPanel::default().show(ui, |ui| {
            match &self.nav.listing {
                Listing::Loading => {
                    ui.centered_and_justified(|ui| ui.spinner().on_hover_text(t!("gallery.loading")));
                    return;
                }
                Listing::Error(e) => {
                    let message = t!("error.read_dir", error = e);
                    ui.centered_and_justified(|ui| ui.colored_label(ui.visuals().error_fg_color, message));
                    return;
                }
                Listing::Ready(entries) if entries.is_empty() => {
                    ui.centered_and_justified(|ui| ui.weak(t!("gallery.empty")));
                    return;
                }
                Listing::Ready(_) => {}
            }
            let zoom = ui.input(|i| i.zoom_delta());
            if zoom != 1.0 && ui.ui_contains_pointer() {
                self.config.tile_size = (self.config.tile_size * zoom).clamp(*TILE_RANGE.start(), *TILE_RANGE.end());
            }
            if let Some(Action::Open(index)) =
                self.gallery.show(ui, self.nav.entries(), self.config.tile_size, &self.thumbs)
            {
                self.open_entry(index);
            }
        });
    }

    fn on_exit(&mut self) {
        self.save_config();
    }
}

/// Use the desktop's UI font (Noto Sans on KDE) so the app does not look foreign;
/// glyphs it lacks still fall back to egui's bundled fonts and the system fallback.
fn use_system_ui_font(ctx: &egui::Context) {
    let Ok(out) = std::process::Command::new("fc-match").args(["-f", "%{file}", "sans-serif:style=Regular"]).output()
    else {
        return;
    };
    let path = String::from_utf8_lossy(&out.stdout).into_owned();
    let Ok(bytes) = std::fs::read(&path) else { return };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("system-ui".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.insert(0, "system-ui".into());
    }
    ctx.set_fonts(fonts);
    log::info!("UI font: {path}");
}

/// Thumbnails per second over the last few seconds, for the status bar.
#[derive(Default)]
struct RateMeter {
    samples: VecDeque<(Instant, usize)>,
}

impl RateMeter {
    fn sample(&mut self, done: usize) -> f32 {
        let now = Instant::now();
        if self.samples.back().is_some_and(|&(_, d)| done < d) {
            self.samples.clear(); // new folder, counter restarted
        }
        if self.samples.back().is_none_or(|&(t, _)| now - t > Duration::from_millis(250)) {
            self.samples.push_back((now, done));
        }
        while self.samples.len() > 2 && now - self.samples[0].0 > Duration::from_secs(3) {
            self.samples.pop_front();
        }
        match (self.samples.front(), self.samples.back()) {
            (Some(&(t0, d0)), Some(&(t1, d1))) if t1 > t0 => (d1 - d0) as f32 / (t1 - t0).as_secs_f32(),
            _ => 0.0,
        }
    }
}
