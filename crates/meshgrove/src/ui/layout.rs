//! The four dockable panes: grab a header to move a pane, drop it on an edge to split, on a header to stack.
//! Locked in the settings, they can only be resized. The chat and plugin panels come and go as tabs.
//! Preview and Info fold away together (by hand, or while a plugin panel is in front).

use std::path::PathBuf;

use egui_tiles::{Container, Tile, TileId, Tiles, Tree};
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

/// The panes that fold away together: the details beside the files.
const DETAILS: [Pane; 2] = [Pane::Preview, Pane::Info];

/// Whether everything below `tile` is details (or the chat, a tab beside them by default).
fn only_details(tiles: &Tiles<Pane>, tile: TileId) -> bool {
    match tiles.get(tile) {
        Some(Tile::Pane(pane)) => DETAILS.contains(pane) || *pane == Pane::Chat,
        Some(Tile::Container(c)) => c.children().all(|&child| only_details(tiles, child)),
        None => true,
    }
}

/// Whether the files or a plugin panel are below `tile`.
fn holds_main(tiles: &Tiles<Pane>, tile: TileId) -> bool {
    match tiles.get(tile) {
        Some(Tile::Pane(pane)) => matches!(pane, Pane::Files | Pane::Plugin { .. }),
        Some(Tile::Container(c)) => c.children().any(|&child| holds_main(tiles, child)),
        None => false,
    }
}

/// For Preview and Info each, the outermost tile that holds nothing but details: the right
/// column of the default layout, or the pane alone when it shares tabs with the files.
fn detail_tiles(tree: &Tree<Pane>) -> Vec<TileId> {
    let tiles = &tree.tiles;
    let mut out = Vec::new();
    for pane in &DETAILS {
        let Some(mut tile) = tiles.find_pane(pane) else { continue };
        while let Some(parent) = tiles.parent_of(tile).filter(|&p| only_details(tiles, p)) {
            tile = parent;
        }
        if !out.contains(&tile) {
            out.push(tile);
        }
    }
    out
}

/// Folds the details away (`on` false) or brings them back. A folded tile lends its share of
/// the row to the part holding the files or a plugin panel, which gets all the room (instead of
/// every pane a bit); unfolding takes it back.
fn fold(tree: &mut Tree<Pane>, on: bool) {
    for tile in detail_tiles(tree) {
        if tree.is_visible(tile) == on {
            continue;
        }
        tree.set_visible(tile, on);
        let tiles = &mut tree.tiles;
        let Some(parent) = tiles.parent_of(tile) else { continue };
        let main = match tiles.get(parent) {
            Some(Tile::Container(c)) => c.children().copied().find(|&c| c != tile && holds_main(tiles, c)),
            _ => None,
        };
        if let (Some(main), Some(Tile::Container(Container::Linear(row)))) = (main, tiles.get_mut(parent)) {
            let share = row.shares[tile];
            let to = &mut row.shares[main];
            *to = if on { (*to - share).max(share * 0.1) } else { *to + share };
        }
    }
    if on {
        // Only the details are ever hidden: anything left hidden comes back too.
        let hidden: Vec<TileId> = tree.tiles.iter().map(|(id, _)| *id).filter(|&id| !tree.is_visible(id)).collect();
        for tile in hidden {
            tree.set_visible(tile, true);
        }
    }
}

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
    /// Whether a plugin panel was in front at the last [`Layout::follow_plugins`].
    plugin_front: Option<bool>,
    /// The details were folded away because a plugin panel came to the front (not by the
    /// user): they come back when it goes, and they are saved unfolded.
    auto_folded: bool,
}

impl Layout {
    /// The saved layout, or the default one.
    pub fn load() -> Layout {
        Layout::with(load().unwrap_or_else(default_tree))
    }

    fn with(tree: Tree<Pane>) -> Layout {
        Layout {
            stored: tree.clone(),
            tree,
            fresh: true,
            focus_chat: false,
            focus: None,
            plugin_front: None,
            auto_folded: false,
        }
    }

    /// Back to the default layout, which is no longer kept in the file.
    pub fn reset(&mut self) {
        self.tree = default_tree();
        self.stored = self.tree.clone();
        self.fresh = true;
        self.auto_folded = false;
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

    /// Whether Preview or Info is folded away.
    pub fn details_folded(&self) -> bool {
        let tiles = &self.tree.tiles;
        DETAILS
            .iter()
            .filter_map(|p| tiles.find_pane(p))
            .any(|tile| std::iter::successors(Some(tile), |&t| tiles.parent_of(t)).any(|t| !tiles.is_visible(t)))
    }

    /// Folds Preview and Info away, or brings them back: the user's choice, kept in the layout.
    pub fn show_details(&mut self, on: bool) {
        self.auto_folded = false;
        self.set_details(on);
    }

    fn set_details(&mut self, on: bool) {
        fold(&mut self.tree, on);
    }

    /// Whether a plugin panel shows now (the front tab where it is a tab).
    fn plugin_in_front(&self) -> bool {
        self.tree
            .active_tiles()
            .into_iter()
            .any(|id| matches!(self.tree.tiles.get(id), Some(Tile::Pane(Pane::Plugin { .. }))))
    }

    /// Call before drawing: with `fold` on, Preview and Info fold away when a plugin panel comes
    /// to the front, and come back when the files (or another pane) do, unless the user folded
    /// or unfolded them in between.
    pub fn follow_plugins(&mut self, fold: bool) {
        let front = self.plugin_in_front();
        // At the start nothing was in front: a panel the last run left there folds them too.
        if self.plugin_front.replace(front).unwrap_or(false) != front {
            if front && fold && !self.details_folded() {
                self.set_details(false);
                self.auto_folded = true;
            } else if !front && self.auto_folded {
                self.set_details(true);
                self.auto_folded = false;
            }
        }
    }

    /// The tree as it is kept: details folded for a plugin are saved unfolded.
    fn to_keep(&self) -> Tree<Pane> {
        let mut kept = self.tree.clone();
        if self.auto_folded {
            fold(&mut kept, true);
        }
        kept
    }

    /// Call after the tree was drawn.
    pub fn drawn(&mut self) {
        if std::mem::take(&mut self.fresh) {
            self.stored = self.to_keep();
        }
    }

    /// Writes the layout if it changed since it was last read or written.
    pub fn save(&mut self) {
        let tree = self.to_keep();
        if tree == self.stored {
            return;
        }
        self.stored = tree.clone();
        let Some(path) = path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match serde_json::to_vec(&tree) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    log::warn!("could not save the layout: {e}");
                }
            }
            Err(e) => log::warn!("could not serialize the layout: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(layout: &Layout, pane: &Pane) -> bool {
        let tile = layout.tree.tiles.find_pane(pane).unwrap();
        layout.tree.active_tiles().contains(&tile)
    }

