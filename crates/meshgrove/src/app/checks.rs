//! Feeding the background printability checks and storing their results.

use std::path::Path;
use std::time::{Duration, Instant};

use meshgrove_core::checks::MeshChecks;

use super::App;
use crate::checker::CheckJob;

/// Files handed to the checker at once.
const BATCH: usize = 100;
/// Results stored together, at most this far apart.
const FLUSH: Duration = Duration::from_secs(2);
/// When everything is checked, look again this much later (new files arrive with their checks).
const RECHECK: Duration = Duration::from_secs(30);

impl App {
    /// A thumbnail just made carries its check: show it for the selected file at once, before
    /// the library has stored it.
    pub(super) fn note_selection_checks(&mut self, path: &Path, checks: Option<&MeshChecks>) {
        if let (Some(c), [one]) = (checks, self.menu_for.as_slice())
            && one == path
        {
            self.selection_checks = Some(c.clone());
        }
    }

    pub(super) fn poll_checks(&mut self) {
        let idle = self.thumbs.is_idle();
        self.checker.set_idle(idle);
        while let Some(result) = self.checker.try_recv() {
            self.checks_done.push(result);
        }
        let flush = self.checks_done.len() >= BATCH || self.checks_flushed.elapsed() >= FLUSH || self.checker.is_empty();
        if !self.checks_done.is_empty() && flush {
            self.library.store_checks(std::mem::take(&mut self.checks_done));
            self.checks_flushed = Instant::now();
        }
        if !idle || !self.checker.is_empty() || Instant::now() < self.checks_next {
            return;
        }
        let Some(index) = &self.index else { return };
        let jobs: Vec<CheckJob> = index
            .needing_checks(BATCH)
            .unwrap_or_default()
            .into_iter()
            // A result may still be on its way to the library: do not check it twice.
            .filter(|(_, hash)| self.checks_queued.insert(hash.clone()))
            .map(|(entry, hash)| CheckJob { path: entry.path, kind: entry.kind, hash })
            .collect();
        if jobs.is_empty() {
            self.checks_next = Instant::now() + RECHECK;
        } else {
            log::info!("checking {} models for printability in the background", jobs.len());
            self.checker.queue(jobs);
        }
    }
}
