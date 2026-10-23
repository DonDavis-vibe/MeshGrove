//! The tile grid. Only rows on screen are laid out (`show_rows`), and the tiles on screen are
//! reported to the thumbnail service so they are generated first. Folder tiles can show a
//! mosaic of the models inside them.

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use egui::text::{LayoutJob, TextWrapping};
use egui::{Align, Color32, FontId, Key, Rect, ScrollArea, Sense, TextureOptions, Ui, pos2, vec2};
use meshgrove_core::Kind;
use meshgrove_core::cache::{ThumbCache, ThumbMeta};
use meshgrove_core::config::SortKey;
use meshgrove_core::listing::{self, Entry};
use meshgrove_core::lists::{ListKind, Marks};
use meshgrove_core::printer::{self, Bed, Fit};

use super::icons;
use super::tags::{self, TagStyle};
use super::turntable::HoveredTile;
use crate::thumbs::{self, Job, ThumbResult, ThumbService};

const SPACING: f32 = 8.0;
const PAD: f32 = 6.0;
const LABEL_FONT: f32 = 12.5;

const TEXTURE_OPTIONS: TextureOptions = TextureOptions {
    magnification: egui::TextureFilter::Linear,
    minification: egui::TextureFilter::Linear,
    wrap_mode: egui::TextureWrapMode::ClampToEdge,
    mipmap_mode: Some(egui::TextureFilter::Linear),
};

pub enum Thumb {
    Pending,
    Ready(egui::TextureHandle, ThumbMeta),
    Failed(String),
}

pub enum Action {
    /// Double-click / Enter on an entry (index into the shown entries): folders and archives are
    /// entered, models get a window of their own, other files open in their default app.
    Open(usize),
    /// Open a model with the system's default app (usually a slicer) instead.
    OpenWithDefaultApp(usize),
    /// Go to the folder that contains a search or "include subfolders" result.
    ShowInFolder(usize),
    /// The following act on the selection.
    /// Large preview of the selected model.
    Preview,
    OpenInSlicer,
    OpenInEditor,
    ShowInFileManager,
    CopyPaths,
    /// Copy the files themselves (Ctrl+C), for pasting in Dolphin or the slicer.
    CopyFiles,
    EditTags,
    /// Favourite on, or off when every selected file is one already.
    ToggleFavorite,
    AddToList(i64),
    /// A new list with the selection on it.
    NewList(ListKind, String),
    RemoveFromList(i64),
    Trash,
}

/// What the context menu can offer for the current selection.
#[derive(Default)]
pub struct MenuContext {
    pub slicer: Option<String>,
    /// The mesh editor, if it opens a file of the selection.
    pub editor: Option<String>,
    /// Why the selection cannot go to the trash (read-only drive, inside an archive).
    pub trash_blocked: Option<String>,
    pub can_tag: bool,
    /// Lists files can be put on: (id, kind, name).
    pub lists: Vec<(i64, ListKind, String)>,
    /// Every selected file is a favourite.
    pub favorite: bool,
    /// The hand-made list shown now (id, name), for "remove from …".
    pub current_list: Option<(i64, String)>,
}

pub struct GalleryOutput {
    pub action: Option<Action>,
    /// Model tile under the pointer, for the hover turntable.
    pub hovered: Option<HoveredTile>,
    /// The visible part of the grid (for clipping overlays).
    pub viewport: Rect,
}

/// How the shown entries are filtered and ordered.
#[derive(Clone, PartialEq)]
pub struct Arrange {
    pub filter: String,
    pub only_models: bool,
    pub sort: SortKey,
    pub descending: bool,
}

#[derive(Default)]
pub struct Gallery {
    keys: Vec<Option<String>>,
    /// Folder entry index → thumbnail keys of the files previewed in its mosaic.
    mosaics: HashMap<usize, Vec<String>>,
    thumbs: HashMap<String, Thumb>,
    shown: Vec<usize>,
    /// The focused entry (details panel, keyboard).
    pub selected: Option<usize>,
    /// Multi-selection (Ctrl/Shift-click, Ctrl+A); contains `selected` when not empty.
    marked: BTreeSet<usize>,
    anchor: Option<usize>,
    /// Files with duplicates in the library → size of their duplicate group.
    duplicates: HashMap<PathBuf, usize>,
    visible: HashSet<String>,
    scroll_to_selected: bool,
    last_offset: f32,
    last_viewport_h: f32,
    /// Tile width as drawn: eases towards the laid-out width when it changes.
    shown_tile: f32,
    /// The tile that stays put on screen while the size animates, and its height from the
    /// viewport's top.
    zoom_anchor: Option<(usize, f32)>,
    /// Geometry of the previous frame (columns, tile width and height, scroll area), to place the anchor.
    last_layout: Option<(usize, f32, f32, Rect)>,
    /// Show the containing folder under each name (search results, flattened views).
    pub show_folders: bool,
    /// A tile was being dragged last frame: the wheel then zooms its preview, not the grid.
    tile_dragged: bool,
    /// Printer bed that model tiles are checked against.
    pub bed: Option<Bed>,
    /// Favourite, print and tag marks of library files.
    marks: HashMap<PathBuf, Marks>,
    pub tag_style: TagStyle,
    /// Tag pills on the tiles (a setting).
    pub tags_on_tiles: bool,
    /// Tags a tile shows at most.
    pub tile_tag_count: usize,
    /// Colour dots instead of names on the tiles.
    pub tag_dots: bool,
    /// Folder tiles with more than four pictures fade through them.
    pub cycle_previews: bool,
    /// Soonest moment (seconds from now) a cycling folder tile needs a redraw.
    next_cycle: std::cell::Cell<Option<f32>>,
}

