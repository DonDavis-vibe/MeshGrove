//! The update check's two kinds of request: GitHub's description of the newest release (and the
//! release's checksum list), and a package downloaded to a file with its SHA-256 worked out on
//! the way. Nothing but this program's name goes with them: no version, no identifier.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// GitHub's description of the newest release.
pub const LATEST: &str = "https://api.github.com/repos/DonDavis-vibe/MeshGrove/releases/latest";
const USER_AGENT: &str = "Meshgrove-update-check";
/// No package is bigger than this (the largest is about 60 MB).
const MAX_PACKAGE: u64 = 1 << 30;

#[derive(Debug, Clone, PartialEq)]
pub enum FetchError {
    /// GitHub asked to wait this many seconds.
    RateLimited(u64),
    /// No usable answer (offline, a timeout, an error status), with what went wrong.
    Failed(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::RateLimited(s) => write!(f, "GitHub asks to wait {} minutes", s.div_ceil(60)),
            FetchError::Failed(m) => f.write_str(m),
        }
    }
}

/// How far a download is, and the way to stop it; shared with the window that shows it.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    /// 0 until the size is known.
    pub total: AtomicU64,
    pub cancel: AtomicBool,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 { 0.0 } else { self.done.load(Ordering::Relaxed) as f32 / total as f32 }
    }
}

pub struct Net {
    api: ureq::Agent,
    files: ureq::Agent,
}

impl Net {
    /// `local`: a test feed whose packages come from 127.0.0.1 over plain HTTP.
    pub fn new(local: bool) -> Net {
        Net {
            api: crate::assist::client::agent(Duration::from_secs(15), !local),
            files: crate::assist::client::agent(Duration::from_secs(60), !local),
        }
    }

    /// A small text (the release, the checksum list), at most `limit` bytes.
    pub fn text(&self, url: &str, limit: u64) -> Result<String, FetchError> {
        let mut request = self.api.get(url).header("User-Agent", USER_AGENT);
        if url.starts_with("https://api.github.com/") {
            request =
                request.header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28");
        }
        let mut response = request.call().map_err(|e| FetchError::Failed(e.to_string()))?;
        let status = response.status().as_u16();
        let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
        if let Some(wait) =
            rate_limit(status, header("retry-after"), header("x-ratelimit-remaining"), header("x-ratelimit-reset"))
        {
            return Err(FetchError::RateLimited(wait));
        }
        if status != 200 {
            return Err(FetchError::Failed(format!("HTTP {status}")));
        }
        response.body_mut().with_config().limit(limit).read_to_string().map_err(|e| FetchError::Failed(e.to_string()))
    }

    /// Downloads `url` into `to` (created anew) and gives the SHA-256 of what arrived (lowercase
    /// hex). `size` (when known) is what must arrive. On any failure, or when `progress` is
    /// cancelled, `to` is removed again.
    pub fn download(&self, url: &str, size: u64, to: &Path, progress: &Progress) -> Result<String, String> {
        let result = self.download_into(url, size, to, progress);
        if result.is_err() {
            let _ = std::fs::remove_file(to);
        }
        result
    }

    fn download_into(&self, url: &str, size: u64, to: &Path, progress: &Progress) -> Result<String, String> {
        let mut response = self.files.get(url).header("User-Agent", USER_AGENT).call().map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(format!("HTTP {status}"));
        }
        let announced = response.headers().get("content-length").and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
        let total = if size > 0 { size } else { announced.unwrap_or(0) };
        progress.total.store(total, Ordering::Relaxed);
        progress.done.store(0, Ordering::Relaxed);
        let mut file = std::fs::File::create(to).map_err(|e| format!("{}: {e}", to.display()))?;
        let mut reader = response.body_mut().as_reader();
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut done = 0u64;
        loop {
            if progress.cancel.load(Ordering::Relaxed) {
                return Err(CANCELLED.to_owned());
            }
            let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            done += n as u64;
            if done > MAX_PACKAGE || (size > 0 && done > size) {
                return Err("the download is larger than the release says".to_owned());
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n]).map_err(|e| format!("{}: {e}", to.display()))?;
            progress.done.store(done, Ordering::Relaxed);
        }
        if size > 0 && done != size {
            return Err(format!("the download stopped after {done} of {size} bytes"));
        }
        file.sync_all().map_err(|e| e.to_string())?;
        Ok(hex(&hasher.finalize()))
    }
}

/// What a cancelled download's error says.
pub const CANCELLED: &str = "cancelled";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The SHA-256 of a file (lowercase hex).
#[cfg(test)]
fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hex(&hasher.finalize()))
}

