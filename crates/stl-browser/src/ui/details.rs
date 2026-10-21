//! Right panel: 3D view of the selected model (or a large preview) and its facts.

use std::sync::Arc;

use egui::{Grid, RichText, Ui};
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

pub struct DetailsContext<'a> {
    pub entry: Option<&'a Entry>,
    pub thumb: Option<&'a Thumb>,
    pub model: &'a ModelStatus,
    /// Plate previews of a 3MF project (plate number, texture).
    pub plates: &'a [(usize, egui::TextureHandle)],
    pub viewer: &'a mut ModelViewer,
    pub render_state: &'a egui_wgpu::RenderState,
    pub renderer: &'a Renderer,
}

/// Returns the build plate the user clicked, if any.
pub fn show(ui: &mut Ui, cx: DetailsContext<'_>) -> Option<usize> {
    let Some(entry) = cx.entry else {
        ui.centered_and_justified(|ui| ui.weak(t!("details.none_selected")));
        return None;
    };
    let mut clicked = None;
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.add_space(4.0);
        ui.label(RichText::new(&entry.name).heading().strong());
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
            clicked = plates(ui, cx.plates, active);
        }
        ui.add_space(6.0);
        ui.add(egui::Label::new(RichText::new(entry.path.display().to_string()).small().weak()).wrap());
    });
    clicked
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
