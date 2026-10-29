//! Left panel: places (home, library roots, drives), lists, tags and a lazily loaded folder tree.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::collapsing_header::CollapsingState;
use egui::{Color32, RichText, Ui};
use meshgrove_core::config::Config;
use meshgrove_core::listing;
use meshgrove_core::lists::{ListInfo, ListKind};

use super::tags::{self, Group, PALETTE, TagStyle};

pub enum SidebarAction {
    Navigate(PathBuf),
    AddRoot(PathBuf),
    RemoveRoot(PathBuf),
    /// Index this library folder and everything below it again.
    Rescan(PathBuf),
    /// Library files carrying all of these tags.
    SearchTags(Vec<String>),
    ShowDuplicates,
    ShowInbox,
    ShowImport,
    OpenList(i64),
    CreateList(ListKind, String),
    /// Ask the user's AI for a list (a print list with `true`) from this description.
    ListFromDescription(String, bool),
    RenameList(i64, String),
    DeleteList(i64),
    /// A palette slot for a tag, or `None` for its automatic colour.
    TagColor(String, Option<u8>),
    /// Finish a print list (or open it again).
    ArchiveList(i64, bool),
    /// Fold a sidebar section or tag group away, or open it again ("tags", "tags:themes" …).
    Fold(String),
    ExportList(i64, ListExport),
}

/// File formats a print list can be exported to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListExport {
    Csv,
    Pdf,
}

/// Library facts the sidebar lists.
pub struct LibraryInfo<'a> {
    pub tags: &'a [(String, usize)],
    pub duplicate_groups: usize,
    pub showing_duplicates: bool,
    /// Files waiting in the inbox; `None` when the inbox is switched off.
    pub inbox: Option<usize>,
    pub showing_inbox: bool,
    /// Files waiting in the import folders; `None` when none is set up.
    pub import: Option<usize>,
    pub showing_import: bool,
    pub lists: &'a [ListInfo],
    pub showing_list: Option<i64>,
    /// Tags the current search asks for (lower-case), highlighted.
    pub active_tags: &'a [String],
    /// Lists and tags need the library index.
    pub has_index: bool,
    pub tag_style: &'a TagStyle,
    /// One alphabetical list of tags instead of groups.
    pub flat_tags: bool,
    /// The user's AI can make a list from a description.
    pub ai_lists: bool,
    /// The last prints are kept (Settings → Programs).
    pub recent: bool,
}

pub struct Sidebar {
    home: Option<PathBuf>,
    devices: Vec<PathBuf>,
    children: HashMap<PathBuf, Vec<PathBuf>>,
    /// The folder the tree last expanded itself for, so the user can still collapse nodes.
    revealed: Option<PathBuf>,
    /// Name typed for a new list, or for renaming one.
    list_name: String,
    /// What the AI should put on a new list.
    list_description: String,
    /// A list's "delete" was clicked once; the second click deletes.
    confirm_delete: Option<i64>,
}

impl Sidebar {
    pub fn new() -> Sidebar {
        Sidebar {
            home: dirs::home_dir(),
            devices: mounted_drives(),
            children: HashMap::new(),
            revealed: None,
            list_name: String::new(),
            list_description: String::new(),
            confirm_delete: None,
        }
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
                    if ui.button(t!("menu.rescan")).clicked() {
                        action = Some(SidebarAction::Rescan(root.clone()));
                        ui.close();
                    }
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
            // Only once something arrived: an empty inbox would just take room.
            if let Some(count) = library.inbox.filter(|&n| n > 0 || library.showing_inbox) {
                let label = format!("📥  {}", t!("sidebar.inbox", count = count));
                if ui.selectable_label(library.showing_inbox, label).on_hover_text(t!("sidebar.inbox_hint")).clicked() {
                    action = Some(SidebarAction::ShowInbox);
                }
            }
            if let Some(count) = library.import {
                let title = t!("sidebar.import");
                let label = if count > 0 { format!("⬇  {title}  ({count})") } else { format!("⬇  {title}") };
                if ui.selectable_label(library.showing_import, label).on_hover_text(t!("sidebar.import_hint")).clicked()
                {
                    action = Some(SidebarAction::ShowImport);
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

            let folded = &config.sidebar_collapsed;
            if library.has_index {
                let count = library.lists.len();
                if folding_section(ui, &t!("sidebar.lists"), "lists", count, folded, |ui| {
                    self.lists(ui, &library, &mut action)
                }) {
                    action = Some(SidebarAction::Fold("lists".into()));
                }
            }
            if !library.tags.is_empty()
                && folding_section(ui, &t!("sidebar.tags"), "tags", library.tags.len(), folded, |ui| {
                    tag_groups(ui, &library, folded, &mut action)
                })
            {
                action = Some(SidebarAction::Fold("tags".into()));
            }

            if !self.devices.is_empty() {
                let devices = self.devices.clone();
                if folding_section(ui, &t!("sidebar.devices"), "devices", devices.len(), folded, |ui| {
                    for dev in &devices {
                        place(ui, "🖴", &listing::file_name(dev), dev, current, &mut action);
                    }
                }) {
                    action = Some(SidebarAction::Fold("devices".into()));
                }
            }

            section(ui, &t!("sidebar.folders"));
            let tree_root = self.tree_root(config, current);
            let reveal = self.revealed.as_deref() != Some(current);
            self.node(ui, &tree_root, current, &config.library_roots, reveal, &mut action);
            self.revealed = Some(current.to_path_buf());
        });
        action
    }

