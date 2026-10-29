//! Placing files by their tags: the folder a tagged file belongs in (Settings → Tags), and
//! move suggestions for files that are not there yet. They are suggestions like an assistant's,
//! so they go through the same review – or, out of the landing folder, the same automatic
//! acceptance.

use std::path::{Path, PathBuf};

use crate::ai::ToolResult;
use crate::ai::proposals::{
    MAX_ITEMS, MAX_PENDING, MovePayload, NewBatch, NewProposal, Origin, ProposalKind, ProposalStore,
};
use crate::config::{AccessClass, Config};
use crate::db::{Error, Library};

/// The source Meshgrove's own suggestions carry in the store.
pub const SOURCE: &str = "meshgrove";

/// The folders (relative to the library root) files with `tags` could go to, each with the tag
/// that gives it, in order of precedence (Settings → Tags): the first is the one suggested, the
/// others are offered in the review. More than one means the choice is not clear-cut.
pub fn candidates(tags: &[String], config: &Config) -> Vec<(String, PathBuf)> {
    if !config.placement.enabled {
        return Vec::new();
    }
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    for (_, folder) in config.folder_rules() {
        if let Some(tag) = tags.iter().find(|t| config.folder_for_tag(t).as_deref() == Some(folder.as_str())) {
            let rel = relative(&folder);
            if !found.iter().any(|(_, f)| *f == rel) {
                found.push((tag.clone(), rel));
            }
        }
    }
    found
}

/// The tag that decides and the folder (relative to the library root) files with `tags` go to.
pub fn pick(tags: &[String], config: &Config) -> Option<(String, PathBuf)> {
    candidates(tags, config).into_iter().next()
}

/// The folder (relative to the library root) files with `tags` go to.
pub fn folder_for(tags: &[String], config: &Config) -> Option<PathBuf> {
    pick(tags, config).map(|(_, folder)| folder)
}

/// A `/`-separated folder setting as a relative path.
fn relative(folder: &str) -> PathBuf {
    folder.split('/').map(str::trim).filter(|s| !s.is_empty()).collect()
}

/// A file that got no move suggestion, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub path: PathBuf,
    /// "outside", "no_rule", "there_already", "duplicate", "taken" or "name".
    pub why: &'static str,
}

/// Move suggestions for the library files at `paths` by their tags.
pub fn proposals_for(
    lib: &Library,
    config: &Config,
    paths: &[PathBuf],
) -> Result<(Vec<NewProposal>, Vec<Skipped>), Error> {
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    let mut targets: Vec<PathBuf> = Vec::new();
    let mut skip = |path: &Path, why: &'static str| skipped.push(Skipped { path: path.to_path_buf(), why });
    for path in paths {
        let Some(root) = config.library_roots.iter().find(|r| path.starts_with(r)) else {
            skip(path, "outside");
            continue;
        };
        let tags: Vec<String> = lib.tags_of(std::slice::from_ref(path))?.into_iter().map(|(t, _)| t).collect();
        let found = candidates(&tags, config);
        let Some((tag, folder)) = found.first().cloned() else {
            skip(path, "no_rule");
            continue;
        };
        let Some(name) = path.file_name() else { continue };
        let to = root.join(folder).join(name);
        if to.parent() == path.parent() {
            skip(path, "there_already");
            continue;
        }
        if targets.iter().any(|t| t == &to) {
            skip(path, "taken");
            continue;
        }
        if to.symlink_metadata().is_ok() {
            // The same content there already: a copy to deal with, not a move.
            let (here, there) = (lib.content_hash_of(path)?, lib.content_hash_of(&to)?);
            skip(path, if here.is_some() && here == there { "duplicate" } else { "taken" });
            continue;
        }
        match crate::moves::plan_move(lib, root, path, &to) {
            Ok((target, mut payload)) => {
                // The other folders that fit, for the review to choose from.
                payload.alternatives =
                    found.iter().skip(1).map(|(_, f)| root.join(f).join(name)).filter(|a| *a != to).collect();
                targets.push(to);
                out.push(NewProposal {
                    kind: ProposalKind::Move,
                    target: Some(target),
                    payload: serde_json::to_value(payload).unwrap_or_default(),
                    reason: Some(format!("Tag: {tag}")),
                    confidence: None,
                });
            }
            Err(why) => skip(path, if why.contains("name") || why.contains("Windows") { "name" } else { "taken" }),
        }
    }
    Ok((out, skipped))
}