impl Gallery {
    pub fn with_bed(bed: Option<Bed>) -> Gallery {
        Gallery { bed, ..Default::default() }
    }

    /// New contents. Thumbnails already loaded for files that are still shown are kept,
    /// so refreshing a folder does not flicker; everything else is queued.
    pub fn set_entries(&mut self, entries: &[Entry], mosaics: HashMap<usize, Vec<Entry>>, service: &ThumbService) {
        let key = |e: &Entry| ThumbCache::key(&e.path, e.size, e.modified, None);
        self.keys = entries.iter().map(|e| thumbs::has_thumbnail(e.kind).then(|| key(e))).collect();
        let mut jobs: Vec<Job> = entries
            .iter()
            .zip(&self.keys)
            .filter_map(|(e, k)| Some(Job { key: k.clone()?, path: e.path.clone(), kind: e.kind, background: false }))
            .collect();
        self.mosaics.clear();
        for (index, files) in mosaics {
            let keys = files.iter().map(key).collect();
            jobs.extend(files.into_iter().map(|f| Job { key: key(&f), path: f.path, kind: f.kind, background: false }));
            self.mosaics.insert(index, keys);
        }
        let mut old = std::mem::take(&mut self.thumbs);
        for job in &jobs {
            let thumb = old.remove(&job.key).unwrap_or(Thumb::Pending);
            self.thumbs.insert(job.key.clone(), thumb);
        }
        // Only what is not on screen yet needs the pipeline (cache hits are cheap, but not free).
        jobs.retain(|j| matches!(self.thumbs.get(&j.key), Some(Thumb::Pending)));
        service.set_jobs(jobs);
        self.visible.clear();
    }

    /// Forgets the selection and scroll position (a different folder or result set).
    pub fn reset_view(&mut self) {
        self.selected = None;
        self.marked.clear();
        self.anchor = None;
        self.last_offset = 0.0;
        self.scroll_to_selected = false;
    }

    pub fn apply_filter(&mut self, entries: &[Entry], arrange: &Arrange) {
        self.shown = arranged(entries, arrange);
        if self.selected.is_some_and(|s| !self.shown.contains(&s)) {
            self.selected = None;
        }
        let shown: HashSet<usize> = self.shown.iter().copied().collect();
        self.marked.retain(|i| shown.contains(i));
    }

    /// Eases the drawn tile width towards `target` (so Ctrl+wheel and the slider glide).
    fn animate_tile(&mut self, ctx: &egui::Context, target: f32) -> f32 {
        if self.shown_tile <= 0.0 {
            self.shown_tile = target;
        }
        let diff = target - self.shown_tile;
        if diff.abs() < 0.3 {
            self.shown_tile = target;
        } else {
            let dt = ctx.input(|i| i.unstable_dt).min(0.05);
            self.shown_tile += diff * (1.0 - (-dt * 7.0).exp());
            ctx.request_repaint();
        }
        self.shown_tile
    }

    /// While the size animates: the scroll offset that keeps one tile where it was on screen,
    /// so zooming does not lose orientation. That tile is the one closest to the pointer (or the
    /// one in the middle of the view when the pointer is elsewhere, as when dragging the slider).
    fn zoom_scroll(&mut self, ui: &Ui, cols: usize, tile_h: f32, animating: bool) -> Option<f32> {
        let (old_cols, old_w, old_h, rect) = self.last_layout?;
        if !animating {
            self.zoom_anchor = None;
            return None;
        }
        if self.shown.is_empty() {
            return None;
        }
        let top_of = |pos: usize, cols: usize, h: f32| (pos / cols) as f32 * (h + SPACING);
        if self.zoom_anchor.is_none() {
            let pointer = ui.input(|i| i.pointer.hover_pos()).filter(|p| rect.contains(*p));
            let p = pointer.unwrap_or(rect.center());
            let row = ((p.y - rect.top() + self.last_offset) / (old_h + SPACING)).floor().max(0.0) as usize;
            let col = (((p.x - rect.left()) / (old_w + SPACING)).floor().max(0.0) as usize).min(old_cols - 1);
            let pos = (row * old_cols + col).min(self.shown.len() - 1);
            self.zoom_anchor = Some((pos, top_of(pos, old_cols, old_h) + old_h / 2.0 - self.last_offset));
        }
        let (pos, y) = self.zoom_anchor?;
        Some((top_of(pos, cols, tile_h) + tile_h / 2.0 - y).max(0.0))
    }