    /// Favourites, the last prints, print lists and collections, and "new list".
    fn lists(&mut self, ui: &mut Ui, library: &LibraryInfo, action: &mut Option<SidebarAction>) {
        let favorites = library.lists.iter().find(|l| l.kind == ListKind::Favorites);
        let label = format!("★  {}  ({})", t!("lists.favorites"), favorites.map_or(0, |l| l.count));
        match favorites {
            Some(list) => {
                if ui.selectable_label(library.showing_list == Some(list.id), label).clicked() {
                    *action = Some(SidebarAction::OpenList(list.id));
                }
            }
            None => {
                ui.add_enabled(false, egui::Button::selectable(false, label))
                    .on_disabled_hover_text(t!("lists.favorites_empty"));
            }
        }
        if library.recent {
            let last = library.lists.iter().find(|l| l.kind == ListKind::Recent);
            let label = format!("🕘  {}  ({})", t!("lists.recent"), last.map_or(0, |l| l.count));
            match last {
                Some(list) => {
                    let response = ui.selectable_label(library.showing_list == Some(list.id), label);
                    if response.on_hover_text(t!("lists.recent_hint")).clicked() {
                        *action = Some(SidebarAction::OpenList(list.id));
                    }
                }
                None => {
                    ui.add_enabled(false, egui::Button::selectable(false, label))
                        .on_disabled_hover_text(t!("lists.recent_empty"));
                }
            }
        }
        let own = |l: &&ListInfo| !matches!(l.kind, ListKind::Favorites | ListKind::Recent) && !l.archived;
        for list in library.lists.iter().filter(own) {
            let icon = match (list.kind, &list.query) {
                (ListKind::Print, _) => "🖨",
                (_, Some(_)) => "🔍",
                _ => "📁",
            };
            let count = if list.kind == ListKind::Print {
                t!("lists.open_of", open = list.open, count = list.count).into_owned()
            } else {
                list.count.to_string()
            };
            let response =
                ui.selectable_label(library.showing_list == Some(list.id), format!("{icon}  {}  ({count})", list.name));
            let response = match &list.query {
                Some(q) => response.on_hover_text(t!("lists.saved_search", query = q)),
                None => response,
            };
            if response.clicked() {
                *action = Some(SidebarAction::OpenList(list.id));
            }
            super::context_menu_with_field(&response, |ui| self.list_menu(ui, list, action));
        }
        let archived: Vec<&ListInfo> = library.lists.iter().filter(|l| l.archived).collect();
        if !archived.is_empty() {
            egui::CollapsingHeader::new(RichText::new(t!("lists.archived", count = archived.len())).weak())
                .id_salt("archived-lists")
                .default_open(false)
                .show(ui, |ui| {
                    for list in archived {
                        let response = ui.selectable_label(
                            library.showing_list == Some(list.id),
                            format!("🖨  {}  ({})", list.name, list.count),
                        );
                        if response.clicked() {
                            *action = Some(SidebarAction::OpenList(list.id));
                        }
                        super::context_menu_with_field(&response, |ui| self.list_menu(ui, list, action));
                    }
                });
        }
        egui::containers::menu::MenuButton::new(t!("lists.new").into_owned()).config(super::typing_menu()).ui(
            ui,
            |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.list_name).hint_text(t!("lists.name_hint")));
                let name = self.list_name.trim().to_owned();
                ui.horizontal(|ui| {
                    for (kind, label) in
                        [(ListKind::Print, t!("lists.new_print")), (ListKind::Collection, t!("lists.new_collection"))]
                    {
                        if ui.add_enabled(!name.is_empty(), egui::Button::new(label)).clicked() {
                            *action = Some(SidebarAction::CreateList(kind, name.clone()));
                            self.list_name.clear();
                            ui.close();
                        }
                    }
                });
                if library.ai_lists {
                    ui.separator();
                    ui.label(RichText::new(t!("actions.list_title")).strong());
                    ui.add(
                        egui::TextEdit::multiline(&mut self.list_description)
                            .hint_text(t!("actions.list_hint"))
                            .desired_rows(2)
                            .desired_width(260.0),
                    );
                    let description = self.list_description.trim().to_owned();
                    ui.horizontal(|ui| {
                        for (print, label) in [(true, t!("actions.list_print")), (false, t!("actions.list_collection"))]
                        {
                            if ui.add_enabled(!description.is_empty(), egui::Button::new(label)).clicked() {
                                *action = Some(SidebarAction::ListFromDescription(description.clone(), print));
                                self.list_description.clear();
                                ui.close();
                            }
                        }
                    });
                    ui.weak(t!("actions.list_explain"));
                }
            },
        );
    }

    fn list_menu(&mut self, ui: &mut Ui, list: &ListInfo, action: &mut Option<SidebarAction>) {
        if list.kind == ListKind::Print {
            let (label, archive) = if list.archived { (t!("lists.reopen"), false) } else { (t!("lists.finish"), true) };
            if ui.button(label).on_hover_text(t!("lists.finish_hint")).clicked() {
                *action = Some(SidebarAction::ArchiveList(list.id, archive));
                ui.close();
            }
            for (format, label) in
                [(ListExport::Pdf, t!("lists.export_pdf")), (ListExport::Csv, t!("lists.export_csv"))]
            {
                if ui.button(label).clicked() {
                    *action = Some(SidebarAction::ExportList(list.id, format));
                    ui.close();
                }
            }
            ui.separator();
        }
        ui.label(RichText::new(t!("lists.rename")).weak());
        if self.list_name.is_empty() {
            self.list_name = list.name.clone();
        }
        let field = ui.text_edit_singleline(&mut self.list_name);
        let name = self.list_name.trim().to_owned();
        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (ui.button(t!("lists.rename_apply")).clicked() || enter) && !name.is_empty() && name != list.name {
            *action = Some(SidebarAction::RenameList(list.id, name));
            self.list_name.clear();
            ui.close();
        }
        ui.separator();
        if self.confirm_delete == Some(list.id) {
            if ui.button(RichText::new(t!("lists.delete_confirm")).color(ui.visuals().warn_fg_color)).clicked() {
                *action = Some(SidebarAction::DeleteList(list.id));
                self.confirm_delete = None;
                ui.close();
            }
        } else if ui.button(t!("lists.delete")).clicked() {
            self.confirm_delete = Some(list.id);
        }
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

    fn node(
        &mut self,
        ui: &mut Ui,
        path: &Path,
        current: &Path,
        roots: &[PathBuf],
        reveal: bool,
        action: &mut Option<SidebarAction>,
    ) {
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
                let response = ui.selectable_label(is_current, text);
                if response.clicked() {
                    *action = Some(SidebarAction::Navigate(path.to_path_buf()));
                }
                if roots.iter().any(|r| path.starts_with(r)) {
                    response.context_menu(|ui| {
                        if ui.button(t!("menu.rescan")).clicked() {
                            *action = Some(SidebarAction::Rescan(path.to_path_buf()));
                            ui.close();
                        }
                    });
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
                    self.node(ui, &child, current, roots, reveal, action);
                }
            });
    }
}

