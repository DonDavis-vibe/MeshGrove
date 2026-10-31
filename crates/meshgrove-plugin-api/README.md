# meshgrove-plugin-api

The plugin interface of [Meshgrove](https://github.com/DonDavis-vibe/MeshGrove): the WIT world
`meshgrove:plugin@0.1.0` (`wit/meshgrove-plugin.wit`) and the JSON types that cross it —
manifest, tool definitions, the widget tree of plugin panels with its events and effects,
bundles of files for the library, and meshes.

A Meshgrove plugin is a WebAssembly component built for `wasm32-wasip2`. It imports `host`
(log, progress, cancel, host info) and `library` (search, facts, triangles — only with the
user's permission) and exports `plugin` (manifest, tools, jobs, file handler, panels). It runs
in a sandbox: no network, no environment, only its own `/data`, the staging folder `/out` and
`/tmp`.

With the feature `guest` the crate brings the Rust bindings and helpers for writing a plugin:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
# Not on crates.io yet: from the Meshgrove repository (or a local checkout with `path = …`).
meshgrove-plugin-api = { git = "https://github.com/DonDavis-vibe/MeshGrove", features = ["guest"] }
```

```sh
rustup target add wasm32-wasip2
cargo build --target wasm32-wasip2 --release
```

Where a distribution's Rust comes first in `PATH` (Fedora's `/usr/bin/cargo`), use rustup's
toolchain for both cargo and rustc, e.g. `PATH="$(dirname "$(rustup which cargo)"):$PATH"`.

See `examples/plugins/hello-box` in the Meshgrove repository for a complete plugin.

## License

This crate is licensed under either of the MIT license (`LICENSE-MIT`) or the Apache License,
Version 2.0 (`LICENSE-APACHE`), at your option. Meshgrove itself is GPL-3.0-or-later, but the
plugin interface is not: **plugins may use any license**, open or closed.
