<p align="center">
  <img src="docs/images/banner.jpg" alt="Meshgrove" width="100%">
</p>

<p align="center">
  <b>A fast, native gallery for your 3D-print files.</b><br>
  Browse STL, 3MF, STEP, OBJ and glTF models like photos: real previews for every model, every archive and every PDF.
</p>

<p align="center">
  <a href="https://github.com/DonDavis-vibe/MeshGrove/actions/workflows/ci.yml"><img src="https://github.com/DonDavis-vibe/MeshGrove/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue" alt="License: GPL-3.0-or-later"></a>
  <img src="https://img.shields.io/badge/platforms-Linux%20%7C%20Windows%20%7C%20macOS-informational" alt="Platforms: Linux, Windows, macOS">
  <img src="https://img.shields.io/badge/made%20with-Rust-orange" alt="Made with Rust">
  <a href="https://ko-fi.com/don_davis"><img src="https://img.shields.io/badge/Ko--fi-leave%20a%20tip-FF5E5B?logo=ko-fi&logoColor=white" alt="Leave a tip on Ko-fi"></a>
</p>

<p align="center">
  <a href="README.de.md">🇩🇪 Deutsche Version</a>
</p>

---

![Meshgrove showing a folder of household prints, with a toothbrush holder in the 3D viewer](docs/images/screenshot-hero.png)

## Contents

