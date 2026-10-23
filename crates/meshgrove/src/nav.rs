//! Current folder, back/forward history and asynchronous directory listing
//! (a sleeping HDD can take seconds to answer; the UI must not freeze meanwhile).

use std::path::{Path, PathBuf};

use crossbeam_channel::{Receiver, bounded};
use meshgrove_core::listing::{self, Entry};

type ListingResult = (PathBuf, Result<Vec<Entry>, String>);

pub enum Listing {
    Loading,
    Ready(Vec<Entry>),
    Error(String),
}

pub struct Navigator {
    current: PathBuf,
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
    pub listing: Listing,
    pending: Option<Receiver<ListingResult>>,
    /// The listing being read (or read last) is a refresh of the same folder, not a navigation.
    refreshing: bool,
    ctx: egui::Context,
}

impl Navigator {
    pub fn new(ctx: egui::Context, start: PathBuf) -> Navigator {
        let mut nav = Navigator {
            current: start,
            back: Vec::new(),
            forward: Vec::new(),
            listing: Listing::Loading,
            pending: None,
            refreshing: false,
            ctx,
        };
        nav.reload();
        nav
    }

    pub fn current(&self) -> &Path {
        &self.current
    }

    pub fn open(&mut self, path: PathBuf) {
        if path != self.current {
            self.back.push(std::mem::replace(&mut self.current, path));
            self.forward.clear();
            self.reload();
        }
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn can_go_up(&self) -> bool {
        self.current.parent().is_some()
    }

    pub fn back(&mut self) {
        if let Some(prev) = self.back.pop() {
            self.forward.push(std::mem::replace(&mut self.current, prev));
            self.reload();
        }
    }

    pub fn forward(&mut self) {
        if let Some(next) = self.forward.pop() {
            self.back.push(std::mem::replace(&mut self.current, next));
            self.reload();
        }
    }

    pub fn up(&mut self) {
        if let Some(parent) = self.current.parent().map(Path::to_path_buf) {
            self.open(parent);
        }
    }

    /// Reads the current folder again in the background, for a view that shows something else
    /// meanwhile (duplicates, a list): see [`Navigator::refreshed`].
    pub fn refresh(&mut self) {
        self.reload();
        self.refreshing = true;
    }

    /// The listing that arrived last came from [`Navigator::refresh`], not from going somewhere.
    pub fn refreshed(&self) -> bool {
        self.refreshing
    }

    pub fn reload(&mut self) {
        self.refreshing = false;
        let (tx, rx) = bounded(1);
        let dir = self.current.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            // An archive opens like a folder: its members are listed instead.
            let result = if meshgrove_core::archive::is_archive_file(&dir) {
                meshgrove_core::archive::entries(&dir)
            } else {
                listing::list_dir(&dir)
            }
            .map_err(|e| e.to_string());
            let _ = tx.send((dir, result));
            ctx.request_repaint();
        });
        self.listing = Listing::Loading;
        self.pending = Some(rx);
    }

    /// Returns true when a new listing arrived this frame.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.pending else { return false };
        let Ok((dir, result)) = rx.try_recv() else { return false };
        self.pending = None;
        if dir != self.current {
            return false; // superseded by a newer navigation
        }
        self.listing = match result {
            Ok(entries) => Listing::Ready(entries),
            Err(e) => Listing::Error(e),
        };
        true
    }

    pub fn entries(&self) -> &[Entry] {
        match &self.listing {
            Listing::Ready(entries) => entries,
            _ => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arrive(nav: &mut Navigator) {
        let start = std::time::Instant::now();
        while !nav.poll() {
            assert!(start.elapsed().as_secs() < 5, "listing never arrived");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn refreshes_are_told_apart_from_navigation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let mut nav = Navigator::new(egui::Context::default(), dir.path().to_path_buf());
        arrive(&mut nav);
        assert!(!nav.refreshed());
        nav.refresh();
        arrive(&mut nav);
        assert!(nav.refreshed());
        // Going somewhere while a refresh is under way is a navigation again.
        nav.refresh();
        nav.open(dir.path().join("sub"));
        arrive(&mut nav);
        assert!(!nav.refreshed());
    }
}
