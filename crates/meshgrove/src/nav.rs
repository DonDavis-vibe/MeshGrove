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

    pub fn reload(&mut self) {
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
