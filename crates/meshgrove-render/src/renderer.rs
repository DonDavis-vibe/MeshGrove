use glam::{Mat4, Vec2, Vec3};
use meshgrove_core::{Aabb, Mesh};
use wgpu::util::DeviceExt;

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// sRGB so the shader can work in linear light and PNGs / egui get gamma-correct pixels.
pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

const FOV_Y_DEG: f32 = 28.0;
/// Keeps vertex buffers well below wgpu's default 256 MiB `max_buffer_size`.
const MAX_CHUNK_BYTES: usize = 64 << 20;

/// How the camera frames the model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Framing {
    /// Tightest fit for exactly this angle, centred on screen (still thumbnails).
    #[default]
    Tight,
    /// Fixed pivot (box centre) and one distance that fits every angle, so turning the model
    /// never zooms or shifts it (3D viewer, hover turntable).
    Orbit,
}

/// How a model is looked at. Angles in degrees; azimuth 0 looks from the front (−Y),
/// positive azimuth moves the camera to the right; elevation 90 looks straight down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewParams {
    pub azimuth_deg: f32,
    pub elevation_deg: f32,
    /// 1.0 = the model fills the view; larger values zoom in.
    pub zoom: f32,
    /// Pan in units of the model's bounding radius (screen right / screen up).
    pub pan: Vec2,
    /// sRGB, 0..1.
    pub model_color: [f32; 3],
    /// Straight sRGB + alpha, 0..1. Alpha 0 gives transparent thumbnails.
    pub background: [f32; 4],
    pub framing: Framing,
}

