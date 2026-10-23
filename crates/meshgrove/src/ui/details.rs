//! Right panel: 3D view of the selected model (or a large preview) and its facts.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Grid, Key, RichText, Ui};
use meshgrove_core::Kind;
use meshgrove_core::config::Program;
use meshgrove_core::listing::Entry;
use meshgrove_core::lists::{ListInfo, ListKind, PrintResult, PrintStatus};
use meshgrove_core::loaders::gcode::GcodeFacts;
use meshgrove_core::printer::{self, Bed, Fit};
use meshgrove_render::Renderer;

use super::gallery::Thumb;
use super::tags::{self, Group, TagStyle};
use super::viewer::ModelViewer;
use crate::i18n;
use crate::meshes::{self, ModelInfo};
use crate::text_preview::{TextPreview, TextState};

pub enum ModelStatus {
    Idle,
    Loading,
    Ready(Arc<ModelInfo>),
    Failed(String),
}

/// Tags of the selection and the library, for the tag editor.
pub struct TagEditor<'a> {
    /// The selection lies in the library (tags live in its index).
    pub enabled: bool,
    pub on_selection: &'a [(String, usize)],
    pub all: &'a [(String, usize)],
    pub input: &'a mut String,
    pub focus: bool,
}

/// The selection's note (shared by all selected files, or `mixed`).
pub struct NoteEditor<'a> {
    pub enabled: bool,
    pub text: &'a mut String,
    pub mixed: bool,
}

#[derive(Default)]
pub struct DetailsOutput {
    pub clicked_plate: Option<usize>,
    pub add_tag: Option<String>,
    pub remove_tag: Option<String>,
    /// The note text was edited this frame.
    pub note_changed: bool,
    /// Result of a viewer tool (picture saved, …) for the status bar.
    pub message: Option<String>,
    /// "Open in slicer" was clicked.
    pub open_in_slicer: bool,
    /// "Open in" the mesh editor was clicked.
    pub open_in_editor: bool,
    /// The star was clicked.
    pub toggle_favorite: bool,
    /// A list chip was clicked.
    pub open_list: Option<i64>,
    /// "Printed" or "failed" was clicked.
    pub print: Option<PrintResult>,
    pub undo_print: bool,
    /// New number of copies on the shown print list.
    pub quantity: Option<u32>,
    /// A file to open in its default app (a folder's readme).
    pub open_file: Option<PathBuf>,
}

/// The selection's print log and, on a shown print list, its copies.
pub struct PrintInfo<'a> {
    /// The selection is in the library.
    pub enabled: bool,
    pub status: &'a PrintStatus,
    /// On a shown print list: its name and the copies wanted and done.
    pub on_list: Option<(&'a str, u32, u32)>,
}

/// The lists holding the selection, for the info pane.
pub struct ListsOfSelection<'a> {
    /// Lists with how many of the selected files each holds.
    pub lists: &'a [(ListInfo, usize)],
    /// Every selected file is a favourite.
    pub favorite: bool,
}

pub struct DetailsContext<'a> {
    pub entry: Option<&'a Entry>,
    /// Number and total size of the selected files (the multi-selection, or just `entry`).
    pub selection: (usize, u64),
    pub tags: TagEditor<'a>,
    pub note: NoteEditor<'a>,
    pub bed: Option<&'a Bed>,
    /// Name of the slicer models can be handed to.
    pub slicer: Option<&'a str>,
    pub editor: Option<&'a Program>,
    pub lists: ListsOfSelection<'a>,
    pub print: PrintInfo<'a>,
    pub tag_style: &'a TagStyle,
    pub thumb: Option<&'a Thumb>,
    pub model: &'a ModelStatus,
    /// Plate previews of a 3MF project (plate number, texture).
    pub plates: &'a [(usize, egui::TextureHandle)],
    pub viewer: &'a mut ModelViewer,
    pub text: &'a mut TextPreview,
    pub render_state: &'a egui_wgpu::RenderState,
    pub renderer: &'a Renderer,
}

