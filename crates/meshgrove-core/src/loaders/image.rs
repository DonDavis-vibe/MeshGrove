//! Thumbnails for photos and renders that sit next to the models (JPEG, PNG, WebP, …).

use std::io::Cursor;

use image::{DynamicImage, ImageDecoder, ImageReader, RgbaImage};

use super::LoadError;

/// Decodes and shrinks to fit `max`×`max`, honouring the EXIF orientation of phone photos.
pub fn thumbnail(bytes: &[u8], max: u32) -> Result<RgbaImage, LoadError> {
    let mut decoder = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok(image.thumbnail(max, max).to_rgba8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrinks_keeping_aspect_ratio() {
        let mut png = Vec::new();
        RgbaImage::new(800, 400).write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let thumb = thumbnail(&png, 200).unwrap();
        assert_eq!((thumb.width(), thumb.height()), (200, 100));
        assert!(thumbnail(b"not an image", 200).is_err());
    }
}
