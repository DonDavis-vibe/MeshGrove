//! The review of AI suggestions and the history of accepted changes: building the views from
//! the suggestion store and the index, and what their buttons do.

use std::time::{Duration, SystemTime};

use meshgrove_core::ai::proposals::{self, BatchInfo, Decision, MovePayload, Picked, ProposalKind, ProposalStore};
use meshgrove_core::moves::MoveProblem;
use meshgrove_core::ai::snippets::Client;
use meshgrove_core::changes::TargetState;

use super::{App, View};
use crate::library::Applied;
use crate::proposals::ProposalWatcher;
use crate::ui::review::{HistoryAction, ReviewAction, ReviewBatch, ReviewItem};

impl App {
    /// Starts or stops watching for suggestions as AI is switched on or off.
    pub(super) fn watch_proposals(&mut self, ctx: &egui::Context) {
        let on = self.config.ai.enabled;
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
            if matches!(self.view, View::Review { .. }) {
                self.show_review();
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
        let store = ProposalStore::default_location().filter(|p| p.exists()).and_then(|p| ProposalStore::open(&p).ok());
        let mut entries = Vec::new();
        let mut batches = Vec::new();
        if let (Some(store), Some(index)) = (&store, &self.index) {
            for b in store.batches(None, None, &["pending", "partly"], 100).unwrap_or_default() {
                let mut items = Vec::new();
                for proposal in store.proposals(b.id).unwrap_or_default().into_iter().filter(|p| p.state == "pending") {
                    let (mut entry, mut moved_to, mut stale, mut copies, mut note_before) = (None, None, false, 0, None);
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
                        items.push(ReviewItem { proposal, entry, moved_to, stale, copies, picked, note_before, taken });
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
                    items.push(ReviewItem { proposal, entry, moved_to, stale, copies, picked, note_before, taken });
                }
                if !items.is_empty() {
                    batches.push(review_batch(&b, items));
                }
            }
        }
        self.leave_search();
        self.flatten = false;
        self.view = View::Review { entries, batches };
        self.refresh_gallery(true);
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
                for item in &b.items {
                    let picked = if all { Picked::all(&item.proposal) } else { item.picked.clone() };
                    let decision = if item.stale {
                        Decision::Stale
                    } else if picked.is_empty() {
                        Decision::Rejected
                    } else {
                        ops.extend(proposals::to_ops(&item.proposal, &picked, b.inbox_done));
                        Decision::Accepted
                    };
                    decisions.push((item.proposal.id, decision));
                }
                self.library.accept(b.id, b.uid, b.title, ops, decisions);
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
        let message = match applied {
            Applied::Accepted { title, report } if report.skipped > 0 => {
                t!("review.accepted_skipped", title = title, count = report.applied, skipped = report.skipped)
            }
            Applied::Accepted { title, report } => t!("review.accepted", title = title, count = report.applied),
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
        }
        match self.view {
            View::Review { .. } => self.show_review(),
            View::History { .. } => self.show_history(),
            _ => {}
        }
    }
}

/// "Claude Desktop · lokal", "Chat · Cloud"
fn source_label(b: &BatchInfo) -> String {
    let who = match (b.source.as_str(), b.client.as_deref()) {
        (_, Some(id)) => Client::ALL.into_iter().find(|c| c.id() == id).map_or_else(|| id.to_owned(), |c| c.name().to_owned()),
        ("chat", None) => t!("review.source_chat").into_owned(),
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
        items,
    }
}
