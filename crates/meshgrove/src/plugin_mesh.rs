//! The triangles of a plugin's 3D view on their way to the screen: prepared on the panel's
//! worker thread (one recentred soup for the GPU and for picking, the tree to pick on, the
//! upload), so a preview of millions of triangles costs the window no more than swapping a
//! pointer; the colours of its overlay; and what painting on it sends back to the plugin.

use std::sync::Arc;

use glam::Vec3;
use meshgrove_core::Aabb;
use meshgrove_core::bvh::Bvh;
use meshgrove_plugin_api::MeshData;
use meshgrove_plugin_api::ui::{Event, MeshHit, MeshPaint, StrokePhase};
use meshgrove_render::{GpuMesh, default_model_color, upload_soup};

use crate::ui::paint::{self, Tool};
use crate::ui::viewer::{PaintMode, SurfaceHit};

/// The device and queue a worker uploads with.
#[derive(Clone)]
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// A plugin's mesh, ready to show.
pub struct PreparedMesh {
    /// Three corners per triangle, in the plugin's order, recentred on `centre`.
    pub soup: Arc<Vec<[f32; 3]>>,
    /// The middle of the mesh in the plugin's coordinates.
    pub centre: Vec3,
    /// In the recentred coordinates.
    pub bounds: Aabb,
    pub tree: Arc<Bvh>,
    /// Uploaded with colour buffers (all the model's grey), when the worker had a GPU.
    pub gpu: Option<Arc<GpuMesh>>,
}

impl PreparedMesh {
    pub fn triangles(&self) -> usize {
        self.soup.len() / 3
    }

    /// Uploads it (when the worker could not): with colour buffers, for overlays and tints.
    pub fn upload(&self, device: &wgpu::Device) -> GpuMesh {
        let plain = vec![default_model_color(); self.soup.len()];
        upload_soup(device, &self.soup, self.bounds, Some(&plain))
    }
}

/// Prepares `data` (checked already); `None` when it has no triangles to show.
pub fn prepare(data: &MeshData, gpu: Option<&Gpu>) -> Option<PreparedMesh> {
    let positions = data.positions.as_chunks::<3>().0;
    if data.triangles() == 0 || data.indices.iter().any(|&i| i as usize >= positions.len()) {
        return None;
    }
    let (lo, hi) = data.bounds()?;
    let (lo, hi) = (Vec3::from(lo), Vec3::from(hi));
    let centre = (lo + hi) * 0.5;
    let soup: Vec<[f32; 3]> =
        data.indices.iter().map(|&i| (Vec3::from(positions[i as usize]) - centre).to_array()).collect();
    let bounds = Aabb { min: lo - centre, max: hi - centre };
    let tree = Arc::new(Bvh::build(&soup));
    let mut prepared = PreparedMesh { soup: Arc::new(soup), centre, bounds, tree, gpu: None };
    if let Some(gpu) = gpu {
        prepared.gpu = Some(Arc::new(prepared.upload(&gpu.device)));
    }
    Some(prepared)
}

