//! Right panel: 3D view of the selected model (or a large preview) and its facts.

use std::sync::Arc;

use egui::{Grid, Key, RichText, Ui};
use meshgrove_core::Kind;
use meshgrove_core::listing::Entry;
use meshgrove_core::printer::{self, Bed, Fit};
use meshgrove_render::Renderer;

use super::gallery::Thumb;
use super::viewer::ModelViewer;
use crate::i18n;
use crate::meshes::{self, ModelInfo};

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
    pub thumb: Option<&'a Thumb>,
    pub model: &'a ModelStatus,
    /// Plate previews of a 3MF project (plate number, texture).
    pub plates: &'a [(usize, egui::TextureHandle)],
    pub viewer: &'a mut ModelViewer,
    pub render_state: &'a egui_wgpu::RenderState,
    pub renderer: &'a Renderer,
}

/// The "Preview" pane: file name and the 3D view (or a large picture), filling the pane.
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
    ui.add(egui::Label::new(RichText::new(meshgrove_core::listing::file_name(&entry.path)).heading().strong()).truncate());
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
        out.open_in_slicer = slicer_button(ui, entry.kind, cx.slicer);
    } else if entry.kind == Kind::Image
        && let Some(Thumb::Ready(texture, _)) = cx.thumb
    {
        ui.add(egui::Image::new(texture).fit_to_exact_size(ui.available_size()).corner_radius(4.0));
    }
    out
}

/// The "Info" pane: facts, plates, tags, note and path of the selection.
pub fn info(ui: &mut Ui, cx: DetailsContext<'_>) -> DetailsOutput {
    let mut out = DetailsOutput::default();
    let Some(entry) = cx.entry else {
        ui.centered_and_justified(|ui| ui.weak(t!("details.none_selected")));
        return out;
    };
    let (count, _) = cx.selection;
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.add_space(4.0);
        facts(ui, entry, cx.thumb, cx.model, cx.bed);
        if !cx.plates.is_empty() {
            let active = match cx.model {
                ModelStatus::Ready(info) => info.plate,
                _ => None,
            };
            out.clicked_plate = plates(ui, cx.plates, active);
        }
        tag_editor(ui, cx.tags, count, &mut out);
        note_editor(ui, cx.note, &mut out);
        ui.add_space(6.0);
        ui.add(egui::Label::new(RichText::new(entry.path.display().to_string()).small().weak()).wrap());
    });
    out
}

/// "Open in Anycubic Slicer Next" for models a slicer opens; true when clicked.
pub fn slicer_button(ui: &mut Ui, kind: Kind, slicer: Option<&str>) -> bool {
    let Some(name) = slicer.filter(|_| kind.is_sliceable()) else { return false };
    ui.add_space(6.0);
    ui.add(egui::Button::new(t!("menu.open_in_slicer", name = name)).min_size(egui::vec2(0.0, 28.0))).clicked()
}

fn tag_editor(ui: &mut Ui, ed: TagEditor<'_>, count: usize, out: &mut DetailsOutput) {
    ui.add_space(10.0);
    ui.label(RichText::new(t!("tags.title")).weak());
    if !ed.enabled {
        ui.weak(t!("tags.only_library"));
        return;
    }
    let chip_fill = ui.visuals().selection.bg_fill.linear_multiply(0.45);
    ui.horizontal_wrapped(|ui| {
        for (tag, n) in ed.on_selection {
            let label = if count > 1 && *n < count { format!("{tag} ({n}/{count})") } else { tag.clone() };
            let chip =
                egui::Button::new(RichText::new(format!("{label}  ✕")).small()).corner_radius(10.0).fill(chip_fill);
            if ui.add(chip).on_hover_text(t!("tags.remove_hint")).clicked() {
                out.remove_tag = Some(tag.clone());
            }
        }
    });
    let field =
        ui.add(egui::TextEdit::singleline(ed.input).hint_text(t!("tags.add_hint")).desired_width(f32::INFINITY));
    if ed.focus {
        field.request_focus();
    }
    if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !ed.input.trim().is_empty() {
        out.add_tag = Some(std::mem::take(ed.input).trim().to_owned());
        field.request_focus();
    }
    // Existing tags not yet on every selected file, matching what is typed.
    let needle = ed.input.trim().to_lowercase();
    let on_all = |tag: &str| ed.on_selection.iter().any(|(t, n)| t == tag && *n >= count);
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
                if ui.small_button(format!("+ {tag}")).clicked() {
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
                .map(|(axis, mm)| format!("{axis} +{} mm", i18n::decimal(mm)))
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
        if let Some(size) = size {
            fact(ui, &t!("details.dimensions"), &i18n::dimensions(size));
            if let Some(bed) = bed {
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
        // The viewer's plate knows the materials; the thumbnail only the colours of the first plate.
        let filaments: Vec<(Option<u32>, [u8; 3], Option<&str>)> = match info {
            Some(info) if !info.filaments.is_empty() => {
                info.filaments.iter().map(|f| (Some(f.slot), f.color, f.material.as_deref())).collect()
            }
            _ => meta.map(|m| m.filaments.iter().map(|&c| (None, c, None)).collect()).unwrap_or_default(),
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
        Kind::Other => t!("kind.other"),
    }
    .into_owned()
}
