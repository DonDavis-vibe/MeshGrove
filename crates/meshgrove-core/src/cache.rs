//! On-disk thumbnail cache. Each thumbnail is a PNG whose text chunks carry the model
//! facts (triangles, size, hashes), so a cache hit needs no model parsing at all.
//! Failed files get a small `.fail` marker so they are not retried until they change.

use std::io::{self, Cursor, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use image::RgbaImage;

use base64::Engine;

use crate::config::{GltfUnit, ModelUnits};
use crate::loaders::gcode::GcodeFacts;

/// Bump whenever thumbnails would look different (camera, lighting, size), to invalidate the cache.
pub const RENDER_VERSION: u32 = 1;
/// 3MF previews gained colours and filament facts in version 2; other kinds kept their previews.
const RENDER_VERSION_3MF: u32 = 2;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThumbMeta {
    pub triangles: Option<usize>,
    pub size_mm: Option<[f32; 3]>,
    pub content_hash: Option<String>,
    pub geo_sig: Option<String>,
    /// The picture is the preview a slicer/CAD program stored in the file, not our own render.
    pub embedded: bool,
    /// Filament colours (sRGB) a slicer project prints with.
    pub filaments: Vec<[u8; 3]>,
    /// What a G-code file's comments say about the print.
    pub gcode: Option<Box<GcodeFacts>>,
    /// Printability check of the mesh, when it was small enough to check with the thumbnail.
    pub checks: Option<Box<crate::checks::MeshChecks>>,
}

#[derive(Debug, Clone)]
pub struct Thumbnail {
    pub image: RgbaImage,
    pub meta: ThumbMeta,
}

#[derive(Debug, Clone)]
pub struct ThumbCache {
    root: PathBuf,
}

impl ThumbCache {
    pub fn new(root: impl Into<PathBuf>) -> ThumbCache {
        ThumbCache { root: root.into() }
    }

    /// `~/.cache/meshgrove/thumbs`
    pub fn default_location() -> Option<PathBuf> {
        Some(dirs::cache_dir()?.join("meshgrove").join("thumbs"))
    }

    /// Identity of one rendered thumbnail: changes when the file (or an archive entry in it) changes.
    pub fn key(path: &Path, size: u64, modified: Option<SystemTime>, entry: Option<&str>) -> String {
        Self::key_in(path, size, modified, entry, (crate::config::gltf_unit(), crate::config::model_units()))
    }

    fn key_in(
        path: &Path,
        size: u64,
        modified: Option<SystemTime>,
        entry: Option<&str>,
        (gltf, model): (GltfUnit, ModelUnits),
    ) -> String {
        let nanos = modified.and_then(|m| m.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
        let mut h = blake3::Hasher::new();
        // An archive member's own name decides, not the archive's.
        let name = entry.map(Path::new).unwrap_or(path);
        let ext = name.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
        let version = match ext.as_deref() {
            Some("3mf") => RENDER_VERSION_3MF.to_string(),
            // glTF sizes follow the unit setting; metres keep the plain version.
            Some("glb" | "gltf") if gltf == GltfUnit::Millimetres => format!("{RENDER_VERSION}mm"),
            // STL, OBJ and PLY sizes follow the unit of the files.
            Some("stl" | "obj" | "ply") if model == ModelUnits::Inches => format!("{RENDER_VERSION}in"),
            _ => RENDER_VERSION.to_string(),
        };
        h.update(format!("{version}\0{}\0{size}\0{nanos}\0{}", path.display(), entry.unwrap_or("")).as_bytes());
        h.finalize().to_hex()[..32].to_string()
    }

    fn file(&self, key: &str, ext: &str) -> PathBuf {
        self.root.join(&key[..2]).join(format!("{key}.{ext}"))
    }

    /// `None`: not cached yet. `Some(Err)`: an earlier attempt failed with this message.
    pub fn load(&self, key: &str) -> Option<Result<Thumbnail, String>> {
        match std::fs::read(self.file(key, "png")) {
            Ok(bytes) => match decode(&bytes) {
                Ok(thumb) => Some(Ok(thumb)),
                Err(_) => None, // corrupt cache entry: regenerate
            },
            Err(_) => std::fs::read_to_string(self.file(key, "fail")).ok().map(Err),
        }
    }

    pub fn store(&self, key: &str, thumb: &Thumbnail) -> io::Result<()> {
        write_atomic(&self.file(key, "png"), &encode(thumb)?)
    }

    /// The thumbnail (or failure) stored under `old` from now on under `new`: for moved files.
    pub fn rekey(&self, old: &str, new: &str) {
        for ext in ["png", "fail"] {
            let (from, to) = (self.file(old, ext), self.file(new, ext));
            if from.exists() {
                if let Some(dir) = to.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::rename(&from, &to);
            }
        }
    }

    pub fn store_failure(&self, key: &str, message: &str) -> io::Result<()> {
        write_atomic(&self.file(key, "fail"), message.as_bytes())
    }
}

/// Writes `bytes` to a temporary file next to `path` and renames it over `path`, so readers in
/// other processes see the old or the new contents, never half a file.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| io::Error::other("no parent folder"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::File::create(&tmp)?.write_all(bytes)?;
    std::fs::rename(&tmp, path)
}

// The "stlb" prefix comes from the project's former name; kept so existing caches stay valid.
const KEY_TRIANGLES: &str = "stlb:triangles";
const KEY_SIZE: &str = "stlb:size_mm";
const KEY_CONTENT: &str = "stlb:content_hash";
const KEY_GEO: &str = "stlb:geo_sig";
const KEY_SOURCE: &str = "stlb:source";
const KEY_FILAMENTS: &str = "stlb:filaments";
/// JSON in base64, since text chunks only hold Latin-1 and printer names may not.
const KEY_GCODE: &str = "stlb:gcode";
/// [`crate::checks::MeshChecks`], JSON in base64 like the G-code facts.
const KEY_CHECKS: &str = "stlb:checks";

fn encode(thumb: &Thumbnail) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, thumb.image.width(), thumb.image.height());
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    let m = &thumb.meta;
    let texts = [
        (KEY_TRIANGLES, m.triangles.map(|t| t.to_string())),
        (KEY_SIZE, m.size_mm.map(|[x, y, z]| format!("{x} {y} {z}"))),
        (KEY_CONTENT, m.content_hash.clone()),
        (KEY_GEO, m.geo_sig.clone()),
        (KEY_SOURCE, m.embedded.then(|| "embedded".to_owned())),
        (
            KEY_FILAMENTS,
            (!m.filaments.is_empty()).then(|| {
                m.filaments.iter().map(|[r, g, b]| format!("{r:02X}{g:02X}{b:02X}")).collect::<Vec<_>>().join(" ")
            }),
        ),
        (
            KEY_GCODE,
            m.gcode
                .as_ref()
                .and_then(|g| serde_json::to_vec(g).ok())
                .map(|json| base64::engine::general_purpose::STANDARD.encode(json)),
        ),
        (
            KEY_CHECKS,
            m.checks
                .as_ref()
                .and_then(|c| serde_json::to_vec(c).ok())
                .map(|json| base64::engine::general_purpose::STANDARD.encode(json)),
        ),
    ];
    for (k, v) in texts {
        if let Some(v) = v {
            enc.add_text_chunk(k.into(), v).map_err(io::Error::other)?;
        }
    }
    let mut writer = enc.write_header().map_err(io::Error::other)?;
    writer.write_image_data(thumb.image.as_raw()).map_err(io::Error::other)?;
    writer.finish().map_err(io::Error::other)?;
    Ok(out)
}

