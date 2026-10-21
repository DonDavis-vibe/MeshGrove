//! What a thumbnail shows for each kind of file — shared by the browser and the Dolphin
//! thumbnailer. Drawing meshes is left to the caller (`render`), which owns the GPU.

use std::path::Path;

use image::RgbaImage;

use crate::cache::{ThumbMeta, Thumbnail};
use crate::dedupe;
use crate::formats::Kind;
use crate::loaders;
use crate::mesh::Mesh;

#[derive(Debug)]
pub enum ThumbError {
    /// The file itself is unusable; worth remembering until it changes.
    Permanent(String),
    /// Disk or GPU trouble; try again later.
    Transient(String),
}

/// Kinds that get a picture (everything else shows an icon).
pub fn supported(kind: Kind) -> bool {
    matches!(kind, Kind::Stl | Kind::ThreeMf | Kind::Step | Kind::Image | Kind::Pdf)
}

/// Builds the thumbnail of `path` (whose contents are `bytes`; empty for PDFs on disk, which
/// poppler reads itself) at `size` pixels.
pub fn make(
    path: &Path,
    kind: Kind,
    bytes: &[u8],
    size: u32,
    render: impl FnOnce(Mesh) -> Result<RgbaImage, String>,
) -> Result<Thumbnail, ThumbError> {
    let bad = |e: loaders::LoadError| ThumbError::Permanent(e.to_string());
    let rendered = |mesh: Mesh, meta: ThumbMeta| -> Result<Thumbnail, ThumbError> {
        Ok(Thumbnail { image: render(mesh).map_err(ThumbError::Transient)?, meta })
    };
    match kind {
        Kind::Stl => {
            let mesh = loaders::stl::parse(bytes).map_err(bad)?;
            let meta = mesh_meta(&mesh, bytes);
            rendered(mesh, meta)
        }
        Kind::ThreeMf => {
            // The slicer's own plate picture is free and shows filament colours; render only without one.
            let parsed = loaders::threemf::parse(bytes);
            let meta = parsed.as_ref().map(|m| mesh_meta(&m.mesh, bytes)).unwrap_or_default();
            if let Some(image) = loaders::threemf::embedded_thumbnail(bytes, size) {
                return Ok(Thumbnail { image, meta: ThumbMeta { embedded: true, ..meta } });
            }
            rendered(parsed.map_err(bad)?.mesh, meta)
        }
        Kind::Step => {
            // Tessellated by the OpenCascade helper (cached); the bytes are only hashed.
            let mesh = loaders::step::load(path).map_err(bad)?;
            let meta = mesh_meta(&mesh, bytes);
            rendered(mesh, meta)
        }
        Kind::Pdf => {
            let image = if bytes.is_empty() {
                loaders::pdf::thumbnail_from_path(path, size)
            } else {
                loaders::pdf::thumbnail_from_bytes(bytes, size)
            };
            Ok(Thumbnail { image: image.map_err(bad)?, meta: ThumbMeta::default() })
        }
        Kind::Image => {
            Ok(Thumbnail { image: loaders::image::thumbnail(bytes, size).map_err(bad)?, meta: ThumbMeta::default() })
        }
        other => Err(ThumbError::Permanent(format!("no thumbnail support for {other:?}"))),
    }
}

fn mesh_meta(mesh: &Mesh, bytes: &[u8]) -> ThumbMeta {
    ThumbMeta {
        triangles: Some(mesh.triangle_count()),
        size_mm: mesh.bounds().map(|b| b.size().to_array()),
        content_hash: (!bytes.is_empty()).then(|| dedupe::content_hash(bytes)),
        geo_sig: dedupe::geo_signature(mesh),
        embedded: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stl_goes_through_the_renderer_with_facts() {
        let bytes = loaders::stl::write_binary(&Mesh::cube(20.0), "x");
        let thumb = make(Path::new("/x/cube.stl"), Kind::Stl, &bytes, 64, |mesh| {
            assert_eq!(mesh.triangle_count(), 12);
            Ok(RgbaImage::new(64, 64))
        })
        .unwrap();
        assert_eq!(thumb.meta.triangles, Some(12));
        assert_eq!(thumb.meta.size_mm, Some([20.0; 3]));
        assert!(thumb.meta.content_hash.is_some() && thumb.meta.geo_sig.is_some());
    }

    #[test]
    fn errors_are_classified() {
        let broken = make(Path::new("/x/a.stl"), Kind::Stl, b"\x00\x01", 64, |_| unreachable!());
        assert!(matches!(broken, Err(ThumbError::Permanent(_))));
        let bytes = loaders::stl::write_binary(&Mesh::cube(1.0), "x");
        let gpu_down = make(Path::new("/x/a.stl"), Kind::Stl, &bytes, 64, |_| Err("no GPU".into()));
        assert!(matches!(gpu_down, Err(ThumbError::Transient(_))));
        assert!(!supported(Kind::Archive) && supported(Kind::Step));
    }
}
