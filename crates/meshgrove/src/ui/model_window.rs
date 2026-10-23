//! A model in a window of its own: double-click (or Enter) on a model opens it next to the main
//! window, freely sizable, with the 3D viewer, its facts and "open in" the slicer and the mesh
//! editor. Several can be
//! open at once; each has its own camera.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use egui::{Key, RichText, Ui, ViewportBuilder, ViewportCommand, ViewportId};
use meshgrove_core::config::{PreviewQuality, Program};
use meshgrove_core::listing::{self, Entry};
use meshgrove_core::lists::PrintResult;
use meshgrove_core::printer::Bed;
use meshgrove_render::Renderer;

use super::details::{self, ModelStatus};
use super::viewer::{MeshSource, ModelViewer};
use crate::meshes::{MeshState, MeshStore};

/// What the windows ask the app to do.
pub enum WindowAction {
    OpenInSlicer(PathBuf),
    OpenInEditor(PathBuf),
    /// Log a print of the window's model.
    Print(PathBuf, PrintResult),
    /// Result of a viewer tool (picture saved, …) for the status bar.
    Message(String),
}

/// Everything the windows share with the main window.
pub struct WindowContext<'a> {
    pub meshes: &'a mut MeshStore,
    pub render_state: &'a egui_wgpu::RenderState,
    pub renderer: &'a Renderer,
    pub bed: Option<&'a Bed>,
    pub slicer: Option<&'a str>,
    pub editor: Option<&'a Program>,
    pub quality: PreviewQuality,
    /// The library index is open (prints are logged there).
    pub can_log_prints: bool,
    pub icon: Arc<egui::IconData>,
}

pub struct ModelWindow {
    id: ViewportId,
    entry: Entry,
    viewer: ModelViewer,
    plate: Option<usize>,
    plates: Vec<(usize, egui::TextureHandle)>,
    model: ModelStatus,
    /// Raise the window on the next frame (it was opened again).
    focus: bool,
}

impl ModelWindow {
    pub fn new(entry: Entry) -> ModelWindow {
        ModelWindow {
            id: ViewportId::from_hash_of(("model-window", &entry.path)),
            entry,
            viewer: ModelViewer::new(),
            plate: None,
            plates: Vec::new(),
            model: ModelStatus::Loading,
            focus: false,
        }
    }

    pub fn path(&self) -> &Path {
        &self.entry.path
    }

    /// Test hook: measure between these points of the view (see [`ModelViewer::script_measure`]).
    pub fn script_measure(&mut self, picks: Vec<egui::Vec2>) {
        self.viewer.script_measure(picks);
    }

    /// Test hook: start with a standard view (see [`ModelViewer::script_view`]).
    pub fn script_view(&mut self, name: &str) {
        self.viewer.script_view(name);
    }

    /// Brings the window to the front.
    pub fn raise(&mut self) {
        self.focus = true;
    }

    /// Shows the window for this frame; false once the user closed it.
    pub fn show(&mut self, ctx: &egui::Context, cx: &mut WindowContext, actions: &mut Vec<WindowAction>) -> bool {
        self.update_model(ctx, cx.meshes);
        self.viewer.set_quality(cx.quality);
        let name = listing::file_name(&self.entry.path);
        let builder = ViewportBuilder::default()
            .with_title(format!("{name} – Meshgrove"))
            .with_app_id("meshgrove")
            .with_icon(cx.icon.clone())
            .with_inner_size([1040.0, 720.0])
            .with_min_inner_size([480.0, 360.0]);
        let mut open = true;
        ctx.show_viewport_immediate(self.id, builder, |ui, _| {
            if std::mem::take(&mut self.focus) {
                ui.ctx().send_viewport_cmd(ViewportCommand::Focus);
            }
            if ui.input(|i| i.viewport().close_requested() || i.key_pressed(Key::Escape)) {
                open = false;
            }
            // Widget ids must not collide with the main window's or other model windows'.
            ui.push_id(self.id, |ui| self.contents(ui, &name, cx, actions));
        });
        open
    }