/// The "Preview" pane: file name and the 3D view (or a large picture: photos, G-code previews),
/// filling the pane.
pub fn preview(ui: &mut Ui, cx: DetailsContext<'_>) -> DetailsOutput {
    let mut out = DetailsOutput::default();
    let Some(entry) = cx.entry else {
        ui.centered_and_justified(|ui| ui.weak(t!("details.none_selected")));
        return out;
    };
    let (count, total_size) = cx.selection;
    ui.add_space(4.0);
    if count > 1 {
        let text = t!("details.multi", count = i18n::count(count), size = i18n::file_size(total_size));
        ui.label(RichText::new(text).strong().color(ui.visuals().selection.stroke.color));
    }
    ui.add(
        egui::Label::new(RichText::new(meshgrove_core::listing::file_name(&entry.path)).heading().strong()).truncate(),
    );
    ui.add_space(4.0);

    if meshes::viewable(entry.kind) {
        let hint = ui.text_style_height(&egui::TextStyle::Small) + 6.0;
        let mut size = ui.available_size();
        size.y = (size.y - hint).max(80.0);
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        match cx.model {
            ModelStatus::Loading => {
                child.centered_and_justified(|ui| ui.spinner());
            }
            ModelStatus::Failed(e) => {
                child.centered_and_justified(|ui| ui.colored_label(ui.visuals().error_fg_color, e));
            }
            _ => out.message = cx.viewer.ui(&mut child, cx.render_state, cx.renderer),
        }
        ui.add(egui::Label::new(RichText::new(t!("viewer.hint")).weak()).truncate());
    } else if matches!(entry.kind, Kind::Image | Kind::Gcode)
        && let Some(Thumb::Ready(texture, _)) = cx.thumb
    {
        ui.add(egui::Image::new(texture).fit_to_exact_size(ui.available_size()).corner_radius(4.0));
    } else if entry.kind == Kind::Text {
        text_view(ui, cx.text, None);
    } else if entry.kind == Kind::Folder {
        text_view(ui, cx.text, Some(&mut out));
    }
    out
}

/// A readme or note: Markdown formatted (links open in the browser), other text as it is.
/// For a folder's readme (`folder` gets the click) its name and an "Open" button come first.
fn text_view(ui: &mut Ui, preview: &mut TextPreview, folder: Option<&mut DetailsOutput>) {
    let text = match &preview.state {
        TextState::Ready(text) => text,
        TextState::Loading => {
            ui.centered_and_justified(|ui| ui.spinner());
            return;
        }
        TextState::Failed(e) => {
            ui.colored_label(ui.visuals().error_fg_color, e);
            return;
        }
        TextState::Idle => return,
    };
    if let Some(out) = folder {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("📄 {}", meshgrove_core::listing::file_name(&text.path))).weak());
            if ui.small_button(t!("text.open")).clicked() {
                out.open_file = Some(text.path.clone());
            }
        });
        ui.add_space(2.0);
    }
    egui::Frame::new().fill(ui.visuals().extreme_bg_color).corner_radius(4.0).inner_margin(10.0).show(ui, |ui| {
        egui::ScrollArea::both().id_salt("text_preview").auto_shrink(false).show(ui, |ui| {
            let nfo = text.path.extension().is_some_and(|e| e.eq_ignore_ascii_case("nfo"));
            if text.markdown {
                egui_commonmark::CommonMarkViewer::new().show(ui, &mut preview.cache, &text.body);
            } else if nfo {
                // Scene .nfo files are ASCII art: monospaced and never wrapped.
                ui.add(egui::Label::new(RichText::new(&text.body).monospace()).extend());
            } else {
                ui.add(egui::Label::new(&text.body).wrap());
            }
            if text.truncated {
                ui.add_space(8.0);
                ui.weak(t!("text.truncated"));
            }
        });
    });
}