- [Why Meshgrove?](#why-meshgrove)
- [Features at a glance](#features-at-a-glance)
- [Screenshots](#screenshots)
- [Installation](#installation)
- [Getting started](#getting-started)
- [User guide](#user-guide)
  - [Browsing the gallery](#browsing-the-gallery)
  - [The 3D viewer and its tools](#the-3d-viewer-and-its-tools)
  - [The details panel](#the-details-panel)
  - [Library, search and filters](#library-search-and-filters)
  - [Tags and notes](#tags-and-notes)
  - [Print bed check](#print-bed-check)
  - [Duplicate finder](#duplicate-finder)
  - [Working with your slicer and other apps](#working-with-your-slicer-and-other-apps)
  - [Archives (ZIP, 7z, RAR, TAR)](#archives-zip-7z-rar-tar)
  - [Moving files to the trash](#moving-files-to-the-trash)
  - [Backups, export and import](#backups-export-and-import)
  - [Thumbnails in Dolphin and file dialogs (Linux)](#thumbnails-in-dolphin-and-file-dialogs-linux)
- [Keyboard and mouse reference](#keyboard-and-mouse-reference)
- [Supported file types](#supported-file-types)
- [Settings and files on disk](#settings-and-files-on-disk)
- [Privacy and safety](#privacy-and-safety)
- [Performance](#performance)
- [Platform support](#platform-support)
- [Troubleshooting and FAQ](#troubleshooting-and-faq)
- [Building from source](#building-from-source)
- [Architecture](#architecture)
- [Development](#development)
- [Roadmap](#roadmap)
- [Contributing](#contributing)
- [Tip jar](#tip-jar)
- [Credits](#credits)
- [License](#license)

## Why Meshgrove?

Anyone who prints a lot ends up with folders full of files called `Body1.stl`,
`final_v3_fixed.3mf` and `pack.zip`. A normal file manager shows a generic icon for each of
them, so you open model after model in the slicer just to find the right one.

Meshgrove shows those folders as a gallery with a rendered preview of every model. That
includes the models inside ZIP/7z/RAR packs and the plate pictures your slicer stored in 3MF
projects. It also gives you what a model collection needs: an instant search across the whole
library, tags and notes, a duplicate finder, a print-bed check, and a one-click hand-over to
your slicer.

What makes it different from other model managers:

- **Your folders stay as they are.** Meshgrove browses and indexes files where they are. It
  never imports, copies, renames or reorganises anything, and it works fine on read-only
  drives.
- **Native and fast.** It is written in Rust and renders with the GPU (Vulkan, Metal or
  DirectX 12 through [wgpu](https://wgpu.rs)). There is no Electron, no web view and no server.
  Thousands of files and models with millions of triangles stay fluid.
- **Free and open source** under the GPL, with no account, subscription or telemetry.
- **Made for real libraries on spinning hard drives.** Reads are scheduled so visible tiles
  come first, and everything else is cached.

## Features at a glance

| Area | What you get |
|---|---|
| **Gallery** | Thumbnails for STL, 3MF, STEP, OBJ, PLY, glTF/GLB, images and PDFs; folder and archive tiles show a 2×2 mosaic of what is inside; resizable tiles; sort by name, date, size or type; "models only"; "include subfolders" (all files below a folder in one grid); multi-selection |
| **Hover turntable** | Point at a model tile and the model spins in place. Drag to turn it yourself, and turn the wheel while dragging to zoom |
| **3D viewer** | In the side panel, or **double-click a model for a window of its own** (resizable, several at once, with "open in slicer"); orbit, pan and zoom with a fixed pivot (turning never shifts the model); standard views (front, back, left, right, top, bottom, angled); **point-to-point measuring in mm**; **picture export** as a 2048 px PNG |
| **3MF projects** | Embedded slicer plate pictures (Bambu Studio, OrcaSlicer, Anycubic, Elegoo, Creality Print, PrusaSlicer); click a plate picture to see that plate in 3D; **multi-colour**: filament colours per object and part, painted models (multi-material painting) and 3MF material colours in the viewer and in rendered previews; filament swatches on tiles and in the details; title, designer and application from the metadata |
| **STEP / STP** | CAD files are tessellated by OpenCascade in a separate helper process, so a broken file cannot crash the app; results are cached |
| **Archives** | Open ZIP, 7z, RAR and TAR like folders; every model inside gets a preview; files are extracted only when another program needs them |
| **Library** | Index (SQLite) of all library folders; instant search such as `dragon ext:3mf tag:painted`; changes on disk are picked up automatically |
| **Tags and notes** | Tags and notes belong to the file *content*, so they survive renames and moves; the search finds note text too; daily automatic backups; export and import as JSON |
| **Print bed check** | Reads the printer selected in Anycubic Slicer Next, OrcaSlicer or Bambu Studio (or takes your own build volume) and marks models that only fit when turned (↻ 41°) or are too big (✕) |
| **Duplicates** | Finds exact copies and the same model saved differently (ASCII vs binary STL, STL vs 3MF, rotated or moved copies), suggests which copy to keep and can move the others to the trash |
| **Slicer and apps** | "Open in slicer" for one or many models (auto-detects Anycubic Slicer Next, OrcaSlicer, Bambu Studio, PrusaSlicer), into the slicer window that is already open where the slicer allows it; **Ctrl+C copies the files** for pasting into the file manager or slicer; "show in file manager"; open with the default app |
| **Desktop integration** | Menu entry, icon, file associations; on Linux the same renderer also provides **thumbnails in Dolphin and file dialogs** |
| **Languages** | English and German, following the system language (switchable in the settings) |

## Screenshots

**Folder tiles show what is inside.** Every folder and archive gets a mosaic of its first
models, so you can navigate by looking instead of reading names.

![Folder mosaics of a model library](docs/images/screenshot-folders.png)

**Models in their colours.** Multi-colour 3MF projects show the filament of every part and
their painted areas, plain 3MF files their material colours, and OBJ, PLY and glTF their vertex
colours and textures. Tiles get a dot per filament; the details list slot and material.

![A painted 3MF logo in the viewer with its two filaments, next to a textured GLB and 3MF tiles with filament dots](docs/images/screenshot-colours.png)

**One search for the whole library.** Here `body` finds every model with "body" in its name,
from four different folders.

![Library search for "body" with eleven results](docs/images/screenshot-search.png)

**Every model in a window of its own.** Double-click a model to open it next to the gallery, as
large as you like and with the same tools. Here it measures a coaster holder: click two points
on the surface to get the distance in mm, with the difference along each axis.

![A coaster holder in its own window next to the main window, measured at 79.3 mm](docs/images/screenshot-window.png)

**Duplicate finder.** Exact copies and identical geometry, grouped, with a suggestion which
copy to keep and one click to move the others to the trash.

![Duplicate groups with a keep suggestion](docs/images/screenshot-duplicates.png)

## Installation

> **Status:** Meshgrove is young. Linux (developed and used daily on Fedora/Nobara with KDE
> Plasma on Wayland) is the main platform. The Windows and macOS builds are new. They are
> built and tested automatically, but have seen little real-world use so far. Please
> [report problems](https://github.com/DonDavis-vibe/MeshGrove/issues).

### Linux

#### From source (recommended)

Building takes a few minutes and gives you every feature, including STEP previews.

1. Install Rust and the libraries:

   | Distribution | Command |
   |---|---|
   | Fedora / Nobara | `sudo dnf install rust cargo gcc-c++ libarchive-devel opencascade-devel poppler-utils` |
   | Debian / Ubuntu | `sudo apt install cargo g++ pkg-config libarchive-dev libxkbcommon-dev libwayland-dev poppler-utils` |
   | Arch / Manjaro | `sudo pacman -S rust gcc libarchive opencascade poppler` |

   OpenCascade is optional. Without it everything works except STEP previews, and it must be
   version 7.8 or newer. Ubuntu 24.04 ships 7.6, which the build detects and skips; there you
   get everything but STEP. poppler is optional too and only provides PDF previews. If your
   distribution's Rust is older than 1.95, install a current one with [rustup](https://rustup.rs).

2. Build and install for your user (no root needed):

   ```bash
   git clone https://github.com/DonDavis-vibe/MeshGrove.git
   cd MeshGrove
   packaging/install.sh
   ```

   This builds release binaries and puts them in `~/.local/bin`. It also adds the menu entry,
   the icon, the `model/step` file type and the thumbnailer in `~/.local/share`. Start
   **Meshgrove** from your application menu, or run `meshgrove` (optionally with a folder or
   file: `meshgrove ~/3D-Models`).

3. Update later with `git pull && packaging/install.sh`. Remove it with
   `packaging/install.sh --uninstall`. Your settings, index and cache are kept; see
   [Settings and files on disk](#settings-and-files-on-disk) if you want to delete them too.

#### Release package

Each [release](https://github.com/DonDavis-vibe/MeshGrove/releases) has a
`Meshgrove-<version>-linux-x86_64.tar.gz`. Unpack it and run `packaging/install.sh` inside
the folder: it installs the included binaries instead of building. The package needs
`libarchive` (installed on most desktops) and is built without OpenCascade, so STEP previews
need the source build.

### Windows

1. Download `Meshgrove-<version>-windows-x64.zip` from the
   [releases](https://github.com/DonDavis-vibe/MeshGrove/releases).
2. Unpack it anywhere, for example to `C:\Programs\Meshgrove`, and start `meshgrove.exe`.
   Windows SmartScreen may warn because the program is not signed: choose *More info → Run
   anyway*.
3. Optional, for PDF previews: install [poppler](https://github.com/oschwartz10612/poppler-windows)
   and make sure `pdftoppm.exe` is on your `PATH`.

The Windows package does not yet include STEP support; see [Roadmap](#roadmap).

### macOS

1. Download `Meshgrove-<version>-macos-arm64.zip` (Apple Silicon) from the
   [releases](https://github.com/DonDavis-vibe/MeshGrove/releases), unpack it and move
   `Meshgrove.app` to *Applications*.
2. The app is not notarized. On the first start, right-click it and choose *Open*, or run
   `xattr -dr com.apple.quarantine /Applications/Meshgrove.app` once.
3. Optional, for PDF previews: `brew install poppler`.

On Intel Macs, build from source (see [Building from source](#building-from-source)). The macOS
package does not yet include STEP support.

## Getting started

1. **Open a folder.** Meshgrove starts in the folder you used last, or your home folder. Use
   the sidebar (*Places*, *Devices*, the folder tree), the breadcrumb at the top, or
   double-click folders. Previews appear within moments: visible tiles first, the rest in the
   background.
2. **Add your model folders to the library.** Click *➕ Add this folder to the library* in the
   sidebar (or use *Settings → Library folders*). Library folders are indexed, so you can
   search them, tag files, write notes and find duplicates. The first indexing of a big
   collection runs in the background; the status bar shows the progress.
3. **Look at a model.** Click a tile to see it in the 3D viewer on the right, with its
   dimensions, triangle count and whether it fits your printer. Point at a tile to make the
   model spin.
4. **Hand it to your slicer.** Right-click → *Open in Anycubic Slicer Next* (or whichever
   slicer was found). Select several models first to open them all at once.

## User guide

### Browsing the gallery

- **Navigate** with double-click (folders and archives open, models open in a window of their
  own, other files in their default app), the breadcrumb, the sidebar, or the keyboard: arrow keys move the selection, Enter
  opens, Backspace or Alt+↑ goes up, Alt+← / Alt+→ go back and forward.
- **Tile size:** Ctrl + mouse wheel, Ctrl + `+` / `-`, or the slider in the *View* menu.
- **View menu:** sort by name, modified date, size or type (ascending or descending); *3D models
  only* hides everything except 3D models (folders stay so you can navigate); *Include
  subfolders* lists every file below the current folder in one grid, each with its folder
  name under it.
- **Filter the folder:** type into the search field (Ctrl+F) with the 📁 scope selected to
  narrow the current folder by name.
- **Select several files:** Ctrl+click toggles, Shift+click selects a range, Ctrl+A selects
  everything shown. Actions in the context menu (right-click) apply to the whole selection.
- **Hover turntable:** point at a model tile and it turns slowly around its own axis. Hold the
  left mouse button and drag to turn it by hand; turn the mouse wheel while holding the button
  to zoom into the tile.
- **Badges on tiles:** the file type at the bottom left; `2×`, `3×` … at the top right when the
  library holds duplicates of this file; ↻ angle or ✕ at the bottom right from the print bed
  check.

### The 3D viewer and its tools

The right panel shows the selected model in 3D. **Double-click a model** (or press Enter) to
open it in a window of its own: it can be made as large as the screen, several models can be
open side by side, each with its own camera, and the window has the same tools, the model's
facts and an *Open in slicer* button. Esc or the close button closes it.

| Action | Mouse |
|---|---|
| Turn | Drag with the left button |
| Move (pan) | Drag with the right or middle button |
| Zoom | Mouse wheel |
| Reset the view | Double-click |

The camera always turns around the centre of the model at a fixed distance, so the model never
jumps or zooms while you turn it.

The buttons above the view:

- **🎥 View** — jump to a standard view: angled (default), front, back, left, right, top,
  bottom.
- **📏 Measure** — click a point on the model, then a second one. Meshgrove shows the
  distance in millimetres and the difference along X, Y and Z (for example `47.2 mm (ΔX 3.1 ·
  ΔY 5.4 · ΔZ 46.8)`). A third click starts a new measurement. Click *Measure* again to leave
  the mode. Points snap to the real surface; the model is loaded into memory once for this.
- **📷 Picture** — saves exactly what you see as a 2048-pixel PNG with a transparent
  background in `Pictures/Meshgrove` (on Linux `~/Bilder/Meshgrove` or `~/Pictures/Meshgrove`,
  depending on your language). This is useful for documentation, sales listings or chats.

**Multi-plate 3MF projects** (Bambu Studio, OrcaSlicer, Anycubic Slicer Next) show the
pictures of all plates below the facts. Click a plate picture to show that plate in the
viewer.

**Multi-colour 3MF projects** show their colours in the viewer, the hover turntable and the
previews Meshgrove renders itself:

- each object and part in the colour of the filament the slicer assigned to it (Bambu Studio,
  OrcaSlicer, Anycubic Slicer Next, PrusaSlicer)
- painted models: multi-material painting (`paint_color` / `mmu_segmentation`) is decoded
  into the painted areas
- plain 3MF files with standard material colours (`basematerials`, `colorgroup`), as CAD
  programs write them
- modifiers, negative volumes and support blockers are not drawn, just as they are not printed

Tiles of slicer projects show a dot per filament colour, and the details list the filaments
with their slot number and material.

### The details panel

Below the viewer you find:

- type, file size and modification date
- **dimensions** in mm (X × Y × Z) and the **triangle** count
- the **print bed** result (see [Print bed check](#print-bed-check))
- for 3MF: title, designer, the application that wrote it, the **filaments** (colour, slot and
  material) and whether the preview comes from the slicer's embedded picture
- **tags** and the **note** (for files in the library)
- the full path

With several files selected, the panel shows how many and their total size. Tags and the note
then apply to all of them.

### Library, search and filters

**Library folders** are the folders Meshgrove indexes. Add as many as you like (sidebar or
*Settings → Library folders*). The index stores names, sizes, dates, dimensions, triangle
counts, content fingerprints, tags and notes. It is kept current automatically: changes are
noticed through the operating system's file notifications, and a quick check runs in regular
intervals as a safety net.

**Library search:** select the 📚 scope next to the search field (or press Ctrl+Shift+F) and
type. Results come from all library folders at once.

| You type | Finds |
|---|---|
| `dragon` | files whose name, folder path or note contains "dragon" (case-insensitive) |
| `castle tower` | files containing both words, anywhere |
| `"castle tower"` | the exact phrase |
| `ext:3mf` (also `type:` / `typ:`) | only 3MF files; works with `stl`, `step`, `obj`, `ply`, `glb`, `zip`, `pdf`, `png` … |
| `tag:painted` | only files with that tag |
| `tag:"to print"` | tags with spaces |
| `dragon ext:stl tag:painted` | all conditions combined |

Right-click a search result → *Show in folder* jumps to the folder containing it. *✕ Close
search* (or Esc in the field) returns to the folder.

The sidebar's **TAGS** section lists every tag with its count. Click one to search for it.

### Tags and notes

- **Add a tag:** select one or more files in the library, type into *Add tag …* in the details
  panel and press Enter, or click one of the suggested existing tags. Right-click → *Edit
  tags …* jumps straight into the field.
- **Remove a tag:** click the tag chip (`painted ✕`). With several files selected, chips show
  how many of them carry the tag (`painted (2/5)`).
- **Notes:** write anything into the *Note* field: print settings, colours, what went wrong
  last time. It is saved automatically a moment after you stop typing. With several files
  selected, the note is written to all of them. If they had different notes, the field says
  so and your text replaces them.
- **They follow the content:** tags and notes are attached to a fingerprint of the file's
  content (BLAKE3), not to its path. Rename or move a file, even into another library folder,
  and its tags and note come along. Identical copies share them.

### Print bed check

Meshgrove compares every model's size with your printer's build volume:

- **Fits as it is:** nothing on the tile; the details panel says *✔ fits on Anycubic Kobra S1*.
- **Fits only when turned:** an amber ↻ badge with the angle (for example ↻ 41° for a long
  part that fits only diagonally). The details panel says so too.
- **Too big:** a red ✕ badge. The details panel tells you by how much, for example
  *too big for Anycubic Kobra S1: X +50.0 mm*.

The printer is read from the profile currently selected in **Anycubic Slicer Next**,
**OrcaSlicer** or **Bambu Studio**, including profiles that inherit from system profiles. In
*Settings → Printer* you can enter your own name and build volume, or go back to *Take from
slicer*.

### Duplicate finder

Click **Duplicates (n)** in the sidebar. Meshgrove groups:

- **Exact copies:** byte-identical files (same BLAKE3 hash).
- **Same geometry:** the same model saved differently, such as ASCII vs binary STL, STL vs 3MF,
  re-exported, moved or rotated. These are detected by a rotation-independent signature of the
  mesh (triangle count, surface area, volume, moments of inertia).

Groups are sorted by wasted space. The ★ *Keep* badge marks the suggested copy: one that is
not inside an archive, then the oldest, then the one with the shortest path. *Move all but ★
to the trash* cleans up a group after a confirmation. Tiles everywhere in the gallery show a
`2×` / `3×` badge for files that have duplicates.

### Working with your slicer and other apps

- **Open in slicer** (context menu, the button in the details panel or in a model window):
  opens the selected models, several at once if you like (STL, 3MF, STEP and OBJ; the slicers
  do not open PLY and glTF). Files inside archives are extracted to a cache folder first.
  Meshgrove finds these slicers automatically:

  | System | Detected |
  |---|---|
  | Linux | Anycubic Slicer Next (in `~/opt/AnycubicSlicer`), OrcaSlicer, PrusaSlicer (system package or Flatpak), Bambu Studio (Flatpak) |
  | Windows | Anycubic Slicer Next, OrcaSlicer, Bambu Studio, PrusaSlicer in *Program Files* or the per-user *Programs* folder |
  | macOS | Anycubic Slicer Next, OrcaSlicer, Bambu Studio, PrusaSlicer in */Applications* or *~/Applications* |

  **If the slicer is already open**, the files go into that window instead of a new one:
  Meshgrove hands them to Anycubic Slicer Next and OrcaSlicer over D-Bus on Linux, and starts
  PrusaSlicer in its single-instance mode. Only when no window is open does a new one start.
  The status bar says which happened. (On macOS the system passes files to a running app
  anyway; on Windows a new window opens for now.)

  Any other slicer (Cura, Simplify3D, ElegooSlicer, …) can be set in *Settings → Slicer*:
  give it a name, the program and optionally a working folder.
- **Copy (Ctrl+C):** puts the selected files on the clipboard as real files. Paste them into
  Dolphin, Explorer or Finder, into many slicers, or into a chat. Archive members are
  extracted first. *Copy path* puts the paths as text instead.
- **Show in file manager:** opens Dolphin/Nautilus, Explorer or Finder with the files selected.
- **Open with default app** (context menu): opens a model in the system's default application,
  for example the slicer registered for STL files; other files open there with a double-click.

### Archives (ZIP, 7z, RAR, TAR)

Double-click an archive to browse it like a folder, including subfolders inside the archive.
Every model, image and PDF inside gets a preview. The archive's tile shows a mosaic of its
contents. Archives are indexed like folders, so the library search, tags, notes and the
duplicate finder include their contents. Archives are never modified. Solid 7z/RAR archives
are unpacked once per visit, not once per file.

### Moving files to the trash

Delete or right-click → *Move to trash* moves the selected files to the system trash (the
freedesktop trash on Linux, the Recycle Bin on Windows, the Trash on macOS) after a
confirmation. You can restore them from there. Deleting is not possible for files inside
archives, and on Linux Meshgrove detects read-only mounts (such as an NTFS drive mounted
read-only) and disables the action with an explanation.

### Backups, export and import

- **Automatic backups:** once a day (when something changed) Meshgrove writes all tags and
  notes to `user-data-YYYY-MM-DD.json` in the backup folder and keeps the newest seven.
  *Settings → Back up tags & notes → Open folder* shows them.
- **Export:** writes all tags and notes to `meshgrove-tags-notes-YYYY-MM-DD.json` in your
  Documents folder. The file is readable JSON and lists, for each fingerprint, the paths where
  those files were.
- **Import:** reads such a file (or a backup) and merges it: tags are added, notes are taken
  over for files that have none. Nothing is deleted. Because everything is keyed by content,
  an import also works after you reorganised your folders or moved to another computer.

### Thumbnails in Dolphin and file dialogs (Linux)

`install.sh` also registers a freedesktop thumbnailer, so Dolphin, the KDE and GTK file
dialogs and other file managers show the same previews for STL, 3MF, STEP, OBJ, PLY and glTF files. In
Dolphin, enable them once: *Settings → Configure Dolphin → Interface → Previews*. Tick the
3D-model entries if they are listed, and raise *Skip previews for local files above* for big
models. Then reopen the folder. The thumbnailer shares Meshgrove's cache, so files you have
already seen in the app show up instantly.

## Keyboard and mouse reference

Ctrl means ⌘ Cmd on macOS.

| Keys | Action |
|---|---|
| ← → ↑ ↓, Home, End | Move the selection |
| Enter | Open the selected folder or archive; a model in its own window; other files in their default app |
| Esc (in a model window) | Close the window |
| Backspace, Alt+↑ | Parent folder |
| Alt+←, Alt+→ | Back, forward |
| F5 | Reload the folder |
| Ctrl+A | Select all shown files |
| Ctrl+click, Shift+click | Toggle a file, select a range |
| Ctrl+C | Copy the selected files to the clipboard |
| Delete | Move the selection to the trash (with confirmation) |
| Ctrl+F | Filter the current folder |
| Ctrl+Shift+F | Search the whole library |
| Esc (in the search field) | Clear the search |
| Ctrl + `+` / `-`, Ctrl + wheel | Tile size |
| Ctrl+, | Settings |
| **Mouse on a model tile** | Hover: spin · drag: turn · wheel while dragging: zoom |
| **Mouse in the 3D viewer** | Left drag: turn · right/middle drag: pan · wheel: zoom · double-click: reset |

## Supported file types

| Type | Extensions | Preview | 3D viewer | Notes |
|---|---|---|---|---|
| STL | `.stl` | ✔ | ✔ | Binary and ASCII; binary files that start with "solid" are recognised correctly |
| 3MF | `.3mf` | ✔ | ✔ | Embedded slicer picture first (blank plates are skipped), otherwise rendered; components, transforms, units, Production extension (`p:path`), multiple build plates, filament, paint and material colours |
| STEP | `.step`, `.stp` | ✔ | ✔ | Needs the OpenCascade helper `meshgrove-step-mesher`; tessellated once, then cached |
| OBJ | `.obj` | ✔ | ✔ | Colours from the `.mtl` next to it (diffuse colour or texture) and vertex colours; read as millimetres like the slicers do |
| PLY | `.ply` | ✔ | ✔ | ASCII and binary; vertex and face colours (scans, MeshLab); point clouds without faces are not shown |
| glTF | `.glb`, `.gltf` | ✔ | ✔ | All meshes of the scene with base colours, vertex colours and textures; external buffers and textures are read from next to the file. glTF counts in **metres**, so a model 1 unit wide shows as 1000 mm; for models meant in millimetres (AI generators, many exports) choose *Settings → 3D models → Millimetres* |
| Archives | `.zip`, `.7z`, `.rar`, `.tar`, `.tar.gz`, `.tgz`, … | Mosaic | Members | Browsed like folders through libarchive |
| Images | `.png`, `.jpg`, `.jpeg`, `.webp`, `.gif`, `.bmp` | ✔ | – | Shown large in the details panel |
| PDF | `.pdf` | First page | – | Needs `pdftoppm` from poppler |
| Everything else | | File icon | – | Shown with its extension so the folder view is complete |

## Settings and files on disk

Everything can be changed in the settings window (⚙ at the top right, or Ctrl+,): language,
library folders, slicer, printer, the unit of glTF models, backups, and the preview cache (with its size and a *Clear
cache* button). The *About* section lists the exact paths on your system.

| What | Linux | Windows | macOS |
|---|---|---|---|
| Settings (`config.toml`) | `~/.config/meshgrove/` | `%APPDATA%\meshgrove\` | `~/Library/Application Support/meshgrove/` |
| Library index (`library.db`) and `backups/` | `~/.local/share/meshgrove/` | `%APPDATA%\meshgrove\` | `~/Library/Application Support/meshgrove/` |
| Cache: previews, STEP meshes, extracted archive files | `~/.cache/meshgrove/` | `%LOCALAPPDATA%\meshgrove\` | `~/Library/Caches/meshgrove/` |
| Exported pictures | `~/Pictures/Meshgrove` (localised, e.g. `~/Bilder`) | `Pictures\Meshgrove` | `~/Pictures/Meshgrove` |

The cache can always be deleted; it is rebuilt when needed. The index can be deleted too, but
**tags and notes live in it**, so export them first or keep the backups.

`config.toml` is plain text. The settings window covers everything, but for reference:

```toml
library_roots = ["/home/me/3D-Models", "/mnt/nas/prints"]
last_folder = "/home/me/3D-Models/minis"
tile_size = 160.0          # 96 … 384
only_models = false
sort_by = "name"           # name | modified | size | type
sort_descending = false
language = "en"            # "de", "en"; leave out to follow the system

[slicer]                   # leave out to detect automatically
name = "My Slicer"
program = "/opt/myslicer/bin/myslicer"
args = []                  # put before the file names
env = [["LD_LIBRARY_PATH", "/opt/myslicer/lib"]]
workdir = "/opt/myslicer"

[printer]                  # leave out to take it from the slicer
name = "Anycubic Kobra S1"
x = 250.0
y = 250.0
z = 250.0
```

Environment variables for special cases:

| Variable | Effect |
|---|---|
| `WGPU_BACKEND=gl` (or `vulkan`, `dx12`, `metal`) | Choose the graphics backend, e.g. if the window stays black |
| `RUST_LOG=meshgrove=debug` | More log output on the terminal |
| `MESHGROVE_STEP_MESHER=/path/to/meshgrove-step-mesher` | Use a specific STEP helper |

Meshgrove was called *STL-Browser* during development. On the first start it moves the
folders of that name (`~/.config/stl-browser` …) to the new ones, and it imports exports
made under the old name.

## Privacy and safety

- **Offline.** Meshgrove makes no network connections: no telemetry, no update checks, no
  accounts.
- **Read-only by design.** It only reads your model files. It writes to its own settings, index
  and cache folders, and otherwise only when you explicitly move files to the trash, export
  tags and notes, or save a picture.
- **Crash-safe helpers.** STEP tessellation and PDF rendering run as separate processes with
  timeouts, so a malformed file cannot take the app down. A file that fails gets a ⚠ tile with
  the reason in its tooltip.

## Performance

Meshgrove is built for collections of thousands of files on ordinary hard drives:

- A single reader thread reads files in folder order, visible tiles first, to avoid seeking.
  Parsing runs on all cores, and one GPU thread renders the previews.
- Previews are stored as PNGs in the cache, so a folder you have seen before appears
  instantly. In a test, 450 previews of a folder tree on a hard drive, never seen before, were
  ready in 21 seconds (RTX 3070 Ti); from the cache they appear immediately.
- The library index makes searching thousands of files instant, and only changed folders are
  re-read when something changes.
- Models with millions of triangles (for example a 210 MB, 4.2-million-triangle miniature)
  display and turn smoothly. Normals are computed on the GPU, so no memory is spent on them.
- The app uses no CPU while idle.

## Platform support

| Feature | Linux | Windows | macOS |
|---|---|---|---|
| Gallery, viewer, library, tags, notes, duplicates, bed check | ✔ | ✔ | ✔ |
| ZIP / 7z / RAR / TAR | ✔ | ✔ | ✔ |
| PDF previews | ✔ with poppler | ✔ with poppler | ✔ with poppler |
| STEP previews | ✔ with OpenCascade | source build with OpenCascade | source build with OpenCascade |
| Copy files to the clipboard | ✔ Wayland (X11: paths as text) | ✔ | ✔ |
| Show in file manager | ✔ D-Bus (Dolphin, Nautilus, …) | ✔ Explorer | ✔ Finder |
| Slicer detection | ✔ | ✔ | ✔ |
| Read-only drive detection | ✔ | – | – |
| Thumbnails in the system file manager | ✔ freedesktop thumbnailer | – | – |
| Menu entry and icon | ✔ `install.sh` | ✔ icon in `meshgrove.exe` | ✔ app bundle |

## Troubleshooting and FAQ

**The window stays black or the app does not start.**
Meshgrove needs Vulkan (Linux, Windows), Metal (macOS) or DirectX 12 (Windows). Update the
graphics driver. As a workaround, try another backend: `WGPU_BACKEND=gl meshgrove`. Start it
from a terminal to see the log.

**Changes on disk show up only after a while (Linux).**
Linux limits how many programs may watch folders (`fs.inotify.max_user_instances`, often 128).
On busy desktops that limit is used up, and Meshgrove then falls back to checking every
minute. To raise the limit: `sudo sysctl fs.inotify.max_user_instances=512`. To keep it
after a reboot, put `fs.inotify.max_user_instances=512` into
`/etc/sysctl.d/90-inotify.conf`. F5 reloads a folder immediately.

**Dolphin shows no 3D previews.**
Enable the previews in Dolphin's settings (see
[Thumbnails in Dolphin](#thumbnails-in-dolphin-and-file-dialogs-linux)) and raise the size
limit. Delete `~/.cache/thumbnails/fail` (install.sh does this) and reopen the folder.
`~/.local/bin/meshgrove-thumbnailer -s 256 model.stl /tmp/test.png` tests the thumbnailer by
hand.

**STEP files show "built without OpenCascade".**
The STEP helper was built without OpenCascade, or with a version older than 7.8. Install the
OpenCascade 7.8+ development files (see [Installation](#linux)) and run `packaging/install.sh`
again.

**PDFs show only an icon.**
Install poppler (`pdftoppm` must be on the `PATH`).

**"Move to trash" is greyed out.**
The file is inside an archive, or the drive is mounted read-only. On Linux, NTFS drives are
often mounted read-only when Windows was not shut down cleanly (Fast Startup).

**The slicer is not found or does not start.**
Set it in *Settings → Slicer*. Anycubic Slicer Next on Linux needs its library folder in
`LD_LIBRARY_PATH`. The automatic detection sets that up, and the settings keep it when you
only change the name.

**Copying files pastes only text (Linux).**
Copying real files needs a Wayland session. Under X11, Meshgrove falls back to copying the
paths as text.

**Where are my tags if I reinstall?**
In the library index and in the daily backups (see the [table](#settings-and-files-on-disk)).
Use *Settings → Back up tags & notes → Export* before moving to another computer, then
*Import* there.

**Does Meshgrove change my 3MF/STL files?**
No. It never writes to model files or archives.

## Building from source

You need Rust 1.95 or newer and libarchive. OpenCascade (STEP) and poppler (PDF, at runtime
only) are optional.

**Linux:** see [Installation](#from-source-recommended). For development:

```bash
cargo run --release -p meshgrove -- ~/3D-Models
```

**Windows** (MSVC toolchain):

```powershell
# libarchive as a static library through vcpkg
git clone https://github.com/microsoft/vcpkg C:\vcpkg
C:\vcpkg\bootstrap-vcpkg.bat
C:\vcpkg\vcpkg install libarchive:x64-windows-static
$env:VCPKG_ROOT = "C:\vcpkg"; $env:VCPKGRS_TRIPLET = "x64-windows-static"
cargo build --release -p meshgrove -p meshgrove-step
```

**macOS:**

```bash
brew install libarchive pkg-config
cargo build --release -p meshgrove -p meshgrove-step
```

**STEP support on any system:** install OpenCascade 7.8 or newer (`dnf install
opencascade-devel`, `brew install opencascade`, `vcpkg install opencascade`, …). If its headers
are not in `/usr/include/opencascade`, point the build at them with `OCCT_INCLUDE` (and
`OCCT_LIB` for the library folder), then build `-p meshgrove-step`. The resulting
`meshgrove-step-mesher` must sit next to the `meshgrove` executable or be on the `PATH`.

## Architecture

Meshgrove is a Cargo workspace of five crates:

| Crate | Purpose |
|---|---|
| [`meshgrove-core`](crates/meshgrove-core) | Everything without a window or GPU: file formats (STL, 3MF, STEP, OBJ, PLY, glTF, archives, images, PDF), the SQLite library index, scanner, search syntax, duplicate signatures, tags, notes and backups, thumbnail cache, settings, printer profiles |
| [`meshgrove-render`](crates/meshgrove-render) | The wgpu mesh renderer, shared by thumbnails (headless), the 3D viewer, the hover turntable and the Dolphin thumbnailer |
| [`meshgrove`](crates/meshgrove) | The egui application |
| [`meshgrove-step`](crates/meshgrove-step) | `meshgrove-step-mesher`: STEP → STL with OpenCascade, as a separate process |
| [`meshgrove-thumbnailer`](crates/meshgrove-thumbnailer) | The freedesktop thumbnailer for Dolphin and file dialogs |

How work flows through the app:

```mermaid
flowchart LR
    UI["UI thread<br/>(egui)"] -- "visible tiles, hover" --> Q["Thumbnail queue<br/>visible first"]
    Q --> IO["Reader thread<br/>one at a time, HDD-friendly"]
    IO --> P["Parser pool<br/>STL · 3MF · OBJ · PLY · glTF · archives"]
    P -- "STEP" --> S["meshgrove-step-mesher<br/>(own process)"]
    P --> G["GPU thread<br/>headless wgpu render"]
    G --> C[("Preview cache<br/>PNG + facts")]
    C --> UI
    L["Library service<br/>scanner + file watcher"] --> DB[("SQLite index<br/>files · tags · notes")]
    G -- "size, hash, geometry" --> L
    DB -- "search, duplicates" --> UI
```

Some design decisions:

- **One renderer everywhere.** Thumbnails, viewer, turntable and the Dolphin thumbnailer use
  the same wgpu pipeline, so a model looks the same wherever you see it. Flat shading uses
  normals computed from screen-space derivatives in the shader. That ignores the often wrong
  normals stored in STL files and halves the memory per model.
- **Orbit framing.** The viewer and turntable turn around the bounding-box centre at one
  distance that fits every angle, so turning never shifts or zooms the model. Thumbnails use
  a tight fit per angle instead.
- **Content fingerprints.** Every model's BLAKE3 hash is computed while it is read for its
  preview anyway. Tags, notes and exact-duplicate detection hang off that hash. A second,
  rotation-independent geometry signature finds re-exports.
- **Embedded pictures first.** For 3MF projects, the picture the slicer saved is the best
  preview, and it costs no parsing. Blank plate pictures from headless slicing are detected and
  skipped.
- **No WebKit.** WebKitGTK still has rendering problems on NVIDIA + Wayland. A native egui UI on
  wgpu avoids them entirely.

## Development

```bash
cargo test --workspace                       # unit, format, database and GPU render tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
scripts/ui-screenshot.sh out.png 8 ~/3D-Models   # screenshot of the UI in a headless nested KWin
```

`scripts/ui-screenshot.sh` runs the app in a virtual KWin session and saves its window, so UI
changes can be checked without touching your desktop. Test hooks (environment variables) can
script the session. Use separate `XDG_CONFIG_HOME` / `XDG_DATA_HOME` / `XDG_CACHE_HOME`
folders to keep test runs away from your real settings:

| Variable | Effect |
|---|---|
| `MESHGROVE_SCREENSHOT=out.png`, `MESHGROVE_SCREENSHOT_DELAY=8` | Save the window after the delay and quit |
| `MESHGROVE_POINTER=x,y` | Pin the pointer, e.g. to test the hover turntable |
| `MESHGROVE_HOLD=1`, `MESHGROVE_WHEEL=dy` | Hold the button / turn the wheel at the pointer |
| `MESHGROVE_CLICKS=x,y;x,y` | Click these points one after another (from 3 s, every 0.8 s); `x,y*2` double-clicks |
| `MESHGROVE_MEASURE=fx,fy;fx,fy` | Measure between these points (fractions of the view) in the first model window |
| `MESHGROVE_TYPE=text` | Type text after the clicks |
| `MESHGROVE_SEARCH=query` | Start with a library search |
| `MESHGROVE_FLATTEN=1` | Start with "include subfolders" |
| `MESHGROVE_VIEW=duplicates` / `settings` | Open the duplicate view / the settings |

CI runs formatting, clippy and the tests on Linux, Windows and macOS. Render tests run on
software Vulkan (lavapipe) on Linux. Pushing a tag `v*` builds the release packages for all
three systems and creates a draft release.

Translations live in [`crates/meshgrove/locales`](crates/meshgrove/locales) (`de.yml`,
`en.yml`). A test makes sure every key used in the code exists in every language.

## Roadmap

Ideas and next steps, roughly in order:

- [ ] STEP support in the Windows and macOS packages (OpenCascade in the release builds)
- [ ] Flatpak / AppImage for Linux, an installer for Windows, a notarized macOS app
- [ ] More formats: OBJ, PLY, G-code previews (embedded thumbnails)
- [ ] Collections (hand-picked sets across folders) and print status (printed, failed, to do)
- [ ] Thumbnails in Windows Explorer and macOS Finder
- [ ] More languages (contributions welcome)

## Contributing

Bug reports, ideas and pull requests are welcome in the
[issue tracker](https://github.com/DonDavis-vibe/MeshGrove/issues). Helpful details in a bug
report: your operating system, graphics card, what you did, and the log from starting
`meshgrove` in a terminal (with `RUST_LOG=meshgrove=debug` for more detail). A sample file
helps a lot when a model shows wrongly, if you are allowed to share it.

For code: run `cargo fmt`, `cargo clippy` and `cargo test` before opening a pull request, and
keep comments and identifiers in English. New user-visible text goes through `t!("…")`, with
entries in both locale files. By submitting a contribution you agree that it is licensed under
the same terms as Meshgrove (GPL-3.0-or-later).

## Tip jar

Meshgrove is free and stays free. If it saves you time or tidies up your print library, you can
leave a tip on **[Ko-fi](https://ko-fi.com/don_davis)**. Bug reports, ideas and a ⭐ on GitHub
are just as welcome.

## Credits

Meshgrove is developed by **Davis** as a project of
**[German Redneck Engineering](https://redneck-design.de/)**.

Built on excellent open-source work: [egui/eframe](https://github.com/emilk/egui),
[wgpu](https://wgpu.rs), [glam](https://github.com/bitshifter/glam-rs),
[rusqlite](https://github.com/rusqlite/rusqlite) with [SQLite](https://sqlite.org),
[libarchive](https://libarchive.org) through
[compress-tools](https://github.com/OSSystems/compress-tools-rs),
[OpenCascade](https://dev.opencascade.org), [poppler](https://poppler.freedesktop.org),
[notify](https://github.com/notify-rs/notify), [zip](https://github.com/zip-rs/zip2),
[quick-xml](https://github.com/tafia/quick-xml), [image](https://github.com/image-rs/image),
[BLAKE3](https://github.com/BLAKE3-team/BLAKE3), [trash](https://github.com/Byron/trash-rs),
[wl-clipboard-rs](https://github.com/YaLTeR/wl-clipboard-rs),
[clipboard-rs](https://github.com/ChurchTao/clipboard-rs) and
[rust-i18n](https://github.com/longbridge/rust-i18n), among others. Logo and banner were
created with the Krea 2 image model.

All models in the screenshots are our own designs (German Redneck Engineering).

## License

Copyright © 2026 Davis · German Redneck Engineering

Meshgrove is free software: you can redistribute it and/or modify it under the terms of the
[GNU General Public License](LICENSE) as published by the Free Software Foundation, either
version 3 of the License, or (at your option) any later version.

Meshgrove is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY;
without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See
the [GNU General Public License](LICENSE) for more details.

The STEP helper uses OpenCascade (LGPL-2.1 with an exception) as a separate program; PDF
previews call poppler's `pdftoppm` (GPL) as an external program. The licenses of all libraries
and fonts built into the program are listed in `THIRD-PARTY-LICENSES.md`, which comes with every
release package.

Anycubic, OrcaSlicer, Bambu Studio, PrusaSlicer, KDE, Dolphin, Windows and macOS are trademarks
of their respective owners. Meshgrove is an independent project and is not affiliated with or
endorsed by them.

Legal notice and privacy policy of German Redneck Engineering:
[Impressum](https://redneck-design.de/impressum.html) ·
[Datenschutz](https://redneck-design.de/datenschutz.html)
