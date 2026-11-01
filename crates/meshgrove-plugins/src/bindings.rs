//! Host bindings of the WIT world (the same file the plugins are built against).

wasmtime::component::bindgen!({
    path: "../meshgrove-plugin-api/wit",
    world: "meshgrove-plugin",
});

#[cfg(feature = "panels")]
pub use exports::meshgrove::plugin::plugin::GuestPanel;
pub use meshgrove::plugin::types::PluginError;
