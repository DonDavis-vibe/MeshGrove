//! The inbox view: building it from the index, and what its buttons do.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use meshgrove_core::listing;

use super::{App, View};
use crate::assist::actions::{Action, TagGroup};
use crate::i18n;
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
                folder: g.folder.clone(),
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
        let View::Inbox { entries, groups } = &self.view else { return };
        match action {
            Some(InboxAction::Select(i)) => self.gallery.select(Some(i)),
            Some(InboxAction::Open(i)) => self.open_entry(i),
            Some(InboxAction::Done(indices)) => {
                let paths: Vec<PathBuf> = indices.iter().filter_map(|&i| entries.get(i)).map(|e| e.path.clone()).collect();
                self.library.inbox_done(Some(paths));
            }
            Some(InboxAction::AllDone) => self.library.inbox_done(None),
            Some(InboxAction::Classify(which)) => {
                let picked: Vec<&InboxGroupInfo> = match which {
                    Some(n) => groups.get(n).into_iter().collect(),
                    None => groups.iter().collect(),
                };
                let tag_groups: Vec<TagGroup> = picked
                    .iter()
                    .map(|g| TagGroup {
                        title: t!("actions.inbox_batch", folder = g.title).into_owned(),
                        folder: Some(g.folder.clone()),
                        paths: g.range.clone().filter_map(|i| entries.get(i)).map(|e| e.path.clone()).collect(),
                    })
                    .collect();
                let files: usize = tag_groups.iter().map(|g| g.paths.len()).sum();
                let (title, user_text) = match picked.as_slice() {
                    [one] => (
                        t!("actions.inbox_chat", folder = one.title).into_owned(),
                        t!("actions.inbox_one", folder = one.title, count = files).into_owned(),
                    ),
                    _ => (
                        t!("actions.inbox_chat", folder = i18n::date(SystemTime::now())).into_owned(),
                        t!("actions.inbox_all", groups = picked.len(), count = files).into_owned(),
                    ),
                };
                let llm = format!(
                    "(One-click action) Suggest tags for {files} new files in the inbox ({} drops); the suggestions \
                     wait in the review, and accepting them takes the files out of the inbox.",
                    picked.len()
                );
                let action = Action::Tags { groups: tag_groups, inbox_done: true };
                self.start_action(title, user_text, llm, files, action, false);
            }
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