fn decode(bytes: &[u8]) -> Result<Thumbnail, png::DecodingError> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().unwrap_or(0)];
    let frame = reader.next_frame(&mut buf)?;
    if frame.color_type != png::ColorType::Rgba || frame.bit_depth != png::BitDepth::Eight {
        return Err(png::DecodingError::LimitsExceeded); // not one of ours
    }
    buf.truncate(frame.buffer_size());
    let mut meta = ThumbMeta::default();
    for chunk in &reader.info().uncompressed_latin1_text {
        let text = chunk.text.as_str();
        match chunk.keyword.as_str() {
            KEY_TRIANGLES => meta.triangles = text.parse().ok(),
            KEY_SIZE => {
                let v: Vec<f32> = text.split(' ').filter_map(|s| s.parse().ok()).collect();
                meta.size_mm = (v.len() == 3).then(|| [v[0], v[1], v[2]]);
            }
            KEY_CONTENT => meta.content_hash = Some(text.to_owned()),
            KEY_GEO => meta.geo_sig = Some(text.to_owned()),
            KEY_SOURCE => meta.embedded = text == "embedded",
            KEY_FILAMENTS => {
                meta.filaments = text
                    .split(' ')
                    .filter_map(|h| {
                        let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
                        Some([byte(0)?, byte(2)?, byte(4)?])
                    })
                    .collect();
            }
            KEY_GCODE => {
                meta.gcode = base64::engine::general_purpose::STANDARD
                    .decode(text)
                    .ok()
                    .and_then(|json| serde_json::from_slice(&json).ok())
                    .map(Box::new);
            }
            KEY_CHECKS => {
                meta.checks = base64::engine::general_purpose::STANDARD
                    .decode(text)
                    .ok()
                    .and_then(|json| serde_json::from_slice(&json).ok())
                    .map(Box::new);
            }
            _ => {}
        }
    }
    let image = RgbaImage::from_raw(frame.width, frame.height, buf).ok_or(png::DecodingError::LimitsExceeded)?;
    Ok(Thumbnail { image, meta })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn roundtrip_with_meta_and_failures() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ThumbCache::new(dir.path());
        let key = ThumbCache::key(Path::new("/x/a.stl"), 10, Some(SystemTime::UNIX_EPOCH), None);
        assert!(cache.load(&key).is_none());

        let mut image = RgbaImage::new(4, 3);
        image.put_pixel(1, 2, image::Rgba([10, 20, 30, 128]));
        let meta = ThumbMeta {
            triangles: Some(4_192_676),
            size_mm: Some([20.5, 10.0, 5.25]),
            content_hash: Some("abc".into()),
            geo_sig: Some("12|2.4e3|...".into()),
            embedded: true,
            filaments: vec![[255, 0, 0], [0, 128, 255]],
            gcode: Some(Box::new(GcodeFacts {
                printer: Some("Creality Ender-3 V3 SE – Düse 0.4".into()),
                filament_g: Some(31.52),
                materials: vec!["PLA".into(), "PETG".into()],
                layers: Some(10),
                ..Default::default()
            })),
            checks: Some(Box::new(crate::checks::check(&crate::Mesh::cube(10.0)))),
        };
        cache.store(&key, &Thumbnail { image: image.clone(), meta: meta.clone() }).unwrap();
        let back = cache.load(&key).unwrap().unwrap();
        assert_eq!(back.image, image);
        assert_eq!(back.meta, meta);

        let bad = ThumbCache::key(Path::new("/x/broken.stl"), 1, None, None);
        cache.store_failure(&bad, "file contains no triangles").unwrap();
        assert_eq!(cache.load(&bad).unwrap().unwrap_err(), "file contains no triangles");
    }

    #[test]
    fn key_changes_with_file_identity() {
        let p = Path::new("/lib/dragon.stl");
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let k = ThumbCache::key(p, 100, Some(t), None);
        assert_eq!(k, ThumbCache::key(p, 100, Some(t), None));
        assert_ne!(k, ThumbCache::key(p, 101, Some(t), None));
        assert_ne!(k, ThumbCache::key(p, 100, Some(t + Duration::from_nanos(100)), None));
        assert_ne!(k, ThumbCache::key(p, 100, Some(t), Some("inner.stl")));
        assert_ne!(k, ThumbCache::key(Path::new("/lib/other.stl"), 100, Some(t), None));
        // The glTF unit setting changes glTF facts, so their keys; other files keep theirs.
        let key = |p: &str, gltf, model| ThumbCache::key_in(Path::new(p), 100, Some(t), None, (gltf, model));
        let mm = ModelUnits::Millimetres;
        assert_ne!(key("/lib/a.glb", GltfUnit::Metres, mm), key("/lib/a.glb", GltfUnit::Millimetres, mm));
        assert_eq!(key("/lib/a.stl", GltfUnit::Metres, mm), key("/lib/a.stl", GltfUnit::Millimetres, mm));
        // The unit of STL, OBJ and PLY files changes their keys, and only theirs.
        let inch = ModelUnits::Inches;
        for ext in ["stl", "obj", "ply"] {
            let path = format!("/lib/a.{ext}");
            assert_ne!(key(&path, GltfUnit::Metres, mm), key(&path, GltfUnit::Metres, inch));
        }
        assert_eq!(key("/lib/a.glb", GltfUnit::Metres, mm), key("/lib/a.glb", GltfUnit::Metres, inch));
        assert_eq!(key("/lib/a.3mf", GltfUnit::Metres, mm), key("/lib/a.3mf", GltfUnit::Metres, inch));
    }
}