    pub fn shown_count(&self) -> usize {
        self.shown.len()
    }

    pub fn select(&mut self, entry: Option<usize>) {
        self.selected = entry;
        self.marked = entry.into_iter().collect();
        self.anchor = entry;
        self.scroll_to_selected = entry.is_some();
    }

    /// The entries actions apply to: the multi-selection, or the focused entry.
    pub fn selection(&self) -> Vec<usize> {
        if self.marked.is_empty() { self.selected.into_iter().collect() } else { self.marked.iter().copied().collect() }
    }

    pub fn set_marks(&mut self, marks: HashMap<PathBuf, Marks>) {
        self.marks = marks;
    }

    pub fn marks_of(&self, path: &Path) -> Option<&Marks> {
        self.marks.get(path)
    }

    pub fn set_duplicates(&mut self, duplicates: HashMap<PathBuf, usize>) {
        self.duplicates = duplicates;
    }

    fn is_marked(&self, entry: usize) -> bool {
        self.marked.contains(&entry) || self.selected == Some(entry)
    }

    fn click(&mut self, entry: usize, modifiers: egui::Modifiers) {
        if modifiers.shift
            && let Some(anchor) = self.anchor
            && let (Some(a), Some(b)) =
                (self.shown.iter().position(|&i| i == anchor), self.shown.iter().position(|&i| i == entry))
        {
            self.marked = self.shown[a.min(b)..=a.max(b)].iter().copied().collect();
            self.selected = Some(entry);
        } else if modifiers.command {
            if self.marked.is_empty() {
                self.marked.extend(self.selected);
            }
            if !self.marked.insert(entry) {
                self.marked.remove(&entry);
            }
            self.selected = Some(entry);
            self.anchor = Some(entry);
        } else {
            self.select(Some(entry));
            self.scroll_to_selected = false;
        }
    }

    pub fn thumb(&self, entry: usize) -> Option<&Thumb> {
        self.keys.get(entry)?.as_ref().and_then(|k| self.thumbs.get(k))
    }

