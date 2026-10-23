//! The inbox view: building it from the index, and what its buttons do.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use meshgrove_core::listing;

use super::{App, View};
use crate::ui::inbox::{InboxAction, InboxGroupInfo};

/// Most files the view lists.
const LIMIT: usize = 2000;

impl App {
    pub(super) fn show_inbox(&mut self) {
        let Some(index) = &self.index else { return };
        let groups = index.inbox(&self.config.library_roots, LIMIT).unwrap_or_default();
        self.inbox_count = index.inbox_count().unwrap_or_default();
        let (mut entries, mut infos) = (Vec::new(), Vec::new());
        for g in groups {
            let start = entries.len();
            entries.extend(g.items.into_iter().map(|i| i.entry));
            infos.push(InboxGroupInfo {
                range: start..entries.len(),
                title: drop_title(&g.folder, &g.root),
                archive: meshgrove_core::archive::is_archive_file(&g.folder),
                newest: (g.newest_ns > 0).then(|| SystemTime::UNIX_EPOCH + Duration::from_nanos(g.newest_ns as u64)),
                known: g.known,
            });
        }
        self.leave_search();
        self.flatten = false;
        self.view = View::Inbox { entries, groups: infos };
        self.refresh_gallery(true);
    }

    pub(super) fn inbox_action(&mut self, action: Option<InboxAction>) {
        let View::Inbox { entries, .. } = &self.view else { return };
        match action {
            Some(InboxAction::Select(i)) => self.gallery.select(Some(i)),
            Some(InboxAction::Open(i)) => self.open_entry(i),
            Some(InboxAction::Done(indices)) => {
                let paths: Vec<PathBuf> = indices.iter().filter_map(|&i| entries.get(i)).map(|e| e.path.clone()).collect();
                self.library.inbox_done(Some(paths));
            }
            Some(InboxAction::AllDone) => self.library.inbox_done(None),
            None => {}
        }
    }

    /// Counts the inbox again a moment after new files were processed (they come in by then).
    pub(super) fn recount_inbox(&mut self) {
        let Some(due) = self.inbox_due else { return };
        if Instant::now() < due {
            return;
        }
        self.inbox_due = None;
        let Some(index) = &self.index else { return };
        let count = index.inbox_count().unwrap_or_default();
        if count != self.inbox_count && matches!(self.view, View::Inbox { .. }) {
            self.show_inbox();
        }
        self.inbox_count = count;
    }
}

/// "kunde-xyz/teile" (relative to the library), or the library's name for its top folder.
fn drop_title(folder: &Path, root: &Path) -> String {
    match folder.strip_prefix(root) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.display().to_string(),
        _ => listing::file_name(folder),
    }
}
