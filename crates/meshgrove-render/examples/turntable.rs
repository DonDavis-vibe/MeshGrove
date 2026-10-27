//! Hover-turntable frames, e.g. for sprites on the website: renders a model from `frames`
//! angles around its upright axis the way the gallery spins it on hover (fixed pivot and
//! distance, starting at the thumbnail angle), with studio light on a transparent background.
//! Writes `000.png`, `001.png`, … and `meta.json` (format, filament colours of a 3MF project).
//!
//!     cargo run --release -p meshgrove-render --example turntable -- <model> <out-dir> [frames=48] [px=360]

use std::path::Path;

use anyhow::Context;
use meshgrove_core::loaders::{self, threemf};
use meshgrove_core::{Kind, archive};
use meshgrove_render::{Framing, HeadlessRenderer, ViewParams};

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, out_dir, rest @ ..] = args.as_slice() else {
        anyhow::bail!("usage: turntable <model> <out-dir> [frames=48] [px=360]");
    };
    let frames: u32 = rest.first().map(|s| s.parse()).transpose()?.unwrap_or(48).max(1);
    let px: u32 = rest.get(1).map(|s| s.parse()).transpose()?.unwrap_or(360);

    // Loaded like the app's viewer and turntable: 3MF with first plate, filament colours and
    // paint (`thumbnail::make` would hand back the slicer's plate picture instead).
    let path = Path::new(input);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let kind = Kind::from_file_name(&name);
    let (mesh, filaments) = match kind {
        Kind::Step => (loaders::step::load(path)?, Vec::new()),
        Kind::ThreeMf => {
            let model = threemf::parse_plate(&archive::read(path).with_context(|| format!("reading {input}"))?, None)?;
            (model.mesh, model.filaments.iter().map(|f| f.color).collect())
        }
        _ => {
            let bytes = archive::read(path).with_context(|| format!("reading {input}"))?;
            let sibling = |name: &str| archive::read_sibling(path, name);
            (loaders::load_mesh_with(kind, &bytes, &sibling)?, Vec::new())
        }
    };

    let t0 = std::time::Instant::now();
    let gpu = HeadlessRenderer::new()?;
    let out = Path::new(out_dir);
    std::fs::create_dir_all(out).with_context(|| format!("creating {out_dir}"))?;
    let start = ViewParams::default();
    for i in 0..frames {
        // The hover spin raises the azimuth over time, so the frames run in the same direction.
        let view = ViewParams {
            azimuth_deg: start.azimuth_deg + 360.0 * i as f32 / frames as f32,
            framing: Framing::Orbit,
            studio: true,
            background: [0.0; 4],
            ..start
        };
        gpu.render(&mesh, px, &view)?.save(out.join(format!("{i:03}.png")))?;
    }

    let format = name.rsplit_once('.').map(|(_, ext)| ext.to_uppercase()).unwrap_or_default();
    let colors: Vec<String> = filaments.iter().map(|[r, g, b]| format!("\"#{r:02x}{g:02x}{b:02x}\"")).collect();
    let meta = format!(
        "{{\"format\": \"{format}\", \"filaments\": [{}], \"triangles\": {}, \"frames\": {frames}, \"px\": {px}}}\n",
        colors.join(", "),
        mesh.triangle_count()
    );
    std::fs::write(out.join("meta.json"), meta)?;
    println!(
        "adapter: {} ({:?})\ntriangles: {}\n{frames} frames of {px} px in {:?}\nwrote {out_dir}",
        gpu.adapter_info.name,
        gpu.adapter_info.backend,
        mesh.triangle_count(),
        t0.elapsed()
    );
    Ok(())
}