    fn front(layout: &mut Layout, pane: Pane) {
        assert!(layout.tree.make_active(|_, t| matches!(t, Tile::Pane(p) if *p == pane)));
    }

    #[test]
    fn details_fold_as_one_column_and_come_back() {
        let mut layout = Layout::with(default_tree());
        assert!(!layout.details_folded());
        layout.show_details(false);
        assert!(layout.details_folded());
        assert!(!shown(&layout, &Pane::Preview) && !shown(&layout, &Pane::Info));
        assert!(shown(&layout, &Pane::Files) && shown(&layout, &Pane::Places));
        assert_eq!(detail_tiles(&layout.tree).len(), 1, "the right column as a whole");
        let (places, files) = (shares(&layout, &Pane::Places), shares(&layout, &Pane::Files));
        assert!((places - 0.17).abs() < 1e-4, "the places keep their width: {places}");
        assert!((files - 0.83).abs() < 1e-4, "the files get the column's room: {files}");
        layout.show_details(true);
        assert!(!layout.details_folded());
        assert!(shown(&layout, &Pane::Preview) && shown(&layout, &Pane::Info));
        assert!((shares(&layout, &Pane::Files) - 0.45).abs() < 1e-4, "and give it back");
        layout.show_details(true);
        assert!((shares(&layout, &Pane::Files) - 0.45).abs() < 1e-4, "unfolding twice takes nothing");
    }

    /// The share of the row that the tile holding `pane` has.
    fn shares(layout: &Layout, pane: &Pane) -> f32 {
        let tiles = &layout.tree.tiles;
        let mut tile = tiles.find_pane(pane).unwrap();
        while let Some(parent) = tiles.parent_of(tile) {
            if let Some(Tile::Container(Container::Linear(row))) = tiles.get(parent) {
                return row.shares[tile];
            }
            tile = parent;
        }
        panic!("no row above {pane:?}")
    }

    #[test]
    fn a_plugin_in_front_folds_the_details_until_the_files_come_back() {
        let mut layout = Layout::with(default_tree());
        let drawer = Pane::Plugin { plugin: "bingrove".into(), panel: "drawer".into() };
        layout.sync_plugins(&[("bingrove".into(), "drawer".into())]);
        layout.follow_plugins(true);
        assert!(!layout.details_folded(), "the files are still in front");
        front(&mut layout, drawer.clone());
        layout.follow_plugins(true);
        assert!(layout.details_folded());
        assert!(!layout.to_keep().tiles.iter().any(|(id, _)| !layout.to_keep().is_visible(*id)), "saved unfolded");
        front(&mut layout, Pane::Files);
        layout.follow_plugins(true);
        assert!(!layout.details_folded());

        // Unfolded by hand while the plugin is in front: they stay when the files come back.
        front(&mut layout, drawer.clone());
        layout.follow_plugins(true);
        layout.show_details(true);
        front(&mut layout, Pane::Files);
        layout.follow_plugins(true);
        assert!(!layout.details_folded());

        // Folded by hand: the files coming back leave them folded, and that is saved.
        layout.show_details(false);
        front(&mut layout, drawer.clone());
        layout.follow_plugins(true);
        front(&mut layout, Pane::Files);
        layout.follow_plugins(true);
        assert!(layout.details_folded());
        assert!(layout.to_keep().tiles.iter().any(|(id, _)| !layout.to_keep().is_visible(*id)));

        // Switched off: nothing folds.
        layout.show_details(true);
        front(&mut layout, drawer);
        layout.follow_plugins(false);
        assert!(!layout.details_folded());
    }

    #[test]
    fn a_panel_left_in_front_folds_the_details_at_the_start() {
        let mut layout = Layout::with(default_tree());
        layout.sync_plugins(&[("bingrove".into(), "drawer".into())]);
        front(&mut layout, Pane::Plugin { plugin: "bingrove".into(), panel: "drawer".into() });
        layout.follow_plugins(true);
        assert!(layout.details_folded());
        front(&mut layout, Pane::Files);
        layout.follow_plugins(true);
        assert!(!layout.details_folded(), "and they come back with the files");
    }
}
