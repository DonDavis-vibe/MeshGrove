use image::RgbaImage;
use stlb_core::Mesh;

use crate::renderer::{Renderer, ViewParams};

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
        pollster::block_on(Self::new_async())
    }

    async fn new_async() -> Result<HeadlessRenderer, RenderError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
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
                label: Some("stlb headless"),
                // Take everything the adapter offers so huge meshes and textures fit.
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .map_err(|e| RenderError::Device(e.to_string()))?;
        let renderer = Renderer::new(&device, &adapter);
        Ok(HeadlessRenderer { device, queue, renderer, adapter_info: adapter.get_info() })
    }

    /// Renders `mesh` into a `size`×`size` image with straight (non-premultiplied) alpha.
    pub fn render(&self, mesh: &Mesh, size: u32, view: &ViewParams) -> Result<RgbaImage, RenderError> {
        let gpu_mesh = self.renderer.upload(&self.device, mesh).ok_or(RenderError::EmptyMesh)?;
        let target = self.renderer.create_target(&self.device, size, size, wgpu::TextureUsages::COPY_SRC);

        // Rows in a texture→buffer copy must be padded to 256 bytes.
        let unpadded = target.width * 4;
        let padded = unpadded.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("stlb readback"),
            size: padded as u64 * target.height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder =
            self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("stlb thumbnail") });
        self.renderer.draw(&self.queue, &mut encoder, &target, &gpu_mesh, view);
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
        self.queue.submit([encoder.finish()]);

        let (tx, rx) = std::sync::mpsc::channel();
        readback.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = tx.send(result);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| RenderError::Readback(e.to_string()))?;
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
