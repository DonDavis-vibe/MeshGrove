<!-- Release notes template: the release workflow fills in the version and adds the checksums.
     Add this release's highlights below before publishing the draft. -->
**Meshgrove @VERSION@**, a fast, native gallery for 3D-print files on Linux, Windows and macOS.
Video, screenshots and every download at a glance: [meshgrove.redneck-design.de](https://meshgrove.redneck-design.de)

![Meshgrove](https://github.com/DonDavis-vibe/MeshGrove/raw/v@VERSION@/docs/images/screenshot-hero.png)

## Highlights

**From Downloads into the library** (new in 0.7)

- Name a downloads folder in *Settings → Library → Import* (one click for your Downloads). At start, or with *⬇ Import* in the sidebar, Meshgrove shows the new models, G-code and archives with models in there; tick what should come in and it moves them into the library's landing folder (`Import`), across drives too, numbered when a name is taken. Archives come in whole or unpacked, as you choose. Files still downloading and content the library has already are left out.
- Imported files land in the inbox like any new file; the history brings them back to where they were. Power users let the import run by itself at start.

<p>
<img src="https://github.com/DonDavis-vibe/MeshGrove/raw/v@VERSION@/docs/images/screenshot-import.png" alt="The import view: new files in Downloads with a tick each, archives whole or unpacked, Import selected" width="60%">
</p>

**Placed by tag** (new in 0.7)

- A file in the landing folder that gets a theme tag – from you, from the inbox's *✧ Sort in* or from your assistant – is placed into that tag's folder (`Garten/`), with missing folders made. *Settings → Tags → Placement by tag* holds the folder for each theme and for your own tags, in order of precedence: with several tags, the topmost wins (your own tags before the themes, a deeper folder before a flat one; drag a row by its handle or use ▲▼ to reorder). The review shows the other folders that fit, to choose instead.
- Placement is a suggestion like an assistant's and waits in the review; with *Accept suggestions automatically* on, moves out of the landing folder are applied by themselves – but only when exactly one folder fits; otherwise the file waits for your choice. Your own folders are never rearranged without you: *Place by tag* in the context menu asks, and every move can be undone.
- Assistants over MCP learn the rules too (`list_tags` names each tag's folder, `library_overview` the landing folder) and the *eingang-einordnen* task places what it tags.

<p>
<img src="https://github.com/DonDavis-vibe/MeshGrove/raw/v@VERSION@/docs/images/screenshot-placement-review.png" alt="The review with a placement batch: a coaster tagged Craft fair, Kitchen and Living room, with a menu to choose its folder" width="49%">
<img src="https://github.com/DonDavis-vibe/MeshGrove/raw/v@VERSION@/docs/images/screenshot-placement.png" alt="Settings → Tags → Placement by tag: the folder for each tag in order of precedence, one row being dragged by its handle" width="49%">
</p>

**Duplicates cleaned up at once** (new in 0.7)

- After looking through the duplicates, tick the groups whose ★ copy is the right one – or *All suggestions are fine* – and *Clean up selected* moves the extra copies of all of them to the trash with one confirmation. Rather keep another copy? Point at it and click ☆ *Keep*.

**Cut and paste** (new in 0.7)

- Ctrl+V (or *Paste* in the context menu, on a folder tile or the empty part of the view) copies the files on the clipboard into the folder: what you copied in Meshgrove with Ctrl+C, in Dolphin, Explorer or Finder, or paths as text. Taken names get a number, nothing is replaced, the copies keep their tags and notes (they are the same model), and the history undoes it.
- Ctrl+X (or *Cut*) marks the selection – the tiles fade – and the next paste moves the files instead: within a library with tags, notes and previews along, from elsewhere like an import; journalled and undoable like every move. Dolphin and Nautilus honour the cut marker, and what they cut is moved when pasted in Meshgrove.

**Smaller things** (new in 0.7)

- *Zuletzt gedruckt / Last printed* below the favourites in the sidebar: the models you last sent to the slicer, newest first. How many it keeps (10 unless you change it, or off) is a slider in *Settings → Programs*.
- Sorting moved into its own toolbar button next to the folder/library switch, and files Meshgrove cannot show (stray `.txt`, `.ini` and the like) now come after the ones it can, whatever the sort order; the same menu turns that off.

**Your AI apps connect themselves** (since 0.6)

- While AI is on, Meshgrove adds itself to the assistants it finds on your computer – Claude Desktop, Claude Code, LM Studio, Cursor, VS Code, Codex, Gemini CLI, Copilot CLI, Windsurf, Zed and OpenCode – and *Settings → AI & MCP* lists them with their state. No more copying entries; restart an app once and it can use Meshgrove.
- Only Meshgrove's own entry changes in their settings, each file is kept as `.bak` before the first change, your own entries are left alone, and switching it off takes the entries out again. `meshgrove-mcp setup` (and `remove`) does the same from a terminal.
- The idea and the code that finds and edits the apps' settings come from [SolidCraft](https://github.com/tangency-labs/solidcraft) – thank you.

**Settings, rearranged** (since 0.6)

- The pages are grouped (display, printing, extend), each says in a line what it holds, and its sections fold away with a click on their title.
- Labels and controls sit in aligned columns, choices among a few are buttons side by side, and *Restore defaults* puts a page back to how Meshgrove comes (your library folders, programs, printers and AI permissions stay).

<p>
<img src="https://github.com/DonDavis-vibe/MeshGrove/raw/v@VERSION@/docs/images/screenshot-settings.png" alt="The settings: pages in groups, folding sections, aligned rows and Restore defaults" width="60%">
</p>

**Reviewing suggestions** (since 0.6)

- A tick for each file and one for the whole batch: leave out what does not fit, then *Accept selected*.
- New, for experienced users: *Accept suggestions automatically*. New suggestions of AI apps and one-click actions are applied without the review; moves still wait for you, and everything can be undone in the history.
- Fixed: back in the library after a tag or a search, its folders had no pictures and files were missing until F5.


**Tiles your way** (since 0.5.2)

- *Settings → Tiles* shows a sample tile with every badge on it. Drag the file type, filament colours, duplicates, tags, print bed check, favourite and print state into any corner of the picture, reorder them within a corner, or drop them into a tray to hide them; a right-click does the same.
- Tags as names or colour dots, filament colours as dots or a stripe along the picture's edge, the bed check with text or as a symbol only, and file names on two, one or no lines, with or without the extension. The tile size and the folder previews have moved there too.
- Nothing changes until you change it: the standard layout looks exactly as before.

<p>
<img src="https://github.com/DonDavis-vibe/MeshGrove/raw/v@VERSION@/docs/images/screenshot-tiles.png" alt="Settings → Tiles: a sample tile with the badges moved around, a hidden one in the tray, and how tags, filaments and the check look" width="60%">
</p>


**Your own AI, if you want it** (since 0.5) – optional and off until you switch it on. Meshgrove brings no AI of its own.

- **MCP server** `meshgrove-mcp`: Claude Desktop, Claude Code, LM Studio, Cursor, VS Code and other MCP assistants can search the libraries you open to them and suggest tags, notes, lists, moves and renames. Ready-made tasks: sort the inbox, put together a print list, clean up tags, tidy up a folder.
- **AI chat** with an OpenAI-compatible endpoint of your own (Ollama, LM Studio, llama.cpp, OpenAI, OpenRouter …): `@` adds files, folders, tags, lists, printers or a search as references, answers link the files they name.
- **One-click actions** that work with small local models too: sort a delivery from the inbox, suggest tags for the selection or right in the tag field, a print list from a description ("gifts under 2 h for the A1 mini").
- **You decide:** suggestions wait in a review with old → new; in the chat's automatic mode an undo arrow sits on your message. Moves always wait for the review and never overwrite anything. Everything accepted can be undone in the history.
- **Per library:** no AI, local AI only (for customer files) or cloud AI too, with or without thumbnails; an access log shows what was looked at.

<p>
<img src="https://github.com/DonDavis-vibe/MeshGrove/raw/v@VERSION@/docs/images/screenshot-chat.png" alt="The AI chat with model links and a print list waiting for review" width="49%">
<img src="https://github.com/DonDavis-vibe/MeshGrove/raw/v@VERSION@/docs/images/screenshot-review.png" alt="The review: renames, a move into a new folder and tag suggestions" width="49%">
</p>
<p>
<img src="https://github.com/DonDavis-vibe/MeshGrove/raw/v@VERSION@/docs/images/screenshot-ai-settings.png" alt="What AI may see, per library" width="60%">
</p>


**For several printers and customer files**

- **Several printers** with their build volumes, found in your slicers; `passt:"A1 mini"` and "fits on / too big for" everywhere.
- **Printability check:** open edges, non-manifold edges, several parts, volume, suspicious sizes (inches?), too big for every printer; `is:problem`.
- **Inbox:** everything new since the last time, grouped by the folder or ZIP it came in, with copies of files you already have counted as known.

<p>
<img src="https://github.com/DonDavis-vibe/MeshGrove/raw/v@VERSION@/docs/images/screenshot-inbox.png" alt="The inbox with a customer folder and a ZIP" width="49%">
<img src="https://github.com/DonDavis-vibe/MeshGrove/raw/v@VERSION@/docs/images/screenshot-checks.png" alt="Fits on / too big for each printer, and the printability check" width="49%">
</p>


**Lists, print log and more** (never released as 0.4, so here they are)

- ★ Favourites, collections (hand-picked or a saved search) and print lists with copies, ticked off as you print, exported as a PDF checklist or CSV.
- A print log (printed / failed) with `is:printed`, `is:unprinted`, `is:failed`.
- G-code previews with slicer, printer, print time and filament; readmes in the preview; a sortable list view; *Rescan folder*.
- Thanks to [@Rec0iL](https://github.com/Rec0iL): several slicers and mesh editors, found automatically in every common install, your own tag themes, files in inches and imperial display units, and 3D mice (SpaceMouse and DIY).
- The sidebar folds: lists, tags (each tag group too) and devices fold away with a click on their title, and stay folded.
- Fixed: models in a folder inside a ZIP or another archive get their sizes, check and duplicate matches; until now the library never finished "Indexing" for them.
- Fixed: copy and paste in text fields and the copy buttons (such as the MCP entry in Settings → AI & MCP), which did nothing before.
- Windows: Meshgrove now also writes its log to `meshgrove.log` (*Settings → About Meshgrove* shows where), so problems can be reported without a console.
- Fixed (Windows): the Claude Code command for the MCP server works in the command prompt (cmd) as well as in PowerShell.
- Fixed: the hints about the trash and copied files no longer name Dolphin on Windows and macOS.

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

**Neu in 0.7:** Nenne unter *Einstellungen → Bibliothek → Import* deinen Download-Ordner, und Meshgrove zeigt beim Start oder über *⬇ Import* in der Seitenleiste, welche neuen Modelle, G-Codes und Archive dort liegen; ein Klick holt sie in den Landeordner der Bibliothek (verschoben, auch über Laufwerke hinweg, Archive ganz oder entpackt, rückgängig machbar). Bekommt eine Datei dort einen Themen-Tag, legt Meshgrove sie in den Ordner des Tags ab – nach den Regeln unter *Einstellungen → Tags → Ablage nach Tag* (bei mehreren Tags zählt der oberste; die Prüfliste bietet die anderen passenden Ordner an), als Vorschlag in der Prüfliste oder mit „automatisch annehmen“ von selbst, wenn genau ein Ordner passt. Deine eigenen Ordner werden nie ungefragt umgeräumt; „Nach Tag ablegen“ im Rechtsklick-Menü fragt immer. Strg+V (oder „Einfügen“ im Rechtsklick-Menü) kopiert Dateien aus der Zwischenablage in den Ordner – aus Meshgrove, dem Dateimanager oder als Pfade – ohne etwas zu ersetzen, mit Tags und Notizen und rückgängig machbar; nach Strg+X („Ausschneiden“) verschiebt das Einfügen stattdessen. Bei den Duplikaten hakst du nach der Sichtung die Gruppen ab, deren ★-Kopie passt, und bereinigst alle auf einmal; ☆ *Behalten* wählt eine andere Kopie. Dazu „Zuletzt gedruckt“ in der Seitenleiste (die zuletzt an den Slicer geschickten Modelle) und ein eigener Sortier-Knopf, bei dem unterstützte Dateien zuerst kommen. Dein KI-Assistent kennt die Regeln ebenfalls.

**Seit 0.6:** Meshgrove trägt sich selbst bei deinen KI-Apps ein (Claude Desktop, Claude Code, LM Studio, Cursor, VS Code, Codex, Gemini CLI und weitere; Idee und Code von SolidCraft) – kein Eintrag muss mehr kopiert werden. Die Einstellungen sind neu geordnet: Seiten in Gruppen, einklappbare Abschnitte, ausgerichtete Zeilen und „Standard wiederherstellen“. In der Prüfliste gibt es Häkchen pro Datei und pro Block, und wer möchte, lässt neue Vorschläge automatisch annehmen (Verschiebungen warten immer). Behoben: Nach einem Tag oder einer Suche zeigte die Bibliothek ihre Ordner ohne Bilder, bis man F5 drückte.

**Seit 0.5:** mehrere Drucker, ein Druckbarkeits-Check und ein Eingang für neue Dateien und Kundenlieferungen. Dazu, freiwillig und ab Werk aus, deine eigene KI: ein MCP-Server für Claude, LM Studio, Cursor & Co. und mit eigenem Endpunkt (Ollama, LM Studio, OpenAI-kompatibel) ein KI-Chat mit `@`-Verweisen und Ein-Klick-Aktionen. Die KI schlägt vor, du nimmst an, und alles lässt sich rückgängig machen. Außerdem lassen sich Listen, Tags und Tag-Gruppen in der Seitenleiste einklappen, und Kopieren und Einfügen in Textfeldern funktioniert.

**Seit 0.5.2:** Unter *Einstellungen → Kacheln* stellst du an einer Beispielkachel selbst zusammen, welche Markierungen wo auf den Kacheln stehen: einfach in eine Ecke ziehen oder ausblenden, dazu Tags als Namen oder Punkte, Filamente als Punkte oder Streifen und den Dateinamen ein- oder zweizeilig, mit oder ohne Endung. Unter Windows schreibt Meshgrove jetzt eine Log-Datei (`meshgrove.log`, Pfad unter *Über Meshgrove*), der MCP-Befehl für Claude Code funktioniert auch in der Eingabeaufforderung, und die Hinweise zu Papierkorb und kopierten Dateien nennen nicht mehr Dolphin.

**Installation** (alle Pakete können STEP, nichts muss gebaut werden):
- **Windows:** `…-windows-x64-setup.exe` ausführen. Installiert ohne Adminrechte für dich (oder für alle Benutzer), mit Startmenü-Eintrag und „Öffnen mit“ für STL, 3MF, STEP, OBJ, PLY und glTF. Bei der SmartScreen-Warnung *Weitere Informationen → Trotzdem ausführen* wählen. Ohne Installation: die `…-portable.zip` entpacken und `meshgrove.exe` starten.
- **macOS** (Apple Silicon und Intel): die DMG öffnen, Meshgrove auf *Programme* ziehen und beim ersten Start mit Rechtsklick → *Öffnen* starten.
- **Linux:** das AppImage ausführbar machen und starten (jede Distribution ab 2022). Für Ubuntu 24.04/26.04 und Debian 13 gibt es .deb-Pakete, für Fedora 44 ein .rpm; diese nutzen libarchive und OpenCascade des Systems und bringen Menüeintrag und Vorschaubilder für Dolphin mit.
- **PDF-Vorschauen** brauchen auf allen Systemen poppler.

Probleme bitte im [Issue-Tracker](https://github.com/DonDavis-vibe/MeshGrove/issues) melden. Wenn dir Meshgrove Zeit spart, freuen wir uns über ein Trinkgeld auf [Ko-fi](https://ko-fi.com/don_davis).

</details>
