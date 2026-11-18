//! The newest release as GitHub's API describes it, and what Meshgrove takes from it: the version,
//! the notes, and the packages with their download links.

use semver::Version;
use serde_json::Value;

/// Where a package may come from: this repository's release downloads (GitHub forwards them to
/// its own storage, over HTTPS).
pub const DOWNLOADS: &str = "https://github.com/DonDavis-vibe/MeshGrove/releases/download/";
/// Where a test feed's packages may come from instead.
pub const LOCAL_DOWNLOADS: &str = "http://127.0.0.1:";
/// The file of every release with the SHA-256 of each package.
pub const SUMS: &str = "SHA256SUMS.txt";

#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    /// In bytes, as GitHub says; 0 when it does not.
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: Version,
    /// Its notes (Markdown), cut to a size a window shows.
    pub notes: String,
    /// The release's page on GitHub.
    pub page: String,
    pub assets: Vec<Asset>,
}

impl Release {
    /// Reads `releases/latest`. Drafts and pre-releases are no release to offer; a package whose
    /// link points anywhere but this repository's downloads is left out (with `local`, links to
    /// 127.0.0.1 are taken too: a test feed).
    pub fn parse(json: &str, local: bool) -> Result<Release, String> {
        let v: Value = serde_json::from_str(json).map_err(|e| format!("not JSON: {e}"))?;
        if v["draft"].as_bool().unwrap_or(false) || v["prerelease"].as_bool().unwrap_or(false) {
            return Err("not a final release".into());
        }
        let tag = v["tag_name"].as_str().ok_or("no tag")?;
        let version = parse_version(tag).ok_or_else(|| format!("no version: {tag}"))?;
        let mut notes = v["body"].as_str().unwrap_or_default().to_owned();
        if notes.len() > 64 * 1024 {
            let mut end = 64 * 1024;
            while !notes.is_char_boundary(end) {
                end -= 1;
            }
            notes.truncate(end);
        }
        let page =
            v["html_url"].as_str().filter(|u| u.starts_with("https://github.com/")).unwrap_or_default().to_owned();
        let allowed = |url: &str| url.starts_with(DOWNLOADS) || (local && url.starts_with(LOCAL_DOWNLOADS));
        let assets = v["assets"]
            .as_array()
            .map(|list| {
                list.iter()
                    .take(400)
                    .filter_map(|a| {
                        let name = a["name"].as_str()?;
                        let url = a["browser_download_url"].as_str().filter(|u| allowed(u))?;
                        let plain = !name.is_empty() && !name.contains(['/', '\\']) && name != "." && name != "..";
                        plain.then(|| Asset {
                            name: name.to_owned(),
                            url: url.to_owned(),
                            size: a["size"].as_u64().unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Release { version, notes, page, assets })
    }

    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

/// The notes as the update window shows them: the highlights, without the pictures and HTML of
/// the release page, and without the downloads and checksums below them.
pub fn readable_notes(notes: &str) -> String {
    // HTML comments (the template's note to the editor) may span lines.
    let mut text = String::with_capacity(notes.len());
    let mut rest = notes;
    while let Some(start) = rest.find("<!--") {
        text.push_str(&rest[..start]);
        rest = rest[start..].find("-->").map_or("", |end| &rest[start + end + 3..]);
    }
    text.push_str(rest);
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("## Downloads") || trimmed.starts_with("## Checksums") {
            break;
        }
        if trimmed.starts_with('<') || trimmed.starts_with("![") {
            continue;
        }
        out.push(line);
    }
    out.join("\n").trim().to_owned()
}

/// "v0.13.0" or "0.13.0".
pub fn parse_version(text: &str) -> Option<Version> {
    Version::parse(text.trim().trim_start_matches('v')).ok()
}

/// The version this program is.
pub fn current() -> Version {
    parse_version(env!("CARGO_PKG_VERSION")).unwrap_or_else(|| Version::new(0, 0, 0))
}

/// Whether `latest` is worth offering over `installed`: a final release (no pre-release part)
/// that comes after it. A release candidate is offered its final release; build metadata does
/// not count.
pub fn is_newer(latest: &Version, installed: &Version) -> bool {
    let plain = |v: &Version| Version { build: semver::BuildMetadata::EMPTY, ..v.clone() };
    latest.pre.is_empty() && plain(latest) > plain(installed)
}

/// The SHA-256 (lowercase hex) `sums` gives for `name`. Lines read `<hex>  <name>` or
/// `<hex> *<name>` (what `sha256sum` writes, in text or binary mode).
pub fn expected_sha256(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.trim_end().split_once(' ')?;
        let file = file.trim_start().trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn latest(tag: &str, assets: Value) -> String {
        json!({
            "tag_name": tag, "draft": false, "prerelease": false, "body": "## New\n- a",
            "html_url": "https://github.com/DonDavis-vibe/MeshGrove/releases/tag/v0.13.0", "assets": assets,
        })
        .to_string()
    }

    #[test]
    fn reads_the_release_and_keeps_only_this_repositorys_packages() {
        let assets = json!([
            {"name": "Meshgrove-0.13.0-x86_64.AppImage", "size": 10, "browser_download_url": format!("{DOWNLOADS}v0.13.0/Meshgrove-0.13.0-x86_64.AppImage")},
            {"name": "evil.AppImage", "browser_download_url": "https://example.com/evil.AppImage"},
            {"name": "../escape", "browser_download_url": format!("{DOWNLOADS}v0.13.0/escape")},
            {"name": "local.zip", "browser_download_url": "http://127.0.0.1:8000/local.zip"},
        ]);
        let r = Release::parse(&latest("v0.13.0", assets.clone()), false).unwrap();
        assert_eq!(r.version, Version::new(0, 13, 0));
        assert_eq!(r.notes, "## New\n- a");
        assert_eq!(r.assets.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["Meshgrove-0.13.0-x86_64.AppImage"]);
        assert_eq!(r.assets[0].size, 10);
        // A test feed may point at 127.0.0.1.
        let r = Release::parse(&latest("v0.13.0", assets), true).unwrap();
        assert!(r.asset("local.zip").is_some());
    }

    #[test]
    fn drafts_and_pre_releases_are_no_offer() {
        let mut v: Value = serde_json::from_str(&latest("v0.13.0", json!([]))).unwrap();
        v["prerelease"] = json!(true);
        assert!(Release::parse(&v.to_string(), false).is_err());
        v["prerelease"] = json!(false);
        v["draft"] = json!(true);
        assert!(Release::parse(&v.to_string(), false).is_err());
        assert!(Release::parse(&latest("nightly", json!([])), false).is_err());
        assert!(Release::parse("<html>", false).is_err());
    }

    #[test]
    fn only_a_later_final_release_is_newer() {
        let v = |s| parse_version(s).unwrap();
        assert!(is_newer(&v("0.13.0"), &v("0.12.0")));
        assert!(is_newer(&v("v1.0.0"), &v("0.12.3")));
        assert!(!is_newer(&v("0.12.0"), &v("0.12.0")));
        assert!(!is_newer(&v("0.11.0"), &v("0.12.0")));
        assert!(!is_newer(&v("0.13.0-rc.1"), &v("0.12.0")));
        // A release candidate is offered its final release.
        assert!(is_newer(&v("0.13.0"), &v("0.13.0-rc.1")));
        assert!(!is_newer(&v("0.13.0+abc"), &v("0.13.0")));
    }

    #[test]
    fn the_window_shows_the_highlights_only() {
        let body = "<!-- a note\n     over two lines -->\n**Meshgrove 0.13.0**\n\n![Meshgrove](https://x/hero.png)\n## Highlights\n<img src=\"a\">\n- updates <!-- x --> here\n\n## Downloads\n| a | b |\n## Checksums (SHA-256)\nabc";
        assert_eq!(readable_notes(body), "**Meshgrove 0.13.0**\n\n## Highlights\n- updates  here");
        assert_eq!(readable_notes("text <!-- never closed"), "text");
    }

    #[test]
    fn finds_a_packages_checksum() {
        let hash = "a".repeat(64);
        let sums =
            format!("{hash}  Meshgrove-0.13.0-x86_64.AppImage\n{}  *meshgrove.zip\nbroken line\n", "B".repeat(64));
        assert_eq!(expected_sha256(&sums, "Meshgrove-0.13.0-x86_64.AppImage"), Some(hash));
        assert_eq!(expected_sha256(&sums, "meshgrove.zip"), Some("b".repeat(64)));
        assert_eq!(expected_sha256(&sums, "other"), None);
        assert_eq!(expected_sha256("abc  short\n", "short"), None);
    }
}
