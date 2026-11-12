//! The review of AI suggestions and the history of accepted changes: building the views from
//! the suggestion store and the index, and what their buttons do.

use std::time::{Duration, SystemTime};

use meshgrove_core::ai::proposals::{
    self, BatchInfo, Decision, DeletePayload, MovePayload, Picked, ProposalKind, ProposalStore, Target,
};
use meshgrove_core::ai::snippets::Client;
use meshgrove_core::changes::TargetState;
use meshgrove_core::config::Config;
use meshgrove_core::moves::MoveProblem;

use super::{App, View};
use crate::library::Applied;
use crate::proposals::ProposalWatcher;
use crate::ui::review::{HistoryAction, ItemDraft, ReviewAction, ReviewBatch, ReviewItem};

impl App {
    /// Starts or stops watching for suggestions as AI (or placement by tag) is switched on or off.
    pub(super) fn watch_proposals(&mut self, ctx: &egui::Context) {
        let on = self.reviews();
        if on && self.proposals.is_none() {
            if let Some(path) = ProposalStore::default_location() {
                let ai = &self.config.ai;
                self.proposals = Some(ProposalWatcher::start(path, ctx.clone(), (ai.log_days, ai.expire_days)));
            }
        } else if !on {
            self.proposals = None;
        }
        // New suggestions while the review is open: show them.
        if let Some(w) = &self.proposals
            && w.generation() != self.proposals_seen
        {
            self.proposals_seen = w.generation();
            self.chat_changed(); // the chat's cards show what became of their suggestions
            self.auto_accept();
            if matches!(self.view, View::Review { .. }) {
                self.show_review();
            }
        }
    }

    /// Suggestions can arrive: from AI, or from placing files by tag.
    pub(super) fn reviews(&self) -> bool {
        self.config.ai.enabled || self.config.placement.enabled
    }

    /// Accepts the suggestions made since the user let Meshgrove do so (Settings → AI): all but
    /// moves of files outside the landing folder, which always wait, and not the chat's, which
    /// has its own switch. Each batch once: the decisions are recorded only after the library
    /// applied them.
    fn auto_accept(&mut self) {
        let Some(since) = self.config.ai.auto_accept_since else { return };
        let since = SystemTime::UNIX_EPOCH + Duration::from_secs(since);
        let (_, batches) = self.pending_batches();
        for b in batches {
            if b.chat || b.created.is_none_or(|c| c < since) || !self.auto_accepted.insert(b.id) {
                continue;
            }
            let mut ops = Vec::new();
            let mut decisions = Vec::new();
            for item in b.items.iter().filter(|i| auto_acceptable(i, &self.config)) {
                let decision = if item.stale {
                    Decision::Stale
                } else {
                    ops.extend(proposals::to_ops(&item.proposal, &Picked::all(&item.proposal), b.inbox_done));
                    Decision::Accepted
                };
                decisions.push((item.proposal.id, decision));
            }
            if !decisions.is_empty() {
                self.library.accept(b.id, b.uid, b.title, ops, decisions);
            }
        }
    }

