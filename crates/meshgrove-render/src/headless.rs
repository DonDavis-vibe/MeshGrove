use image::RgbaImage;
use meshgrove_core::Mesh;

use crate::renderer::{GpuMesh, Renderer, ViewParams};

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("no usable GPU adapter: {0}")]
    NoAdapter(String),
    #[error("could not create GPU device: {0}")]
    Device(String),
    #[error("mesh has no finite vertices")]
    EmptyMesh,
    #[error("GPU readback failed: {0}")]
    Readback(String),
}

/// A GPU device without any window, for thumbnails and the Dolphin thumbnailer.
/// Backend selection honours `WGPU_BACKEND` / `WGPU_ADAPTER_NAME` (e.g. lavapipe in CI).
pub struct HeadlessRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    pub adapter_info: wgpu::AdapterInfo,
}

impl HeadlessRenderer {
    pub fn new() -> Result<HeadlessRenderer, RenderError> {
        pollster::block_on(Self::with(crate::renderer::device_features))
    }

    /// With a device that has the `features` chosen for its adapter.
    async fn with(features: impl Fn(&wgpu::Adapter) -> wgpu::Features) -> Result<HeadlessRenderer, RenderError> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        // Vulkan (lavapipe in CI) is all we need; skipping GL avoids EGL noise at shutdown.
        desc.backends = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY);
        let instance = wgpu::Instance::new(desc);
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|e| RenderError::NoAdapter(e.to_string()))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("meshgrove headless"),
                // Take everything the adapter offers so huge meshes and textures fit.
                required_limits: adapter.limits(),
                required_features: features(&adapter),
                ..Default::default()
            })
            .await
            .map_err(|e| RenderError::Device(e.to_string()))?;
        let renderer = Renderer::new(&device, &adapter);
        Ok(HeadlessRenderer { device, queue, renderer, adapter_info: adapter.get_info() })
    }

    /// The device, its queue and the renderer, for uploading and drawing meshes directly.
    pub fn parts(&self) -> (&wgpu::Device, &wgpu::Queue, &Renderer) {
        (&self.device, &self.queue, &self.renderer)
    }

    /// Renders `mesh` into a `size`×`size` image with straight (non-premultiplied) alpha.
    pub fn render(&self, mesh: &Mesh, size: u32, view: &ViewParams) -> Result<RgbaImage, RenderError> {
        let gpu_mesh = self.renderer.upload(&self.device, mesh).ok_or(RenderError::EmptyMesh)?;
        self.renderer.render_image(&self.device, &self.queue, &gpu_mesh, view, size, size)
    }
}

impl Renderer {
    /// Draws an uploaded mesh into a new `width`×`height` image (straight alpha) and waits for
    /// it — thumbnails, and picture export from the viewer.
    pub fn render_image(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: &GpuMesh,
        view: &ViewParams,
        width: u32,
        height: u32,
    ) -> Result<RgbaImage, RenderError> {
        let target = self.create_picture_target(device, width, height, wgpu::TextureUsages::COPY_SRC);
        self.read(device, queue, &target, mesh, view)
    }

    /// Draws `mesh` into `target` (made with `COPY_SRC`) and reads the picture back.
    fn read(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &crate::RenderTarget,
        mesh: &GpuMesh,
        view: &ViewParams,
    ) -> Result<RgbaImage, RenderError> {
        // Rows in a texture→buffer copy must be padded to 256 bytes.
        let unpadded = target.width * 4;
        let padded = unpadded.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("meshgrove readback"),
            size: padded as u64 * target.height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("meshgrove image") });
        self.draw(queue, &mut encoder, target, mesh, view);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target.color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: None },
            },
            wgpu::Extent3d { width: target.width, height: target.height, depth_or_array_layers: 1 },
        );
        queue.submit([encoder.finish()]);

        let (tx, rx) = std::sync::mpsc::channel();
        readback.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = tx.send(result);
        });
        device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| RenderError::Readback(e.to_string()))?;
        rx.recv()
            .map_err(|e| RenderError::Readback(e.to_string()))?
            .map_err(|e| RenderError::Readback(e.to_string()))?;

        let mut pixels = Vec::with_capacity((unpadded * target.height) as usize);
        {
            let data = readback.get_mapped_range(..).map_err(|e| RenderError::Readback(e.to_string()))?;
            for row in data.chunks_exact(padded as usize) {
                pixels.extend_from_slice(&row[..unpadded as usize]);
            }
        }
        readback.unmap();
        unpremultiply(&mut pixels);
        Ok(RgbaImage::from_raw(target.width, target.height, pixels).expect("buffer size matches image size"))
    }
}

