//! A model in a window of its own: double-click (or Enter) on a model opens it next to the main
//! window, freely sizable, with the 3D viewer, its facts and "open in" the slicer and the mesh
//! editor. Several can be
//! open at once; each has its own camera. A start with model files alone (`viewer_app`) shows
//! the first of them in the program's own window, without the main window behind it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use egui::{Key, RichText, Ui, ViewportBuilder, ViewportCommand, ViewportId};
use meshgrove_core::config::PreviewQuality;
use meshgrove_core::listing::{self, Entry};
use meshgrove_core::lists::PrintResult;
use meshgrove_core::printer::Printer;
use meshgrove_render::Renderer;

use super::details::{self, ModelStatus};
use super::viewer::{MeshSource, ModelViewer, SpaceInput};
use crate::meshes::{MeshState, MeshStore};

/// What the windows ask the app to do.
pub enum WindowAction {
    OpenInSlicer(PathBuf, usize),
    OpenInEditor(PathBuf, usize),
    /// Log a print of the window's model.
    Print(PathBuf, PrintResult),
    /// Result of a viewer tool (picture saved, …) for the status bar.
    Message(String),
    /// Show the model in the main window, which is not running (`standalone`).
    ShowInGallery(PathBuf),
}

/// How long a notice stands in place of the mouse hints.
const NOTICE: Duration = Duration::from_secs(6);

/// Everything the windows share with the main window.
pub struct WindowContext<'a> {
    pub meshes: &'a mut MeshStore,
    pub render_state: &'a egui_wgpu::RenderState,
    pub renderer: &'a Renderer,
    pub printers: &'a [Printer],
    /// What the 3D mouse did this frame; the window that has the focus follows it.
    pub space: Option<&'a SpaceInput>,
    /// Free orbit in the viewer.
    pub free_orbit: bool,
    pub slicers: super::SlicerList<'a>,
    pub editors: super::EditorList<'a>,
    pub quality: PreviewQuality,
    /// The library index is open (prints are logged there).
    pub can_log_prints: bool,
    pub icon: Arc<egui::IconData>,
    /// Meshgrove draws the title bar itself (the settings' choice), with its icon `brand`.
    pub own_title_bar: bool,
    pub brand: egui::TextureId,
    /// No main window is behind these windows: they offer to show their model in it.
    pub standalone: bool,
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
    /// What a tool or a program last said and until when it stands in place of the mouse hints
    /// (a start with model files alone has no status bar).
    notice: Option<(String, Instant)>,
}