    /// Deletes the suggestion store with the access log ("KI-Daten löschen"). The journal of
    /// accepted changes stays with the library, so they can still be undone.
    pub(super) fn clear_ai_data(&mut self) {
        self.proposals = None; // the watcher lets go of the file
        let Some(path) = ProposalStore::default_location() else { return };
        let mut failed = None;
        for suffix in ["", "-wal", "-shm"] {
            let file = std::path::PathBuf::from(format!("{}{suffix}", path.display()));
            if let Err(e) = std::fs::remove_file(&file)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                failed = Some(e);
            }
        }
        self.toast(match failed {
            Some(e) => t!("settings.ai_cleared_failed", error = e.to_string()).into_owned(),
            None => t!("settings.ai_cleared").into_owned(),
        });
        if matches!(self.view, View::Review { .. }) {
            self.show_review();
        }
    }

    pub(super) fn pending_proposals(&self) -> usize {
        self.proposals.as_ref().map_or(0, ProposalWatcher::pending)
    }

    pub(super) fn show_review(&mut self) {
        // Opened (not shown anew): its tab comes to the front.
        if !matches!(self.view, View::Review { .. }) {
            self.layout.focus_files();
        }
        let (entries, batches) = self.pending_batches();
        self.leave_search();
        self.flatten = false;
        self.view = View::Review { entries, batches };
        self.refresh_gallery(true);
    }

    /// The suggestions waiting, by batch, with the files they are about (still there, moved or
    /// gone) as entries.
    fn pending_batches(&self) -> (Vec<meshgrove_core::listing::Entry>, Vec<ReviewBatch>) {
        let store = ProposalStore::default_location().filter(|p| p.exists()).and_then(|p| ProposalStore::open(&p).ok());
        let mut entries = Vec::new();
        let mut batches = Vec::new();
        if let (Some(store), Some(index)) = (&store, &self.index) {
            for b in store.batches(None, None, &["pending", "partly"], 100).unwrap_or_default() {
                let mut items = Vec::new();
                for proposal in store.proposals(b.id).unwrap_or_default().into_iter().filter(|p| p.state == "pending") {
                    let (mut entry, mut moved_to, mut stale, mut copies, mut note_before) =
                        (None, None, false, 0, None);
                    let mut taken = false;
                    if proposal.kind == ProposalKind::Move {
                        // A move works on the place it names, not on the content.
                        let payload: Option<MovePayload> = serde_json::from_value(proposal.payload.clone()).ok();
                        if let (Some(t), Some(m)) = (&proposal.target, payload) {
                            match meshgrove_core::moves::check_move(t, &m.to, m.folder) {
                                Ok(()) => {}
                                Err(MoveProblem::Taken) => taken = true,
                                Err(MoveProblem::Gone) => stale = true,
                            }
                            if let Some(e) = index.entry_of(&t.path).ok().flatten().filter(|_| !m.folder) {
                                entry = Some(entries.len());
                                entries.push(e);
                            }
                        }
                        let picked = Picked::all(&proposal);
                        let stale = stale || taken;
                        items.push(ReviewItem {
                            proposal,
                            entry,
                            moved_to,
                            stale,
                            copies,
                            picked,
                            note_before,
                            taken,
                            draft: ItemDraft::default(),
                        });
                        continue;
                    }
                    if proposal.kind == ProposalKind::Delete {
                        // A deletion works on the place it names, as a move does.
                        let payload: Option<DeletePayload> = serde_json::from_value(proposal.payload.clone()).ok();
                        if let (Some(t), Some(d)) = (&proposal.target, payload) {
                            stale = !still_there(t, d.folder);
                            if let Some(e) = index.entry_of(&t.path).ok().flatten().filter(|_| !d.folder) {
                                entry = Some(entries.len());
                                entries.push(e);
                            }
                        }
                        let picked = Picked::all(&proposal);
                        items.push(ReviewItem {
                            proposal,
                            entry,
                            moved_to,
                            stale,
                            copies,
                            picked,
                            note_before,
                            taken,
                            draft: ItemDraft::default(),
                        });
                        continue;
                    }
                    if let Some(t) = &proposal.target {
                        let state = index.resolve_target(t).unwrap_or(TargetState::Stale);
                        let path = match &state {
                            TargetState::Here => Some(t.path.clone()),
                            TargetState::Moved(p) => Some(p.clone()),
                            TargetState::Stale => None,
                        };
                        stale = state == TargetState::Stale;
                        if let TargetState::Moved(p) = state {
                            moved_to = Some(p);
                        }
                        if let Some(e) = path.and_then(|p| index.entry_of(&p).ok().flatten()) {
                            note_before = index.notes_of(std::slice::from_ref(&e.path)).ok().and_then(|n| n.0);
                            entry = Some(entries.len());
                            entries.push(e);
                        }
                        copies = t.hash.as_deref().and_then(|h| index.copies_of(h).ok()).unwrap_or(0);
                    }
                    let picked = Picked::all(&proposal);
                    items.push(ReviewItem {
                        proposal,
                        entry,
                        moved_to,
                        stale,
                        copies,
                        picked,
                        note_before,
                        taken,
                        draft: ItemDraft::default(),
                    });
                }
                if !items.is_empty() {
                    batches.push(review_batch(&b, items));
                }
            }
        }
        (entries, batches)
    }

    pub(super) fn review_action(&mut self, action: Option<ReviewAction>) {
        let View::Review { batches, .. } = &mut self.view else { return };
        match action {
            Some(ReviewAction::Select(i)) => self.gallery.select(Some(i)),
            Some(ReviewAction::Open(i)) => self.open_entry(i),
            Some(ReviewAction::Accept { batch, all }) => {
                let b = batches.remove(batch);
                let mut ops = Vec::new();
                let mut decisions = Vec::new();
                // Deletions go to the system's trash, not through the library's journal.
                let mut trash = Vec::new();
                for item in &b.items {
                    let picked = if all { Picked::all(&item.proposal) } else { item.picked.clone() };
                    let decision = if item.stale {
                        Decision::Stale
                    } else if picked.is_empty() {
                        Decision::Rejected
                    } else if item.proposal.kind == ProposalKind::Delete {
                        match item.proposal.target.as_ref().map(|t| t.path.clone()) {
                            Some(path) if crate::actions::trash_blocked(std::slice::from_ref(&path)).is_none() => {
                                trash.push(path);
                                Decision::Accepted
                            }
                            _ => Decision::Stale,
                        }
                    } else {
                        ops.extend(proposals::to_ops(&item.proposal, &picked, b.inbox_done));
                        Decision::Accepted
                    };
                    decisions.push((item.proposal.id, decision));
                }
                if !trash.is_empty() {
                    self.toast(t!("review.trashing", title = b.title.clone(), count = trash.len()).into_owned());
                    self.trash_later(trash);
                }
                if ops.is_empty() {
                    self.library.decide(b.id, decisions);
                } else {
                    self.library.accept(b.id, b.uid, b.title, ops, decisions);
                }
            }
            Some(ReviewAction::Reject(batch)) => {
                let b = batches.remove(batch);
                let decisions = b.items.iter().map(|i| (i.proposal.id, Decision::Rejected)).collect();
                self.library.decide(b.id, decisions);
                self.toast(t!("review.rejected", title = b.title).into_owned());
            }
            Some(ReviewAction::History) => self.show_history(),
            None => {}
        }
    }

    pub(super) fn show_history(&mut self) {
        if !matches!(self.view, View::History { .. }) {
            self.layout.focus_files();
        }
        let changes = self.index.as_ref().and_then(|i| i.changes(200).ok()).unwrap_or_default();
        self.leave_search();
        self.flatten = false;
        self.view = View::History { changes };
        self.refresh_gallery(true);
    }

    pub(super) fn history_action(&mut self, action: Option<HistoryAction>) {
        match action {
            Some(HistoryAction::Undo(id)) => self.library.undo(id),
            Some(HistoryAction::Redo(id)) => self.library.redo(id),
            Some(HistoryAction::Review) => self.show_review(),
            None => {}
        }
    }

    /// Says what came of accepting, undoing or redoing, and shows the views anew.
    pub(super) fn applied(&mut self, applied: Applied) {
        #[cfg(feature = "plugins")]
        if let Some((from, title, outcome)) = crate::plugins::bundle_outcome(&applied) {
            self.plugins.bundle_finished(&from.0, &from.1, title, outcome);
        }
        let message = match applied {
            Applied::Accepted { title, report } if report.skipped > 0 => {
                t!("review.accepted_skipped", title = title, count = report.applied, skipped = report.skipped)
            }
            Applied::Accepted { title, report } => t!("review.accepted", title = title, count = report.applied),
            Applied::Bundle { title, report, .. } => {
                t!("plugins.bundle_applied", title = title, count = report.applied)
            }
            Applied::BundleFailed { error, .. } => t!("plugins.bundle_failed", error = error),
            Applied::Undone { title, report } if report.kept > 0 => {
                t!("history.undone_kept", title = title, kept = report.kept)
            }
            Applied::Undone { title, .. } => t!("history.undone", title = title),
            Applied::Redone { title, report } => t!("history.redone", title = title, count = report.applied),
            Applied::Failed(e) => t!("review.failed", error = e),
        };
        self.toast(message.into_owned());
        // A move may have taken the open folder away: show the folder it was in.
        if self.nav.current().symlink_metadata().is_err()
            && let Some(up) = self.nav.current().ancestors().find(|a| a.is_dir()).map(std::path::Path::to_path_buf)
        {
            self.nav.open(up);
        } else {
            // The library wrote, moved or removed these files itself and put them into the index
            // at once, so the next scan finds nothing new and does not read the folder again: a
            // plugin's file beside its original, or one undone, would stay unseen until a reload.
            self.reload();
        }
        match self.view {
            View::Review { .. } => self.show_review(),
            View::History { .. } => self.show_history(),
            View::Import { .. } => self.start_import_scan(),
            _ => {}
        }
    }
}

