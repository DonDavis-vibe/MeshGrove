# Patches to the vendored SolidCraft crates (taken over from LightGrove's copy)

Base: SolidCraft `b68a81dd84533bf60c81c3ea8fdcec354f0518cf` (2026-11-10), see `UPSTREAM_COMMIT`.
`scripts/sync-solidcraft.sh patches` prints the exact diffs; this file says why. Every patched line
carries a `LightGrove:` comment (rows 1 and 2, taken over) or a `Meshgrove:` comment (rows 3 to 7).

| # | File | Change | Why |
|---|---|---|---|
| 1 | `solidcraft-ui-kit/Cargo.toml` | Remove `solidcraft-viewport-egui` and `solidcraft-viewport-gpu`; add feature `blur = []` (off) | Those two crates exist only for the frosted panel backing and pull in the CAD kernel (`solidcore`, `-convert`, `-record`, `-tess`, `solidcraft-viewport`) |
| 2 | `solidcraft-ui-kit/src/blur.rs` | Compile `blur_native.rs` only with `feature = "blur"`; otherwise use upstream's web stub (plain translucent fill) on every target; gate the stub's `viewport()` on `blur` | Same as 1. The stub already exists upstream for wasm, so no new code paths |

| 3 | `solidcraft-ui-kit/src/choice.rs` | New `ink_on_selection(wanted)` and `primary_mark()`; `segmented_icons` paints the chosen sign in `selected_ink` where the icons' ink is below 3:1 on `selected_bg` | The chosen sign of an icon switch was black on black in 1-bit and hard to see in SolidCraft 95. The two helpers serve rows 4 to 6 and the host |
| 4 | `solidcraft-ui-kit/src/panel.rs` | The page list writes the chosen page in `ink_on_selection(form_primary)` | `form_primary` on `selected_bg` is the same blue in SolidCraft 95: the chosen page's name was invisible |
| 5 | `solidcraft-ui-kit/src/fields.rs` | `slider` takes `primary_mark()` for its knob and filled part | In the 1-bit themes `form_primary` is the ink on the primary fill (white on white on a control); `form_primary_fill` is the fill |
| 6 | `solidcraft-ui-kit/src/status.rs` | `progress` fills with `primary_mark()` | Same as 5 |
| 7 | `solidcraft-ui-kit/src/prefs_window.rs` | "Restore defaults" is `Button::Normal`, not `Button::Ghost` | It should look like Cancel, Apply and OK beside it (Meshgrove's review, 2026-11-15) |

Rows 3 to 6 are theme faults SolidCraft itself has with these themes: worth reporting there, and
the rows can go once it is fixed upstream. Row 7 is a matter of taste and stays ours.

Total: 7 files. `blur_native.rs` and `blur.wgsl` stay byte-identical (not compiled).

## Not patches

- Each crate also holds upstream's root `LICENSE-MIT`, `LICENSE-APACHE` and `NOTICE` (added files).
- The crates keep their names (`solidcraft-*`) and their `*.workspace = true` keys; the spike's root
  `Cargo.toml` supplies those keys (same versions as upstream, path dependencies with
  `version = "=0.40.0-rc.3"` so `cargo deny` does not count them as wildcards).
- Host-side adaptations live outside `vendor/`: `crates/meshgrove/build.rs` drops theme `tabs`
  tables (Meshgrove has no workspace tabs; onyx.json), and the theme JSON stays byte-identical.

## Rules

1. Patch only what cannot be solved in the host. Prefer a cargo feature or a `cfg` over an edit.
2. Mark each changed line with `Meshgrove:` (`LightGrove:` on what was taken over) and add a row here.
3. After `scripts/sync-solidcraft.sh merge`: build, `cargo test -p solidcraft-ui-kit` (291 tests),
   run the gallery, run `cargo deny check`.
