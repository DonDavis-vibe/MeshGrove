# Patches to the vendored SolidCraft crates (taken over from LightGrove's copy)

Base: SolidCraft `b68a81dd84533bf60c81c3ea8fdcec354f0518cf` (2026-11-10), see `UPSTREAM_COMMIT`.
`scripts/sync-solidcraft.sh patches` prints the exact diffs; this file says why. Every patched line
carries a `LightGrove:` comment.

| # | File | Change | Why |
|---|---|---|---|
| 1 | `solidcraft-ui-kit/Cargo.toml` | Remove `solidcraft-viewport-egui` and `solidcraft-viewport-gpu`; add feature `blur = []` (off) | Those two crates exist only for the frosted panel backing and pull in the CAD kernel (`solidcore`, `-convert`, `-record`, `-tess`, `solidcraft-viewport`) |
| 2 | `solidcraft-ui-kit/src/blur.rs` | Compile `blur_native.rs` only with `feature = "blur"`; otherwise use upstream's web stub (plain translucent fill) on every target; gate the stub's `viewport()` on `blur` | Same as 1. The stub already exists upstream for wasm, so no new code paths |

Total: 2 files, +12/−7 lines. `blur_native.rs` and `blur.wgsl` stay byte-identical (not compiled).

## Not patches

- Each crate also holds upstream's root `LICENSE-MIT`, `LICENSE-APACHE` and `NOTICE` (added files).
- The crates keep their names (`solidcraft-*`) and their `*.workspace = true` keys; the spike's root
  `Cargo.toml` supplies those keys (same versions as upstream, path dependencies with
  `version = "=0.40.0-rc.3"` so `cargo deny` does not count them as wildcards).
- Host-side adaptations live outside `vendor/`: `crates/meshgrove/build.rs` drops theme `tabs`
  tables (Meshgrove has no workspace tabs; onyx.json), and the theme JSON stays byte-identical.

## Rules

1. Patch only what cannot be solved in the host. Prefer a cargo feature or a `cfg` over an edit.
2. Mark each changed line with `LightGrove:` and add a row here.
3. After `scripts/sync-solidcraft.sh merge`: build, `cargo test -p solidcraft-ui-kit` (291 tests),
   run the gallery, run `cargo deny check`.
