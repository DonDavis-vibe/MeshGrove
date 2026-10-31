//! For writing a plugin in Rust: the bindings of the WIT world and a few helpers. Build the
//! plugin as a `cdylib` for `wasm32-wasip2` and export it with [`export_plugin!`]:
//!
//! ```ignore
//! use meshgrove_plugin_api::guest::{Guest, GuestPanel, PluginError, ToolOutput, MeshData};
//!
//! struct MyPlugin;
//!
//! impl Guest for MyPlugin {
//!     type Panel = MyPanel;
//!     fn manifest() -> String { /* JSON of a Manifest */ }
//!     // …
//! }
//!
//! meshgrove_plugin_api::export_plugin!(MyPlugin);
//! ```
//!
//! [`export_plugin!`]: crate::export_plugin

wit_bindgen::generate!({
    path: "wit",
    world: "meshgrove-plugin",
    pub_export_macro: true,
    export_macro_name: "export_plugin",
    default_bindings_module: "meshgrove_plugin_api::guest",
    runtime_path: "meshgrove_plugin_api::guest::wit_bindgen::rt",
});

/// The runtime the generated bindings use, so plugins need not depend on `wit-bindgen`.
pub use wit_bindgen;

pub use exports::meshgrove::plugin::plugin::{Guest, GuestPanel, Panel, PanelBorrow};
pub use meshgrove::plugin::host::{self, Level};
pub use meshgrove::plugin::library;
pub use meshgrove::plugin::types::{Image, MeshData, PluginError, ToolOutput};

use serde::Serialize;

use crate::bundle::Bundle;
use crate::manifest::HostInfo;
use crate::ui::Effect;

impl From<crate::MeshData> for MeshData {
    fn from(m: crate::MeshData) -> MeshData {
        MeshData { positions: m.positions, indices: m.indices }
    }
}

impl From<MeshData> for crate::MeshData {
    fn from(m: MeshData) -> crate::MeshData {
        crate::MeshData { positions: m.positions, indices: m.indices }
    }
}

impl From<serde_json::Error> for PluginError {
    fn from(e: serde_json::Error) -> PluginError {
        PluginError::Invalid(e.to_string())
    }
}

impl From<std::io::Error> for PluginError {
    fn from(e: std::io::Error) -> PluginError {
        PluginError::Failed(e.to_string())
    }
}

/// What `host.info()` says: language, theme, printers.
pub fn info() -> HostInfo {
    serde_json::from_str(&host::info()).unwrap_or_default()
}

pub fn log(level: Level, message: &str) {
    host::log(level, message);
}

/// Arguments of a tool call or job, as JSON.
pub fn parse<T: serde::de::DeserializeOwned>(json: &str) -> Result<T, PluginError> {
    Ok(serde_json::from_str(json)?)
}

/// JSON text of anything serializable (manifests, tool lists, views, results).
pub fn json<T: Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".into())
}

/// The answer of `panel.event()`.
pub fn effects(effects: &[Effect]) -> String {
    json(effects)
}

/// A tool's answer: `result` for the AI, optionally with a bundle of what it wrote below `/out`.
pub fn tool_output<T: Serialize + ?Sized>(result: &T, bundle: Option<&Bundle>) -> ToolOutput {
    ToolOutput { result: json(result), images: Vec::new(), bundle: bundle.map(json) }
}

/// Writes `mesh` as binary STL to `path` (below `/out` for a bundle).
pub fn write_stl(path: &str, mesh: &crate::MeshData, name: &str) -> std::io::Result<()> {
    if let Some(dir) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, mesh.to_stl(name))
}
