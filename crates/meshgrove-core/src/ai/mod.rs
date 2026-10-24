//! The library as AI tools see it, shared by `meshgrove-mcp`, the chat and the one-click
//! actions: reading (always within a [`Scope`]) and suggesting changes, which wait in the
//! [`proposals`] store until the user accepts them in Meshgrove.
//!
//! Meshgrove brings no AI of its own; this is only what the user's AI may look at and ask for.

pub mod proposals;
pub mod read;
pub mod scope;
pub mod snippets;
#[cfg(test)]
mod tests;
pub mod types;
pub mod vocab;

use std::time::SystemTime;

pub use scope::{Scope, ScopedRoot, aliases, guess_local};

use crate::cache::ThumbCache;
use crate::config::Config;
use crate::db::Library;
use crate::printer::Printer;

/// Why a tool could not answer. The texts go to the AI, so they say what to do.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("AI access is switched off in Meshgrove (Settings → AI & MCP)")]
    Disabled,
    #[error("the Meshgrove library is not indexed yet: open Meshgrove once and let it scan")]
    NotIndexed,
    #[error(
        "the library index was made by another Meshgrove version ({db}, this one reads {ours}): \
         restart your AI client so it starts the meshgrove-mcp that came with the installed Meshgrove"
    )]
    VersionMismatch { db: i32, ours: i32 },
    /// Unknown or hidden: the same answer, so nothing is told about hidden libraries.
    #[error("unknown id: {0} (ids come from search_models, list_inbox or folder_summary)")]
    UnknownId(String),
    #[error("{0}")]
    Invalid(String),
    #[error("the suggestion store was made by a newer Meshgrove (version {0}): update Meshgrove")]
    StoreNewer(i32),
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("file: {0}")]
    Io(#[from] std::io::Error),
}

pub type ToolResult<T> = Result<T, ToolError>;

/// What the tools work on: the index (read-only is enough), what may be seen, the settings,
/// the thumbnail cache.
pub struct Context<'a> {
    pub lib: &'a Library,
    pub scope: Scope,
    pub config: &'a Config,
    pub thumbs: Option<&'a ThumbCache>,
    /// The printers, main one first.
    pub printers: Vec<Printer>,
    pub now: SystemTime,
}

impl<'a> Context<'a> {
    /// The `lib` must know the printers ([`Library::set_printers`]) for `fits_printer` searches.
    pub fn new(
        lib: &'a Library,
        config: &'a Config,
        class: crate::config::AccessClass,
        thumbs: Option<&'a ThumbCache>,
    ) -> ToolResult<Context<'a>> {
        let scope = Scope::new(config, class);
        if !config.ai.enabled {
            return Err(ToolError::Disabled);
        }
        Ok(Context { lib, scope, config, thumbs, printers: config.printers_in_order(), now: SystemTime::now() })
    }

    /// The file behind an id, if this scope may see it.
    pub fn resolve(&self, id: &str) -> ToolResult<std::path::PathBuf> {
        self.scope.resolve(id).ok_or_else(|| ToolError::UnknownId(id.to_owned()))
    }

    pub fn id_of(&self, path: &std::path::Path) -> String {
        self.scope.id_of(path).unwrap_or_default()
    }
}

/// Text from files and notes is the user's data, not instructions: marked as such for the AI.
pub fn untrusted(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        out.push_str(" …");
    }
    out
}
