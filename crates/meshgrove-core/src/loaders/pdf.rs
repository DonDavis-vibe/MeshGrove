//! First-page previews of PDFs (build instructions, rule books) via poppler's `pdftoppm`.

use std::io::{Read, Write};
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use image::RgbaImage;

use super::LoadError;

const TIMEOUT: Duration = Duration::from_secs(15);

/// From a file on disk: poppler only reads what the first page needs (fast even for 600 MB PDFs).
pub fn thumbnail_from_path(path: &Path, max: u32) -> Result<RgbaImage, LoadError> {
    render(path.as_os_str(), None, max)
}

/// From bytes (e.g. an archive member), piped through stdin.
pub fn thumbnail_from_bytes(bytes: &[u8], max: u32) -> Result<RgbaImage, LoadError> {
    render("-".as_ref(), Some(bytes), max)
}

fn render(input: &std::ffi::OsStr, stdin: Option<&[u8]>, max: u32) -> Result<RgbaImage, LoadError> {
    let mut child = crate::process::helper(crate::process::exe_name("pdftoppm"))
        .args(["-f", "1", "-l", "1", "-singlefile", "-png", "-scale-to", &max.to_string()])
        .arg(input)
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| LoadError::Malformed(format!("pdftoppm not available: {e}")))?;
    // Feed and drain on threads: a full pipe on either side would otherwise block forever.
    let feeder = stdin.map(|bytes| {
        let mut pipe = child.stdin.take().expect("piped stdin");
        let bytes = bytes.to_vec();
        std::thread::spawn(move || {
            let _ = pipe.write_all(&bytes);
        })
    });
    let mut stdout = child.stdout.take().expect("piped stdout");
    let reader = std::thread::spawn(move || {
        let mut png = Vec::new();
        let _ = stdout.read_to_end(&mut png);
        png
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(LoadError::Malformed("pdftoppm timed out".into()));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let png = reader.join().unwrap_or_default();
    if let Some(feeder) = feeder {
        let _ = feeder.join();
    }
    if !status.success() || png.is_empty() {
        return Err(LoadError::Malformed("PDF could not be rendered".into()));
    }
    Ok(image::load_from_memory(&png)?.to_rgba8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_pdfs() {
        // Works whether or not poppler is installed: either way this is an error.
        assert!(thumbnail_from_bytes(b"definitely not a pdf", 64).is_err());
    }
}