/// GitHub's way of saying "too many requests": 429, or 403 with no requests left (or a
/// `retry-after`). How long to wait: `retry-after`, else until `x-ratelimit-reset`, else an hour.
fn rate_limit(
    status: u16,
    retry_after: Option<String>,
    remaining: Option<String>,
    reset: Option<String>,
) -> Option<u64> {
    let retry_after = retry_after.and_then(|v| v.trim().parse::<u64>().ok());
    let exhausted = remaining.as_deref().map(str::trim) == Some("0");
    if !(status == 429 || (status == 403 && (exhausted || retry_after.is_some()))) {
        return None;
    }
    let until_reset = reset.and_then(|v| v.trim().parse::<u64>().ok()).map(|at| at.saturating_sub(super::state::now()));
    Some(retry_after.or(until_reset).unwrap_or(super::state::HOUR))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// One answer of [`serve`]: status, extra headers, body.
    pub type Answer = (u16, Vec<(&'static str, String)>, Vec<u8>);

    /// A tiny HTTP server answering each request with the next of `answers`, on 127.0.0.1.
    /// Gives its address and the requests it saw.
    pub fn serve(answers: Vec<Answer>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for (status, headers, body) in answers {
                let Ok((mut stream, _)) = listener.accept() else { return };
                let mut request = Vec::new();
                let mut buf = [0u8; 4096];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..n]);
                }
                log.lock().unwrap().push(String::from_utf8_lossy(&request).to_string());
                let mut head =
                    format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
                for (name, value) in headers {
                    head.push_str(&format!("{name}: {value}\r\n"));
                }
                head.push_str("\r\n");
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        (url, seen)
    }

    #[test]
    fn asks_with_this_programs_name_only() {
        let (url, seen) = serve(vec![(200, vec![], b"{\"tag_name\":\"v1\"}".to_vec())]);
        assert_eq!(Net::new(true).text(&format!("{url}/latest"), 1 << 20).unwrap(), "{\"tag_name\":\"v1\"}");
        let request = seen.lock().unwrap()[0].to_lowercase();
        assert!(request.contains("user-agent: meshgrove-update-check"), "{request}");
        assert!(!request.contains("cookie") && !request.contains("authorization"), "{request}");
    }

    #[test]
    fn hears_githubs_rate_limit() {
        let reset = (super::super::state::now() + 600).to_string();
        let (url, _) = serve(vec![
            (403, vec![("x-ratelimit-remaining", "0".into()), ("x-ratelimit-reset", reset)], b"{}".to_vec()),
            (429, vec![("retry-after", "120".into())], vec![]),
            (403, vec![], vec![]),
            (500, vec![], vec![]),
        ]);
        let net = Net::new(true);
        match net.text(&url, 1024) {
            Err(FetchError::RateLimited(s)) => assert!((590..=600).contains(&s), "{s}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(net.text(&url, 1024), Err(FetchError::RateLimited(120)));
        assert_eq!(net.text(&url, 1024), Err(FetchError::Failed("HTTP 403".into())));
        assert_eq!(net.text(&url, 1024), Err(FetchError::Failed("HTTP 500".into())));
    }

    #[test]
    fn too_long_an_answer_is_refused() {
        let (url, _) = serve(vec![(200, vec![], vec![b'x'; 5000])]);
        assert!(matches!(Net::new(true).text(&url, 1000), Err(FetchError::Failed(_))));
    }

    #[test]
    fn downloads_with_its_checksum_and_cleans_up_a_short_one() {
        let dir = tempfile::tempdir().unwrap();
        let body = b"new program".to_vec();
        let (url, _) =
            serve(vec![(200, vec![], body.clone()), (200, vec![], b"short".to_vec()), (404, vec![], vec![])]);
        let (net, progress) = (Net::new(true), Progress::default());
        let to = dir.path().join("a.update");
        let hash = net.download(&url, body.len() as u64, &to, &progress).unwrap();
        assert_eq!(hash, sha256_file(&to).unwrap());
        assert_eq!(std::fs::read(&to).unwrap(), body);
        assert_eq!(progress.fraction(), 1.0);
        let to = dir.path().join("b.update");
        assert!(net.download(&url, 100, &to, &progress).is_err());
        assert!(!to.exists(), "a download that stopped short is removed");
        assert_eq!(net.download(&url, 0, &to, &progress), Err("HTTP 404".into()));
        assert!(!to.exists());
    }

    /// The one real request, by hand: `cargo test -p meshgrove real_github -- --ignored`.
    #[test]
    #[ignore = "asks GitHub"]
    fn real_github_answers_with_a_release() {
        let text = Net::new(false).text(LATEST, 1 << 20).unwrap();
        let latest = crate::update::release::Release::parse(&text, false).unwrap();
        assert!(latest.version >= semver::Version::new(0, 12, 0), "{}", latest.version);
        assert!(latest.page.starts_with("https://github.com/DonDavis-vibe/MeshGrove/releases/tag/"), "{}", latest.page);
        assert!(!latest.assets.is_empty());
        eprintln!("latest: {} with {} packages", latest.version, latest.assets.len());
    }

    #[test]
    fn production_requests_go_over_https_only() {
        let (url, _) = serve(vec![(200, vec![], b"x".to_vec())]);
        assert!(Net::new(false).text(&url, 1024).is_err());
    }
}