    fn contents(&mut self, ui: &mut Ui, name: &str, cx: &mut WindowContext, actions: &mut Vec<WindowAction>) {
        egui::Panel::right("model-facts").default_size(320.0).min_size(240.0).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                ui.add_space(6.0);
                ui.label(RichText::new(name).heading().strong());
                ui.add_space(10.0);
                details::facts(ui, &self.entry, None, &self.model, cx.bed);
                let (slicer, editor) = details::open_buttons(ui, self.entry.kind, cx.slicer, cx.editor);
                if slicer {
                    actions.push(WindowAction::OpenInSlicer(self.entry.path.clone()));
                }
                if editor {
                    actions.push(WindowAction::OpenInEditor(self.entry.path.clone()));
                }
                if cx.can_log_prints {
                    ui.horizontal(|ui| {
                        if ui.small_button(t!("print.mark_printed")).clicked() {
                            actions.push(WindowAction::Print(self.entry.path.clone(), PrintResult::Printed));
                        }
                        if ui.small_button(t!("print.mark_failed")).clicked() {
                            actions.push(WindowAction::Print(self.entry.path.clone(), PrintResult::Failed));
                        }
                    });
                }
                if !self.plates.is_empty() {
                    let active = match &self.model {
                        ModelStatus::Ready(info) => info.plate,
                        _ => None,
                    };
                    if let Some(plate) = details::plates(ui, &self.plates, active) {
                        self.plate = Some(plate);
                    }
                }
                ui.add_space(6.0);
                let path = RichText::new(self.entry.path.display().to_string()).small().weak();
                ui.add(egui::Label::new(path).wrap());
            });
        });
        egui::Panel::bottom("model-hint").show(ui, |ui| {
            ui.add_space(2.0);
            ui.weak(t!("viewer.hint"));
            ui.add_space(2.0);
        });
        egui::CentralPanel::default().show(ui, |ui| match &self.model {
            ModelStatus::Loading | ModelStatus::Idle => {
                ui.centered_and_justified(|ui| ui.spinner());
            }
            ModelStatus::Failed(e) => {
                ui.centered_and_justified(|ui| ui.colored_label(ui.visuals().error_fg_color, e));
            }
            ModelStatus::Ready(_) => {
                if let Some(message) = self.viewer.ui(ui, cx.render_state, cx.renderer) {
                    actions.push(WindowAction::Message(message));
                }
            }
        });
    }

    /// Follows the mesh store: the model loads (or reloads for another plate) in the background.
    fn update_model(&mut self, ctx: &egui::Context, meshes: &mut MeshStore) {
        let (path, kind) = (self.entry.path.clone(), self.entry.kind);
        match meshes.request(&path, kind, self.plate) {
            MeshState::Loading => {
                self.viewer.set_mesh(None);
                self.model = ModelStatus::Loading;
            }
            MeshState::Failed(e) => {
                self.viewer.set_mesh(None);
                self.model = ModelStatus::Failed(e.clone());
            }
            MeshState::Ready(loaded) => {
                let loaded = loaded.clone();
                self.viewer.set_mesh(Some((loaded.mesh, MeshSource { path, kind, plate: self.plate })));
                if self.plates.is_empty() && !loaded.info.plates.is_empty() {
                    let id = self.id;
                    self.plates = loaded
                        .info
                        .plates
                        .iter()
                        .map(|(n, img)| {
                            let name = format!("{id:?}-plate-{n}");
                            (*n, ctx.load_texture(name, img.clone(), egui::TextureOptions::LINEAR))
                        })
                        .collect();
                }
                self.model = ModelStatus::Ready(loaded.info);
            }
        }
    }
}

/// The app icon for model windows, decoded once (the builder is passed every frame).
pub fn icon(decode: impl FnOnce() -> egui::IconData) -> Arc<egui::IconData> {
    static ICON: OnceLock<Arc<egui::IconData>> = OnceLock::new();
    ICON.get_or_init(|| Arc::new(decode())).clone()
}