/// What may be accepted without the review: everything but a move and a plugin's files, a move
/// that takes a file out of the landing folder when one place fits (the user's own folders are
/// never rearranged by themselves, and nothing is guessed when several folders would do), and
/// files of a plugin the user lets them be accepted by themselves.
pub(super) fn auto_acceptable(item: &ReviewItem, config: &Config) -> bool {
    if item.proposal.kind == ProposalKind::Bundle {
        // A tool's job made it after the AI had its answer: the user looks at it first.
        if item.proposal.payload["review"].as_bool() == Some(true) {
            return false;
        }
        let plugin = item.proposal.payload["plugin"].as_str().unwrap_or_default();
        return config.plugin(plugin).is_some_and(|p| p.enabled && p.grants.bundles && p.grants.auto_accept);
    }
    if item.proposal.kind == ProposalKind::Delete {
        return false;
    }
    if item.proposal.kind != ProposalKind::Move {
        return true;
    }
    let out_of_landing = item.proposal.target.as_ref().is_some_and(|t| config.in_landing(&t.path));
    let clear =
        serde_json::from_value::<MovePayload>(item.proposal.payload.clone()).is_ok_and(|m| m.alternatives.is_empty());
    out_of_landing && clear
}

/// Whether what a deletion names is still there as it was (a folder: at all).
fn still_there(t: &Target, folder: bool) -> bool {
    let Ok(meta) = t.path.symlink_metadata() else { return false };
    meta.is_dir() == folder
        && (folder || (meta.len() == t.size && meshgrove_core::db::nanos(meta.modified().ok()) == t.mtime_ns))
}

