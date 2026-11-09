//! One renderer for every place a model is drawn: gallery thumbnails (headless),
//! the 3D viewer and hover turntable (inside the GUI), and the Dolphin thumbnailer.

mod headless;
mod renderer;

pub use headless::{HeadlessRenderer, RenderError};
pub use renderer::{
    COLOR_FORMAT, Framing, GpuMesh, RenderTarget, Renderer, ViewParams, default_model_color, project_point,
    upload_mesh, upload_soup, view_ray,
};