/// Tags grouped like the suggestions (themes, print properties, state, own), each with its
/// colour dot, or as one alphabetical list. A click searches for the tag; Ctrl+click adds it to
/// (or takes it out of) the current tag search; right-click chooses its colour.
fn tag_groups(ui: &mut Ui, library: &LibraryInfo, folded: &[String], action: &mut Option<SidebarAction>) {
    if library.flat_tags {
        for (tag, count) in alphabetical(library.tags) {
            tag_row(ui, library, tag, *count, action);
        }
        return;
    }
    for group in [Group::Themes, Group::Properties, Group::State, Group::Own] {
        let in_group: Vec<&(String, usize)> =
            library.tags.iter().filter(|(t, _)| library.tag_style.group_of(t) == group).collect();
        if in_group.is_empty() {
            continue;
        }
        let key = format!("tags:{}", group.key());
        let open = !folded.contains(&key);
        let title = if open { group.title() } else { format!("{}  ({})", group.title(), in_group.len()) };
        let response = egui::CollapsingHeader::new(RichText::new(title).small().weak())
            .id_salt(("sidebar", key.as_str()))
            .open(Some(open))
            .show_unindented(ui, |ui| {
                for (tag, count) in in_group {
                    tag_row(ui, library, tag, *count, action);
                }
            });
        if response.header_response.clicked() {
            *action = Some(SidebarAction::Fold(key));
        }
    }
}

