use glam::{Mat4, Vec2, Vec3};
use stlb_core::{Aabb, Mesh};
use wgpu::util::DeviceExt;

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// sRGB so the shader can work in linear light and PNGs / egui get gamma-correct pixels.
pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

const FOV_Y_DEG: f32 = 28.0;
/// Keeps vertex buffers well below wgpu's default 256 MiB `max_buffer_size`.
const MAX_CHUNK_BYTES: usize = 64 << 20;

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
        let sample_count = if adapter
            .get_texture_format_features(COLOR_FORMAT)
            .flags
            .sample_count_supported(4)
        {
            4
        } else {
            1
        };
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("stlb mesh shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/mesh.wgsl").into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("stlb uniforms"),
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
            label: Some("stlb mesh layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("stlb mesh pipeline"),
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
            multisample: wgpu::MultisampleState {
                count: sample_count,
                ..Default::default()
            },
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
        Renderer {
            pipeline,
            bind_group_layout,
            sample_count,
        }
    }

    pub fn upload(&self, device: &wgpu::Device, mesh: &Mesh) -> Option<GpuMesh> {
        let bounds = mesh.bounds()?;
        let center = bounds.center();
        let soup: Vec<[f32; 3]> = mesh
            .to_soup()
            .into_iter()
            .map(|p| (Vec3::from(p) - center).to_array())
            .collect();
        let verts_per_chunk = (MAX_CHUNK_BYTES / 12) / 3 * 3;
        let chunks = soup
            .chunks(verts_per_chunk)
            .map(|chunk| {
                let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("stlb mesh vertices"),
                    contents: bytemuck::cast_slice(chunk),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                (buffer, chunk.len() as u32)
            })
            .collect();
        Some(GpuMesh {
            chunks,
            bounds: Aabb {
                min: bounds.min - center,
                max: bounds.max - center,
            },
            triangle_count: mesh.triangle_count(),
        })
    }

    /// `extra_usage` lets callers add e.g. `COPY_SRC` (readback) or `TEXTURE_BINDING` (egui).
    pub fn create_target(
        &self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        extra_usage: wgpu::TextureUsages,
    ) -> RenderTarget {
        let size = wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        };
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
        let color = texture(
            "stlb color",
            COLOR_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | extra_usage,
        );
        let msaa_view = (self.sample_count > 1).then(|| {
            texture(
                "stlb msaa",
                COLOR_FORMAT,
                self.sample_count,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
            .create_view(&Default::default())
        });
        let depth_view = texture(
            "stlb depth",
            DEPTH_FORMAT,
            self.sample_count,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        )
        .create_view(&Default::default());
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stlb uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("stlb uniforms"),
            layout: &self.bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
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
            label: Some("stlb mesh pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: view_tex,
                depth_slice: None,
                resolve_target: resolve,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: if resolve.is_some() {
                        wgpu::StoreOp::Discard
                    } else {
                        wgpu::StoreOp::Store
                    },
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &target.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
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

struct Camera {
    eye: Vec3,
    view_proj: Mat4,
    right: Vec3,
    up: Vec3,
    forward: Vec3,
}

impl Camera {
    /// Places the camera so the model's bounding box fills ~92 % of the view and is centred
    /// on screen, then applies the user's zoom and pan.
    fn fit(bounds: &Aabb, view: &ViewParams, aspect: f32) -> Camera {
        let (az, el) = (
            view.azimuth_deg.to_radians(),
            view.elevation_deg.clamp(-89.5, 89.5).to_radians(),
        );
        let to_eye = Vec3::new(el.cos() * az.sin(), -el.cos() * az.cos(), el.sin());
        let forward = -to_eye;
        let right = forward.cross(Vec3::Z).normalize();
        let up = right.cross(forward);

        let tan_y = (FOV_Y_DEG.to_radians() * 0.5).tan();
        let tan_x = tan_y * aspect;
        let radius = bounds.radius().max(1e-4);
        let mut target = bounds.center();
        let mut dist = radius / tan_x.min(tan_y).atan().sin();
        let corners: Vec<Vec3> = (0..8)
            .map(|i| {
                Vec3::new(
                    if i & 1 == 0 { bounds.min.x } else { bounds.max.x },
                    if i & 2 == 0 { bounds.min.y } else { bounds.max.y },
                    if i & 4 == 0 { bounds.min.z } else { bounds.max.z },
                )
            })
            .collect();
        // Perspective makes the fit non-linear; a few refinement rounds converge well.
        for _ in 0..4 {
            let eye = target + to_eye * dist;
            let (mut lo, mut hi) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
            for c in &corners {
                let d = *c - eye;
                let depth = d.dot(forward).max(1e-6);
                let ndc = Vec2::new(d.dot(right) / (depth * tan_x), d.dot(up) / (depth * tan_y));
                lo = lo.min(ndc);
                hi = hi.max(ndc);
            }
            let mid = (lo + hi) * 0.5;
            target += right * mid.x * dist * tan_x + up * mid.y * dist * tan_y;
            let extent = ((hi - lo) * 0.5).max_element();
            dist *= (extent / 0.92).max(0.05);
        }

        target += (right * view.pan.x + up * view.pan.y) * radius;
        dist /= view.zoom.max(0.01);
        let eye = target + to_eye * dist;
        let near = (dist - radius * 1.5).max(dist * 0.01);
        let far = dist + radius * 1.5;
        // wgpu NDC is Y-up with depth in [0, 1], i.e. the DirectX convention.
        let proj = glam::camera::rh::proj::directx::perspective(FOV_Y_DEG.to_radians(), aspect, near, far);
        let view_m = glam::camera::rh::view::look_at_mat4(eye, target, up);
        Camera {
            eye,
            view_proj: proj * view_m,
            right,
            up,
            forward,
        }
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

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
