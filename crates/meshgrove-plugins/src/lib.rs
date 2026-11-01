//! Meshgrove's plugin host: plugins are WebAssembly components (`wasm32-wasip2`, the WIT world
//! of [`meshgrove_plugin_api`]) run by wasmtime in a sandbox — no network, no environment, only
//! their own `/data`, a staging folder `/out` and `/tmp`, a memory limit and deadlines.
//!
//! One [`engine`] per process compiles them (compiled code is cached on disk). Every tool call
//! and job gets a fresh instance ([`Plugin::call_tool`], [`jobs::JobPool`]); an open panel keeps
//! its instance on a thread of its own ([`panel::PanelHandle`]). What the user granted
//! (`[[plugins]]` in config.toml) is checked on every call, not once when loading.
//!
//! The library changes only through bundles: files a plugin wrote into a staging folder,
//! checked by [`meshgrove_core::bundle::resolve`] and applied as one change that can be undone.

mod bindings;
pub mod engine;
#[cfg(feature = "panels")]
pub mod jobs;
pub mod library;
#[cfg(feature = "panels")]
pub mod panel;
mod plugin;
pub mod registry;
pub mod staging;
mod state;

use std::time::Duration;

pub use library::{LibraryAccess, LibraryReader};
pub use meshgrove_core::config::{LibraryRead, PluginGrants};
pub use plugin::{Image, Plugin, Settings, StagedBundle, ToolOutcome};
pub use registry::Registry;
pub use staging::Staging;
pub use state::{CancelToken, Progress};

/// How long a panel may take for one event (render, mesh and snapshot included).
pub const EVENT_DEADLINE: Duration = Duration::from_secs(2);
/// How long a tool call may take.
pub const TOOL_DEADLINE: Duration = Duration::from_secs(60);
/// Memory of one instance when neither the user nor the plugin says otherwise.
pub const DEFAULT_MEMORY_MB: u32 = 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file is no usable plugin (not a component, another API, a bad manifest).
    #[error("{0}")]
    Load(String),
    /// Something the user did not grant (or the manifest did not ask for).
    #[error("not allowed: {0}")]
    Denied(String),
    /// The plugin found the arguments wrong.
    #[error("{0}")]
    Invalid(String),
    #[error("the plugin failed: {0}")]
    Failed(String),
    #[error("cancelled")]
    Cancelled,
    #[error("the plugin took longer than {0:?}")]
    Timeout(Duration),
    #[error("the plugin needed more than its {0} MiB of memory")]
    Memory(u32),
    /// The plugin crashed (a trap: a panic, a bug); its instance is gone.
    #[error("the plugin stopped with an error: {0}")]
    Trap(String),
    #[error(transparent)]
    Bundle(#[from] meshgrove_core::bundle::BundleError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// `data_dir/meshgrove/plugins`: where plugins are installed, with their data, staging folders
/// and the cache of compiled code.
pub fn default_base() -> Option<std::path::PathBuf> {
    Some(dirs::data_dir()?.join("meshgrove").join("plugins"))
}