/// MSAA-resolved edges against a transparent clear are premultiplied; PNG wants straight alpha.
fn unpremultiply(pixels: &mut [u8]) {
    for px in pixels.as_chunks_mut::<4>().0 {
        let a = px[3];
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On a software renderer the views of a window are drawn with one sample per pixel, so
    /// every pixel is the model or not; a picture that is kept has its edges smoothed.
    #[test]
    fn a_software_renderers_views_go_without_smoothed_edges() {
        let gpu = match HeadlessRenderer::new() {
            Ok(gpu) => gpu,
            Err(e) if std::env::var_os("MESHGROVE_REQUIRE_GPU").is_none() => {
                eprintln!("skipping GPU test: {e}");
                return;
            }
            Err(e) => panic!("{e}"),
        };
        let (device, queue, _) = gpu.parts();
        let smooth = gpu.renderer.smooth();
        let renderer = Renderer::with(device, smooth, true);
        assert!(renderer.software());
        let mesh = renderer.upload(device, &Mesh::cube(20.0)).unwrap();
        let view = ViewParams::default();
        let half = |image: &RgbaImage| image.pixels().filter(|p| p[3] != 0 && p[3] != 255).count();
        let covered = |image: &RgbaImage| image.pixels().filter(|p| p[3] == 255).count();

        let target = renderer.create_target(device, 128, 128, wgpu::TextureUsages::COPY_SRC);
        let shown = renderer.read(device, queue, &target, &mesh, &view).unwrap();
        assert_eq!(half(&shown), 0, "a view has no half-covered pixels");
        assert!(covered(&shown) > 2000, "the cube is drawn: {}", covered(&shown));

        let kept = renderer.render_image(device, queue, &mesh, &view, 128, 128).unwrap();
        if smooth > 1 {
            assert!(half(&kept) > 50, "a kept picture has smoothed edges: {}", half(&kept));
        }
        // With a graphics card both are the same.
        let renderer = Renderer::with(device, smooth, false);
        assert!(!renderer.software());
        let target = renderer.create_target(device, 128, 128, wgpu::TextureUsages::COPY_SRC);
        let shown = renderer.read(device, queue, &target, &mesh, &view).unwrap();
        assert_eq!(shown, kept);
    }

    /// A mesh whose buffers the program fills itself (a software renderer's) is drawn like
    /// any other, colours and their later changes included.
    #[test]
    fn a_mesh_in_buffers_the_program_fills_looks_the_same() {
        let direct = wgpu::Features::MAPPABLE_PRIMARY_BUFFERS;
        let (Ok(usual), Ok(filled)) = (
            pollster::block_on(HeadlessRenderer::with(|_| wgpu::Features::empty())),
            pollster::block_on(HeadlessRenderer::with(|adapter| adapter.features() & direct)),
        ) else {
            assert!(std::env::var_os("MESHGROVE_REQUIRE_GPU").is_none(), "no GPU");
            eprintln!("skipping GPU test: no adapter");
            return;
        };
        if !filled.device.features().contains(direct) {
            eprintln!("skipping: the adapter has no buffers a program can fill itself");
            return;
        }
        assert!(!usual.device.features().contains(direct));
        let mut cube = Mesh::cube(20.0);
        let view = ViewParams::default();
        assert_eq!(filled.render(&cube, 128, &view).unwrap(), usual.render(&cube, 128, &view).unwrap());

        let n = cube.triangle_count();
        cube.colors = Some((0..n).map(|i| if i < n / 2 { [230, 30, 30, 255] } else { [30, 30, 230, 255] }).collect());
        let pictures = [&filled, &usual].map(|gpu| {
            let mesh = gpu.renderer.upload(&gpu.device, &cube).unwrap();
            let before = gpu.renderer.render_image(&gpu.device, &gpu.queue, &mesh, &view, 128, 128).unwrap();
            assert!(mesh.tint_triangles(&gpu.queue, &[0, 1, 2], [20, 200, 20, 255]));
            let after = gpu.renderer.render_image(&gpu.device, &gpu.queue, &mesh, &view, 128, 128).unwrap();
            assert_ne!(before, after, "the new colour shows");
            (before, after)
        });
        assert_eq!(pictures[0], pictures[1]);
    }
}
