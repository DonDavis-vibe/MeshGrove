//! The text of a selected readme, licence or note for the preview pane, and the readme of a
//! selected folder for the info pane; read in the background whenever the selection changes.

use std::io::Read;
use std::path::{Path, PathBuf};

use crossbeam_channel::{Receiver, bounded};
use meshgrove_core::listing::{self, Entry};
use meshgrove_core::loaders::text;
use meshgrove_core::{Kind, archive};

/// Plenty for a readme; of longer files the start is shown.
const MAX_BYTES: usize = 512 * 1024;

pub struct Text {
    pub path: PathBuf,
    /// Markdown already prepared for display (see [`text::display_markdown`]).
    pub body: String,
    pub markdown: bool,
    /// The file is longer than what is shown.
    pub truncated: bool,
}

#[derive(Default)]
pub enum TextState {
    /// Nothing to show: no text selected, or a folder without a readme.
    #[default]
    Idle,
    Loading,
    Ready(Text),
    Failed(String),
}

#[derive(Default)]
pub struct TextPreview {
    /// The text file or folder the state belongs to.
    target: Option<PathBuf>,
    pub state: TextState,
    rx: Option<Receiver<Result<Option<Text>, String>>>,
    pub cache: egui_commonmark::CommonMarkCache,
}

impl TextPreview {
    /// Follows the selection: a text file shows itself, a folder its readme (or licence).
    pub fn follow(&mut self, entry: Option<&Entry>, ctx: &egui::Context) {
        let wanted = entry.filter(|e| e.kind == Kind::Text || e.kind == Kind::Folder);
        let target = wanted.map(|e| e.path.clone());
        if target != self.target {
            self.target = target;
            self.state = TextState::Idle;
            self.rx = None;
            if let Some(entry) = wanted {
                let (path, folder) = (entry.path.clone(), entry.kind == Kind::Folder);
                let (tx, rx) = bounded(1);
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let result = if folder { readme_in(&path) } else { read_text(&path).map(Some) };
                    let _ = tx.send(result);
                    ctx.request_repaint();
                });
                self.rx = Some(rx);
                // A folder without a readme shows nothing, not a spinner.
                self.state = if folder { TextState::Idle } else { TextState::Loading };
            }
        }
        if let Some(result) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.rx = None;
            self.state = match result {
                Ok(Some(text)) => TextState::Ready(text),
                Ok(None) => TextState::Idle,
                Err(e) => TextState::Failed(e),
            };
        }
    }
}

fn read_text(path: &Path) -> Result<Text, String> {
    let mut bytes = if archive::split(path).is_some() {
        archive::read(path).map_err(|e| e.to_string())?
    } else {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .and_then(|f| f.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes))
            .map_err(|e| e.to_string())?;
        bytes
    };
    let truncated = bytes.len() > MAX_BYTES;
    bytes.truncate(MAX_BYTES);
    let markdown = text::is_markdown(&listing::file_name(path));
    let body = text::decode(&bytes);
    let body = if markdown { text::display_markdown(&body) } else { body };
    Ok(Text { path: path.to_path_buf(), body, markdown, truncated })
}

/// The folder's readme, or else its licence.
fn readme_in(dir: &Path) -> Result<Option<Text>, String> {
    let entries = listing::list_dir(dir).map_err(|e| e.to_string())?;
    let best = entries
        .iter()
        .filter(|e| e.kind == Kind::Text)
        .filter_map(|e| Some((rank(&e.name)?, e)))
        .min_by(|(ra, a), (rb, b)| ra.cmp(rb).then(a.name.len().cmp(&b.name.len())).then(a.name.cmp(&b.name)));
    best.map(|(_, e)| read_text(&e.path)).transpose()
}

/// Readmes first, then licences; other texts are no folder description.
fn rank(name: &str) -> Option<u8> {
    let lower = name.to_lowercase();
    if lower.starts_with("readme") || lower.starts_with("liesmich") {
        Some(0)
    } else if lower.starts_with("license") || lower.starts_with("licence") || lower.starts_with("copying") {
        Some(1)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_shows_its_readme_before_its_licence() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("LICENSE.txt"), "CC BY").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
        assert_eq!(readme_in(dir.path()).unwrap().unwrap().body, "CC BY");
        std::fs::write(dir.path().join("README.md"), "# Lamp\n<br>").unwrap();
        let readme = readme_in(dir.path()).unwrap().unwrap();
        assert_eq!((readme.body.as_str(), readme.markdown), ("# Lamp\n", true));
        let empty = tempfile::tempdir().unwrap();
        assert!(readme_in(empty.path()).unwrap().is_none());
    }

    #[test]
    fn long_files_show_their_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.txt");
        std::fs::write(&path, "a".repeat(MAX_BYTES + 10)).unwrap();
        let text = read_text(&path).unwrap();
        assert!(text.truncated && text.body.len() == MAX_BYTES);
    }
}