    /// Takes one finished thumbnail from the service (foreground results carry a picture).
    pub fn accept(&mut self, ctx: &egui::Context, result: &ThumbResult) {
        match result {
            ThumbResult::Ready { job, image: Some(image), meta } => {
                if let Some(slot) = self.thumbs.get_mut(&job.key) {
                    *slot =
                        Thumb::Ready(ctx.load_texture(job.key.as_str(), image.clone(), TEXTURE_OPTIONS), meta.clone());
                }
            }
            ThumbResult::Ready { image: None, .. } => {}
            ThumbResult::Failed { job, error } => {
                if let Some(slot) = self.thumbs.get_mut(&job.key) {
                    *slot = Thumb::Failed(error.clone());
                }
            }
        }
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        entries: &[Entry],
        tile: f32,
        service: &ThumbService,
        menu: &MenuContext,
    ) -> GalleryOutput {
        let rows_of_text = if self.show_folders { 3.0 } else { 2.0 };
        let label_h = rows_of_text * ui.ctx().fonts_mut(|f| f.row_height(&FontId::proportional(LABEL_FONT))) + 4.0;
        let avail = ui.available_width();
        // Where the tiles end up: whole columns, stretched a little so every row ends flush on the right.
        let target_cols = (((avail + SPACING) / (tile + SPACING)).floor() as usize).max(1);
        let target_w = ((avail + SPACING) / target_cols as f32 - SPACING).max(tile.min(avail));
        // The drawn width glides there; in between the columns follow the width, so the tiles
        // grow continuously (the right edge is ragged until they arrive).
        let tile_w = self.animate_tile(ui.ctx(), target_w);
        let settled = tile_w == target_w;
        let cols =
            if settled { target_cols } else { (((avail + SPACING) / (tile_w + SPACING)).floor() as usize).max(1) };
        let tile_h = tile_w + label_h + PAD;
        let rows = self.shown.len().div_ceil(cols);
        let zoom_offset = self.zoom_scroll(ui, cols, tile_h, !settled);

        let mut action = self.keyboard(ui, cols);

        // (Mouse drags do not scroll by default, so they stay free for turning previews.)
        let wheel_scrolls = !self.tile_dragged;
        let mut area = ScrollArea::vertical()
            .auto_shrink(false)
            .scroll_source(egui::scroll_area::ScrollSource { mouse_wheel: wheel_scrolls, ..Default::default() });
        if std::mem::take(&mut self.scroll_to_selected)
            && let Some(pos) = self.selected.and_then(|s| self.shown.iter().position(|&i| i == s))
        {
            let row_top = (pos / cols) as f32 * (tile_h + SPACING);
            let row_bottom = row_top + tile_h;
            if row_top < self.last_offset {
                area = area.vertical_scroll_offset(row_top);
            } else if row_bottom > self.last_offset + self.last_viewport_h {
                area = area.vertical_scroll_offset(row_bottom - self.last_viewport_h);
            }
        }

        if let Some(offset) = zoom_offset {
            area = area.vertical_scroll_offset(offset);
        }

        ui.spacing_mut().item_spacing = vec2(SPACING, SPACING);
        let mut visible = HashSet::new();
        let mut hovered = None;
        let mut tile_dragged = false;
        let output = area.show_rows(ui, tile_h, rows, |ui, row_range| {
            for row in row_range {
                let row_entries: Vec<usize> = self.shown.iter().skip(row * cols).take(cols).copied().collect();
                ui.horizontal(|ui| {
                    for entry in row_entries {
                        visible.extend(self.keys.get(entry).cloned().flatten());
                        visible.extend(self.mosaics.get(&entry).into_iter().flatten().cloned());
                        let response = self.tile(ui, &entries[entry], entry, tile_w, tile_h);
                        if response.clicked() {
                            let modifiers = ui.input(|i| i.modifiers);
                            self.click(entry, modifiers);
                        }
                        if response.secondary_clicked() && !self.is_marked(entry) {
                            self.select(Some(entry));
                            self.scroll_to_selected = false;
                        }
                        if response.double_clicked() {
                            action = Some(Action::Open(entry));
                        }
                        let show_folders = self.show_folders;
                        let sliceable = entries[entry].kind.is_sliceable();
                        let model = entries[entry].kind.is_model();
                        let viewable = crate::meshes::viewable(entries[entry].kind);
                        response.context_menu(|ui| {
                            let mut item = |ui: &mut Ui, text: std::borrow::Cow<str>, act: Action| {
                                if ui.button(text).clicked() {
                                    action = Some(act);
                                    ui.close();
                                }
                            };
                            item(ui, t!("menu.open"), Action::Open(entry));
                            if viewable {
                                item(ui, t!("menu.preview"), Action::Preview);
                                item(ui, t!("menu.open_with_default_app"), Action::OpenWithDefaultApp(entry));
                            }
                            if sliceable && let Some(slicer) = &menu.slicer {
                                item(ui, t!("menu.open_in_slicer", name = slicer), Action::OpenInSlicer);
                            }
                            if model && let Some(editor) = &menu.editor {
                                item(ui, t!("menu.open_in_editor", name = editor), Action::OpenInEditor);
                            }
                            if show_folders {
                                item(ui, t!("menu.show_in_folder"), Action::ShowInFolder(entry));
                            }
                            item(ui, t!("menu.show_in_file_manager"), Action::ShowInFileManager);
                            item(ui, t!("menu.copy_files"), Action::CopyFiles);
                            item(ui, t!("menu.copy_path"), Action::CopyPaths);
                            if menu.can_tag {
                                item(ui, t!("menu.edit_tags"), Action::EditTags);
                                let star = if menu.favorite { t!("menu.unfavorite") } else { t!("menu.favorite") };
                                item(ui, star, Action::ToggleFavorite);
                                ui.menu_button(t!("menu.add_to_list"), |ui| {
                                    for (id, kind, name) in &menu.lists {
                                        let icon = if *kind == ListKind::Print { "🖨" } else { "📁" };
                                        item(ui, format!("{icon}  {name}").into(), Action::AddToList(*id));
                                    }
                                    if !menu.lists.is_empty() {
                                        ui.separator();
                                    }
                                    let mut name =
                                        ui.data_mut(|d| d.get_temp::<String>(new_list_id())).unwrap_or_default();
                                    ui.add(egui::TextEdit::singleline(&mut name).hint_text(t!("lists.name_hint")));
                                    let typed = name.trim().to_owned();
                                    ui.data_mut(|d| d.insert_temp(new_list_id(), name));
                                    if !typed.is_empty() {
                                        item(
                                            ui,
                                            t!("lists.new_print_with"),
                                            Action::NewList(ListKind::Print, typed.clone()),
                                        );
                                        item(
                                            ui,
                                            t!("lists.new_collection_with"),
                                            Action::NewList(ListKind::Collection, typed),
                                        );
                                    }
                                });
                                if let Some((id, name)) = &menu.current_list {
                                    item(ui, t!("menu.remove_from_list", list = name), Action::RemoveFromList(*id));
                                }
                            }
                            ui.separator();
                            let trash = ui
                                .add_enabled(menu.trash_blocked.is_none(), egui::Button::new(t!("menu.trash")))
                                .on_disabled_hover_text(menu.trash_blocked.clone().unwrap_or_default());
                            if trash.clicked() {
                                action = Some(Action::Trash);
                                ui.close();
                            }
                        });
                        if matches!(action, Some(Action::NewList(..))) {
                            ui.data_mut(|d| d.remove::<String>(new_list_id()));
                        }
                        let e = &entries[entry];
                        // Held button counts too: press-and-scroll without moving should zoom as well.
                        let held = response.dragged() || response.is_pointer_button_down_on();
                        tile_dragged |= held;
                        if (response.hovered() || held) && crate::meshes::viewable(e.kind) {
                            let selected = self.is_marked(entry);
                            let drag = if response.dragged_by(egui::PointerButton::Primary) {
                                response.drag_delta()
                            } else {
                                egui::Vec2::ZERO
                            };
                            hovered = Some(HoveredTile {
                                path: e.path.clone(),
                                kind: e.kind,
                                image_rect: image_rect(response.rect),
                                // Opaque, or the still thumbnail would show through the
                                // selection's translucent tint.
                                background: over(
                                    tile_background(ui.visuals(), selected, true),
                                    ui.visuals().panel_fill,
                                ),
                                drag,
                                dragging: held,
                            });
                        }
                    }
                });
            }
        });
        if let Some(wait) = self.next_cycle.take() {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f32(wait.max(0.016)));
        }
        self.tile_dragged = tile_dragged;
        self.last_offset = output.state.clamped_offset().y;
        self.last_viewport_h = output.inner_rect.height();
        self.last_layout = Some((cols, tile_w, tile_h, output.inner_rect));

