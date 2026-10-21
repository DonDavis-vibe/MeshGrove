//! Freedesktop thumbnailer: `meshgrove-thumbnailer -s SIZE INPUT OUTPUT`
//! (registered through `meshgrove.thumbnailer`, so Dolphin and file dialogs show the same
//! previews as the browser). The browser's thumbnail cache is used — and filled — first.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use image::imageops::FilterType;
use meshgrove_core::cache::ThumbCache;
use meshgrove_core::listing;
use meshgrove_core::thumbnail::{self, ThumbError};
use meshgrove_core::{Kind, archive};
use meshgrove_render::{HeadlessRenderer, ViewParams};

/// Same size the browser renders at, so both share cache entries.
const CACHE_PX: u32 = 384;

fn main() -> ExitCode {
    // Dolphin may ask for previews before the app has run once after the rename.
    meshgrove_core::config::migrate_legacy_dirs();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (size, input, output) = match args.as_slice() {
        [flag, size, input, output] if flag == "-s" => (size.parse().unwrap_or(256), input, output),
        [input, output] => (256, input, output),
        _ => {
            eprintln!("usage: meshgrove-thumbnailer -s SIZE INPUT OUTPUT");
            return ExitCode::from(2);
        }
    };
    match run(&path_from_arg(input), Path::new(output), size) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("meshgrove-thumbnailer: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Thumbnailers may be handed `file://` URIs instead of paths.
fn path_from_arg(arg: &str) -> PathBuf {
    let Some(rest) = arg.strip_prefix("file://") else { return arg.into() };
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(v) = rest.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(out)
    };
    #[cfg(not(unix))]
    let path = String::from_utf8_lossy(&out).into_owned();
    PathBuf::from(path)
}

fn run(input: &Path, output: &Path, size: u32) -> Result<(), String> {
    let kind = Kind::from_file_name(&listing::file_name(input));
    if !thumbnail::supported(kind) {
        return Err(format!("unsupported file type {kind:?}"));
    }
    let meta = std::fs::metadata(input).map_err(|e| e.to_string())?;
    let key = ThumbCache::key(input, meta.len(), meta.modified().ok(), None);
    let cache = ThumbCache::default_location().map(ThumbCache::new);

    let image = match cache.as_ref().and_then(|c| c.load(&key)) {
        Some(Ok(thumb)) => thumb.image,
        Some(Err(e)) => return Err(e),
        None => {
            let pdf_on_disk = kind == Kind::Pdf;
            let bytes = if pdf_on_disk { Vec::new() } else { archive::read(input).map_err(|e| e.to_string())? };
            let render = |mesh| {
                let gpu = HeadlessRenderer::new().map_err(|e| e.to_string())?;
                gpu.render(&mesh, CACHE_PX, &ViewParams::default()).map_err(|e| e.to_string())
            };
            match thumbnail::make(input, kind, &bytes, CACHE_PX, render) {
                Ok(thumb) => {
                    if let Some(cache) = &cache {
                        let _ = cache.store(&key, &thumb);
                    }
                    thumb.image
                }
                Err(ThumbError::Permanent(e)) => {
                    if let Some(cache) = &cache {
                        let _ = cache.store_failure(&key, &e);
                    }
                    return Err(e);
                }
                Err(ThumbError::Transient(e)) => return Err(e),
            }
        }
    };
    let scaled = if image.width().max(image.height()) > size {
        let scale = size as f32 / image.width().max(image.height()) as f32;
        let (w, h) = (
            (image.width() as f32 * scale).round().max(1.0) as u32,
            (image.height() as f32 * scale).round().max(1.0) as u32,
        );
        image::imageops::resize(&image, w, h, FilterType::Triangle)
    } else {
        image
    };
    // Freedesktop thumbnails are PNG; the caller picks the output name.
    scaled.save_with_format(output, image::ImageFormat::Png).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_file_uris() {
        assert_eq!(path_from_arg("file:///a%20b/Gr%C3%B6%C3%9Fe.stl"), PathBuf::from("/a b/Größe.stl"));
        assert_eq!(path_from_arg("/plain/path.stl"), PathBuf::from("/plain/path.stl"));
    }
}
