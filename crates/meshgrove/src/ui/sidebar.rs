//! Left panel: places (home, library roots, drives), lists, tags and a lazily loaded folder tree.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::collapsing_header::CollapsingState;
use egui::{Color32, RichText, Ui};
use solidcraft_theme::Tokens;
use solidcraft_ui_kit as sc;
use meshgrove_core::config::Config;
use meshgrove_core::listing;
use meshgrove_core::lists::{ListInfo, ListKind};

use super::menus;
use super::tags::{Group, PALETTE, TagStyle};

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
                place(ui, "home", &t!("sidebar.home"), &home, current, &mut action);
            }

            section(ui, &t!("sidebar.library"));
            for root in &config.library_roots {
                let response = place(ui, "library", &listing::file_name(root), root, current, &mut action);
                if let Some(response) = response {
                    menus::context(&response, |ui| {
                        if menus::item(ui, "compute", &t!("menu.rescan")) {
                            action = Some(SidebarAction::Rescan(root.clone()));
                        }
                        if menus::item(ui, "close", &t!("sidebar.remove_root")) {
                            action = Some(SidebarAction::RemoveRoot(root.clone()));
                        }
                    });
                }
            }
            if library.duplicate_groups > 0 {
                let label = t!("sidebar.duplicates", count = library.duplicate_groups);
                let look = sc::Row { icon: "copy", selected: library.showing_duplicates, ..named(&label) };
                if row(ui, "duplicates", look).clicked {
                    action = Some(SidebarAction::ShowDuplicates);
                }
            }
            // Only once something arrived: an empty inbox would just take room.
            if let Some(count) = library.inbox.filter(|&n| n > 0 || library.showing_inbox) {
                let (label, hint) = (t!("sidebar.inbox", count = count), t!("sidebar.inbox_hint"));
                let look =
                    sc::Row { icon: "import_tray", selected: library.showing_inbox, tip: Some(&hint), ..named(&label) };
                if row(ui, "inbox", look).clicked {
                    action = Some(SidebarAction::ShowInbox);
                }
            }
            if let Some(count) = library.import {
                let (label, hint) = (t!("sidebar.import"), t!("sidebar.import_hint"));
                let count = if count > 0 { count.to_string() } else { String::new() };
                let look = sc::Row {
                    icon: "import",
                    selected: library.showing_import,
                    count: &count,
                    tip: Some(&hint),
                    ..named(&label)
                };
                if row(ui, "import", look).clicked {
                    action = Some(SidebarAction::ShowImport);
                }
            }
            if !config.library_roots.iter().any(|r| current.starts_with(r)) {
                if config.library_roots.is_empty() {
                    note(ui, &t!("sidebar.library_empty"));
                }
                // Cut to the pane's width: a button wider than the pane would widen every row
                // after it and push their counts out of sight.
                let label = t!("sidebar.add_root");
                let add = inset(ui, |ui| {
                    let room = ui.available_width();
                    super::settings_kit::bar_button_within(ui, &label, room)
                });
                if add.on_hover_text(format!("{label}\n{}", current.display())).clicked() {
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
                        place(ui, "save", &listing::file_name(dev), dev, current, &mut action);
                    }
                }) {
                    action = Some(SidebarAction::Fold("devices".into()));
                }
            }

            section(ui, &t!("sidebar.folders"));
            let tree_root = self.tree_root(config, current);
            let reveal = self.revealed.as_deref() != Some(current);
            self.node(ui, &tree_root, 0, current, &config.library_roots, reveal, &mut action);
            self.revealed = Some(current.to_path_buf());
        });
        action
    }

    /// Favourites, the last prints, print lists and collections, and "new list".
    fn lists(&mut self, ui: &mut Ui, library: &LibraryInfo, action: &mut Option<SidebarAction>) {
        // Favourites and the last prints are there from the start, greyed until they hold something.
        let mut fixed = |ui: &mut Ui, kind: ListKind, icon: &str, label: &str, hint: &str, empty: &str| {
            let list = library.lists.iter().find(|l| l.kind == kind);
            let count = list.map_or(0, |l| l.count).to_string();
            let look = sc::Row {
                icon,
                count: &count,
                selected: list.is_some_and(|l| library.showing_list == Some(l.id)),
                dim: list.is_none(),
                tip: Some(if list.is_some() { hint } else { empty }),
                ..named(label)
            };
            let out = row(ui, ("fixed-list", icon), look);
            if let Some(list) = list
                && out.clicked
            {
                *action = Some(SidebarAction::OpenList(list.id));
            }
        };
        let favorites = t!("lists.favorites");
        fixed(ui, ListKind::Favorites, "star", &favorites, &favorites, &t!("lists.favorites_empty"));
        if library.recent {
            fixed(ui, ListKind::Recent, "restore", &t!("lists.recent"), &t!("lists.recent_hint"), &t!("lists.recent_empty"));
        }
        let own = |l: &&ListInfo| !matches!(l.kind, ListKind::Favorites | ListKind::Recent) && !l.archived;
        for list in library.lists.iter().filter(own) {
            let icon = match (list.kind, &list.query) {
                (ListKind::Print, _) => "list",
                (_, Some(_)) => "inspect",
                _ => "feature_group",
            };
            let count = if list.kind == ListKind::Print {
                t!("lists.open_of", open = list.open, count = list.count).into_owned()
            } else {
                list.count.to_string()
            };
            let hint = match &list.query {
                Some(q) => t!("lists.saved_search", query = q).into_owned(),
                None => list.name.clone(),
            };
            self.list_row(ui, library, list, 0, icon, &count, &hint, action);
        }
        let archived: Vec<&ListInfo> = library.lists.iter().filter(|l| l.archived).collect();
        if !archived.is_empty() {
            // Folded away until asked for; egui remembers which, and slides them in and out.
            let id = egui::Id::unique("sidebar-archived-lists");
            let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, false);
            let title = t!("lists.archived", count = archived.len());
            let head = row(ui, "archived-lists", sc::Row { fold: Some(state.is_open()), dim: true, ..named(&title) });
            if head.clicked || head.fold {
                state.toggle(ui);
            }
            state.show_body_unindented(ui, |ui| {
                for list in archived {
                    self.list_row(ui, library, list, 1, "list", &list.count.to_string(), &list.name, action);
                }
            });
        }
        inset(ui, |ui| self.new_list(ui, library, action));
    }

    /// One list: a click opens it, a right click has its menu.
    #[allow(clippy::too_many_arguments)]
    fn list_row(
        &mut self,
        ui: &mut Ui,
        library: &LibraryInfo,
        list: &ListInfo,
        depth: usize,
        icon: &str,
        count: &str,
        hint: &str,
        action: &mut Option<SidebarAction>,
    ) {
        let look = sc::Row {
            depth,
            icon,
            count,
            selected: library.showing_list == Some(list.id),
            tip: Some(hint),
            ..named(&list.name)
        };
        let out = row(ui, ("list", list.id), look);
        if out.clicked {
            *action = Some(SidebarAction::OpenList(list.id));
        }
        if let Some(response) = out.resp {
            menus::context(&response, |ui| self.list_menu(ui, list, action));
        }
    }

    /// "New list …": a name, then which kind; with the user's AI also from a description.
    fn new_list(&mut self, ui: &mut Ui, library: &LibraryInfo, action: &mut Option<SidebarAction>) {
        // Stays open on clicks inside: the name is typed in it.
        menus::button_staying(ui, &t!("lists.new"), |ui| {
            menus::inset(ui, |ui| {
                let width = ui.available_width();
                super::settings_kit::text_field(ui, &mut self.list_name, &t!("lists.name_hint"), width);
            });
            let name = self.list_name.trim().to_owned();
            for (kind, icon, label) in [
                (ListKind::Print, "list", t!("lists.new_print")),
                (ListKind::Collection, "feature_group", t!("lists.new_collection")),
            ] {
                if menus::item_if(ui, icon, &label, !name.is_empty(), &t!("lists.name_first"), false) {
                    *action = Some(SidebarAction::CreateList(kind, name.clone()));
                    self.list_name.clear();
                }
            }
            if library.ai_lists {
                menus::separator(ui);
                menus::note(ui, &t!("actions.list_title"));
                menus::inset(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.list_description)
                            .hint_text(t!("actions.list_hint"))
                            .desired_rows(2)
                            .desired_width(ui.available_width()),
                    );
                });
                let description = self.list_description.trim().to_owned();
                for (print, icon, label) in
                    [(true, "list", t!("actions.list_print")), (false, "feature_group", t!("actions.list_collection"))]
                {
                    if menus::item_if(ui, icon, &label, !description.is_empty(), &t!("actions.list_describe_first"), false)
                    {
                        *action = Some(SidebarAction::ListFromDescription(description.clone(), print));
                        self.list_description.clear();
                    }
                }
                menus::note(ui, &t!("actions.list_explain"));
            }
        });
    }

    fn list_menu(&mut self, ui: &mut Ui, list: &ListInfo, action: &mut Option<SidebarAction>) {
        if list.kind == ListKind::Print {
            let (icon, label, archive) = if list.archived {
                ("restore", t!("lists.reopen"), false)
            } else {
                ("activate", t!("lists.finish"), true)
            };
            let row = menus::step(ui, icon, &label, true, false).on_hover_text(t!("lists.finish_hint"));
            if row.clicked() {
                *action = Some(SidebarAction::ArchiveList(list.id, archive));
                ui.close();
            }
            for (format, label) in
                [(ListExport::Pdf, t!("lists.export_pdf")), (ListExport::Csv, t!("lists.export_csv"))]
            {
                if menus::item(ui, "export", &label) {
                    *action = Some(SidebarAction::ExportList(list.id, format));
                }
            }
            menus::separator(ui);
        }
        menus::heading(ui, &t!("lists.rename"));
        if self.list_name.is_empty() {
            self.list_name = list.name.clone();
        }
        let field = menus::inset(ui, |ui| {
            super::settings_kit::text_field(ui, &mut self.list_name, &t!("lists.name_hint"), 220.0)
        });
        let name = self.list_name.trim().to_owned();
        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let renamed = !name.is_empty() && name != list.name;
        if (menus::item_if(ui, "rename", &t!("lists.rename_apply"), renamed, "", false) || enter) && renamed {
            *action = Some(SidebarAction::RenameList(list.id, name));
            self.list_name.clear();
            ui.close();
        }
        menus::separator(ui);
        // Two clicks: the first only asks, so the menu stays.
        if self.confirm_delete == Some(list.id) {
            if menus::item_if(ui, "delete", &t!("lists.delete_confirm"), true, "", true) {
                *action = Some(SidebarAction::DeleteList(list.id));
                self.confirm_delete = None;
            }
        } else if menus::step(ui, "delete", &t!("lists.delete"), true, true).clicked() {
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

    #[allow(clippy::too_many_arguments)]
    fn node(
        &mut self,
        ui: &mut Ui,
        path: &Path,
        depth: usize,
        current: &Path,
        roots: &[PathBuf],
        reveal: bool,
        action: &mut Option<SidebarAction>,
    ) {
        let id = egui::Id::unique(("sidebar-tree", path));
        let on_way = current.starts_with(path) && current != path;
        // Whether it is open is kept as egui keeps it for its own folding headers.
        let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, on_way);
        if reveal && on_way {
            state.set_open(true);
        }
        let open = state.is_open();
        let name = listing::file_name(path);
        let is_current = path == current;
        let tip = path.display().to_string();
        let look = sc::Row {
            depth,
            fold: Some(open),
            icon: if is_current { "open" } else { "folder" },
            selected: is_current,
            tip: Some(&tip),
            ..named(&name)
        };
        let out = row(ui, ("tree", path), look);
        if out.fold {
            state.toggle(ui);
        }
        if out.clicked {
            *action = Some(SidebarAction::Navigate(path.to_path_buf()));
        }
        if let Some(response) = out.resp
            && roots.iter().any(|r| path.starts_with(r))
        {
            menus::context(&response, |ui| {
                if menus::item(ui, "compute", &t!("menu.rescan")) {
                    *action = Some(SidebarAction::Rescan(path.to_path_buf()));
                }
            });
        }
        // The folders below slide in and out; while it is shut nothing is read from the disk.
        state.show_body_unindented(ui, |ui| {
            let children = self
                .children
                .entry(path.to_path_buf())
                .or_insert_with(|| listing::subdirs(path).unwrap_or_default())
                .clone();
            if children.is_empty() {
                let none = t!("sidebar.no_subfolders");
                row(ui, ("tree-empty", path), sc::Row { depth: depth + 1, dim: true, ..named(&none) });
            }
            for child in children {
                self.node(ui, &child, depth + 1, current, roots, reveal, action);
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
            tag_row(ui, library, tag, *count, 0, action);
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
        let (title, hidden) = (group.title(), if open { String::new() } else { in_group.len().to_string() });
        let head = row(ui, key.as_str(), sc::Row { fold: Some(open), dim: true, count: &hidden, ..named(&title) });
        fold_body(ui, key.as_str(), open, |ui| {
            for (tag, count) in in_group {
                tag_row(ui, library, tag, *count, 0, action);
            }
        });
        if head.clicked || head.fold {
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

fn tag_row(
    ui: &mut Ui,
    library: &LibraryInfo,
    tag: &String,
    count: usize,
    depth: usize,
    action: &mut Option<SidebarAction>,
) {
    let active = library.active_tags.contains(&tag.to_lowercase());
    let (count, hint) = (count.to_string(), t!("tags.sidebar_hint"));
    let look = sc::Row {
        depth,
        swatch: Some(library.tag_style.colors(tag).0),
        count: &count,
        selected: active,
        tip: Some(&hint),
        ..named(tag)
    };
    let out = row(ui, ("tag", tag), look);
    let Some(response) = out.resp else { return };
    if out.clicked {
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
    menus::context(&response, |ui| {
        menus::heading(ui, &t!("tags.color"));
        menus::inset(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.set_max_width(6.0 * 26.0);
                ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                for (slot, (bg, _)) in PALETTE.iter().enumerate() {
                    let fill = Color32::from_rgb(bg[0], bg[1], bg[2]);
                    let swatch = egui::Button::new("").fill(fill).min_size(egui::vec2(20.0, 20.0)).corner_radius(10.0);
                    if ui.add(swatch).clicked() {
                        *action = Some(SidebarAction::TagColor(tag.clone(), Some(slot as u8)));
                        ui.close();
                    }
                }
            });
        });
        if menus::item(ui, "appearance", &t!("tags.color_auto")) {
            *action = Some(SidebarAction::TagColor(tag.clone(), None));
        }
    });
}

/// A row of the sidebar: the kit's tree row (its fold arrow, icon or colour dot, name, a count
/// at the end, the theme's colours for the one that is chosen). `key` tells the rows apart.
fn row(ui: &mut Ui, key: impl std::hash::Hash + std::fmt::Debug, look: sc::Row<'_>) -> sc::RowResp {
    // An id of its own, whatever ui the row is drawn in: a fold's body is a ui of its own, and
    // a menu hanging on a row must not lose it when rows above come or go.
    sc::tree_row(ui, egui::Id::unique(("sidebar-row", key)), &look)
}

/// A row's text as it is: a folder's, list's or tag's name, or a text of ours that is
/// translated already (the kit would otherwise look the text up as one of its own).
fn named(label: &str) -> sc::Row<'_> {
    sc::Row { label, name: true, ..Default::default() }
}

/// Something that is not a row (a button, a sentence), set in to where the rows' names begin.
/// What a fold hides, sliding in and out as egui's own folding headers do: `open` is where it
/// is going, `key` tells the folds apart.
fn fold_body(ui: &mut Ui, key: impl std::hash::Hash + std::fmt::Debug, open: bool, body: impl FnOnce(&mut Ui)) {
    let id = egui::Id::unique(("sidebar-fold", key));
    let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, open);
    state.set_open(open);
    state.show_body_unindented(ui, body);
}

fn inset<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.add_space(SECTION_INSET);
        add(ui)
    })
    .inner
}

