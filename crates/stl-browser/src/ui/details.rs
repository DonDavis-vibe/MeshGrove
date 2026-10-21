//! Right panel: 3D view of the selected model (or a large preview) and its facts.

use std::sync::Arc;

use egui::{Grid, Key, RichText, Ui};
use stlb_core::Kind;
use stlb_core::listing::Entry;
use stlb_render::Renderer;

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

#[derive(Default)]
pub struct DetailsOutput {
    pub clicked_plate: Option<usize>,
    pub add_tag: Option<String>,
    pub remove_tag: Option<String>,
}

pub struct DetailsContext<'a> {
    pub entry: Option<&'a Entry>,
    /// Number and total size of the selected files (the multi-selection, or just `entry`).
    pub selection: (usize, u64),
    pub tags: TagEditor<'a>,
    pub thumb: Option<&'a Thumb>,
    pub model: &'a ModelStatus,
    /// Plate previews of a 3MF project (plate number, texture).
    pub plates: &'a [(usize, egui::TextureHandle)],
    pub viewer: &'a mut ModelViewer,
    pub render_state: &'a egui_wgpu::RenderState,
    pub renderer: &'a Renderer,
}

pub fn show(ui: &mut Ui, cx: DetailsContext<'_>) -> DetailsOutput {
    let mut out = DetailsOutput::default();
    let Some(entry) = cx.entry else {
        ui.centered_and_justified(|ui| ui.weak(t!("details.none_selected")));
        return out;
    };
    let (count, total_size) = cx.selection;
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.add_space(4.0);
        if count > 1 {
            let text = t!("details.multi", count = i18n::count(count), size = i18n::file_size(total_size));
            ui.label(RichText::new(text).strong().color(ui.visuals().selection.stroke.color));
        }
        ui.label(RichText::new(stlb_core::listing::file_name(&entry.path)).heading().strong());
        ui.add_space(4.0);

        let side = ui.available_width();
        if meshes::viewable(entry.kind) {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side.min(420.0)), egui::Sense::hover());
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            match cx.model {
                ModelStatus::Loading => {
                    child.centered_and_justified(|ui| ui.spinner());
                }
                ModelStatus::Failed(e) => {
                    child.centered_and_justified(|ui| ui.colored_label(ui.visuals().error_fg_color, e));
                }
                _ => cx.viewer.ui(&mut child, cx.render_state, cx.renderer),
            }
            ui.weak(t!("viewer.hint"));
        } else if entry.kind == Kind::Image
            && let Some(Thumb::Ready(texture, _)) = cx.thumb
        {
            ui.add(egui::Image::new(texture).max_width(side).corner_radius(4.0));
        }

        ui.add_space(8.0);
        facts(ui, entry, cx.thumb, cx.model);
        if !cx.plates.is_empty() {
            let active = match cx.model {
                ModelStatus::Ready(info) => info.plate,
                _ => None,
            };
            out.clicked_plate = plates(ui, cx.plates, active);
        }
        tag_editor(ui, cx.tags, count, &mut out);
        ui.add_space(6.0);
        ui.add(egui::Label::new(RichText::new(entry.path.display().to_string()).small().weak()).wrap());
    });
    out
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

fn facts(ui: &mut Ui, entry: &Entry, thumb: Option<&Thumb>, model: &ModelStatus) {
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
fn plates(ui: &mut Ui, plates: &[(usize, egui::TextureHandle)], active: Option<usize>) -> Option<usize> {
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
        Kind::Archive => t!("kind.archive"),
        Kind::Image => t!("kind.image"),
        Kind::Pdf => t!("kind.pdf"),
        Kind::Other => t!("kind.other"),
    }
    .into_owned()
}
