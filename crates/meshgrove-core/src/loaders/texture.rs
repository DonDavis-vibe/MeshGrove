//! Colours of meshes that bring their own (OBJ, PLY, glTF): vertex colours, material colours
//! and textures all end up as one sRGB colour per vertex, which the renderer blends across
//! each triangle. Textures are sampled at the vertices, which looks right for the dense meshes
//! of scans and generated models.

use image::RgbaImage;

/// The renderer's default model colour (light grey) for parts without a colour of their own.
pub const NEUTRAL: [u8; 4] = [204, 209, 217, 255];

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// Linear RGBA (0..1) → sRGB bytes.
pub fn to_srgb8(linear: [f32; 4]) -> [u8; 4] {
    let byte = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    let [r, g, b, a] = linear;
    [byte(linear_to_srgb(r)), byte(linear_to_srgb(g)), byte(linear_to_srgb(b)), byte(a)]
}

/// sRGB bytes → linear RGBA (0..1).
pub fn to_linear(srgb: [u8; 4]) -> [f32; 4] {
    let [r, g, b, a] = srgb.map(|c| f32::from(c) / 255.0);
    [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a]
}

pub fn multiply(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|i| a[i] * b[i])
}

/// Textures above this many pixels are skipped: decoding them would cost more than the colour
/// at the vertices is worth.
const MAX_PIXELS: u64 = 8192 * 8192;

/// A decoded texture, sampled with bilinear filtering and repeat wrapping.
pub struct Texture(RgbaImage);

impl Texture {
    pub fn decode(bytes: &[u8]) -> Option<Texture> {
        let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
        let (w, h) = reader.into_dimensions().ok()?;
        if u64::from(w) * u64::from(h) > MAX_PIXELS || w == 0 || h == 0 {
            return None;
        }
        Some(Texture(image::load_from_memory(bytes).ok()?.to_rgba8()))
    }

    /// Linear RGBA at `uv`, with u to the right and v down from the top-left corner (glTF's
    /// convention; OBJ counts v up and passes `1 - v`).
    pub fn sample(&self, [u, v]: [f32; 2]) -> [f32; 4] {
        let (w, h) = self.0.dimensions();
        if !u.is_finite() || !v.is_finite() {
            return [1.0; 4];
        }
        let x = u.rem_euclid(1.0) * w as f32 - 0.5;
        let y = v.rem_euclid(1.0) * h as f32 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let texel = |dx: f32, dy: f32| {
            let px = ((x0 + dx) as i64).rem_euclid(i64::from(w)) as u32;
            let py = ((y0 + dy) as i64).rem_euclid(i64::from(h)) as u32;
            to_linear(self.0.get_pixel(px, py).0)
        };
        let (a, b, c, d) = (texel(0.0, 0.0), texel(1.0, 0.0), texel(0.0, 1.0), texel(1.0, 1.0));
        std::array::from_fn(|i| {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bottom = c[i] + (d[i] - c[i]) * fx;
            top + (bottom - top) * fy
        })
    }
}

/// Whether the colours say anything: one grey or white for the whole model (Blender's default
/// material, untextured exports, our own [`NEUTRAL`]) does not, and the model is better shown
/// in the user's colour.
pub fn meaningful(colors: &[[u8; 4]]) -> bool {
    let Some(first) = colors.first() else { return false };
    let grey = |c: &[u8; 4]| c[..3].iter().max().unwrap_or(&0) - c[..3].iter().min().unwrap_or(&0) <= 16;
    !(grey(first) && colors.iter().all(|c| c == first))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(pixels: &[[u8; 4]], w: u32) -> Vec<u8> {
        let img = RgbaImage::from_fn(w, pixels.len() as u32 / w, |x, y| image::Rgba(pixels[(y * w + x) as usize]));
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn samples_texel_centres_and_wraps() {
        let tex = Texture::decode(&png(&[[255, 0, 0, 255], [0, 0, 255, 255]], 2)).unwrap();
        assert_eq!(to_srgb8(tex.sample([0.25, 0.5])), [255, 0, 0, 255]);
        assert_eq!(to_srgb8(tex.sample([0.75, 0.5])), [0, 0, 255, 255]);
        assert_eq!(to_srgb8(tex.sample([1.25, 0.5])), [255, 0, 0, 255], "repeat wrapping");
        assert!(Texture::decode(b"not an image").is_none());
    }

    #[test]
    fn colour_space_round_trip_and_meaning() {
        for c in [0u8, 1, 50, 128, 200, 255] {
            assert_eq!(to_srgb8(to_linear([c, c, c, 255]))[0], c);
        }
        assert!(!meaningful(&[[204, 204, 204, 255]; 3]));
        assert!(!meaningful(&[NEUTRAL; 2]));
        assert!(!meaningful(&[]));
        assert!(meaningful(&[[255, 0, 0, 255]; 3]));
        assert!(meaningful(&[[204, 204, 204, 255], [255, 255, 255, 255]]));
    }
}
