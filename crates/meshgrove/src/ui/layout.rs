//! The four dockable panes: grab a header to move a pane, drop it on an edge to split, on a header to stack.
//! Locked in the settings, they can only be resized. The chat and plugin panels come and go as tabs.

use std::path::PathBuf;

use egui_tiles::{Container, Tile, Tiles, Tree};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pane {
    Places,
    Files,
    Preview,
    Info,
    /// The chat with the user's own model: only while AI is on and an endpoint is set up.
    Chat,
    /// A panel of a plugin (ids from its manifest), while it is open.
    Plugin {
        plugin: String,
        panel: String,
    },
}

impl Pane {
    pub fn title(&self) -> String {
        match self {
            Pane::Places => t!("pane.places"),
            Pane::Files => t!("pane.files"),
            Pane::Preview => t!("pane.preview"),
            Pane::Info => t!("pane.info"),
            Pane::Chat => t!("pane.chat"),
            Pane::Plugin { panel, .. } => return panel.clone(),
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
        column.shares.set_share(preview, 0.55);
        column.shares.set_share(info, 0.45);
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
    /// Not drawn yet: egui_tiles tidies a new tree up on its first frame, which is no change
    /// of the user's and must not be saved (or later default layouts would never show).
    fresh: bool,
    /// The chat's tab comes to the front before the next drawing (the tree is taken out of
    /// the layout while it is drawn, so not during).
    focus_chat: bool,
    /// This pane's tab comes to the front before the next drawing ([`Layout::apply_focus`]).
    focus: Option<Pane>,
}

impl Layout {
    /// The saved layout, or the default one.
    pub fn load() -> Layout {
        let tree = load().unwrap_or_else(default_tree);
        Layout { stored: tree.clone(), tree, fresh: true, focus_chat: false, focus: None }
    }

    /// Back to the default layout, which is no longer kept in the file.
    pub fn reset(&mut self) {
        self.tree = default_tree();
        self.stored = self.tree.clone();
        self.fresh = true;
        if let Some(path) = path() {
            let _ = std::fs::remove_file(path);
        }
    }

    /// Brings the chat pane's tab to the front (with the next [`Layout::show_chat`]).
    pub fn focus_chat(&mut self) {
        self.focus_chat = true;
    }

    /// Puts the chat pane next to the info pane (as a tab), or takes it out again.
    pub fn show_chat(&mut self, on: bool) {
        match (on, self.tree.tiles.find_pane(&Pane::Chat)) {
            (true, None) => self.add_tab(Pane::Chat, &[Pane::Info, Pane::Preview]),
            (false, Some(chat)) => {
                self.tree.remove_recursively(chat);
            }
            _ => {}
        }
        if std::mem::take(&mut self.focus_chat) && on {
            self.tree.make_active(|_, tile| matches!(tile, Tile::Pane(Pane::Chat)));
        }
    }

    /// Brings a plugin panel's tab to the front before the next drawing.
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    pub fn focus_plugin(&mut self, plugin: &str, panel: &str) {
        self.focus = Some(Pane::Plugin { plugin: plugin.to_owned(), panel: panel.to_owned() });
    }

    /// Brings the files' tab to the front before the next drawing (a view opened in it).
    pub fn focus_files(&mut self) {
        self.focus = Some(Pane::Files);
    }

    /// Call before drawing: brings the pane asked for to the front, once it is there.
    pub fn apply_focus(&mut self) {
        if let Some(pane) = self.focus.take()
            && !self.tree.make_active(|_, tile| matches!(tile, Tile::Pane(p) if *p == pane))
            && matches!(pane, Pane::Plugin { .. })
        {
            self.focus = Some(pane); // the panel's tab comes with its first view
        }
    }

    /// The plugin panels in the layout (also those saved from the last run).
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    pub fn plugin_panes(&self) -> Vec<(String, String)> {
        let mut panes: Vec<(String, String)> = self
            .tree
            .tiles
            .iter()
            .filter_map(|(_, t)| match t {
                Tile::Pane(Pane::Plugin { plugin, panel }) => Some((plugin.clone(), panel.clone())),
                _ => None,
            })
            .collect();
        panes.sort();
        panes
    }

    /// Shows exactly the plugin panels that are `open`, as tabs next to the files.
    #[cfg_attr(not(feature = "plugins"), allow(dead_code))]
    pub fn sync_plugins(&mut self, open: &[(String, String)]) {
        for (plugin, panel) in self.plugin_panes() {
            if !open.iter().any(|(p, q)| *p == plugin && *q == panel)
                && let Some(tile) = self.tree.tiles.find_pane(&Pane::Plugin { plugin, panel })
            {
                self.tree.remove_recursively(tile);
            }
        }
        for (plugin, panel) in open {
            let pane = Pane::Plugin { plugin: plugin.clone(), panel: panel.clone() };
            if self.tree.tiles.find_pane(&pane).is_none() {
                self.add_tab(pane, &[Pane::Files, Pane::Preview]);
            }
        }
    }

    /// Adds `pane` as a tab beside the first of `anchors` there is (a pane standing alone and
    /// it become tabs in its place), else to the root.
    fn add_tab(&mut self, pane: Pane, anchors: &[Pane]) {
        let tiles = &mut self.tree.tiles;
        let new = tiles.insert_pane(pane);
        let anchor = anchors.iter().find_map(|a| tiles.find_pane(a));
        let parent = anchor.and_then(|a| tiles.parent_of(a));
        match (anchor, parent.and_then(|p| tiles.get_mut(p))) {
            (_, Some(Tile::Container(Container::Tabs(tabs)))) => tabs.add_child(new),
            (Some(anchor), Some(Tile::Container(container))) => {
                let slot = container.remove_child(anchor);
                let tabs = tiles.insert_tab_tile(vec![anchor, new]);
                if let Some(Tile::Container(container)) = parent.and_then(|p| tiles.get_mut(p)) {
                    match container {
                        Container::Linear(l) => l.children.insert(slot.unwrap_or(l.children.len()), tabs),
                        other => other.add_child(tabs),
                    }
                }
            }
            _ => {
                if let Some(root) = self.tree.root
                    && let Some(Tile::Container(c)) = self.tree.tiles.get_mut(root)
                {
                    c.add_child(new);
                }
            }
        }
    }

    /// Call after the tree was drawn.
    pub fn drawn(&mut self) {
        if std::mem::take(&mut self.fresh) {
            self.stored = self.tree.clone();
        }
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
