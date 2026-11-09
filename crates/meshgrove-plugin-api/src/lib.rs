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

/// Version of the WIT package: the functions a plugin imports and exports. A plugin links
/// against exactly these, so nothing is added to the world within 0.x; a plugin built against any
/// 0.x API runs on every later 0.x Meshgrove.
pub const WIT_VERSION: &str = "0.1.0";

/// Version of the JSON shapes in this crate (manifests, widgets, events, effects, bundles), which
/// is what a plugin names in [`Manifest::api`]. It grows by new fields with defaults and new
/// variants; what a host does not know it skips ([`ui::Node::Unknown`], [`ui::Effect::Unknown`]).
pub const API_VERSION: &str = "0.2.0";

/// Whether a plugin built against API `version` runs on this one ([`api_check`]).
pub fn api_compatible(version: &str) -> bool {
    api_check(API_VERSION, version).is_ok()
}

/// Whether a plugin built against API `plugin` runs on a host of API `host`: the same major
/// version, and a minor version no newer than the host's (a host keeps what older minor
/// versions had; a plugin for a newer one may use what this host lacks). The text says what
/// to do about it.
pub fn api_check(host: &str, plugin: &str) -> Result<(), String> {
    let parse = |v: &str| -> Option<(u32, u32)> {
        let mut parts = v.trim().split('.');
        Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
    };
    let (Some((major, minor)), Some((ours, our_minor))) = (parse(plugin), parse(host)) else {
        return Err(format!("plugin API {plugin:?} is not a version (major.minor.patch)"));
    };
    if major != ours {
        return Err(format!(
            "built for plugin API {plugin}, this Meshgrove supports API {ours}.x ({host}): \
             the plugin needs {} Meshgrove",
            if major > ours { "a newer" } else { "an older" }
        ));
    }
    if minor > our_minor {
        return Err(format!("built for plugin API {plugin}, newer than this Meshgrove's {host}: update Meshgrove"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_versions() {
        assert!(api_compatible("0.1.0") && api_compatible("0.1.7") && api_compatible("0.1"));
        assert!(api_compatible("0.2.0") && api_compatible(API_VERSION), "plugins of this API and the one before");
        assert!(!api_compatible("0.3.0") && !api_compatible("1.0.0") && !api_compatible("x"));
        // The world a plugin links against has a version of its own, which stays through 0.x.
        assert!(WIT.contains(&format!("package meshgrove:plugin@{WIT_VERSION};")));
        assert_eq!(WIT_VERSION, "0.1.0", "a new WIT version would stop every installed plugin from loading");
        assert_eq!(api_check(API_VERSION, WIT_VERSION), Ok(()));
    }

    /// A host a minor version on still runs the plugins of before; another major version does not.
    #[test]
    fn version_skew() {
        assert_eq!(api_check("0.2.0", "0.1.0"), Ok(()), "an older minor version runs");
        assert_eq!(api_check("0.2.3", "0.2.0"), Ok(()));
        let newer = api_check("0.1.0", "0.2.0").unwrap_err();
        assert!(newer.contains("0.2.0") && newer.contains("update Meshgrove"), "{newer}");
        let major = api_check("0.2.0", "1.0.0").unwrap_err();
        assert!(major.contains("1.0.0") && major.contains("API 0.x") && major.contains("newer Meshgrove"), "{major}");
        let old = api_check("2.0.0", "1.4.0").unwrap_err();
        assert!(old.contains("an older Meshgrove"), "{old}");
        assert!(api_check("0.1.0", "one").unwrap_err().contains("not a version"));
    }
}