/// Suggests `items` as batches of Meshgrove's own, at most [`MAX_ITEMS`] each, leaving out
/// files that already have such a move waiting and stopping at [`MAX_PENDING`] batches. How
/// many suggestions were made.
pub fn propose(store: &mut ProposalStore, title: &str, items: Vec<NewProposal>) -> ToolResult<usize> {
    let waiting = store.pending_move_sources(SOURCE)?;
    let items: Vec<NewProposal> =
        items.into_iter().filter(|p| p.target.as_ref().is_none_or(|t| !waiting.contains(&t.path))).collect();
    let origin = Origin { source: SOURCE.into(), client: None, class: AccessClass::Local };
    let mut made = 0;
    for chunk in items.chunks(MAX_ITEMS) {
        if store.pending_of(&origin)? >= MAX_PENDING {
            break;
        }
        let batch = NewBatch { origin: &origin, kind: "move", title, summary: None, inbox_done: false };
        store.insert_batch(&batch, chunk)?;
        made += chunk.len();
    }
    Ok(made)
}

/// The move payload of a suggestion, for whoever shows it.
pub fn payload(p: &crate::ai::proposals::Proposal) -> Option<MovePayload> {
    serde_json::from_value(p.payload.clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::proposals::{Picked, to_ops};
    use crate::cache::ThumbMeta;
    use crate::config::FolderRule;
    use crate::formats::Kind;
    use crate::listing::Entry;

    fn entry(dir: &Path, name: &str) -> Entry {
        let meta = dir.join(name).metadata().unwrap();
        Entry {
            name: name.into(),
            path: dir.join(name),
            kind: Kind::from_file_name(name),
            size: meta.len(),
            modified: meta.modified().ok(),
        }
    }

    #[test]
    fn the_folder_follows_the_precedence_of_the_settings() {
        let mut config = Config::default();
        let tags = |list: &[&str]| list.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>();
        // Garten comes before Deko among the themes, whatever the order of the file's tags.
        assert_eq!(folder_for(&tags(&["Deko", "Garten"]), &config), Some(PathBuf::from("Garten")));
        assert_eq!(pick(&tags(&["Garten", "Deko"]), &config).unwrap().0, "Garten");
        assert_eq!(
            candidates(&tags(&["Deko", "Garten"]), &config),
            [("Garten".to_owned(), PathBuf::from("Garten")), ("Deko".to_owned(), PathBuf::from("Deko"))],
            "both offered, in order"
        );
        assert_eq!(
            folder_for(&tags(&["PLA", "eigenes Design"]), &config),
            None,
            "properties and states have no folder"
        );
        config.placement.rules = vec![
            FolderRule { tag: "Deko".into(), folder: String::new() },
            FolderRule { tag: "Kunde Müller".into(), folder: " Kunden / Müller ".into() },
            FolderRule { tag: "Projekt X".into(), folder: "Projekte".into() },
        ];
        assert_eq!(folder_for(&tags(&["Deko", "Garten"]), &config), Some(PathBuf::from("Garten")), "Deko switched off");
        let muller = Path::new("Kunden").join("Müller");
        assert_eq!(folder_for(&tags(&["Kunde Müller"]), &config), Some(muller.clone()));
        assert_eq!(
            folder_for(&tags(&["Garten", "Kunde Müller"]), &config),
            Some(muller.clone()),
            "an own tag beats a theme"
        );
        assert_eq!(
            folder_for(&tags(&["Projekt X", "Kunde Müller"]), &config),
            Some(muller.clone()),
            "the deeper folder is the more specific"
        );
        config.placement.order = vec!["Garten".into()];
        assert_eq!(folder_for(&tags(&["Garten", "Kunde Müller"]), &config), Some(PathBuf::from("Garten")), "ordered");
        assert_eq!(candidates(&tags(&["Garten", "Kunde Müller"]), &config)[1].1, muller);
        config.placement.enabled = false;
        assert_eq!(folder_for(&tags(&["Garten"]), &config), None);
        assert!(candidates(&tags(&["Garten"]), &config).is_empty());
    }

    #[test]
    fn proposals_for_files_in_the_landing_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib");
        let landing = root.join("Import");
        std::fs::create_dir_all(&landing).unwrap();
        std::fs::create_dir_all(root.join("Garten")).unwrap();
        for (name, content) in [("rose.stl", "rose"), ("zaun.stl", "zaun"), ("ohne.stl", "ohne"), ("kopie.stl", "k")] {
            std::fs::write(landing.join(name), content).unwrap();
        }
        std::fs::write(root.join("Garten").join("kopie.stl"), "k").unwrap();
        let config = Config { library_roots: vec![root.clone()], ..Default::default() };
        let mut lib = Library::open_in_memory().unwrap();
        lib.finish_baseline(&root).unwrap();
        let in_landing: Vec<Entry> =
            ["rose.stl", "zaun.stl", "ohne.stl", "kopie.stl"].iter().map(|n| entry(&landing, n)).collect();
        lib.update_dir(&root, &root, 1, &[], &[landing.clone(), root.join("Garten")]).unwrap();
        lib.update_dir(&root, &landing, 1, &in_landing, &[]).unwrap();
        lib.update_dir(&root, &root.join("Garten"), 1, &[entry(&root.join("Garten"), "kopie.stl")], &[]).unwrap();
        for e in in_landing.iter().chain([entry(&root.join("Garten"), "kopie.stl")].iter()) {
            let hash = crate::dedupe::content_hash(&std::fs::read(&e.path).unwrap());
            lib.store_meta(&e.path, "k", &ThumbMeta { content_hash: Some(hash), ..Default::default() }).unwrap();
        }
        lib.tag(&[landing.join("rose.stl"), landing.join("zaun.stl"), landing.join("kopie.stl")], "Garten").unwrap();
        lib.tag(&[landing.join("rose.stl")], "Deko").unwrap();

        let paths: Vec<PathBuf> = in_landing.iter().map(|e| e.path.clone()).collect();
        let (items, skipped) = proposals_for(&lib, &config, &paths).unwrap();
        assert_eq!(items.len(), 2, "{skipped:?}");
        assert_eq!(items[0].reason.as_deref(), Some("Tag: Garten"), "Garten comes before Deko");
        let alternatives =
            |i: usize| serde_json::from_value::<MovePayload>(items[i].payload.clone()).unwrap().alternatives;
        assert_eq!(alternatives(0), [root.join("Deko").join("rose.stl")], "Deko offered as well");
        assert!(alternatives(1).is_empty(), "zaun has one tag");
        assert_eq!(
            skipped,
            [
                Skipped { path: landing.join("ohne.stl"), why: "no_rule" },
                Skipped { path: landing.join("kopie.stl"), why: "duplicate" }
            ]
        );
        let mut store = ProposalStore::open_in_memory().unwrap();
        assert_eq!(propose(&mut store, "Ablage", items).unwrap(), 2);
        let batches = store.batches(Some(SOURCE), None, &["pending"], 9).unwrap();
        assert_eq!((batches.len(), batches[0].kind.as_str(), batches[0].items), (1, "move", 2));
        // Suggested again while that waits: nothing new.
        let (items, _) = proposals_for(&lib, &config, &paths).unwrap();
        assert_eq!(propose(&mut store, "Ablage", items).unwrap(), 0);
        // Accepted, the files are in Garten with their tags.
        let ops: Vec<_> =
            store.proposals(batches[0].id).unwrap().iter().flat_map(|p| to_ops(p, &Picked::all(p), false)).collect();
        let report = lib.apply_change("proposal:p", "Ablage", &ops).unwrap();
        assert_eq!(report.applied, 2);
        assert!(root.join("Garten").join("rose.stl").is_file() && !landing.join("rose.stl").exists());
        assert_eq!(lib.tags_of(&[root.join("Garten").join("zaun.stl")]).unwrap()[0].0, "Garten");
        let (items, skipped) = proposals_for(&lib, &config, &[root.join("Garten").join("rose.stl")]).unwrap();
        assert!(items.is_empty() && skipped[0].why == "there_already");
    }
}
