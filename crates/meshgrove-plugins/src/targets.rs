//! Folders the user chose for a panel ([`meshgrove_plugin_api::ui::Effect::PickFolder`]). The
//! plugin gets a token for each ([`meshgrove_plugin_api::ui::Event::FolderPicked`]) and names it
//! in a bundle ([`meshgrove_plugin_api::Bundle::target`]); only the host knows the folder behind
//! it, so a plugin learns nothing about where the libraries are and can put files only where the
//! user said. Shared between the window and the panel's handle; the window keeps them across
//! restarts, so a token in the panel's snapshot still counts after one.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// A folder in a library, chosen by the user. It need not exist yet: it is made when files land
/// in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// The library.
    pub root: PathBuf,
    /// The folder, `root` itself or below it.
    pub dir: PathBuf,
    /// As the user reads it: "Werkstatt/Boxen" (the library's name, then the folders below it).
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    token: String,
    #[serde(flatten)]
    target: Target,
}

/// The folders chosen for one panel, newest last, each with its token.
#[derive(Debug, Clone, Default)]
pub struct Targets(Arc<Mutex<Vec<Entry>>>);

impl Targets {
    /// How many folders a panel keeps: the oldest go first.
    pub const MAX: usize = 64;

    /// Keeps `target` and gives the token that stands for it; a folder chosen before keeps its
    /// token and becomes the newest.
    pub fn add(&self, target: Target) -> String {
        let mut list = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let token = match list.iter().position(|e| e.target.root == target.root && e.target.dir == target.dir) {
            Some(i) => list.remove(i).token,
            None => crate::staging::new_uid(),
        };
        list.push(Entry { token: token.clone(), target });
        let over = list.len().saturating_sub(Targets::MAX);
        list.drain(..over);
        token
    }

    /// The folder `token` stands for.
    pub fn get(&self, token: &str) -> Option<Target> {
        let list = self.0.lock().unwrap_or_else(|e| e.into_inner());
        list.iter().find(|e| e.token == token.trim()).map(|e| e.target.clone())
    }

    /// The folder chosen last.
    pub fn last(&self) -> Option<Target> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).last().map(|e| e.target.clone())
    }

    /// What `file` holds (a JSON list); nothing when it is missing or broken.
    pub fn load(file: &Path) -> Targets {
        let list = std::fs::read(file).ok().and_then(|b| serde_json::from_slice::<Vec<Entry>>(&b).ok());
        Targets(Arc::new(Mutex::new(list.unwrap_or_default())))
    }

    pub fn save(&self, file: &Path) -> std::io::Result<()> {
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let list = self.0.lock().unwrap_or_else(|e| e.into_inner()).clone();
        std::fs::write(file, serde_json::to_vec(&list).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(dir: &str) -> Target {
        Target { root: PathBuf::from("/lib"), dir: Path::new("/lib").join(dir), label: format!("lib/{dir}") }
    }

    #[test]
    fn chosen_folders_keep_their_tokens_and_survive_a_restart() {
        let targets = Targets::default();
        let boxes = targets.add(target("Boxen"));
        let garden = targets.add(target("Garten"));
        assert_ne!(boxes, garden);
        assert_eq!(boxes.len(), 16);
        assert_eq!(targets.get(&boxes), Some(target("Boxen")));
        assert_eq!(targets.get("0000000000000000"), None, "a token the host did not give");
        assert_eq!(targets.last(), Some(target("Garten")));
        assert_eq!(targets.add(target("Boxen")), boxes, "the same folder, the same token");
        assert_eq!(targets.last(), Some(target("Boxen")), "chosen again: the newest");

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("state").join("box.targets.json");
        targets.save(&file).unwrap();
        let again = Targets::load(&file);
        assert_eq!((again.get(&boxes), again.get(&garden)), (Some(target("Boxen")), Some(target("Garten"))));
        assert_eq!(again.last(), Some(target("Boxen")));
        assert!(Targets::load(&dir.path().join("missing.json")).last().is_none());
        std::fs::write(&file, "not json").unwrap();
        assert!(Targets::load(&file).last().is_none(), "broken: nothing, no error");

        let many = Targets::default();
        let first = many.add(target("0"));
        (1..=Targets::MAX).for_each(|n| {
            many.add(target(&n.to_string()));
        });
        assert_eq!(many.get(&first), None, "the oldest went");
    }
}