/// "Claude Desktop · lokal", "Chat · Cloud", "Meshgrove"
fn source_label(b: &BatchInfo) -> String {
    let who = match (b.source.as_str(), b.client.as_deref()) {
        (_, Some(id)) => {
            Client::ALL.into_iter().find(|c| c.id() == id).map_or_else(|| id.to_owned(), |c| c.name().to_owned())
        }
        ("chat", None) => t!("review.source_chat").into_owned(),
        (source, None) if source.starts_with("plugin:") => {
            return t!("review.source_plugin", name = &source["plugin:".len()..]).into_owned();
        }
        (meshgrove_core::placement::SOURCE, None) => return t!("review.source_meshgrove").into_owned(),
        _ => t!("review.source_assistant").into_owned(),
    };
    let class = if b.access_class == "local" { t!("review.local") } else { t!("review.cloud") };
    format!("{who} · {class}")
}

fn review_batch(b: &BatchInfo, items: Vec<ReviewItem>) -> ReviewBatch {
    ReviewBatch {
        id: b.id,
        uid: b.uid.clone(),
        title: b.title.clone(),
        source: source_label(b),
        created: (b.created_ns > 0).then(|| SystemTime::UNIX_EPOCH + Duration::from_nanos(b.created_ns as u64)),
        inbox_done: b.inbox_done,
        chat: b.source == "chat" || b.source.starts_with("plugin:"),
        items,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meshgrove_core::ai::proposals::{Proposal, Target};
    use std::path::{Path, PathBuf};

    fn item(kind: ProposalKind, path: &str) -> ReviewItem {
        item_with(kind, path, Vec::new())
    }

    fn item_with(kind: ProposalKind, path: &str, alternatives: Vec<PathBuf>) -> ReviewItem {
        let target = Target { path: PathBuf::from(path), hash: None, size: 1, mtime_ns: 1 };
        let payload = MovePayload {
            root: PathBuf::from("/lib"),
            to: PathBuf::from("/lib/Garten/rose.stl"),
            folder: false,
            alternatives,
        };
        let proposal = Proposal {
            id: 1,
            batch_id: 1,
            kind,
            target: Some(target),
            payload: serde_json::to_value(payload).unwrap(),
            reason: None,
            confidence: None,
            state: "pending".into(),
        };
        ReviewItem {
            proposal,
            entry: None,
            moved_to: None,
            stale: false,
            copies: 0,
            picked: Picked::default(),
            note_before: None,
            taken: false,
            draft: ItemDraft::default(),
        }
    }

    #[test]
    fn deletions_never_apply_by_themselves() {
        let config = Config::default();
        assert!(!auto_acceptable(&item(ProposalKind::Delete, "/lib/Import/a.stl"), &config));
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.stl");
        std::fs::write(&file, b"solid").unwrap();
        let meta = file.metadata().unwrap();
        let t = Target {
            path: file.clone(),
            hash: None,
            size: 5,
            mtime_ns: meshgrove_core::db::nanos(meta.modified().ok()),
        };
        assert!(still_there(&t, false));
        assert!(!still_there(&t, true), "a file is no folder");
        std::fs::write(&file, b"solid changed").unwrap();
        assert!(!still_there(&t, false), "changed since");
        let folder = Target { path: dir.path().to_path_buf(), hash: None, size: 0, mtime_ns: 0 };
        assert!(still_there(&folder, true));
    }

    #[test]
    fn only_moves_out_of_the_landing_folder_are_accepted_by_themselves() {
        let config = Config { library_roots: vec![PathBuf::from("/lib")], ..Default::default() };
        assert!(auto_acceptable(&item(ProposalKind::Tags, "/lib/Garten/rose.stl"), &config));
        assert!(auto_acceptable(&item(ProposalKind::Move, "/lib/Import/rose.stl"), &config));
        assert!(!auto_acceptable(&item(ProposalKind::Move, "/lib/Garten/rose.stl"), &config));
        assert!(!auto_acceptable(&item(ProposalKind::Move, "/lib/Import"), &config), "not the folder itself");
        let several = item_with(ProposalKind::Move, "/lib/Import/rose.stl", vec![PathBuf::from("/lib/Deko/rose.stl")]);
        assert!(!auto_acceptable(&several, &config), "several folders fit: the user chooses");
        assert!(Path::new("/lib/Import/rose.stl").starts_with("/lib/Import"));
    }

    #[test]
    fn plugin_files_are_accepted_by_themselves_only_with_the_plugins_grant() {
        use meshgrove_core::config::{PluginConfig, PluginGrants};
        let mut bundle = item(ProposalKind::Bundle, "/lib/Import");
        bundle.proposal.target = None;
        bundle.proposal.payload = serde_json::json!({"plugin": "bingrove"});
        let grants = PluginGrants { ai_tools: true, bundles: true, auto_accept: false, ..Default::default() };
        let entry = PluginConfig {
            id: "bingrove".into(),
            file: PathBuf::from("/p/bingrove.wasm"),
            blake3: "h".into(),
            enabled: true,
            grants,
            memory_mb: None,
        };
        let mut config =
            Config { library_roots: vec![PathBuf::from("/lib")], plugins: vec![entry], ..Default::default() };
        assert!(!auto_acceptable(&bundle, &config), "waits for the review");
        config.plugins[0].grants.auto_accept = true;
        assert!(auto_acceptable(&bundle, &config));
        bundle.proposal.payload["review"] = serde_json::json!(true);
        assert!(!auto_acceptable(&bundle, &config), "a tool's job: always the review");
        config.plugins[0].enabled = false;
        assert!(!auto_acceptable(&bundle, &config), "switched off");
        assert!(!auto_acceptable(&bundle, &Config::default()), "unknown plugin");
    }
}