/// The "Info" pane: facts, plates, tags, note and path of the selection.
pub fn info(ui: &mut Ui, cx: DetailsContext<'_>) -> DetailsOutput {
    let mut out = DetailsOutput::default();
    // Without a selection the preview pane says what to do; once is enough.
    let Some(entry) = cx.entry else { return out };
    let (count, _) = cx.selection;
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.add_space(4.0);
        facts(ui, entry, cx.thumb, cx.model, cx.bed);
        (out.open_in_slicer, out.open_in_editor) = open_buttons(ui, entry.kind, cx.slicer, cx.editor);
        if !cx.plates.is_empty() {
            let active = match cx.model {
                ModelStatus::Ready(info) => info.plate,
                _ => None,
            };
            out.clicked_plate = plates(ui, cx.plates, active);
        }
        lists_row(ui, &cx.lists, cx.tags.enabled, count, &mut out);
        if entry.kind.is_printable() {
            print_row(ui, &cx.print, &mut out);
        }
        tag_editor(ui, cx.tags, cx.tag_style, count, &mut out);
        note_editor(ui, cx.note, &mut out);
        ui.add_space(6.0);
        ui.add(egui::Label::new(RichText::new(entry.path.display().to_string()).small().weak()).wrap());
    });
    out
}

/// "Open in Anycubic Slicer Next" and "Open in FreeCAD" for a model, as far as the slicer and
/// the mesh editor open its kind. Which of them was clicked: (slicer, editor).
pub fn open_buttons(ui: &mut Ui, kind: Kind, slicer: Option<&str>, editor: Option<&Program>) -> (bool, bool) {
    let slicer = slicer.filter(|_| kind.is_sliceable());
    let editor = editor.filter(|e| e.opens(kind));
    let mut clicked = (false, false);
    if slicer.is_none() && editor.is_none() {
        return clicked;
    }
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        let button = |text: std::borrow::Cow<str>| egui::Button::new(text).min_size(egui::vec2(0.0, 28.0));
        if let Some(name) = slicer {
            clicked.0 = ui.add(button(t!("menu.open_in_slicer", name = name))).clicked();
        }
        if let Some(editor) = editor {
            clicked.1 = ui.add(button(t!("menu.open_in_editor", name = editor.name))).clicked();
        }
    });
    clicked
}

/// The star and the lists holding the selection (library files only; the tag section says so).
fn lists_row(ui: &mut Ui, lists: &ListsOfSelection, enabled: bool, count: usize, out: &mut DetailsOutput) {
    if !enabled {
        return;
    }
    ui.add_space(10.0);
    ui.label(RichText::new(t!("lists.title")).weak());
    ui.horizontal_wrapped(|ui| {
        let (star, hint) = if lists.favorite {
            (
                RichText::new(format!("★ {}", t!("lists.favorite"))).color(egui::Color32::from_rgb(245, 197, 66)),
                t!("lists.unfavorite_hint"),
            )
        } else {
            (RichText::new(format!("☆ {}", t!("lists.favorite"))), t!("lists.favorite_hint"))
        };
        if ui.add(egui::Button::new(star.small()).corner_radius(9.0)).on_hover_text(hint).clicked() {
            out.toggle_favorite = true;
        }
        for (list, n) in lists.lists.iter().filter(|(l, _)| l.kind != ListKind::Favorites) {
            let icon = if list.kind == ListKind::Print { "🖨" } else { "📁" };
            let label = if count > 1 && *n < count {
                format!("{icon} {} ({n}/{count})", list.name)
            } else {
                format!("{icon} {}", list.name)
            };
            if ui
                .add(egui::Button::new(RichText::new(label).small()).corner_radius(9.0))
                .on_hover_text(t!("lists.open_hint"))
                .clicked()
            {
                out.open_list = Some(list.id);
            }
        }
    });
}

