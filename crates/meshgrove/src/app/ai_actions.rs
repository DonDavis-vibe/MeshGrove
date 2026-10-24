//! The one-click AI actions outside the chat pane: tags for the selection (as suggestions, or as
//! ✧ candidates in the tag editor), a list from a description, and references for an assistant.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crossbeam_channel::{Receiver, bounded};
use meshgrove_core::ai::Scope;
use meshgrove_core::config::AccessClass;
use meshgrove_core::listing;

use super::App;
use super::chat::Pending;
use crate::assist::actions::{self, Action, TagGroup};
use crate::assist::secret;
use crate::i18n;

/// The AI's tags for the tag editor: for which files, and those that came (or are coming).
#[derive(Default)]
pub struct AiTags {
    paths: Vec<PathBuf>,
    tags: Vec<String>,
    pending: Option<Receiver<Result<Vec<String>, String>>>,
}

impl AiTags {
    /// The tag editor's ✧ state for these files: the AI's tags and whether they are coming.
    pub(super) fn for_paths(&self, paths: &[PathBuf]) -> (&[String], bool) {
        if self.paths != paths {
            return (&[], false);
        }
        (&self.tags, self.pending.is_some())
    }
}

impl App {
    /// The chat's endpoint may see some of `paths`.
    pub(super) fn chat_sees(&self, paths: &[PathBuf]) -> bool {
        let scope = Scope::new(&self.config, self.chat_class());
        paths.iter().any(|p| scope.allows(p))
    }

    /// Some AI (a local one at least) may see some of `paths`.
    pub(super) fn ai_sees_any(&self, paths: &[PathBuf]) -> bool {
        let scope = Scope::new(&self.config, AccessClass::Local);
        paths.iter().any(|p| scope.allows(p))
    }

    /// Asks the AI for tags for these files; they wait in the review.
    pub(super) fn suggest_tags(&mut self, paths: Vec<PathBuf>) {
        let files = paths.len();
        let folder = paths.first().and_then(|p| p.parent()).filter(|f| paths.iter().all(|p| p.parent() == Some(*f)));
        let name =
            if files == 1 { listing::file_name(&paths[0]) } else { t!("chat.ref_files", count = files).into_owned() };
        let group = TagGroup {
            title: t!("actions.selection_batch", name = name).into_owned(),
            folder: folder.map(Path::to_path_buf),
            paths,
        };
        let title = t!("actions.selection_chat", name = name, date = i18n::date(SystemTime::now())).into_owned();
        let user_text = t!("actions.selection_text", name = name).into_owned();
        let llm =
            format!("(One-click action) Suggest tags for {files} selected files; the suggestions wait in the review.");
        self.start_action(title, user_text, llm, files, Action::Tags { groups: vec![group], inbox_done: false }, false);
    }

    /// A list (print list or collection) from what the user described.
    pub(super) fn list_from_description(&mut self, description: String, print: bool) {
        let short: String = description.trim().chars().take(40).collect();
        let title = t!("actions.list_chat", description = short).into_owned();
        let user_text = t!("actions.list_text", description = description.trim()).into_owned();
        let kind = if print { "print list" } else { "collection" };
        let llm = format!(
            "(One-click action) Make a {kind} from this description: \"{}\". The list waits in the review.",
            description.trim()
        );
        // What goes out is the library's tags and the files found: as many as a list may get.
        let files = meshgrove_core::ai::assist::MAX_CANDIDATES;
        self.start_action(title, user_text, llm, files, Action::List { description, print }, false);
    }

    /// `meshgrove:<id>` for each file an AI may see, one per line, for the user's assistant.
    pub(super) fn copy_references(&mut self, paths: &[PathBuf], ctx: &egui::Context) {
        let scope = Scope::new(&self.config, AccessClass::Local);
        let ids: Vec<String> =
            paths.iter().filter_map(|p| scope.id_of(p)).map(|id| format!("meshgrove:{id}")).collect();
        if ids.is_empty() {
            return;
        }
        ctx.copy_text(ids.join("\n"));
        self.toast(t!("actions.references_copied", count = ids.len()).into_owned());
    }

    /// ✧ in the tag editor: asks the AI for tags for the selection, on a thread of its own.
    pub(super) fn ask_tag_candidates(&mut self, confirmed: bool) {
        let Some(endpoint) = self.config.ai.endpoint.clone() else { return };
        let paths = self.menu_for.clone();
        if !endpoint.local && !self.chat.cloud_ok && !confirmed {
            let pictures = if endpoint.vision { paths.len() } else { 0 };
            let host = super::chat::host_of(&endpoint.base_url);
            let text = t!("actions.confirm_text", host = host, files = paths.len(), pictures = pictures);
            self.chat.confirm = Some((text.into_owned(), Pending::TagCandidates));
            return;
        }
        if confirmed {
            self.chat.cloud_ok = true;
        }
        let (tx, rx) = bounded(1);
        let (db, config, ctx) = (self.db_path.clone(), self.config.clone(), self.egui_ctx.clone());
        let key = secret::load(&endpoint.base_url, endpoint.key_in_keyring, &endpoint.key_env);
        let files = paths.clone();
        std::thread::Builder::new()
            .name("ai-tags".into())
            .spawn(move || {
                let _ = tx.send(actions::tag_candidates(&db, &config, key, &files));
                ctx.request_repaint();
            })
            .expect("spawning a thread");
        self.ai_tags = AiTags { paths, tags: Vec::new(), pending: Some(rx) };
    }

    /// Takes the AI's tags for the tag editor when they came.
    pub(super) fn poll_tag_candidates(&mut self) {
        let Some(rx) = &self.ai_tags.pending else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.ai_tags.pending = None;
        match result {
            Ok(tags) if tags.is_empty() => self.toast(t!("actions.no_tags").into_owned()),
            Ok(tags) => self.ai_tags.tags = tags,
            Err(e) => self.toast(t!("actions.tags_failed", error = e).into_owned()),
        }
    }
}
