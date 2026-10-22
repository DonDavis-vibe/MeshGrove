//! The four dockable panes: grab a header to move a pane, drop it on an edge to split, on a header to stack.
//! Locked in the settings, they can only be resized.

use std::path::PathBuf;

use egui_tiles::{Container, Tile, Tiles, Tree};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pane {
    Places,
    Files,
    Preview,
    Info,
}

impl Pane {
    pub fn title(self) -> String {
        match self {
            Pane::Places => t!("pane.places"),
            Pane::Files => t!("pane.files"),
            Pane::Preview => t!("pane.preview"),
            Pane::Info => t!("pane.info"),
        }
        .into_owned()
    }
}

const TREE_ID: &str = "meshgrove_layout";

/// Places | Files | (Preview over Info).
fn default_tree() -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let places = tiles.insert_pane(Pane::Places);
    let files = tiles.insert_pane(Pane::Files);
    let preview = tiles.insert_pane(Pane::Preview);
    let info = tiles.insert_pane(Pane::Info);
    let right = tiles.insert_vertical_tile(vec![preview, info]);
    let root = tiles.insert_horizontal_tile(vec![places, files, right]);
    if let Some(Tile::Container(Container::Linear(row))) = tiles.get_mut(root) {
        row.shares.set_share(places, 0.17);
        row.shares.set_share(files, 0.45);
        row.shares.set_share(right, 0.38);
    }
    if let Some(Tile::Container(Container::Linear(column))) = tiles.get_mut(right) {
        column.shares.set_share(preview, 0.72);
        column.shares.set_share(info, 0.28);
    }
    Tree::new(egui::Id::unique(TREE_ID), root, tiles)
}

fn path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("meshgrove").join("layout.json"))
}

/// The saved layout, if it still holds all four panes.
fn load() -> Option<Tree<Pane>> {
    let tree: Tree<Pane> = serde_json::from_slice(&std::fs::read(path()?).ok()?).ok()?;
    let all = [Pane::Places, Pane::Files, Pane::Preview, Pane::Info];
    all.iter().all(|p| tree.tiles.iter().any(|(_, t)| matches!(t, Tile::Pane(q) if q == p))).then_some(tree)
}

/// The pane tree and the file it is kept in.
pub struct Layout {
    pub tree: Tree<Pane>,
    /// The tree as last read or written, so an unchanged layout is not written again.
    stored: Tree<Pane>,
}

impl Layout {
    /// The saved layout, or the default one.
    pub fn load() -> Layout {
        let tree = load().unwrap_or_else(default_tree);
        Layout { stored: tree.clone(), tree }
    }

    pub fn reset(&mut self) {
        self.tree = default_tree();
    }

    /// Writes the layout if it changed since it was last read or written.
    pub fn save(&mut self) {
        if self.tree == self.stored {
            return;
        }
        self.stored = self.tree.clone();
        let Some(path) = path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match serde_json::to_vec(&self.tree) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    log::warn!("could not save the layout: {e}");
                }
            }
            Err(e) => log::warn!("could not serialize the layout: {e}"),
        }
    }
}