/// What the print log says, the print buttons and, on a print list, the copies.
fn print_row(ui: &mut Ui, print: &PrintInfo, out: &mut DetailsOutput) {
    if !print.enabled {
        return;
    }
    ui.add_space(10.0);
    ui.label(RichText::new(t!("print.title")).weak());
    let status = print.status;
    let summary = match (status.last, status.last_printed) {
        (Some((PrintResult::Failed, at)), _) => {
            t!("print.last_failed", date = i18n::datetime(at), count = status.printed).into_owned()
        }
        (_, Some(at)) => t!("print.printed_times", count = status.printed, date = i18n::datetime(at)).into_owned(),
        _ => t!("print.never").into_owned(),
    };
    ui.label(summary);
    ui.horizontal_wrapped(|ui| {
        if ui.button(t!("print.mark_printed")).clicked() {
            out.print = Some(PrintResult::Printed);
        }
        if ui.button(t!("print.mark_failed")).clicked() {
            out.print = Some(PrintResult::Failed);
        }
        if status.last.is_some() && ui.button(t!("print.undo")).on_hover_text(t!("print.undo_hint")).clicked() {
            out.undo_print = true;
        }
    });
    if let Some((list, wanted, done)) = print.on_list {
        ui.horizontal(|ui| {
            ui.label(t!("print.on_list", list = list));
            if ui.add_enabled(wanted > 1, egui::Button::new("−").small()).clicked() {
                out.quantity = Some(wanted - 1);
            }
            ui.label(RichText::new(wanted.to_string()).strong());
            if ui.small_button("+").clicked() {
                out.quantity = Some(wanted + 1);
            }
            ui.weak(t!("print.copies_done", done = done));
        });
    }
}

fn tag_editor(ui: &mut Ui, ed: TagEditor<'_>, style: &TagStyle, count: usize, out: &mut DetailsOutput) {
    ui.add_space(10.0);
    ui.label(RichText::new(t!("tags.title")).weak());
    if !ed.enabled {
        ui.weak(t!("tags.only_library"));
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for (tag, n) in ed.on_selection {
            let label =
                if count > 1 && *n < count { format!("{tag} ({n}/{count})  ✕") } else { format!("{tag}  ✕") };
            if tags::chip(ui, style, tag, &label).on_hover_text(t!("tags.remove_hint")).clicked() {
                out.remove_tag = Some(tag.clone());
            }
        }
    });
    let on_all = |tag: &str| ed.on_selection.iter().any(|(t, n)| t.eq_ignore_ascii_case(tag) && *n >= count);
    let mut field = None;
    ui.horizontal(|ui| {
        // Suggested tags by group, the most used own tags first.
        ui.menu_button("▾", |ui| {
            let own: Vec<&String> = ed
                .all
                .iter()
                .filter(|(t, _)| tags::group_of(t) == Group::Own && !on_all(t))
                .take(10)
                .map(|(t, _)| t)
                .collect();
            if !own.is_empty() {
                ui.label(RichText::new(Group::Own.title()).weak());
                for tag in own {
                    if ui.button(tag).clicked() {
                        out.add_tag = Some(tag.clone());
                        ui.close();
                    }
                }
                ui.separator();
            }
            for group in Group::SUGGESTED {
                ui.menu_button(group.title(), |ui| {
                    for tag in tags::suggested(group).into_iter().filter(|t| !on_all(t)) {
                        let response = ui.horizontal(|ui| {
                            tags::dot(ui, style, tag);
                            ui.button(tag)
                        });
                        if response.inner.clicked() {
                            out.add_tag = Some(tag.to_owned());
                            ui.close();
                        }
                    }
                });
            }
        })
        .response
        .on_hover_text(t!("tags.suggestions_hint"));
        field = Some(
            ui.add(egui::TextEdit::singleline(ed.input).hint_text(t!("tags.add_hint")).desired_width(f32::INFINITY)),
        );
    });
    let Some(field) = field else { return };
    if ed.focus {
        field.request_focus();
    }
    if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !ed.input.trim().is_empty() {
        out.add_tag = Some(std::mem::take(ed.input).trim().to_owned());
        field.request_focus();
    }
    // Existing tags not yet on every selected file, matching what is typed.
    let needle = ed.input.trim().to_lowercase();
    let suggestions: Vec<&String> = ed
        .all
        .iter()
        .filter(|(tag, _)| !on_all(tag) && (needle.is_empty() || tag.to_lowercase().contains(&needle)))
        .map(|(tag, _)| tag)
        .take(8)
        .collect();
    if !suggestions.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for tag in suggestions {
                if tags::chip(ui, style, tag, &format!("+ {tag}")).clicked() {
                    out.add_tag = Some(tag.clone());
                    ed.input.clear();
                }
            }
        });
    }
}

