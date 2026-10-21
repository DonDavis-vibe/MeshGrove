//! Spike A: render a cube (or any model file given as argument) headless to a PNG. 3MF files
//! are drawn like the viewer shows them: first plate, filament colours and paint.
//!
//!     cargo run -p meshgrove-render --example render_cube -- [model.stl|-] [out.png] [size]

use anyhow::Context;
use meshgrove_core::loaders::{self, threemf};
use meshgrove_core::{Kind, Mesh};
use meshgrove_render::{HeadlessRenderer, ViewParams};

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mesh = match args.first().filter(|a| !a.is_empty() && *a != "-") {
        Some(path) => {
            let bytes = std::fs::read(path).with_context(|| format!("reading {path}"))?;
            match Kind::from_path(path.as_ref()) {
                Kind::ThreeMf => threemf::parse_plate(&bytes, None)?.mesh,
                kind => loaders::load_mesh_with(kind, &bytes, &|name| {
                    meshgrove_core::archive::read_sibling(path.as_ref(), name)
                })?,
            }
        }
        None => Mesh::cube(20.0),
    };
    let out = args.get(1).map(String::as_str).unwrap_or("cube.png");
    let size: u32 = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(384);

    let t0 = std::time::Instant::now();
    let gpu = HeadlessRenderer::new()?;
    let t1 = std::time::Instant::now();
    let image = gpu.render(&mesh, size, &ViewParams::default())?;
    let t2 = std::time::Instant::now();
    image.save(out)?;
    println!(
        "adapter: {} ({:?})\ntriangles: {}\ninit: {:?}, render+readback: {:?}\nwrote {out}",
        gpu.adapter_info.name,
        gpu.adapter_info.backend,
        mesh.triangle_count(),
        t1 - t0,
        t2 - t1
    );
    Ok(())
}
