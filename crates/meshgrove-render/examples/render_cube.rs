//! Spike A: render a cube (or any STL given as argument) headless to a PNG.
//!
//!     cargo run -p meshgrove-render --example render_cube -- [model.stl|-] [out.png] [size]

use anyhow::Context;
use meshgrove_core::{Mesh, loaders::stl};
use meshgrove_render::{HeadlessRenderer, ViewParams};

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mesh = match args.first().filter(|a| !a.is_empty() && *a != "-") {
        Some(path) => stl::parse(&std::fs::read(path).with_context(|| format!("reading {path}"))?)?,
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
