<!-- Release notes template: the release workflow fills in the version and adds the checksums.
     Add this release's highlights below before publishing the draft. -->
**Meshgrove @VERSION@**, a fast, native gallery for 3D-print files on Linux, Windows and macOS.

![Meshgrove](https://github.com/DonDavis-vibe/MeshGrove/raw/v@VERSION@/docs/images/screenshot-hero.png)

## Highlights

- …

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

**Meshgrove @VERSION@**: eine schnelle, native Galerie für 3D-Druck-Dateien unter Linux, Windows und macOS, mit echten Vorschauen für STL, 3MF, STEP, OBJ, PLY, glTF, Modelle in ZIP/7z/RAR und PDFs.

**Installation** (alle Pakete können STEP, nichts muss gebaut werden):
- **Windows:** `…-windows-x64-setup.exe` ausführen. Installiert ohne Adminrechte für dich (oder für alle Benutzer), mit Startmenü-Eintrag und „Öffnen mit“ für STL, 3MF, STEP, OBJ, PLY und glTF. Bei der SmartScreen-Warnung *Weitere Informationen → Trotzdem ausführen* wählen. Ohne Installation: die `…-portable.zip` entpacken und `meshgrove.exe` starten.
- **macOS** (Apple Silicon und Intel): die DMG öffnen, Meshgrove auf *Programme* ziehen und beim ersten Start mit Rechtsklick → *Öffnen* starten.
- **Linux:** das AppImage ausführbar machen und starten (jede Distribution ab 2022). Für Ubuntu 24.04/26.04 und Debian 13 gibt es .deb-Pakete, für Fedora 44 ein .rpm; diese nutzen libarchive und OpenCascade des Systems und bringen Menüeintrag und Vorschaubilder für Dolphin mit.
- **PDF-Vorschauen** brauchen auf allen Systemen poppler.

Probleme bitte im [Issue-Tracker](https://github.com/DonDavis-vibe/MeshGrove/issues) melden. Wenn dir Meshgrove Zeit spart, freuen wir uns über ein Trinkgeld auf [Ko-fi](https://ko-fi.com/don_davis).

</details>
