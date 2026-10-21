//! Right panel: 3D view of the selected model (or a large preview) and its facts.

use egui::{Grid, RichText, Ui};
use stlb_core::Kind;
use stlb_core::listing::Entry;
use stlb_render::Renderer;

use super::gallery::Thumb;
use super::viewer::ModelViewer;
use crate::i18n;

pub enum ModelStatus {
    Idle,
    Loading,
    Ready { triangles: usize, size_mm: glam::Vec3 },
    Failed(String),
}

pub struct DetailsContext<'a> {
    pub entry: Option<&'a Entry>,
    pub thumb: Option<&'a Thumb>,
    pub model: &'a ModelStatus,
    pub viewer: &'a mut ModelViewer,
    pub render_state: &'a egui_wgpu::RenderState,
    pub renderer: &'a Renderer,
}

pub fn show(ui: &mut Ui, cx: DetailsContext<'_>) {
    let Some(entry) = cx.entry else {
        ui.centered_and_justified(|ui| ui.weak(t!("details.none_selected")));
        return;
    };
    ui.add_space(4.0);
    ui.label(RichText::new(&entry.name).heading().strong());
    ui.add_space(4.0);

    let side = ui.available_width();
    match entry.kind {
        Kind::Stl => {
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
        }
        Kind::Image => {
            if let Some(Thumb::Ready(texture, _)) = cx.thumb {
                ui.add(egui::Image::new(texture).max_width(side).corner_radius(4.0));
            }
        }
        _ => {}
    }

    ui.add_space(8.0);
    Grid::new("details_facts").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        fact(ui, &t!("details.type"), &kind_name(entry.kind));
        if !entry.is_dir() {
            fact(ui, &t!("details.file_size"), &i18n::file_size(entry.size));
        }
        if let Some(modified) = entry.modified {
            fact(ui, &t!("details.modified"), &i18n::datetime(modified));
        }
        let meta = match cx.thumb {
            Some(Thumb::Ready(_, meta)) => Some(meta),
            _ => None,
        };
        let (triangles, size) = match cx.model {
            ModelStatus::Ready { triangles, size_mm } => (Some(*triangles), Some(*size_mm)),
            _ => (meta.and_then(|m| m.triangles), meta.and_then(|m| m.size_mm).map(glam::Vec3::from)),
        };
        if let Some(size) = size {
            fact(ui, &t!("details.dimensions"), &i18n::dimensions(size));
        }
        if let Some(triangles) = triangles {
            fact(ui, &t!("details.triangles"), &i18n::count(triangles));
        }
        if let Some(Thumb::Failed(error)) = cx.thumb {
            ui.label(t!("details.problem"));
            ui.colored_label(ui.visuals().warn_fg_color, error);
            ui.end_row();
        }
    });
    ui.add_space(6.0);
    ui.add(egui::Label::new(RichText::new(entry.path.display().to_string()).small().weak()).wrap());
}

fn fact(ui: &mut Ui, label: &str, value: &str) {
    ui.label(RichText::new(label).weak());
    ui.label(value);
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