fn note(ui: &mut Ui, text: &str) {
    inset(ui, |ui| ui.label(RichText::new(text).weak()));
}

/// Where a section's title starts: behind the room for its fold arrow, so the titles line up.
const SECTION_INSET: f32 = 16.0;

fn section(ui: &mut Ui, title: &str) {
    section_title(ui, title, None);
}

/// A section's title: small capitals in the quiet ink, as the kit writes the headings of its
/// menus and settings, with the kit's fold arrow when it folds (`open`). True when clicked.
fn section_title(ui: &mut Ui, title: &str, open: Option<bool>) -> bool {
    ui.add_space(6.0);
    let t = Tokens::get();
    let sense = if open.is_some() { egui::Sense::click() } else { egui::Sense::hover() };
    let (r, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), t.space_24), sense);
    let hot = open.is_some() && response.hovered();
    let ink = if hot { t.text } else { t.text_dim };
    if let Some(open) = open {
        sc::disclosure(ui.painter(), egui::pos2(r.left() + 8.0, r.center().y), open, ink);
    }
    let mut job =
        egui::text::LayoutJob::simple_singleline(title.to_uppercase(), sc::type_face::semibold(t.font_small), ink);
    for section in &mut job.sections {
        section.format.extra_letter_spacing = t.font_small * t.font_section_tracking;
    }
    let galley = ui.painter().layout_job(job);
    let at = egui::pos2(r.left() + SECTION_INSET, r.center().y - galley.size().y / 2.0);
    ui.painter().with_clip_rect(ui.clip_rect().intersect(r)).galley(at, galley, ink);
    if hot {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
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
    let open = !folded.iter().any(|f| f == key);
    let title = if open { title.to_owned() } else { format!("{title}  ({count})") };
    let clicked = section_title(ui, &title, Some(open));
    fold_body(ui, ("section", key), open, body);
    clicked
}

/// A place to go to: a click shows the folder. Its response (for a menu), unless it is
/// scrolled out of view.
fn place(
    ui: &mut Ui,
    icon: &str,
    label: &str,
    path: &Path,
    current: &Path,
    action: &mut Option<SidebarAction>,
) -> Option<egui::Response> {
    let tip = path.display().to_string();
    // The icon is part of the key: a drive that is also a library folder has two rows.
    let look = sc::Row { icon, selected: current == path, tip: Some(&tip), ..named(label) };
    let out = row(ui, ("place", icon, path), look);
    if out.clicked {
        *action = Some(SidebarAction::Navigate(path.to_path_buf()));
    }
    out.resp
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
