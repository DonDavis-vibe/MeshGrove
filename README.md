# STL Browser

A fast, native gallery for 3D-print files on Linux. Browse your model folders the way a file
manager browses photos — every **STL, 3MF and STEP** file, every model inside **ZIP/7z/RAR**
packs and every **PDF** shows a real preview, so you no longer open files in the slicer just to
see what they are.

Built in Rust with [egui](https://github.com/emilk/egui) and [wgpu](https://wgpu.rs) (Vulkan),
so it runs natively on Wayland and NVIDIA without a web view.

## Features

- **Gallery** of all files with rendered model previews, folder and archive mosaics, sorting,
  filtering, "include subfolders", keyboard control (arrows, Enter, Backspace, Ctrl+A, Del).
- **3MF:** slicer plate pictures with filament colours (Bambu Studio, OrcaSlicer, Anycubic,
  Elegoo, Creality Print, PrusaSlicer); multi-plate projects show one plate at a time.
- **STEP** through OpenCascade in a separate helper process (crash-safe, cached).
- **3D viewer** next to the grid; **hover turntable** spins the model in its tile (drag to turn,
  wheel with a button held to zoom).
- **Library index** (SQLite): instant search across all library folders (`dragon ext:3mf
  tag:pirate`), tags that follow file content through renames, **duplicate detection** (exact
  copies and same geometry, rotation-independent) with a keep suggestion.
- **Actions:** open in your slicer (auto-detects Anycubic Slicer Next, OrcaSlicer, PrusaSlicer,
  Bambu Studio), show in Dolphin, copy path, move to trash (blocked on read-only drives).
- **Dolphin integration:** the same renderer provides thumbnails for Dolphin and file dialogs.
- **German and English** UI (follows the system language).

## Install (current user, no root needed)

Requirements (Fedora): `sudo dnf install rust cargo libarchive-devel opencascade-devel poppler-utils`

```bash
packaging/install.sh
```

This builds release binaries and installs them to `~/.local/bin`, plus a menu entry, icon,
the `model/step` file type and the thumbnailer in `~/.local/share`. Remove with
`packaging/install.sh --uninstall`.

For previews in Dolphin: *Settings → Configure Dolphin → Interface → Previews*, enable the
3D-model entries and raise the file size limit for big models.

## Develop

```bash
cargo test --workspace          # unit, loader, database and GPU render tests
cargo run --release -p stl-browser -- /path/to/models
scripts/ui-screenshot.sh out.png 8 /path/to/models   # UI screenshot in a headless nested KWin
```

| Crate | Purpose |
|---|---|
| `stlb-core` | file formats (STL, 3MF, STEP, archives, images, PDF), library index, scanner, search, duplicates, thumbnail cache |
| `stlb-render` | wgpu mesh renderer shared by thumbnails, viewer, turntable and thumbnailer |
| `stlb-step` | `stlb-step-mesher`: STEP → STL with OpenCascade |
| `stl-browser` | the egui application |
| `stl-browser-thumbnailer` | freedesktop thumbnailer for Dolphin and file dialogs |

## Deutsch

**STL-Browser** zeigt 3D-Druck-Dateien wie ein Dateimanager Fotos: STL, 3MF, STEP, Modelle in
Archiven und PDFs mit echter Vorschau. Dazu kommen 3D-Ansicht, Drehvorschau beim Darüberfahren,
Bibliothekssuche, Tags, Duplikaterkennung, „In Anycubic Slicer öffnen“ und Vorschauen in Dolphin.
Installation ohne Root: `packaging/install.sh` (Entfernen: `packaging/install.sh --uninstall`).
Die Oberfläche folgt der Systemsprache (Deutsch/Englisch) und lässt sich in den Einstellungen
umstellen.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
The optional STEP helper links OpenCascade (LGPL-2.1 with exception) dynamically.