/// `#rrggbb` or `#rrggbbaa` as sRGB bytes with straight alpha.
fn rgba(text: &str) -> Option<[u8; 4]> {
    let hex = text.trim().strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    match hex.len() {
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 255]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

/// `color` laid over the model's grey by its alpha, opaque.
fn over_model(color: [u8; 4]) -> [u8; 4] {
    let base = default_model_color();
    let a = u32::from(color[3]);
    let mix = |i: usize| ((u32::from(color[i]) * a + u32::from(base[i]) * (255 - a) + 127) / 255) as u8;
    [mix(0), mix(1), mix(2), 255]
}

/// The colour of each triangle by its overlay class: `palette[class]` over the model's grey;
/// a class without an entry (or one that does not read) shows the grey. `None` when there are
/// not exactly `triangles` classes: an overlay made for another mesh is left out.
pub fn overlay_colors(classes: &[u32], palette: &[String], triangles: usize) -> Option<Vec<[u8; 4]>> {
    if classes.len() != triangles {
        return None;
    }
    let colors: Vec<[u8; 4]> = palette.iter().map(|c| rgba(c).map_or_else(default_model_color, over_model)).collect();
    Some(classes.iter().map(|&k| colors.get(k as usize).copied().unwrap_or_else(default_model_color)).collect())
}

/// No overlay: the model's grey everywhere.
pub fn plain_colors(triangles: usize) -> Vec<[u8; 4]> {
    vec![default_model_color(); triangles]
}

/// What the viewer paints with, from what the plugin asks for.
pub fn paint_mode(paint: &MeshPaint) -> PaintMode {
    let tool = Tool::of(paint.tool);
    let tint = paint.tint.as_deref().and_then(rgba).unwrap_or([255, 150, 50, 200]);
    let erase = paint.erase_tint.as_deref().and_then(rgba).unwrap_or([0, 0, 0, 0]);
    let tinted = matches!(tool, Tool::Brush | Tool::Triangles);
    PaintMode {
        tool,
        radius_mm: if paint.radius_mm.is_finite() { paint.radius_mm.clamp(0.0, 1e6) } else { 5.0 },
        tint: tinted.then(|| over_model(tint)),
        erase_tint: tinted.then(|| over_model(erase)),
    }
}

/// A hit as the plugin gets it: in its own coordinates.
fn to_plugin(hit: SurfaceHit, centre: Vec3) -> MeshHit {
    MeshHit { triangle: hit.triangle, point: (hit.point + centre).to_array(), normal: hit.normal.to_array() }
}

/// What painting on mesh view `id` sends the plugin.
pub fn to_event(id: &str, centre: Vec3, out: paint::Out<SurfaceHit>) -> Event {
    match out {
        paint::Out::Pick { hit, shift } => {
            Event::MeshPick { id: id.to_owned(), hit: hit.map(|h| to_plugin(h, centre)), shift }
        }
        paint::Out::Stroke { stroke, phase, erase, hits } => Event::MeshStroke {
            id: id.to_owned(),
            stroke,
            phase: match phase {
                paint::Phase::Start => StrokePhase::Start,
                paint::Phase::Move => StrokePhase::Move,
                paint::Phase::End => StrokePhase::End,
            },
            erase,
            hits: hits.into_iter().map(|h| to_plugin(h, centre)).collect(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mesh_is_prepared_recentred_in_its_order() {
        let data = MeshData::cuboid([10.0, 20.0, 30.0]);
        let p = prepare(&data, None).unwrap();
        assert_eq!((p.triangles(), p.tree.len()), (12, 12));
        assert_eq!(p.centre, Vec3::new(5.0, 10.0, 15.0));
        assert_eq!((p.bounds.min, p.bounds.max), (Vec3::new(-5.0, -10.0, -15.0), Vec3::new(5.0, 10.0, 15.0)));
        // Triangle 5 of the soup is triangle 5 of the plugin's mesh.
        let corner = data.indices[15] as usize * 3;
        let original = Vec3::from_slice(&data.positions[corner..corner + 3]);
        assert_eq!(Vec3::from(p.soup[15]) + p.centre, original);
        assert!(p.gpu.is_none(), "no GPU here");
        assert!(prepare(&MeshData::default(), None).is_none());
        let broken = MeshData { indices: vec![0, 1, 99], ..data };
        assert!(prepare(&broken, None).is_none());
    }

    #[test]
    fn an_overlay_colours_by_class_and_only_when_it_fits() {
        let grey = default_model_color();
        let palette = vec!["#00000000".to_owned(), "#ff0000".into(), "#0000ff80".into(), "nonsense".into()];
        let colors = overlay_colors(&[0, 1, 2, 3, 7], &palette, 5).unwrap();
        assert_eq!(colors[0], grey, "transparent: the model shows");
        assert_eq!(colors[1], [255, 0, 0, 255]);
        let half = colors[2];
        assert!(half[2] > half[0] && half[0] < grey[0] && half[3] == 255, "mixed with the grey: {half:?}");
        assert_eq!((colors[3], colors[4]), (grey, grey), "unreadable and missing entries");
        assert_eq!(overlay_colors(&[0, 1], &palette, 3), None, "made for another mesh");
        assert_eq!(overlay_colors(&[0, 1, 1, 0], &palette, 3), None);
        assert_eq!(plain_colors(2), [grey, grey]);
    }

    #[test]
    fn hits_go_to_the_plugin_in_its_coordinates() {
        let centre = Vec3::new(100.0, 0.0, 5.0);
        let hit = SurfaceHit { triangle: 3, point: Vec3::new(1.0, 2.0, 3.0), normal: Vec3::Z };
        let pick = to_event("view", centre, paint::Out::Pick { hit: Some(hit), shift: true });
        let Event::MeshPick { id, hit: Some(h), shift: true } = pick else { panic!("{pick:?}") };
        assert_eq!((id.as_str(), h.triangle, h.point, h.normal), ("view", 3, [101.0, 2.0, 8.0], [0.0, 0.0, 1.0]));
        let stroke = paint::Out::Stroke { stroke: 2, phase: paint::Phase::Move, erase: false, hits: vec![hit, hit] };
        let Event::MeshStroke { stroke: 2, phase: StrokePhase::Move, erase: false, hits, .. } =
            to_event("view", centre, stroke)
        else {
            panic!()
        };
        assert_eq!(hits.len(), 2);
        // The plugin's paint settings.
        let mode = paint_mode(&MeshPaint { radius_mm: f32::NAN, tint: Some("#00ff00".into()), ..MeshPaint::default() });
        assert_eq!((mode.tool, mode.radius_mm, mode.tint), (Tool::Brush, 5.0, Some([0, 255, 0, 255])));
        assert_eq!(mode.erase_tint, Some(default_model_color()), "erasing shows the model");
        let click = paint_mode(&MeshPaint { tool: meshgrove_plugin_api::ui::PaintTool::Click, ..MeshPaint::default() });
        assert_eq!((click.tool, click.tint), (Tool::Click, None), "a click tints nothing");
    }
}
