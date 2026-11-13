//! Meshgrove's own folder chooser, for a plugin's panel (`Effect::PickFolder`): a library, a
//! folder in it, and optionally a new folder below that, which is made only when files land in
//! it. The plugin hears a token for the folder, never its path.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::containers::collapsing_header::CollapsingState;
use egui::{Key, RichText, Ui};
use meshgrove_core::listing;

/// A library the user may choose a folder in.
#[derive(Debug, Clone, PartialEq)]
pub struct Library {
    pub root: PathBuf,
    /// Its name in ids ([`meshgrove_core::ai::scope::aliases`]).
    pub alias: String,
    /// Its drive is mounted read-only: nothing can land there.
    pub read_only: bool,
}

/// The folder the user chose.
#[derive(Debug, Clone, PartialEq)]
pub struct Chosen {
    pub root: PathBuf,
    /// The folder, perhaps one that is not there yet (a new folder below the one picked).
    pub dir: PathBuf,
    /// "Werkstatt/Boxen": the library's name, then the folders below it.
    pub label: String,
}

pub enum Answer {
    Chosen(Chosen),
    Cancelled,
}

pub struct FolderPick {
    title: String,
    /// Who asks: the plugin's name.
    asker: String,
    libraries: Vec<Library>,
    library: usize,
    /// The folder picked in the tree (the library's root or below it).
    dir: PathBuf,
    /// A new folder below `dir`, `/`-separated; empty: `dir` itself.
    new_folder: String,
    children: HashMap<PathBuf, Vec<PathBuf>>,
    /// The tree opens down to `dir` once.
    reveal: bool,
}

impl FolderPick {
    /// The chooser for `libraries`, starting at `start` (library root, folder) when that is in
    /// one of them, else at the first library one can write to.
    pub fn new(title: String, asker: String, libraries: Vec<Library>, start: Option<(PathBuf, PathBuf)>) -> FolderPick {
        let at_start = start.and_then(|(root, dir)| {
            let i = libraries.iter().position(|l| l.root == root && !l.read_only)?;
            // A folder named new back then, or one gone since: its nearest folder that is there.
            let dir = dir.ancestors().find(|d| d.starts_with(&root) && d.is_dir())?.to_path_buf();
            Some((i, dir))
        });
        let (library, dir) = at_start.unwrap_or_else(|| {
            let i = libraries.iter().position(|l| !l.read_only).unwrap_or(0);
            (i, libraries.get(i).map(|l| l.root.clone()).unwrap_or_default())
        });
        FolderPick {
            title,
            asker,
            libraries,
            library,
            dir,
            new_folder: String::new(),
            children: HashMap::new(),
            reveal: true,
        }
    }

    /// The folder as it stands now, or why it cannot be one.
    pub fn chosen(&self) -> Result<Chosen, String> {
        let library = self.libraries.get(self.library).ok_or_else(|| t!("folder_pick.no_library").into_owned())?;
        if library.read_only {
            return Err(t!("folder_pick.read_only").into_owned());
        }
        let mut dir = self.dir.clone();
        let rel = dir.strip_prefix(&library.root).map_err(|_| t!("folder_pick.no_library").into_owned())?;
        let mut parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        let typed = self.new_folder.trim().trim_matches('/');
        if !typed.is_empty() {
            for part in typed.split('/').map(str::trim) {
                if part.is_empty() || part == "." || part == ".." || meshgrove_core::fsops::check_name(part).is_err() {
                    return Err(t!("folder_pick.bad_name", name = part).into_owned());
                }
                dir.push(part);
                parts.push(part.to_owned());
            }
        }
        let label = std::iter::once(library.alias.clone()).chain(parts).collect::<Vec<_>>().join("/");
        Ok(Chosen { root: library.root.clone(), dir, label })
    }

