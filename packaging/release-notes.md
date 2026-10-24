<!-- Release notes template: the release workflow fills in the version and adds the checksums.
     Add this release's highlights below before publishing the draft. -->
**Meshgrove @VERSION@**, a fast, native gallery for 3D-print files on Linux, Windows and macOS.

![Meshgrove](https://github.com/DonDavis-vibe/MeshGrove/raw/v@VERSION@/docs/images/screenshot-hero.png)

## Highlights

**Your own AI, if you want it** – new, optional and off until you switch it on. Meshgrove brings no AI of its own.

- **MCP server** `meshgrove-mcp`: Claude Desktop, Claude Code, LM Studio, Cursor, VS Code and other MCP assistants can search the libraries you open to them and suggest tags, notes, lists, moves and renames. Ready-made tasks: sort the inbox, put together a print list, clean up tags, tidy up a folder.
- **AI chat** with an OpenAI-compatible endpoint of your own (Ollama, LM Studio, llama.cpp, OpenAI, OpenRouter …): `@` adds files, folders, tags, lists, printers or a search as references, answers link the files they name.
- **One-click actions** that work with small local models too: sort a delivery from the inbox, suggest tags for the selection or right in the tag field, a print list from a description ("gifts under 2 h for the A1 mini").
- **You decide:** suggestions wait in a review with old → new; in the chat's automatic mode an undo arrow sits on your message. Moves always wait for the review and never overwrite anything. Everything accepted can be undone in the history.
- **Per library:** no AI, local AI only (for customer files) or cloud AI too, with or without thumbnails; an access log shows what was looked at.

**For several printers and customer files**

- **Several printers** with their build volumes, found in your slicers; `passt:"A1 mini"` and "fits on / too big for" everywhere.
- **Printability check:** open edges, non-manifold edges, several parts, volume, suspicious sizes (inches?), too big for every printer; `is:problem`.
- **Inbox:** everything new since the last time, grouped by the folder or ZIP it came in, with copies of files you already have counted as known.

**Lists, print log and more** (never released as 0.4, so here they are)

- ★ Favourites, collections (hand-picked or a saved search) and print lists with copies, ticked off as you print, exported as a PDF checklist or CSV.
- A print log (printed / failed) with `is:printed`, `is:unprinted`, `is:failed`.
- G-code previews with slicer, printer, print time and filament; readmes in the preview; a sortable list view; *Rescan folder*.
- Thanks to [@Rec0iL](https://github.com/Rec0iL): several slicers and mesh editors, found automatically in every common install, your own tag themes, files in inches and imperial display units, and 3D mice (SpaceMouse and DIY).

## Downloads

Every package includes STEP previews (OpenCascade). Nothing needs to be built.

| System | File | Notes |
|---|---|---|
| **Windows** 10/11 x64 | `Meshgrove-@VERSION@-windows-x64-setup.exe` | Installer. Installs for your user without admin rights (or for all users), with a Start menu entry and "Open with" for STL, 3MF, STEP, OBJ, PLY and glTF files. SmartScreen may warn because the program is not signed: choose *More info → Run anyway*. |
| | `Meshgrove-@VERSION@-windows-x64-portable.zip` | The same without installing: unpack anywhere and start `meshgrove.exe`. |
| **macOS** 11 or newer, Apple Silicon and Intel | `Meshgrove-@VERSION@-macos-universal.dmg` | Open it and drag Meshgrove onto *Applications*. The app is not notarized, so on the first start right-click it and choose *Open*, or run `xattr -dr com.apple.quarantine /Applications/Meshgrove.app`. |
| **Linux** x86_64, any distribution | `Meshgrove-@VERSION@-x86_64.AppImage` | Make it executable (`chmod +x`) and start it. Runs on distributions from 2022 on (glibc 2.35, e.g. Ubuntu 22.04, Debian 12). |
| **Ubuntu 24.04** | `meshgrove_@VERSION@-1_ubuntu24.04_amd64.deb` | `sudo apt install ./meshgrove_…deb`. Uses the system's libarchive and OpenCascade, and adds the menu entry and Dolphin/file-manager thumbnails. Also for Linux Mint 22 and other 24.04-based systems. |
| **Ubuntu 26.04** | `meshgrove_@VERSION@-1_ubuntu26.04_amd64.deb` | As above, for Ubuntu 26.04 and systems based on it. |
| **Debian 13** | `meshgrove_@VERSION@-1_debian13_amd64.deb` | As above, for Debian 13 (trixie). |
| **Fedora 44** | `meshgrove-@VERSION@-1.fc44.x86_64.rpm` | `sudo dnf install ./meshgrove-…rpm`. Uses the system's libarchive and OpenCascade, with menu entry and thumbnails. |

- **PDF previews** need poppler's `pdftoppm` on all systems (`dnf`/`apt install poppler-utils`, `brew install poppler`, or poppler for Windows). The .deb and .rpm recommend it.
- Other distributions: take the AppImage, or build from source (see the [README](https://github.com/DonDavis-vibe/MeshGrove#installation)).

## Known limitations

- Linux is the main platform. The Windows and macOS builds are built and tested automatically but have seen little real-world use yet, so please [report problems](https://github.com/DonDavis-vibe/MeshGrove/issues).
- On Windows, "Open in slicer" always opens a new slicer window.
- There are no thumbnails in Windows Explorer or the macOS Finder; the AppImage brings none for Linux file managers either (the .deb and .rpm do).

## License

Meshgrove is free software under the GPL-3.0-or-later. Each package contains `LICENSE` and `THIRD-PARTY-LICENSES.md` with the licenses of all libraries and fonts built into the program. The Windows, macOS and AppImage packages contain OpenCascade (LGPL-2.1) in the STEP helper; its source code is attached here as `opencascade-…-source.tar.gz`.

If Meshgrove saves you time, you can leave a tip on [Ko-fi](https://ko-fi.com/don_davis). Thanks!

<details>
<summary><b>🇩🇪 Kurzfassung auf Deutsch</b></summary>

**Meshgrove @VERSION@**: eine schnelle, native Galerie für 3D-Druck-Dateien unter Linux, Windows und macOS, mit echten Vorschauen für STL, 3MF, STEP, OBJ, PLY, glTF, G-code, Modelle in ZIP/7z/RAR, PDFs und Readmes, dazu Favoriten, Drucklisten und einem Druckprotokoll.

**Neu:** mehrere Drucker, ein Druckbarkeits-Check und ein Eingang für neue Dateien und Kundenlieferungen. Dazu, freiwillig und ab Werk aus, deine eigene KI: ein MCP-Server für Claude, LM Studio, Cursor & Co. und mit eigenem Endpunkt (Ollama, LM Studio, OpenAI-kompatibel) ein KI-Chat mit `@`-Verweisen und Ein-Klick-Aktionen. Die KI schlägt vor, du nimmst an, und alles lässt sich rückgängig machen.

**Installation** (alle Pakete können STEP, nichts muss gebaut werden):
- **Windows:** `…-windows-x64-setup.exe` ausführen. Installiert ohne Adminrechte für dich (oder für alle Benutzer), mit Startmenü-Eintrag und „Öffnen mit“ für STL, 3MF, STEP, OBJ, PLY und glTF. Bei der SmartScreen-Warnung *Weitere Informationen → Trotzdem ausführen* wählen. Ohne Installation: die `…-portable.zip` entpacken und `meshgrove.exe` starten.
- **macOS** (Apple Silicon und Intel): die DMG öffnen, Meshgrove auf *Programme* ziehen und beim ersten Start mit Rechtsklick → *Öffnen* starten.
- **Linux:** das AppImage ausführbar machen und starten (jede Distribution ab 2022). Für Ubuntu 24.04/26.04 und Debian 13 gibt es .deb-Pakete, für Fedora 44 ein .rpm; diese nutzen libarchive und OpenCascade des Systems und bringen Menüeintrag und Vorschaubilder für Dolphin mit.
- **PDF-Vorschauen** brauchen auf allen Systemen poppler.

Probleme bitte im [Issue-Tracker](https://github.com/DonDavis-vibe/MeshGrove/issues) melden. Wenn dir Meshgrove Zeit spart, freuen wir uns über ein Trinkgeld auf [Ko-fi](https://ko-fi.com/don_davis).

</details>
