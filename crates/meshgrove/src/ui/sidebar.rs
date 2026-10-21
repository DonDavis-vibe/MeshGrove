//! Left panel: places (home, library roots, drives) and a lazily loaded folder tree.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::collapsing_header::CollapsingState;
use egui::{RichText, Ui};
use meshgrove_core::config::Config;
use meshgrove_core::listing;

pub enum SidebarAction {
    Navigate(PathBuf),
    AddRoot(PathBuf),
    RemoveRoot(PathBuf),
    SearchTag(String),
    ShowDuplicates,
}

/// Library facts the sidebar lists.
pub struct LibraryInfo<'a> {
    pub tags: &'a [(String, usize)],
    pub duplicate_groups: usize,
    pub showing_duplicates: bool,
}

pub struct Sidebar {
    home: Option<PathBuf>,
    devices: Vec<PathBuf>,
    children: HashMap<PathBuf, Vec<PathBuf>>,
    /// The folder the tree last expanded itself for, so the user can still collapse nodes.
    revealed: Option<PathBuf>,
}

impl Sidebar {
    pub fn new() -> Sidebar {
        Sidebar { home: dirs::home_dir(), devices: mounted_drives(), children: HashMap::new(), revealed: None }
    }

    pub fn refresh(&mut self) {
        self.devices = mounted_drives();
        self.children.clear();
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        config: &Config,
        current: &Path,
        library: LibraryInfo,
    ) -> Option<SidebarAction> {
        let mut action = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            section(ui, &t!("sidebar.places"));
            if let Some(home) = self.home.clone() {
                place(ui, "🏠", &t!("sidebar.home"), &home, current, &mut action);
            }

            section(ui, &t!("sidebar.library"));
            for root in &config.library_roots {
                let response = place(ui, "📚", &listing::file_name(root), root, current, &mut action);
                response.context_menu(|ui| {
                    if ui.button(t!("sidebar.remove_root")).clicked() {
                        action = Some(SidebarAction::RemoveRoot(root.clone()));
                        ui.close();
                    }
                });
            }
            if library.duplicate_groups > 0 {
                let label = format!("⧉  {}", t!("sidebar.duplicates", count = library.duplicate_groups));
                if ui.selectable_label(library.showing_duplicates, label).clicked() {
                    action = Some(SidebarAction::ShowDuplicates);
                }
            }
            if !config.library_roots.iter().any(|r| current.starts_with(r)) {
                if config.library_roots.is_empty() {
                    ui.weak(t!("sidebar.library_empty"));
                }
                if ui.button(t!("sidebar.add_root")).on_hover_text(current.display().to_string()).clicked() {
                    action = Some(SidebarAction::AddRoot(current.to_path_buf()));
                }
            }

            if !library.tags.is_empty() {
                section(ui, &t!("sidebar.tags"));
                for (tag, count) in library.tags {
                    if ui.selectable_label(false, format!("🏷  {tag}  ({count})")).clicked() {
                        action = Some(SidebarAction::SearchTag(tag.clone()));
                    }
                }
            }

            if !self.devices.is_empty() {
                section(ui, &t!("sidebar.devices"));
                for dev in self.devices.clone() {
                    place(ui, "🖴", &listing::file_name(&dev), &dev, current, &mut action);
                }
            }

            section(ui, &t!("sidebar.folders"));
            let tree_root = self.tree_root(config, current);
            let reveal = self.revealed.as_deref() != Some(current);
            self.node(ui, &tree_root, current, reveal, &mut action);
            self.revealed = Some(current.to_path_buf());
        });
        action
    }

    /// The tree starts at the library root, drive or home folder containing the current folder.
    fn tree_root(&self, config: &Config, current: &Path) -> PathBuf {
        config
            .library_roots
            .iter()
            .chain(&self.devices)
            .chain(&self.home)
            .filter(|r| current.starts_with(r))
            .max_by_key(|r| r.components().count())
            .cloned()
            .unwrap_or_else(|| current.ancestors().last().unwrap_or(current).to_path_buf())
    }

    fn node(&mut self, ui: &mut Ui, path: &Path, current: &Path, reveal: bool, action: &mut Option<SidebarAction>) {
        let id = ui.make_persistent_id(("tree", path));
        let on_way = current.starts_with(path) && current != path;
        let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, on_way);
        if reveal && on_way {
            state.set_open(true);
        }
        let name = listing::file_name(path);
        let is_current = path == current;
        state
            .show_header(ui, |ui| {
                let text = if is_current { RichText::new(&name).strong() } else { RichText::new(&name) };
                if ui.selectable_label(is_current, text).clicked() {
                    *action = Some(SidebarAction::Navigate(path.to_path_buf()));
                }
            })
            .body(|ui| {
                let children = self
                    .children
                    .entry(path.to_path_buf())
                    .or_insert_with(|| listing::subdirs(path).unwrap_or_default())
                    .clone();
                if children.is_empty() {
                    ui.weak(t!("sidebar.no_subfolders"));
                }
                for child in children {
                    self.node(ui, &child, current, reveal, action);
                }
            });
    }
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).small().strong().weak());
}

fn place(
    ui: &mut Ui,
    icon: &str,
    label: &str,
    path: &Path,
    current: &Path,
    action: &mut Option<SidebarAction>,
) -> egui::Response {
    let response =
        ui.selectable_label(current == path, format!("{icon}  {label}")).on_hover_text(path.display().to_string());
    if response.clicked() {
        *action = Some(SidebarAction::Navigate(path.to_path_buf()));
    }
    response
}

/// Removable and extra drives as KDE shows them: mounts below /run/media/<user> and /mnt.
#[cfg(target_os = "linux")]
fn mounted_drives() -> Vec<PathBuf> {
    let Ok(mounts) = std::fs::read_to_string("/proc/self/mounts") else {
        return Vec::new();
    };
    let mut drives: Vec<PathBuf> = mounts
        .lines()
        .filter_map(|line| line.split(' ').nth(1))
        .map(meshgrove_core::fsinfo::unescape)
        .filter(|p| p.starts_with("/run/media/") || p.starts_with("/mnt/"))
        .map(PathBuf::from)
        .collect();
    drives.sort();
    drives.dedup();
    drives
}

/// Drive letters that are present (A: and B: are skipped: probing old floppy drives is slow).
#[cfg(windows)]
fn mounted_drives() -> Vec<PathBuf> {
    (b'C'..=b'Z').map(|letter| PathBuf::from(format!("{}:\\", letter as char))).filter(|p| p.exists()).collect()
}

/// Volumes in /Volumes, without the link back to the system disk.
#[cfg(target_os = "macos")]
fn mounted_drives() -> Vec<PathBuf> {
    let volumes = std::fs::read_dir("/Volumes").into_iter().flatten().flatten();
    let mut drives: Vec<PathBuf> = volumes.map(|e| e.path()).filter(|p| p.is_dir() && !p.is_symlink()).collect();
    drives.sort();
    drives
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn mounted_drives() -> Vec<PathBuf> {
    Vec::new()
}