impl Default for ViewParams {
    fn default() -> Self {
        ViewParams {
            azimuth_deg: 30.0,
            elevation_deg: 25.0,
            zoom: 1.0,
            pan: Vec2::ZERO,
            model_color: [0.80, 0.82, 0.85],
            background: [0.0, 0.0, 0.0, 0.0],
            framing: Framing::Tight,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    base_color: [f32; 4],
    key_dir: [f32; 4],
    fill_dir: [f32; 4],
}

/// A mesh uploaded to the GPU, recentred on its bounding-box centre for float precision.
pub struct GpuMesh {
    chunks: Vec<(wgpu::Buffer, u32)>,
    /// Bounds relative to the uploaded (recentred) coordinates.
    pub bounds: Aabb,
    pub triangle_count: usize,
}

impl GpuMesh {
    /// GPU memory used by the vertex buffers.
    pub fn byte_size(&self) -> usize {
        self.chunks.iter().map(|(_, count)| *count as usize * 12).sum()
    }
}

/// Where one view gets drawn: an optional MSAA buffer, the resolved colour texture and depth.
pub struct RenderTarget {
    pub width: u32,
    pub height: u32,
    msaa_view: Option<wgpu::TextureView>,
    pub color: wgpu::Texture,
    pub color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

pub struct Renderer {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sample_count: u32,
}

impl Renderer {
    pub fn new(device: &wgpu::Device, adapter: &wgpu::Adapter) -> Renderer {
        let sample_count =
            if adapter.get_texture_format_features(COLOR_FORMAT).flags.sample_count_supported(4) { 4 } else { 1 };
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("meshgrove mesh shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/mesh.wgsl").into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("meshgrove uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("meshgrove mesh layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("meshgrove mesh pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // No culling: winding in STL files is unreliable and meshes may be open.
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: sample_count, ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Renderer { pipeline, bind_group_layout, sample_count }
    }

    pub fn upload(&self, device: &wgpu::Device, mesh: &Mesh) -> Option<GpuMesh> {
        upload_mesh(device, mesh)
    }

    /// `extra_usage` lets callers add e.g. `COPY_SRC` (readback) or `TEXTURE_BINDING` (egui).
    pub fn create_target(
        &self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        extra_usage: wgpu::TextureUsages,
    ) -> RenderTarget {
        let size = wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 };
        let texture = |label, format, samples, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let color = texture("meshgrove color", COLOR_FORMAT, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | extra_usage);
        let msaa_view = (self.sample_count > 1).then(|| {
            texture("meshgrove msaa", COLOR_FORMAT, self.sample_count, wgpu::TextureUsages::RENDER_ATTACHMENT)
                .create_view(&Default::default())
        });
        let depth_view =
            texture("meshgrove depth", DEPTH_FORMAT, self.sample_count, wgpu::TextureUsages::RENDER_ATTACHMENT)
                .create_view(&Default::default());
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("meshgrove uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("meshgrove uniforms"),
            layout: &self.bind_group_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        RenderTarget {
            width: size.width,
            height: size.height,
            msaa_view,
            color_view: color.create_view(&Default::default()),
            color,
            depth_view,
            uniforms,
            bind_group,
        }
    }

    /// Records one draw of `mesh` into `target`. The uniform write goes through the queue,
    /// so draw a given target at most once per submission.
    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &RenderTarget,
        mesh: &GpuMesh,
        view: &ViewParams,
    ) {
        let aspect = target.width as f32 / target.height as f32;
        let camera = Camera::fit(&mesh.bounds, view, aspect);
        queue.write_buffer(&target.uniforms, 0, bytemuck::bytes_of(&camera.uniforms(view)));

        let [r, g, b, a] = view.background;
        // Clear values are linear and premultiplied, matching what egui and our PNG export expect.
        let clear = wgpu::Color {
            r: srgb_to_linear(r) as f64 * a as f64,
            g: srgb_to_linear(g) as f64 * a as f64,
            b: srgb_to_linear(b) as f64 * a as f64,
            a: a as f64,
        };
        let (view_tex, resolve) = match &target.msaa_view {
            Some(msaa) => (msaa, Some(&target.color_view)),
            None => (&target.color_view, None),
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("meshgrove mesh pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: view_tex,
                depth_slice: None,
                resolve_target: resolve,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: if resolve.is_some() { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store },
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &target.depth_view,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &target.bind_group, &[]);
        for (buffer, count) in &mesh.chunks {
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..*count, 0..1);
        }
    }
}

/// The ray from the camera through a point of the view (`ndc` −1..1, y up), in the mesh's
/// recentred coordinates: (origin, normalised direction). For picking points on the model.
pub fn view_ray(bounds: &Aabb, view: &ViewParams, aspect: f32, ndc: Vec2) -> (Vec3, Vec3) {
    let inv = Camera::fit(bounds, view, aspect).view_proj.inverse();
    let near = inv.project_point3(Vec3::new(ndc.x, ndc.y, 0.0));
    let far = inv.project_point3(Vec3::new(ndc.x, ndc.y, 1.0));
    (near, (far - near).normalize())
}

/// Where a point of the mesh (recentred coordinates) appears in the view, in NDC (y up);
/// `None` when it is behind the camera.
pub fn project_point(bounds: &Aabb, view: &ViewParams, aspect: f32, point: Vec3) -> Option<Vec2> {
    let clip = Camera::fit(bounds, view, aspect).view_proj * point.extend(1.0);
    (clip.w > 1e-6).then(|| Vec2::new(clip.x / clip.w, clip.y / clip.w))
}

/// Uploads a mesh; needs only the device, so it can run on a background thread.
pub fn upload_mesh(device: &wgpu::Device, mesh: &Mesh) -> Option<GpuMesh> {
    let bounds = mesh.bounds()?;
    let center = bounds.center();
    let soup: Vec<[f32; 3]> = mesh.to_soup().into_iter().map(|p| (Vec3::from(p) - center).to_array()).collect();
    let verts_per_chunk = (MAX_CHUNK_BYTES / 12) / 3 * 3;
    let chunks = soup
        .chunks(verts_per_chunk)
        .map(|chunk| {
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("meshgrove mesh vertices"),
                contents: bytemuck::cast_slice(chunk),
                usage: wgpu::BufferUsages::VERTEX,
            });
            (buffer, chunk.len() as u32)
        })
        .collect();
    Some(GpuMesh {
        chunks,
        bounds: Aabb { min: bounds.min - center, max: bounds.max - center },
        triangle_count: mesh.triangle_count(),
    })
}

struct Camera {
    eye: Vec3,
    #[cfg_attr(not(test), allow(dead_code))]
    target: Vec3,
    view_proj: Mat4,
    right: Vec3,
    up: Vec3,
    forward: Vec3,
}

/// Camera axes for a viewing direction: (towards the eye, right, up, forward).
fn basis(azimuth_deg: f32, elevation_deg: f32) -> (Vec3, Vec3, Vec3, Vec3) {
    let (az, el) = (azimuth_deg.to_radians(), elevation_deg.clamp(-89.5, 89.5).to_radians());
    let to_eye = Vec3::new(el.cos() * az.sin(), -el.cos() * az.cos(), el.sin());
    let forward = -to_eye;
    let right = forward.cross(Vec3::Z).normalize();
    let up = right.cross(forward);
    (to_eye, right, up, forward)
}

fn corners(b: &Aabb) -> [Vec3; 8] {
    std::array::from_fn(|i| {
        Vec3::new(
            if i & 1 == 0 { b.min.x } else { b.max.x },
            if i & 2 == 0 { b.min.y } else { b.max.y },
            if i & 4 == 0 { b.min.z } else { b.max.z },
        )
    })
}

/// Fraction of the half view the model may fill.
const FILL: f32 = 0.92;

impl Camera {
    fn fit(bounds: &Aabb, view: &ViewParams, aspect: f32) -> Camera {
        let (to_eye, right, up, forward) = basis(view.azimuth_deg, view.elevation_deg);
        let tan_y = (FOV_Y_DEG.to_radians() * 0.5).tan();
        let tan_x = tan_y * aspect;
        let radius = bounds.radius().max(1e-4);
        let corners = corners(bounds);
        let start = radius / tan_x.min(tan_y).atan().sin();
        // Never put the camera inside the bounding sphere: a corner at the eye projects to infinity.
        let refine = |dist: f32, fill: f32| (dist * (fill / FILL).max(0.05)).max(radius * 1.5);

        let (mut target, mut dist) = match view.framing {
            Framing::Tight => {
                let (mut target, mut dist) = (bounds.center(), start);
                // Perspective makes the fit non-linear; a few refinement rounds converge well.
                for _ in 0..4 {
                    let (lo, hi) = project(&corners, target + to_eye * dist, (right, up, forward), tan_x, tan_y);
                    let mid = (lo + hi) * 0.5;
                    target += right * mid.x * dist * tan_x + up * mid.y * dist * tan_y;
                    dist = refine(dist, ((hi - lo) * 0.5).max_element());
                }
                (target, dist)
            }
            Framing::Orbit => {
                // Pivot stays at the box centre; the distance is the largest any viewing angle needs.
                let target = bounds.center();
                let dist = [0.0, ViewParams::default().elevation_deg, 60.0, 89.0]
                    .iter()
                    .flat_map(|&el| (0..24).map(move |i| basis(i as f32 * 15.0, el)))
                    .map(|(to_eye, right, up, forward)| {
                        let mut dist = start;
                        for _ in 0..5 {
                            let (lo, hi) =
                                project(&corners, target + to_eye * dist, (right, up, forward), tan_x, tan_y);
                            dist = refine(dist, lo.abs().max(hi.abs()).max_element());
                        }
                        dist
                    })
                    .fold(0.0, f32::max);
                (target, dist)
            }
        };

        target += (right * view.pan.x + up * view.pan.y) * radius;
        dist /= view.zoom.max(0.01);
        let eye = target + to_eye * dist;
        let near = (dist - radius * 1.5).max(dist * 0.01);
        let far = dist + radius * 1.5;
        // wgpu NDC is Y-up with depth in [0, 1], i.e. the DirectX convention.
        let proj = glam::camera::rh::proj::directx::perspective(FOV_Y_DEG.to_radians(), aspect, near, far);
        let view_m = glam::camera::rh::view::look_at_mat4(eye, target, up);
        Camera { eye, target, view_proj: proj * view_m, right, up, forward }
    }

    fn uniforms(&self, view: &ViewParams) -> Uniforms {
        let [r, g, b] = view.model_color.map(srgb_to_linear);
        let to_eye = -self.forward;
        let key = (-self.right * 0.45 + self.up * 0.55 + to_eye * 0.75).normalize();
        let fill = (self.right * 0.9 - self.up * 0.2 + to_eye * 0.3).normalize();
        Uniforms {
            view_proj: self.view_proj.to_cols_array_2d(),
            eye: self.eye.extend(1.0).to_array(),
            base_color: [r, g, b, 1.0],
            key_dir: key.extend(0.0).to_array(),
            fill_dir: fill.extend(0.0).to_array(),
        }
    }
}

/// Bounding rectangle of the corners in normalised device coordinates, seen from `eye`.
fn project(
    corners: &[Vec3; 8],
    eye: Vec3,
    (right, up, forward): (Vec3, Vec3, Vec3),
    tan_x: f32,
    tan_y: f32,
) -> (Vec2, Vec2) {
    let (mut lo, mut hi) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
    for c in corners {
        let d = *c - eye;
        let depth = d.dot(forward).max(1e-6);
        let ndc = Vec2::new(d.dot(right) / (depth * tan_x), d.dot(up) / (depth * tan_y));
        lo = lo.min(ndc);
        hi = hi.max(ndc);
    }
    (lo, hi)
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rod() -> Aabb {
        Aabb { min: Vec3::new(-100.0, -5.0, 0.0), max: Vec3::new(100.0, 5.0, 10.0) }
    }

    #[test]
    fn orbit_framing_keeps_pivot_and_distance_while_turning() {
        let view =
            |az, el| ViewParams { azimuth_deg: az, elevation_deg: el, framing: Framing::Orbit, ..Default::default() };
        let a = Camera::fit(&rod(), &view(0.0, 25.0), 1.0);
        for (az, el) in [(37.0, 25.0), (90.0, 25.0), (211.0, 60.0), (300.0, -10.0)] {
            let b = Camera::fit(&rod(), &view(az, el), 1.0);
            assert_eq!(b.target, a.target, "pivot moved at az {az} el {el}");
            let (da, db) = ((a.eye - a.target).length(), (b.eye - b.target).length());
            assert!((db - da).abs() < da * 1e-5, "zoomed at az {az}: {da} vs {db}");
            assert!(da < rod().radius() * 6.0, "camera too far away: {da}");
        }
    }

    #[test]
    fn rays_and_projection_agree() {
        let b = rod();
        let view = ViewParams { azimuth_deg: 40.0, elevation_deg: 20.0, framing: Framing::Orbit, ..Default::default() };
        let corner = Vec3::new(100.0, -5.0, 10.0);
        let ndc = project_point(&b, &view, 1.5, corner).unwrap();
        assert!(ndc.abs().max_element() < 1.0, "corner in view: {ndc}");
        let (origin, dir) = view_ray(&b, &view, 1.5, ndc);
        // The ray through the corner's screen position passes the corner.
        let along = (corner - origin).dot(dir);
        assert!((origin + dir * along - corner).length() < 1e-2);
        // Straight through the centre of the view hits the pivot line.
        let (o, d) = view_ray(&b, &view, 1.5, Vec2::ZERO);
        assert!((o + d * (b.center() - o).dot(d) - b.center()).length() < 1e-2);
    }

    #[test]
    fn orbit_fits_the_model_at_every_angle_and_tight_differs() {
        let b = rod();
        for az in (0..360).step_by(10) {
            let view = ViewParams { azimuth_deg: az as f32, framing: Framing::Orbit, ..Default::default() };
            let cam = Camera::fit(&b, &view, 1.0);
            let (lo, hi) = project(
                &corners(&b),
                cam.eye,
                (cam.right, cam.up, cam.forward),
                (FOV_Y_DEG.to_radians() * 0.5).tan(),
                (FOV_Y_DEG.to_radians() * 0.5).tan(),
            );
            assert!(lo.min_element() >= -1.0 && hi.max_element() <= 1.0, "cut off at az {az}: {lo} {hi}");
        }
        let tight = |az| Camera::fit(&b, &ViewParams { azimuth_deg: az, ..Default::default() }, 1.0);
        assert!((tight(0.0).eye - tight(0.0).target).length() != (tight(90.0).eye - tight(90.0).target).length());
    }
}