/// The tags sorted by name, ignoring case and accents (Ä sorts with A, ß with ss), unlike the
/// index's ordering, which puts every non-ASCII letter after "z".
fn alphabetical(tags: &[(String, usize)]) -> Vec<&(String, usize)> {
    let mut sorted: Vec<&(String, usize)> = tags.iter().collect();
    sorted.sort_by_cached_key(|(t, _)| sort_key(t));
    sorted
}

fn sort_key(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .flat_map(|c| {
            match c {
                'ä' | 'à' | 'á' | 'â' | 'ã' | 'å' => "a",
                'ö' | 'ò' | 'ó' | 'ô' | 'õ' | 'ø' => "o",
                'ü' | 'ù' | 'ú' | 'û' => "u",
                'é' | 'è' | 'ê' | 'ë' => "e",
                'í' | 'ì' | 'î' | 'ï' => "i",
                'ç' => "c",
                'ñ' => "n",
                'ß' => "ss",
                _ => "",
            }
            .chars()
            .chain((!"äàáâãåöòóôõøüùúûéèêëíìîïçñß".contains(c)).then_some(c))
        })
        .collect()
}

fn tag_row(ui: &mut Ui, library: &LibraryInfo, tag: &String, count: usize, action: &mut Option<SidebarAction>) {
    let active = library.active_tags.contains(&tag.to_lowercase());
    let response = ui
        .horizontal(|ui| {
            tags::dot(ui, library.tag_style, tag);
            ui.selectable_label(active, format!("{tag}  ({count})"))
        })
        .inner
        .on_hover_text(t!("tags.sidebar_hint"));
    if response.clicked() {
        let mut wanted: Vec<String> = Vec::new();
        if ui.input(|i| i.modifiers.command) {
            wanted = library
                .tags
                .iter()
                .map(|(t, _)| t)
                .filter(|t| library.active_tags.contains(&t.to_lowercase()))
                .cloned()
                .collect();
            match wanted.iter().position(|t| t == tag) {
                Some(i) => {
                    wanted.remove(i);
                }
                None => wanted.push(tag.clone()),
            }
        } else {
            wanted.push(tag.clone());
        }
        if !wanted.is_empty() {
            *action = Some(SidebarAction::SearchTags(wanted));
        }
    }
    response.context_menu(|ui| {
        ui.label(RichText::new(t!("tags.color")).weak());
        ui.horizontal_wrapped(|ui| {
            ui.set_max_width(6.0 * 26.0);
            for (slot, (bg, _)) in PALETTE.iter().enumerate() {
                let fill = Color32::from_rgb(bg[0], bg[1], bg[2]);
                let swatch = egui::Button::new("").fill(fill).min_size(egui::vec2(20.0, 20.0)).corner_radius(10.0);
                if ui.add(swatch).clicked() {
                    *action = Some(SidebarAction::TagColor(tag.clone(), Some(slot as u8)));
                    ui.close();
                }
            }
        });
        if ui.button(t!("tags.color_auto")).clicked() {
            *action = Some(SidebarAction::TagColor(tag.clone(), None));
            ui.close();
        }
    });
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).small().strong().weak());
}

/// A section that folds away with a click on its title, showing how many entries it hides;
/// whether it is folded lives in the config (`key` in `folded`). True when the title was clicked.
fn folding_section(
    ui: &mut Ui,
    title: &str,
    key: &str,
    count: usize,
    folded: &[String],
    body: impl FnOnce(&mut Ui),
) -> bool {
    ui.add_space(6.0);
    let open = !folded.iter().any(|f| f == key);
    let title = if open { title.to_owned() } else { format!("{title}  ({count})") };
    egui::CollapsingHeader::new(RichText::new(title).small().strong().weak())
        .id_salt(("sidebar", key))
        .open(Some(open))
        .show_unindented(ui, body)
        .header_response
        .clicked()
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

#[cfg(test)]
mod tests {
    use super::alphabetical;

    #[test]
    fn the_flat_tag_list_is_one_alphabetical_list() {
        let tags: Vec<(String, usize)> =
            ["Zange", "garten", "Drache", "Küche", "Ärmel", "Bad"].iter().map(|t| (t.to_string(), 1)).collect();
        let names: Vec<&str> = alphabetical(&tags).iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(names, ["Ärmel", "Bad", "Drache", "garten", "Küche", "Zange"]);
    }
}