        if visible != self.visible {
            service.set_visible(|k| visible.contains(k));
            self.visible = visible;
        }
        GalleryOutput { action, hovered, viewport: output.inner_rect }
    }

    fn keyboard(&mut self, ui: &Ui, cols: usize) -> Option<Action> {
        if ui.ctx().egui_wants_keyboard_input() || self.shown.is_empty() {
            return None;
        }
        let pos = self.selected.and_then(|s| self.shown.iter().position(|&i| i == s));
        let last = self.shown.len() - 1;
        let target = ui.input(|i| {
            let p = pos?;
            if i.key_pressed(Key::ArrowRight) {
                Some((p + 1).min(last))
            } else if i.key_pressed(Key::ArrowLeft) {
                Some(p.saturating_sub(1))
            } else if i.key_pressed(Key::ArrowDown) {
                Some((p + cols).min(last))
            } else if i.key_pressed(Key::ArrowUp) {
                Some(p.saturating_sub(cols))
            } else if i.key_pressed(Key::Home) {
                Some(0)
            } else if i.key_pressed(Key::End) {
                Some(last)
            } else {
                None
            }
        });
        let any_arrow = ui.input(|i| {
            [Key::ArrowRight, Key::ArrowLeft, Key::ArrowDown, Key::ArrowUp, Key::Home, Key::End]
                .iter()
                .any(|k| i.key_pressed(*k))
        });
        if let Some(t) = target.or_else(|| (any_arrow && pos.is_none()).then_some(0)) {
            self.select(Some(self.shown[t]));
        }
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::A)) {
            self.marked = self.shown.iter().copied().collect();
        }
        // Ctrl+C arrives as a copy event, not as a key press.
        if self.selected.is_some() && ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
            return Some(Action::CopyFiles);
        }
        if ui.input(|i| i.key_pressed(Key::Enter)) {
            return self.selected.map(Action::Open);
        }
        None
    }

    /// The four cells of a folder tile. With more pictures than cells (and cycling on) every
    /// cell, staggered, fades into the next picture now and then.
    fn cycled(&self, ctx: &egui::Context, index: usize, ready: &[icons::Preview]) -> Vec<icons::MosaicCell> {
        const PERIOD: f32 = 5.0; // seconds a picture stays
        const FADE: f32 = 1.2; // seconds the fade takes
        const STAGGER: f32 = 0.9; // seconds between neighbouring cells
        if !self.cycle_previews || ready.len() <= 4 {
            return ready.iter().take(4).map(|&p| (p, None)).collect();
        }
        let n = ready.len();
        // Folders do not change in lockstep.
        let t = ctx.input(|i| i.time) as f32 + (index % 7) as f32 * 1.3;
        let mut wake = f32::MAX;
        let cells = (0..4)
            .map(|i| {
                let local = (t - i as f32 * STAGGER) / PERIOD;
                let step = local.floor().max(0.0) as usize;
                let frac = local - local.floor();
                let fade_from = 1.0 - FADE / PERIOD;
                let now = ready[(step * 4 + i) % n];
                if frac < fade_from {
                    wake = wake.min((fade_from - frac) * PERIOD);
                    (now, None)
                } else {
                    wake = wake.min(0.033);
                    let x = ((frac - fade_from) / (1.0 - fade_from)).clamp(0.0, 1.0);
                    let smooth = x * x * (3.0 - 2.0 * x);
                    (now, Some((ready[((step + 1) * 4 + i) % n], smooth)))
                }
            })
            .collect();
        self.next_cycle.set(Some(self.next_cycle.get().map_or(wake, |w| w.min(wake))));
        cells
    }

    fn tile(&self, ui: &mut Ui, entry: &Entry, index: usize, w: f32, h: f32) -> egui::Response {
        // Dragging on a model tile turns the hover preview by hand.
        let (rect, response) = ui.allocate_exact_size(vec2(w, h), Sense::click_and_drag());
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let visuals = ui.visuals();
        let selected = self.is_marked(index);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 6.0, tile_background(visuals, selected, response.hovered()));

        let img = image_rect(rect);
        let ink = visuals.text_color();
        let paper = visuals.widgets.inactive.bg_fill;
        match (entry.kind, self.thumb(index)) {
            (Kind::Folder | Kind::Archive, _) if entry.is_dir() || self.mosaics.contains_key(&index) => {
                let ready: Vec<icons::Preview> = self
                    .mosaics
                    .get(&index)
                    .into_iter()
                    .flatten()
                    .filter_map(|k| match self.thumbs.get(k) {
                        Some(Thumb::Ready(t, _)) => Some((t.id(), t.size_vec2())),
                        _ => None,
                    })
                    .collect();
                let previews = self.cycled(ui.ctx(), index, &ready);
                let color = if entry.is_dir() { icons::FOLDER_BLUE } else { icons::ARCHIVE_AMBER };
                if previews.is_empty() && entry.is_dir() {
                    icons::folder(&painter, img);
                } else if previews.is_empty() {
                    icons::file(&painter, img, &icons::extension_label(&entry.name), paper, ink);
                } else {
                    icons::folder_mosaic(&painter, img, &previews, visuals.extreme_bg_color, color);
                }
            }
            (_, Some(Thumb::Ready(texture, _))) => {
                let fit = fit_inside(texture.size_vec2(), img);
                painter.image(texture.id(), fit, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            (_, Some(Thumb::Pending)) => {
                icons::file(&painter, img, "", paper.linear_multiply(0.5), ink.linear_multiply(0.3));
                egui::Spinner::new()
                    .size((w * 0.16).clamp(12.0, 28.0))
                    .paint_at(ui, Rect::from_center_size(img.center(), vec2(32.0, 32.0)));
            }
            (_, Some(Thumb::Failed(_))) => {
                icons::file(&painter, img, "⚠", paper, visuals.warn_fg_color);
            }
            (_, None) => icons::file(&painter, img, &icons::extension_label(&entry.name), paper, ink),
        }
        if let Some(Thumb::Ready(_, meta)) = self.thumb(index)
            && entry.kind.is_model()
            && !meta.filaments.is_empty()
        {
            // Two slots with the same colour look like one filament on a tile.
            let mut colors = meta.filaments.clone();
            let mut seen = std::collections::HashSet::new();
            colors.retain(|c| seen.insert(*c));
            icons::filament_dots(&painter, img, &colors);
        }
        let duplicate = self.duplicates.get(&entry.path);
        if let Some(n) = duplicate {
            icons::corner_badge(
                &painter,
                img,
                &format!("{n}×"),
                visuals.warn_fg_color.linear_multiply(0.9),
                Color32::BLACK,
            );
        }
        let marks = self.marks.get(&entry.path);
        if self.tags_on_tiles
            && let Some(marks) = marks.filter(|m| !m.tags.is_empty())
        {
            // Top to bottom below the duplicate badge, as many as set and as fit above the
            // badges at the bottom; the last row says "+N" when there are more.
            let top = img.min.y + 4.0 + if duplicate.is_some() { icons::BADGE_HEIGHT + 2.0 } else { 0.0 };
            let row = if self.tag_dots { 13.0 } else { 18.0 };
            let rows = (((img.max.y - 26.0 - top) / row).floor().max(0.0) as usize).min(self.tile_tag_count);
            let (right, max_width) = (img.max.x - 4.0, img.width() * 0.45);
            for (i, tag) in marks.tags.iter().enumerate().take(rows) {
                let y = top + i as f32 * row;
                let more = marks.tags.len() - i;
                if i + 1 == rows && more > 1 {
                    let grey = (Color32::from_black_alpha(170), Color32::from_gray(230));
                    tags::paint_pill(&painter, pos2(right, y), &format!("+{more}"), grey, max_width);
                } else if self.tag_dots {
                    tags::paint_dot(&painter, pos2(right, y), self.tag_style.colors(tag).0);
                } else {
                    tags::paint_pill(&painter, pos2(right, y), tag, self.tag_style.colors(tag), max_width);
                }
            }
        }
        if entry.kind.is_model() || (entry.kind == Kind::Archive && self.mosaics.contains_key(&index)) {
            let label = icons::extension_label(&entry.name);
            icons::badge(&painter, img, &label, Color32::from_black_alpha(150), Color32::from_gray(230));
        }
        let fit = self.bed_fit(entry, index);
        let bed_badge = match fit {
            Some((Fit::Rotated(angle), _)) => {
                let label = format!("↻ {}°", super::details::angle(angle));
                Some(icons::bottom_right_badge(
                    &painter,
                    img,
                    &label,
                    visuals.warn_fg_color.linear_multiply(0.9),
                    Color32::BLACK,
                ))
            }
            Some((Fit::TooBig(_), _)) => {
                let label = t!("bed.badge_too_big");
                Some(icons::bottom_right_badge(&painter, img, &label, visuals.error_fg_color, Color32::WHITE))
            }
            _ => None,
        };
        if marks.is_some_and(|m| m.favorite) {
            // Left of the bed badge, or in the corner.
            let corner = bed_badge.map_or(pos2(img.max.x - 4.0, img.max.y - 4.0), |r| pos2(r.min.x - 3.0, r.max.y));
            icons::badge_left_of(&painter, corner, "★", Color32::from_black_alpha(150), FAVORITE_GOLD);
        }

        let wrap = |rows| TextWrapping {
            max_width: w - 4.0,
            max_rows: rows,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        // Archive members carry their inner path in `name`; show just the file name.
        let shown_name = listing::file_name(&entry.path);
        let mut job = LayoutJob::simple(shown_name.clone(), FontId::proportional(LABEL_FONT), ink, w - 4.0);
        job.wrap = wrap(2);
        job.halign = Align::Center;
        let galley = painter.layout_job(job);
        let name_bottom = img.bottom() + 4.0 + galley.size().y;
        painter.galley(pos2(rect.center().x, img.bottom() + 4.0), galley, ink);
        let folder = entry.path.parent().map(listing::file_name).unwrap_or_default();
        if self.show_folders {
            let weak = visuals.weak_text_color();
            let mut job =
                LayoutJob::simple(format!("📁 {folder}"), FontId::proportional(LABEL_FONT - 1.5), weak, w - 4.0);
            job.wrap = wrap(1);
            job.halign = Align::Center;
            painter.galley(pos2(rect.center().x, name_bottom + 1.0), painter.layout_job(job), weak);
        }

        let mut tooltip = shown_name;
        if let Some(parent) = entry.path.parent() {
            tooltip += &format!("\n{}", parent.display());
        }
        if let Some(n) = self.duplicates.get(&entry.path) {
            tooltip += &format!("\n{}", t!("gallery.duplicate_hint", count = n));
        }
        if let Some(marks) = self.marks.get(&entry.path).filter(|m| !m.tags.is_empty()) {
            tooltip += &format!("\n🏷 {}", marks.tags.join(", "));
        }
        if let Some((fit @ (Fit::Rotated(_) | Fit::TooBig(_)), bed)) = &fit {
            tooltip += &format!("\n{}", super::details::fit_text(*fit, bed));
        }
        if let Some(Thumb::Failed(error)) = self.thumb(index) {
            tooltip += &format!("\n⚠ {error}");
        }
        response.on_hover_text_at_pointer(tooltip)
    }

    /// How a model tile's size (known once its thumbnail facts are in) fits the printer bed.
    fn bed_fit(&self, entry: &Entry, index: usize) -> Option<(Fit, &Bed)> {
        let bed = self.bed.as_ref()?;
        if !entry.kind.is_model() {
            return None;
        }
        match self.thumb(index)? {
            Thumb::Ready(_, meta) => Some((printer::fit(meta.size_mm?, bed), bed)),
            _ => None,
        }
    }
}

fn tile_background(visuals: &egui::Visuals, selected: bool, hovered: bool) -> Color32 {
    if selected {
        visuals.selection.bg_fill.linear_multiply(0.55)
    } else if hovered {
        visuals.widgets.hovered.weak_bg_fill
    } else {
        Color32::TRANSPARENT
    }
}

/// The favourite star on tiles.
const FAVORITE_GOLD: Color32 = Color32::from_rgb(245, 197, 66);

/// Memory slot of the name typed for a new list in the context menu.
fn new_list_id() -> egui::Id {
    egui::Id::unique("gallery-new-list-name")
}

/// `top` painted over `bottom`, fully opaque (egui colours are premultiplied).
fn over(top: Color32, bottom: Color32) -> Color32 {
    let keep = 255 - u16::from(top.a());
    let mix = |t: u8, b: u8| (u16::from(t) + u16::from(b) * keep / 255).min(255) as u8;
    Color32::from_rgb(mix(top.r(), bottom.r()), mix(top.g(), bottom.g()), mix(top.b(), bottom.b()))
}

/// The square thumbnail area at the top of a tile.
fn image_rect(tile: Rect) -> Rect {
    let side = tile.width() - 2.0 * PAD;
    Rect::from_min_size(tile.min + vec2(PAD, PAD), vec2(side, side))
}

/// Indices of the entries to show, in display order: matching the filter text (case-insensitive)
/// and the model-only switch, folders first, then sorted. Folders stay visible with "only models"
/// so you can still navigate.
fn arranged(entries: &[Entry], a: &Arrange) -> Vec<usize> {
    let needle = a.filter.trim().to_lowercase();
    let mut shown: Vec<usize> = (0..entries.len())
        .filter(|&i| {
            let e = &entries[i];
            (!a.only_models || e.is_dir() || e.kind.is_model())
                && (needle.is_empty() || e.name.to_lowercase().contains(&needle))
        })
        .collect();
    shown.sort_by(|&x, &y| {
        let (ex, ey) = (&entries[x], &entries[y]);
        let by_name = || listing::natural_cmp(&ex.name, &ey.name);
        let order = match a.sort {
            SortKey::Name => by_name(),
            SortKey::Modified => ex.modified.cmp(&ey.modified).then_with(by_name),
            SortKey::Size => ex.size.cmp(&ey.size).then_with(by_name),
            SortKey::Type => icons::extension_label(&ex.name).cmp(&icons::extension_label(&ey.name)).then_with(by_name),
        };
        let order = if a.descending { order.reverse() } else { order };
        // Folders stay on top whatever the direction.
        ey.is_dir().cmp(&ex.is_dir()).then(order).then(Ordering::Equal)
    });
    shown
}

/// Largest rect with the image's aspect ratio that fits centred in `area`.
fn fit_inside(size: egui::Vec2, area: Rect) -> Rect {
    let scale = (area.width() / size.x).min(area.height() / size.y);
    Rect::from_center_size(area.center(), size * scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn entry(name: &str, kind: Kind, size: u64, age: u64) -> Entry {
        Entry {
            name: name.into(),
            path: name.into(),
            kind,
            size,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1000 - age)),
        }
    }

    fn arrange(filter: &str, only_models: bool, sort: SortKey, descending: bool) -> Arrange {
        Arrange { filter: filter.into(), only_models, sort, descending }
    }

    fn entries() -> [Entry; 5] {
        [
            entry("dragon_photo.jpg", Kind::Image, 50, 1),
            entry("Dragon.stl", Kind::Stl, 900, 30),
            entry("bases", Kind::Folder, 0, 99),
            entry("plate.3mf", Kind::ThreeMf, 10, 5),
            entry("arms", Kind::Folder, 0, 1),
        ]
    }

    #[test]
    fn filter_by_text_and_model_switch() {
        let e = entries();
        assert_eq!(arranged(&e, &arrange("", false, SortKey::Name, false)), [4, 2, 1, 0, 3]);
        assert_eq!(arranged(&e, &arrange("DRAGON", false, SortKey::Name, false)), [1, 0]);
        assert_eq!(arranged(&e, &arrange("", true, SortKey::Name, false)), [4, 2, 1, 3]);
        assert_eq!(arranged(&e, &arrange(" dragon ", true, SortKey::Name, false)), [1]);
    }

    #[test]
    fn sorting_keeps_folders_first() {
        let e = entries();
        assert_eq!(arranged(&e, &arrange("", false, SortKey::Size, false)), [4, 2, 3, 0, 1]);
        assert_eq!(arranged(&e, &arrange("", false, SortKey::Size, true)), [2, 4, 1, 0, 3]);
        assert_eq!(arranged(&e, &arrange("", false, SortKey::Modified, true)), [4, 2, 0, 3, 1], "newest first");
        assert_eq!(arranged(&e, &arrange("", false, SortKey::Type, false)), [4, 2, 3, 0, 1]);
    }

    #[test]
    fn fit_keeps_aspect_ratio() {
        let r = fit_inside(egui::vec2(400.0, 200.0), Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(100.0, 100.0)));
        assert_eq!(r, Rect::from_min_max(pos2(0.0, 25.0), pos2(100.0, 75.0)));
    }

    #[test]
    fn selection_cover_is_opaque() {
        let panel = Color32::from_gray(27);
        let tint = Color32::from_rgb(0, 92, 128).linear_multiply(0.55);
        let cover = over(tint, panel);
        assert_eq!(cover.a(), 255, "the still thumbnail must not show through");
        assert_eq!(over(Color32::TRANSPARENT, panel), panel);
        assert_eq!(over(Color32::from_rgb(1, 2, 3), panel), Color32::from_rgb(1, 2, 3));
        assert!(cover.b() > panel.b() && cover.r() < panel.r() + 1);
    }
}