    /// Draws the chooser as a modal; the answer once the user gave one.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Answer> {
        let mut answer = None;
        egui::Modal::new(egui::Id::unique("plugin-folder-pick")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.heading(self.title.as_str());
            ui.weak(t!("folder_pick.asker", name = self.asker.as_str()));
            ui.add_space(6.0);
            self.library_row(ui);
            ui.add_space(4.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
                    if let Some(root) = self.libraries.get(self.library).map(|l| l.root.clone()) {
                        self.node(ui, &root);
                    }
                });
            });
            self.reveal = false;
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(t!("folder_pick.new_folder"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_folder)
                        .hint_text(t!("folder_pick.new_folder_hint"))
                        .desired_width(f32::INFINITY),
                );
            });
            ui.add_space(4.0);
            let chosen = self.chosen();
            match &chosen {
                Ok(c) => {
                    let new = !c.dir.exists();
                    ui.label(RichText::new(t!("folder_pick.into", label = c.label.as_str())).strong());
                    if new {
                        ui.weak(t!("folder_pick.made_later"));
                    }
                }
                Err(why) => {
                    ui.colored_label(ui.visuals().error_fg_color, why);
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let ok =
                    ui.add_enabled(chosen.is_ok(), egui::Button::new(RichText::new(t!("folder_pick.ok")).strong()));
                let enter = ui.input(|i| i.key_pressed(Key::Enter));
                if let Ok(c) = &chosen
                    && (ok.clicked() || enter)
                {
                    answer = Some(Answer::Chosen(c.clone()));
                }
                if ui.button(t!("trash.cancel")).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    answer = Some(Answer::Cancelled);
                }
            });
        });
        answer
    }

    fn library_row(&mut self, ui: &mut Ui) {
        if self.libraries.len() < 2 {
            return;
        }
        ui.horizontal(|ui| {
            ui.label(t!("folder_pick.library"));
            let shown = self.libraries.get(self.library).map(|l| l.alias.clone()).unwrap_or_default();
            egui::ComboBox::from_id_salt("folder-pick-library").selected_text(shown).show_ui(ui, |ui| {
                for (i, library) in self.libraries.iter().enumerate() {
                    let text = match library.read_only {
                        true => format!("{} ({})", library.alias, t!("folder_pick.read_only_short")),
                        false => library.alias.clone(),
                    };
                    let response =
                        ui.add_enabled(!library.read_only, egui::Button::selectable(i == self.library, text));
                    if response.clicked() && i != self.library {
                        self.library = i;
                        self.dir = library.root.clone();
                    }
                }
            });
        });
    }

    /// A folder of the tree with its subfolders, read when it is opened.
    fn node(&mut self, ui: &mut Ui, path: &Path) {
        let id = ui.make_persistent_id(("folder-pick", path));
        let on_way = self.dir.starts_with(path) && self.dir != path;
        let is_root = self.libraries.iter().any(|l| l.root == path);
        let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, on_way || is_root);
        if self.reveal && on_way {
            state.set_open(true);
        }
        let name = match self.libraries.iter().find(|l| l.root == path) {
            Some(library) => format!("📚 {}", library.alias),
            None => listing::file_name(path),
        };
        let picked = path == self.dir;
        state
            .show_header(ui, |ui| {
                let text = if picked { RichText::new(&name).strong() } else { RichText::new(&name) };
                if ui.selectable_label(picked, text).clicked() {
                    self.dir = path.to_path_buf();
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
                    self.node(ui, &child);
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn libraries(dir: &Path) -> Vec<Library> {
        let lib = |name: &str, read_only| Library { root: dir.join(name), alias: name.into(), read_only };
        vec![lib("Eigene", false), lib("Archiv", true), lib("Werkstatt", false)]
    }

    #[test]
    fn the_chosen_folder_is_named_by_library_and_folders() {
        let dir = tempfile::tempdir().unwrap();
        let shelf = dir.path().join("Werkstatt").join("Regal");
        std::fs::create_dir_all(&shelf).unwrap();
        let start = Some((dir.path().join("Werkstatt"), shelf.join("Neu")));
        let mut pick = FolderPick::new("Where?".into(), "Boxgrove".into(), libraries(dir.path()), start);
        // The folder named new last time is not there: its parent, which is.
        assert_eq!(pick.dir, shelf);
        let chosen = pick.chosen().unwrap();
        assert_eq!((chosen.root, chosen.label.as_str()), (dir.path().join("Werkstatt"), "Werkstatt/Regal"));
        pick.new_folder = " Boxen/Schubladen/ ".into();
        let chosen = pick.chosen().unwrap();
        assert_eq!(chosen.dir, shelf.join("Boxen").join("Schubladen"));
        assert_eq!(chosen.label, "Werkstatt/Regal/Boxen/Schubladen");
        for bad in ["../weg", "a//b", "a:b"] {
            pick.new_folder = bad.into();
            assert!(pick.chosen().is_err(), "{bad}");
        }
    }

    #[test]
    fn a_read_only_library_is_no_place() {
        let dir = tempfile::tempdir().unwrap();
        let start = Some((dir.path().join("Archiv"), dir.path().join("Archiv")));
        let pick = FolderPick::new("Where?".into(), "Boxgrove".into(), libraries(dir.path()), start);
        assert_eq!(pick.library, 0, "not the read-only one: the first that takes files");
        let mut pick = pick;
        pick.library = 1;
        assert!(pick.chosen().is_err());
        let none = FolderPick::new("Where?".into(), "Boxgrove".into(), Vec::new(), None);
        assert!(none.chosen().is_err());
    }
}