impl ModelWindow {
    pub fn new(entry: Entry) -> ModelWindow {
        // A number, not the path: the window stays the same one while it steps through its folder.
        static NEXT: AtomicU64 = AtomicU64::new(0);
        ModelWindow {
            id: ViewportId::from_hash_of(("model-window", NEXT.fetch_add(1, Ordering::Relaxed))),
            entry,
            viewer: ModelViewer::new(),
            plate: None,
            plates: Vec::new(),
            model: ModelStatus::Loading,
            focus: false,
            notice: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.entry.path
    }

    /// Whether the model is loaded (shown, or about to be with this frame).
    pub fn loaded(&self) -> bool {
        matches!(self.model, ModelStatus::Ready(_))
    }

    /// Says this in place of the mouse hints for a while.
    pub fn notify(&mut self, text: String) {
        self.notice = Some((text, Instant::now() + NOTICE));
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
        self.follow(ctx, cx);
        let builder = viewport(&self.name(), cx.icon.clone(), cx.own_title_bar);
        let mut open = true;
        ctx.show_viewport_immediate(self.id, builder, |ui, _| open = self.frame(ui, cx, actions));
        open
    }

    /// Shows the model in the program's own window (`ui`) instead of one beside it: a start
    /// with model files alone. False once the user closed it.
    pub fn show_root(&mut self, ui: &mut Ui, cx: &mut WindowContext, actions: &mut Vec<WindowAction>) -> bool {
        self.follow(&ui.ctx().clone(), cx);
        self.frame(ui, cx, actions)
    }

    /// The window's title for the desktop.
    pub fn title(&self) -> String {
        title(&self.name())
    }

    /// The model's file name (an entry of a flattened view is named with its folders).
    fn name(&self) -> String {
        listing::file_name(&self.entry.path)
    }

    fn follow(&mut self, ctx: &egui::Context, cx: &mut WindowContext) {
        self.update_model(ctx, cx.meshes);
        self.viewer.set_quality(cx.quality);
        self.viewer.set_free_orbit(cx.free_orbit);
    }

    /// What the window holds, in a window of its own or in the program's: Meshgrove's title bar
    /// (if it draws one), the viewer and the facts. False once the user closed the window.
    fn frame(&mut self, ui: &mut Ui, cx: &mut WindowContext, actions: &mut Vec<WindowAction>) -> bool {
        let mut open = true;
        if std::mem::take(&mut self.focus) {
            ui.ctx().send_viewport_cmd(ViewportCommand::Focus);
        }
        if ui.input(|i| i.viewport().close_requested() || i.key_pressed(Key::Escape)) {
            open = false;
        }
        if let Some(input) = cx.space
            && ui.input(|i| i.viewport().focused == Some(true))
        {
            self.viewer.apply_space(input);
        }
        // As a picture viewer does: the arrow keys show the model before and the one after.
        if !ui.ctx().egui_wants_keyboard_input() {
            let pressed = |key| ui.input(|i| i.key_pressed(key));
            let by = isize::from(pressed(Key::ArrowRight)) - isize::from(pressed(Key::ArrowLeft));
            if by != 0 {
                self.step(by);
            }
            if pressed(Key::W) {
                self.viewer.toggle_wireframe();
            }
        }
        let name = self.name();
        if cx.own_title_bar {
            // As the main window's: the model's name, and whose window it is.
            use solidcraft_ui_kit::window_chrome::WindowAction as Button;
            match super::appearance::title_bar(ui, cx.brand, &name, Some("Meshgrove"), |_, _| {}) {
                Some(Button::Close) => open = false,
                Some(button) => button.send(ui.ctx()),
                None => {}
            }
        }
        // Widget ids must not collide with the main window's or other model windows'.
        ui.push_id(self.id, |ui| self.contents(ui, &name, cx, actions));
        if cx.own_title_bar {
            solidcraft_ui_kit::window_chrome::resize_zones(ui);
        }
        open
    }

    /// Shows the model `by` places after this one among the models of its folder (before it for
    /// a negative `by`), in the order the gallery lists them; at either end it stays as it is.
    fn step(&mut self, by: isize) {
        if let Some(entry) = neighbour(&self.entry.path, by) {
            self.entry = entry;
            self.plate = None;
            self.plates.clear();
            self.model = ModelStatus::Loading;
            self.notice = None;
        }
    }

    fn contents(&mut self, ui: &mut Ui, name: &str, cx: &mut WindowContext, actions: &mut Vec<WindowAction>) {
        egui::Panel::right("model-facts").default_size(320.0).min_size(240.0).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                ui.add_space(6.0);
                ui.label(RichText::new(name).heading().strong());
                ui.add_space(10.0);
                details::facts(ui, &self.entry, None, &self.model, cx.printers, (None, false));
                let (slicer, editor, _) = details::open_buttons(ui, self.entry.kind, cx.slicers, cx.editors, &[]);
                if let Some(index) = slicer {
                    actions.push(WindowAction::OpenInSlicer(self.entry.path.clone(), index));
                }
                if let Some(index) = editor {
                    actions.push(WindowAction::OpenInEditor(self.entry.path.clone(), index));
                }
                if cx.standalone {
                    let show = super::settings_kit::bar_button(ui, &t!("viewer.show_in_gallery"));
                    if show.on_hover_text(t!("viewer.show_in_gallery_hint")).clicked() {
                        actions.push(WindowAction::ShowInGallery(self.entry.path.clone()));
                    }
                }
                if cx.can_log_prints {
                    ui.horizontal(|ui| {
                        if super::settings_kit::bar_button(ui, &t!("print.mark_printed")).clicked() {
                            actions.push(WindowAction::Print(self.entry.path.clone(), PrintResult::Printed));
                        }
                        if super::settings_kit::bar_button(ui, &t!("print.mark_failed")).clicked() {
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
                ui.add_space(6.0);
                ui.add(egui::Label::new(RichText::new(t!("viewer.step_hint")).small().weak()).wrap());
            });
        });
        egui::Panel::bottom("model-hint").show(ui, |ui| {
            ui.add_space(2.0);
            let now = Instant::now();
            match self.notice.as_ref().filter(|(_, until)| now < *until) {
                Some((text, until)) => {
                    ui.ctx().request_repaint_after(*until - now);
                    ui.add(egui::Label::new(text.as_str()).truncate());
                }
                None => {
                    ui.weak(t!("viewer.hint"));
                }
            }
            ui.add_space(2.0);
        });
        egui::CentralPanel::default().show(ui, |ui| match &self.model {
            ModelStatus::Loading | ModelStatus::Idle => {
                ui.centered_and_justified(|ui| crate::ui::loading::mark_sized(ui, 44.0));
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
                self.viewer.set_mesh(Some((loaded.mesh, MeshSource::File { path, kind, plate: self.plate })));
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

/// The model `by` places after `path` among the models of its folder, as the gallery lists them
/// (natural name order), if there is one.
fn neighbour(path: &Path, by: isize) -> Option<Entry> {
    let models = listing::list_dir(path.parent()?).ok()?.into_iter().filter(|e| crate::meshes::viewable(e.kind));
    let models: Vec<Entry> = models.collect();
    let at = models.iter().position(|e| e.path == path)?;
    models.into_iter().nth(at.checked_add_signed(by)?)
}

/// A model window's frame, for one beside the main window and for the program's own: the
/// app's icon and kind of title bar, sized for a model and its facts.
pub fn viewport(name: &str, icon: Arc<egui::IconData>, own_title_bar: bool) -> ViewportBuilder {
    ViewportBuilder::default()
        .with_title(title(name))
        // Matches the .desktop file name so KDE shows the right icon on Wayland.
        .with_app_id("meshgrove")
        .with_icon(icon)
        .with_decorations(!own_title_bar)
        .with_inner_size([1040.0, 720.0])
        .with_min_inner_size([480.0, 360.0])
}

/// The model's file name, and whose window it is.
fn title(name: &str) -> String {
    format!("{name} – Meshgrove")
}

/// The app icon for model windows, decoded once (the builder is passed every frame).
pub fn icon(decode: impl FnOnce() -> egui::IconData) -> Arc<egui::IconData> {
    static ICON: OnceLock<Arc<egui::IconData>> = OnceLock::new();
    ICON.get_or_init(|| Arc::new(decode())).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The arrow keys go through the folder's models as the gallery lists them: natural order,
    /// models only, and no further than the first and the last.
    #[test]
    fn stepping_follows_the_folders_models_in_natural_order() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["part10.stl", "part2.stl", "notes.txt", "plate.3mf", ".hidden.stl", "print.gcode"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        std::fs::create_dir(dir.path().join("more.stl")).unwrap();
        let at = |name: &str| dir.path().join(name);
        let next = |name: &str, by| neighbour(&at(name), by).map(|e| e.name);
        assert_eq!(next("part2.stl", 1).as_deref(), Some("part10.stl"));
        assert_eq!(next("part10.stl", 1).as_deref(), Some("plate.3mf"));
        assert_eq!(next("plate.3mf", -1).as_deref(), Some("part10.stl"));
        assert_eq!(next("part2.stl", -1), None);
        assert_eq!(next("plate.3mf", 1), None);
        // A file the folder does not list (hidden, or gone meanwhile) has no place in the row.
        assert_eq!(next(".hidden.stl", 1), None);
        assert_eq!(next("gone.stl", 1), None);

        let mut window = ModelWindow::new(Entry::from_path(&at("part2.stl")).unwrap());
        window.notify("picture saved".into());
        window.step(1);
        assert_eq!((window.path(), window.title().as_str()), (at("part10.stl").as_path(), "part10.stl – Meshgrove"));
        assert!(window.notice.is_none() && matches!(window.model, ModelStatus::Loading));
        window.step(-5);
        assert_eq!(window.path(), at("part10.stl"));
    }
}
