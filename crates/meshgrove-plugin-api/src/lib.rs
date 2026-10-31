//! The plugin interface of Meshgrove: what a plugin is (a WebAssembly component for
//! `wasm32-wasip2` with the world in [`WIT`]) and the JSON it exchanges with Meshgrove: its
//! [`Manifest`], the [`ToolDef`]s the AI may call, the widget tree of its panels ([`ui`]) and
//! the [`Bundle`]s of files it hands over for the library.
//!
//! This crate is MIT OR Apache-2.0 (Meshgrove itself is GPL-3.0-or-later): plugins built on it
//! may use any license. With the feature `guest` it also brings the bindings and helpers for
//! writing a plugin in Rust ([`guest`]).

#[cfg(feature = "guest")]
extern crate self as meshgrove_plugin_api;

pub mod bundle;
#[cfg(feature = "guest")]
pub mod guest;
pub mod manifest;
pub mod mesh;
pub mod ui;

pub use bundle::{Bundle, BundleFile, BundleItem, BundleList, ListKind};
/// Exports a plugin: `meshgrove_plugin_api::export_plugin!(MyPlugin);` ([`guest`]).
#[cfg(feature = "guest")]
pub use guest::export_plugin;
pub use manifest::{FileType, HostInfo, Manifest, PanelDef, Permission, PrinterInfo, ToolDef};
pub use mesh::MeshData;

/// The WIT world `meshgrove:plugin/meshgrove-plugin`.
pub const WIT: &str = include_str!("../wit/meshgrove-plugin.wit");

/// Version of the WIT package and of the JSON shapes in this crate.
pub const API_VERSION: &str = "0.1.0";

/// Whether a plugin built against API `version` runs on this one: the same minor version while
/// the API is 0.x, the same major version after.
pub fn api_compatible(version: &str) -> bool {
    let parse = |v: &str| -> Option<(u32, u32)> {
        let mut parts = v.trim().split('.');
        Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
    };
    match (parse(version), parse(API_VERSION)) {
        (Some((0, minor)), Some((0, ours))) => minor == ours,
        (Some((major, _)), Some((ours, _))) => major == ours,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_versions() {
        assert!(api_compatible("0.1.0") && api_compatible("0.1.7") && api_compatible("0.1"));
        assert!(!api_compatible("0.2.0") && !api_compatible("1.0.0") && !api_compatible("x"));
        assert!(WIT.contains(&format!("package meshgrove:plugin@{API_VERSION};")));
    }
}
