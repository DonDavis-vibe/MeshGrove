# Vendored SolidCraft UI kit

Copies of five SolidCraft crates (https://github.com/bherbruck/solidcraft, MIT OR Apache-2.0,
Copyright (c) 2026 SolidCraft contributors), taken from the commit in `UPSTREAM_COMMIT`:

| Here | Upstream | What it is |
|---|---|---|
| `solidcraft-color` | `crates/color` | sRGB transfer function, analysis palette |
| `solidcraft-theme` | `crates/theme` | `Tokens` (the token contract), light/dark palettes, colour vision, tab accents, per-thread theme state |
| `solidcraft-ui-text` | `crates/ui-text` | Message catalogs, `tr`/`tr_label`/`format`, language scope, bidi |
| `solidcraft-ui-registry` | `crates/ui-registry` | Widget handles and rects for UI tests and the control channel |
| `solidcraft-ui-kit` | `crates/ui-kit` | The kit: rows, fields, menus, panels, containers, window chrome, theme files |

Each crate keeps its package name and carries upstream's `LICENSE-MIT`, `LICENSE-APACHE` and
`NOTICE`. Our changes are listed in `PATCHES.md`. `window_chrome.rs` is itself adapted from
VectorCraft (MIT OR Apache-2.0), as its header says.

The crates were taken as LightGrove vendors them: the same upstream commit and the same two
patches, marked `LightGrove:` in the source (`PATCHES.md`). The theme files, icons and font that
go with them are in `assets/` (`assets/README.md`).

Sync with upstream: `scripts/sync-solidcraft.sh` (`status`, `patches`, `upstream`, `merge`).