fn note_editor(ui: &mut Ui, ed: NoteEditor<'_>, out: &mut DetailsOutput) {
    if !ed.enabled {
        return; // the tag section already explains that this needs the library
    }
    ui.add_space(10.0);
    ui.label(RichText::new(t!("notes.title")).weak());
    let hint = if ed.mixed { t!("notes.mixed_hint") } else { t!("notes.hint") };
    let field = egui::TextEdit::multiline(ed.text).hint_text(hint).desired_rows(3).desired_width(f32::INFINITY);
    out.note_changed = ui.add(field).changed();
}

/// "45" or "22,5" — rotation angles for the bed check.
pub fn angle(degrees: f32) -> String {
    if degrees.fract() == 0.0 { format!("{degrees:.0}") } else { i18n::decimal(degrees) }
}

/// "passt auf Anycubic Kobra S1", "passt nur gedreht (45°) …", "zu groß für …: X +12,0 mm".
pub fn fit_text(fit: Fit, bed: &Bed) -> String {
    match fit {
        Fit::Fits => t!("bed.fits", bed = bed.name),
        Fit::Rotated(a) => t!("bed.rotated", bed = bed.name, angle = angle(a)),
        Fit::TooBig(over) => {
            let parts: Vec<String> = ["X", "Y", "Z"]
                .iter()
                .zip(over)
                .filter(|(_, mm)| *mm > 0.0)
                .map(|(axis, mm)| format!("{axis} +{}", i18n::length(mm)))
                .collect();
            let over = if parts.is_empty() { t!("bed.diagonal").into_owned() } else { parts.join(", ") };
            t!("bed.too_big", bed = bed.name, over = over)
        }
    }
    .into_owned()
}

pub fn facts(ui: &mut Ui, entry: &Entry, thumb: Option<&Thumb>, model: &ModelStatus, bed: Option<&Bed>) {
    Grid::new("details_facts").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        fact(ui, &t!("details.type"), &kind_name(entry.kind));
        if !entry.is_dir() {
            fact(ui, &t!("details.file_size"), &i18n::file_size(entry.size));
        }
        if let Some(modified) = entry.modified {
            fact(ui, &t!("details.modified"), &i18n::datetime(modified));
        }
        let meta = match thumb {
            Some(Thumb::Ready(_, meta)) => Some(meta),
            _ => None,
        };
        let info = match model {
            ModelStatus::Ready(info) => Some(info.as_ref()),
            _ => None,
        };
        let size = info.map(|i| i.size_mm).or_else(|| meta.and_then(|m| m.size_mm).map(glam::Vec3::from));
        let triangles = info.map(|i| i.triangles).or_else(|| meta.and_then(|m| m.triangles));
        let gcode = meta.and_then(|m| m.gcode.as_ref());
        if let Some(size) = size {
            fact(ui, &t!("details.dimensions"), &i18n::dimensions(size));
            // G-code is placed already; whether it suits the printer is told by its printer row.
            if let Some(bed) = bed.filter(|_| gcode.is_none()) {
                let fit = printer::fit(size.to_array(), bed);
                ui.label(RichText::new(t!("bed.label")).weak());
                let text = fit_text(fit, bed);
                match fit {
                    Fit::Fits => ui.add(egui::Label::new(format!("✔ {text}")).wrap()),
                    Fit::Rotated(_) => ui.colored_label(ui.visuals().warn_fg_color, format!("↻ {text}")),
                    Fit::TooBig(_) => ui.colored_label(ui.visuals().error_fg_color, format!("✕ {text}")),
                };
                ui.end_row();
            }
        }
        if let Some(triangles) = triangles {
            fact(ui, &t!("details.triangles"), &i18n::count(triangles));
        }
        if let Some(info) = info
            && let Some(plate) = info.plate
            && info.plate_count > 1
        {
            fact(ui, &t!("details.shown"), &t!("details.plate_of", n = plate, total = info.plate_count));
        }
        if let Some(info) = info {
            for (label, value) in [
                ("details.title", &info.title),
                ("details.designer", &info.designer),
                ("details.application", &info.application),
            ] {
                if let Some(value) = value {
                    fact(ui, &t!(label), value);
                }
            }
        }
        if let Some(gcode) = gcode {
            gcode_facts(ui, gcode, meta.is_some_and(|m| m.filaments.is_empty()), bed);
        }
        // The viewer's plate knows the materials; the thumbnail only the colours of the first
        // plate, G-code the materials of its extruders as well.
        let material = |i: usize| gcode.and_then(|g| g.materials.get(i)).map(String::as_str);
        let filaments: Vec<(Option<u32>, [u8; 3], Option<&str>)> = match info {
            Some(info) if !info.filaments.is_empty() => {
                info.filaments.iter().map(|f| (Some(f.slot), f.color, f.material.as_deref())).collect()
            }
            _ => meta
                .map(|m| m.filaments.iter().enumerate().map(|(i, &c)| (None, c, material(i))).collect())
                .unwrap_or_default(),
        };
        if !filaments.is_empty() {
            ui.label(RichText::new(t!("details.filaments")).weak());
            filament_swatches(ui, &filaments);
            ui.end_row();
        }
        if meta.is_some_and(|m| m.embedded) {
            fact(ui, &t!("details.preview"), &t!("details.preview_embedded"));
        }
        if let Some(Thumb::Failed(error)) = thumb {
            ui.label(RichText::new(t!("details.problem")).weak());
            ui.colored_label(ui.visuals().warn_fg_color, error);
            ui.end_row();
        }
    });
}

