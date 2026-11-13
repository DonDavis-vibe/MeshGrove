# Assets of the interface

| Here | From | Licence |
|---|---|---|
| `themes/*.json` (25) | SolidCraft `assets/styles/ui/themes/`, byte-identical | MIT OR Apache-2.0, Copyright (c) 2026 SolidCraft contributors; each file names its palette's origin (`provenance`) |
| `icons/palette.toml`, `icons/aliases.toml` | SolidCraft `assets/icons/ui/design/` | MIT OR Apache-2.0 |
| `icons/svg/flat/*.svg`, `icons/svg/model/*.svg` | SolidCraft `assets/icons/ui/design/svg/`, original artwork | MIT OR Apache-2.0 |
| `fonts/ibm-plex-sans/`, `fonts/ibm-plex-mono/` | IBM Plex Sans (variable) and Plex Mono, as SolidCraft ships them | SIL Open Font License 1.1 (`OFL.txt` beside each) |
| `themes-meshgrove/*.json` (2) | Meshgrove's own look, dark and light: SolidCraft's built-in palettes with neutral greys and Breeze blue, made by `scripts/meshgrove-themes.py` | MIT OR Apache-2.0 (the palettes they are made from), the changes GPL-3.0-or-later |

The theme files are embedded by `crates/meshgrove/build.rs`, which leaves out the per-tab tables
(`tabs`) of SolidCraft's workspace tabs; the files here stay as upstream has them, so
`scripts/sync-solidcraft.sh` can compare them. The code that draws with them is the vendored
SolidCraft UI kit (`vendor/README.md`).

The kit's own words (tooltips and menus of its controls) in English and German are in
`crates/meshgrove/kit-texts/`: SolidCraft's message ids with the texts LightGrove carries for them.
