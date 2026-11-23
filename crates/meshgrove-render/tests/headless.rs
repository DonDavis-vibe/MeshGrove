use meshgrove_core::Mesh;
use meshgrove_render::{HeadlessRenderer, ViewParams};

/// GPU tests skip on machines without any adapter unless MESHGROVE_REQUIRE_GPU is set (CI sets it).
fn gpu() -> Option<HeadlessRenderer> {
    match HeadlessRenderer::new() {
        Ok(gpu) => Some(gpu),
        Err(e) if std::env::var_os("MESHGROVE_REQUIRE_GPU").is_none() => {
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
    assert!((center[0] - 0.5).abs() < 0.05 && (center[1] - 0.5).abs() < 0.05, "centre {center:?}");
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
    assert!((center[0] - 0.5).abs() < 0.05 && (center[1] - 0.5).abs() < 0.05, "centre {center:?}");
}

#[test]
fn coloured_triangles_keep_their_colours() {
    let Some(gpu) = gpu() else { return };
    let mut cube = Mesh::cube(20.0);
    // Half the triangles red, half blue; the model colour must not show up anywhere.
    let n = cube.triangle_count();
    cube.colors = Some((0..n).map(|i| if i < n / 2 { [230, 30, 30, 255] } else { [30, 30, 230, 255] }).collect());
    let view = ViewParams { model_color: [0.0, 1.0, 0.0], ..Default::default() };
    let img = gpu.render(&cube, 128, &view).unwrap();
    let (mut red, mut blue, mut green) = (0, 0, 0);
    for p in img.pixels().filter(|p| p[3] > 250) {
        let [r, g, b, _] = p.0;
        if r > g.saturating_add(40) && r > b.saturating_add(40) {
            red += 1;
        } else if b > r.saturating_add(40) && b > g.saturating_add(40) {
            blue += 1;
        } else if g > r.saturating_add(40) && g > b.saturating_add(40) {
            green += 1;
        }
    }
    assert!(red > 200 && blue > 200, "red {red}, blue {blue}");
    assert_eq!(green, 0);
    // Plain meshes still use the model colour.
    let plain = gpu.render(&Mesh::cube(20.0), 128, &view).unwrap();
    assert!(plain.pixels().any(|p| p[3] > 250 && p[1] > p[0].saturating_add(40)));
}

/// Counts of clearly red, clearly blue and grey (model coloured) pixels of a picture.
fn tally(img: &image::RgbaImage) -> (usize, usize, usize) {
    let (mut red, mut blue, mut grey) = (0, 0, 0);
    for p in img.pixels().filter(|p| p[3] > 250) {
        let [r, g, b, _] = p.0;
        if r > g.saturating_add(40) && r > b.saturating_add(40) {
            red += 1;
        } else if b > r.saturating_add(40) && b > g.saturating_add(40) {
            blue += 1;
        } else if r.abs_diff(g) < 20 && g.abs_diff(b) < 30 {
            grey += 1;
        }
    }
    (red, blue, grey)
}

/// A soup uploaded with colours can be coloured anew, all of it or a few triangles: what an
/// overlay and the tint of a brush do.
#[test]
fn triangle_colours_change_after_the_upload() {
    use meshgrove_render::{default_model_color, upload_soup};
    let Some(gpu) = gpu() else { return };
    let (device, queue, renderer) = gpu.parts();
    let cube = Mesh::cube(20.0);
    let bounds = cube.bounds().unwrap();
    let centre = bounds.center();
    let soup: Vec<[f32; 3]> = cube.to_soup().into_iter().map(|p| (glam::Vec3::from(p) - centre).to_array()).collect();
    let bounds = meshgrove_core::Aabb { min: bounds.min - centre, max: bounds.max - centre };
    let plain = vec![default_model_color(); soup.len()];
    let mesh = upload_soup(device, &soup, bounds, Some(&plain));
    assert_eq!(mesh.triangle_count, 12);
    let view = ViewParams::default();
    let picture = |mesh| renderer.render_image(device, queue, mesh, &view, 128, 128).unwrap();
    let (red, blue, grey) = tally(&picture(&mesh));
    assert!(red == 0 && blue == 0 && grey > 1000, "model coloured: {red} {blue} {grey}");
    // Every triangle red.
    assert!(mesh.set_triangle_colors(queue, &[[230, 30, 30, 255]; 12]));
    let (red, _, grey) = tally(&picture(&mesh));
    assert!(red > 1000 && grey == 0, "{red} {grey}");
    // The wrong length changes nothing.
    assert!(!mesh.set_triangle_colors(queue, &[[30, 30, 230, 255]; 11]));
    assert_eq!(tally(&picture(&mesh)).1, 0);
    // A run of triangles blue: the top and the front (4..8), seen from above at the front.
    assert!(mesh.tint_triangles(queue, &[4, 5, 6, 7], [30, 30, 230, 255]));
    let (red, blue, _) = tally(&picture(&mesh));
    assert!(red > 100 && blue > 500, "{red} {blue}");
    // A mesh uploaded without colours has none to change.
    let bare = upload_soup(device, &soup, bounds, None);
    assert!(!bare.set_triangle_colors(queue, &[[230, 30, 30, 255]; 12]) && !bare.tint_triangles(queue, &[0], [0; 4]));
}

/// The wireframe draws the triangles' edges on the surface: dark lines on the light cube, its
/// outline and what is between the lines unchanged. A coloured mesh gets them as well.
#[test]
fn wireframe_draws_the_edges_on_the_surface() {
    let Some(gpu) = gpu() else { return };
    let cube = Mesh::cube(20.0);
    let plain = gpu.render(&cube, 256, &ViewParams::default()).unwrap();
    let wired = gpu.render(&cube, 256, &ViewParams { edges: 2.0, ..ViewParams::default() }).unwrap();
    let brightness = |p: &image::Rgba<u8>| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]);
    let (mut covered, mut darker, mut lighter, mut outline) = (0u32, 0u32, 0u32, 0u32);
    for (a, b) in plain.pixels().zip(wired.pixels()) {
        outline += u32::from((a[3] > 128) != (b[3] > 128));
        if a[3] > 128 {
            covered += 1;
            darker += u32::from(brightness(b) + 90 < brightness(a));
            lighter += u32::from(brightness(b) > brightness(a) + 6);
        }
    }
    assert_eq!(outline, 0, "the wireframe changes the model's outline");
    assert_eq!(lighter, 0, "lines on a light surface are dark");
    let share = darker as f32 / covered as f32;
    assert!((0.02..0.25).contains(&share), "lines cover {share} of the cube");

    let mut painted = cube.clone();
    painted.colors = Some(vec![[20, 20, 20, 255]; painted.triangle_count()]);
    let view = ViewParams::default();
    let dark = gpu.render(&painted, 256, &view).unwrap();
    let dark_wired = gpu.render(&painted, 256, &ViewParams { edges: 2.0, ..view }).unwrap();
    let lit = dark.pixels().zip(dark_wired.pixels()).filter(|(a, b)| brightness(b) > brightness(a) + 90).count();
    assert!(lit > 200, "light lines on a dark surface: {lit}");
}