/// What the slicer wrote into a G-code file; warns when it was sliced for another printer.
/// `materials` adds the materials to the filament row (when no colour swatches show them).
fn gcode_facts(ui: &mut Ui, g: &GcodeFacts, materials: bool, bed: Option<&Bed>) {
    if let Some(slicer) = &g.slicer {
        fact(ui, &t!("gcode.slicer"), slicer);
    }
    if let Some(printer) = &g.printer {
        ui.label(RichText::new(t!("gcode.printer")).weak());
        match bed.filter(|bed| !printer::same_printer(printer, bed)) {
            Some(bed) => {
                ui.colored_label(ui.visuals().warn_fg_color, format!("⚠ {printer}")).on_hover_text(t!(
                    "gcode.other_printer",
                    printer = printer,
                    bed = bed.name
                ));
            }
            None => {
                ui.add(egui::Label::new(printer).wrap());
            }
        }
        ui.end_row();
    }
    if let Some(time) = &g.print_time {
        fact(ui, &t!("gcode.print_time"), time);
    }
    let mut used: Vec<String> = Vec::new();
    if materials && !g.materials.is_empty() {
        used.push(g.materials.join(", "));
    }
    if let Some(grams) = g.filament_g {
        used.push(format!("{} g", i18n::decimal(grams)));
    }
    if let Some(metres) = g.filament_m {
        used.push(format!("{} m", i18n::decimal(metres)));
    }
    if !used.is_empty() {
        fact(ui, &t!("gcode.filament_used"), &used.join(" · "));
    }
    let layer = g.layer_height.map(i18n::short_length);
    let layers = match (g.layers, layer) {
        (Some(n), Some(h)) => Some(format!("{} × {h}", i18n::count(n as usize))),
        (Some(n), None) => Some(i18n::count(n as usize)),
        (None, h) => h,
    };
    if let Some(layers) = layers {
        fact(ui, &t!("gcode.layers"), &layers);
    }
    if let Some(nozzle) = g.nozzle {
        fact(ui, &t!("gcode.nozzle"), &i18n::short_length(nozzle));
    }
    // The slicer does not say which currency; the user knows their filament prices.
    if let Some(cost) = g.cost.filter(|c| *c > 0.0) {
        fact(ui, &t!("gcode.cost"), &i18n::money(cost));
    }
}

