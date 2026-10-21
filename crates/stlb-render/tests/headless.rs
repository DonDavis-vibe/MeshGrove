use stlb_core::Mesh;
use stlb_render::{HeadlessRenderer, ViewParams};

/// GPU tests skip on machines without any adapter unless STLB_REQUIRE_GPU is set (CI sets it).
fn gpu() -> Option<HeadlessRenderer> {
    match HeadlessRenderer::new() {
        Ok(gpu) => Some(gpu),
        Err(e) if std::env::var_os("STLB_REQUIRE_GPU").is_none() => {
            eprintln!("skipping GPU test: {e}");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

/// Fraction of opaque pixels and the centre of their bounding box (0..1 image coordinates).
fn coverage(img: &image::RgbaImage) -> (f32, [f32; 2]) {
    let (mut n, mut min, mut max) = (0u32, [u32::MAX; 2], [0u32; 2]);
    for (x, y, p) in img.enumerate_pixels() {
        if p[3] > 128 {
            n += 1;
            min = [min[0].min(x), min[1].min(y)];
            max = [max[0].max(x), max[1].max(y)];
        }
    }
    let (w, h) = (img.width() as f32, img.height() as f32);
    let center = [(min[0] + max[0]) as f32 / 2.0 / w, (min[1] + max[1]) as f32 / 2.0 / h];
    (n as f32 / (w * h), center)
}

#[test]
fn cube_fills_and_centres_the_thumbnail() {
    let Some(gpu) = gpu() else { return };
    let img = gpu.render(&Mesh::cube(20.0), 128, &ViewParams::default()).unwrap();
    let (cov, center) = coverage(&img);
    assert!((0.2..0.8).contains(&cov), "coverage {cov}");
    assert!(
        (center[0] - 0.5).abs() < 0.05 && (center[1] - 0.5).abs() < 0.05,
        "centre {center:?}"
    );
    // Corners stay transparent.
    assert_eq!(img.get_pixel(0, 0)[3], 0);
}

#[test]
fn long_thin_model_is_framed_too() {
    let Some(gpu) = gpu() else { return };
    let mut rod = Mesh::cube(1.0);
    for p in &mut rod.positions {
        *p = [p[0] * 200.0 + 1000.0, p[1] * 5.0 - 300.0, p[2] * 5.0];
    }
    let img = gpu.render(&rod, 128, &ViewParams::default()).unwrap();
    let (cov, center) = coverage(&img);
    assert!(cov > 0.01, "rod must be visible, coverage {cov}");
    assert!(
        (center[0] - 0.5).abs() < 0.05 && (center[1] - 0.5).abs() < 0.05,
        "centre {center:?}"
    );
}