/// Plate pictures; clicking one shows that plate in the viewer. Returns the clicked plate.
pub fn plates(ui: &mut Ui, plates: &[(usize, egui::TextureHandle)], active: Option<usize>) -> Option<usize> {
    let mut clicked = None;
    ui.add_space(8.0);
    ui.label(RichText::new(t!("details.plates")).weak()).on_hover_text(t!("details.plates_hint"));
    egui::ScrollArea::horizontal().id_salt("plates").show(ui, |ui| {
        ui.horizontal(|ui| {
            for (number, texture) in plates {
                ui.vertical(|ui| {
                    let image = egui::Image::new(texture).fit_to_exact_size(egui::vec2(96.0, 96.0)).corner_radius(4.0);
                    let response = ui.add(image.sense(egui::Sense::click()));
                    if active == Some(*number) {
                        let stroke = egui::Stroke::new(2.0, ui.visuals().selection.stroke.color);
                        ui.painter().rect_stroke(response.rect.expand(2.0), 5.0, stroke, egui::StrokeKind::Outside);
                    }
                    if response.clicked() {
                        clicked = Some(*number);
                    }
                    response.on_hover_ui(|ui| {
                        ui.add(egui::Image::new(texture).fit_to_exact_size(egui::vec2(360.0, 360.0)));
                    });
                    ui.small(t!("details.plate", n = number));
                });
            }
        });
    });
    clicked
}

/// One colour chip per filament (slot number inside, details on hover), then the materials.
fn filament_swatches(ui: &mut Ui, filaments: &[(Option<u32>, [u8; 3], Option<&str>)]) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for &(slot, [r, g, b], material) in filaments {
            let (rect, response) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(rect, 4.0, egui::Color32::from_rgb(r, g, b));
            painter.rect_stroke(rect, 4.0, ui.visuals().widgets.noninteractive.fg_stroke, egui::StrokeKind::Inside);
            if let Some(slot) = slot {
                // Dark digits on light filaments and the other way round.
                let luma = 0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b);
                let ink = if luma > 140.0 { egui::Color32::BLACK } else { egui::Color32::WHITE };
                painter.text(rect.center(), egui::Align2::CENTER_CENTER, slot, egui::FontId::proportional(11.0), ink);
            }
            let hex = format!("#{r:02X}{g:02X}{b:02X}");
            response.on_hover_text(match (slot, material) {
                (Some(n), Some(material)) => t!("details.filament_material", n = n, material = material, color = hex),
                (Some(n), None) => t!("details.filament", n = n, color = hex),
                _ => hex.into(),
            });
        }
        let mut materials: Vec<&str> = filaments.iter().filter_map(|f| f.2).collect();
        materials.dedup();
        materials.sort_unstable();
        materials.dedup();
        if !materials.is_empty() {
            ui.add_space(4.0);
            ui.label(materials.join(", "));
        }
    });
}

fn fact(ui: &mut Ui, label: &str, value: &str) {
    ui.label(RichText::new(label).weak());
    ui.add(egui::Label::new(value).wrap());
    ui.end_row();
}

pub fn kind_name(kind: Kind) -> String {
    match kind {
        Kind::Folder => t!("kind.folder"),
        Kind::Stl => t!("kind.stl"),
        Kind::ThreeMf => t!("kind.3mf"),
        Kind::Step => t!("kind.step"),
        Kind::Obj => t!("kind.obj"),
        Kind::Ply => t!("kind.ply"),
        Kind::Gltf => t!("kind.gltf"),
        Kind::Archive => t!("kind.archive"),
        Kind::Image => t!("kind.image"),
        Kind::Pdf => t!("kind.pdf"),
        Kind::Gcode => t!("kind.gcode"),
        Kind::Text => t!("kind.text"),
        Kind::Other => t!("kind.other"),
    }
    .into_owned()
}
