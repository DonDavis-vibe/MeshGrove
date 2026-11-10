# Writing plugins for Meshgrove

A Meshgrove plugin is a WebAssembly component (`wasm32-wasip2`): one `.wasm` file runs on Linux,
Windows and macOS. Meshgrove runs it in a sandbox with no network and no environment. The
plugin can see only its own folders, and it has a memory limit and deadlines. Plugins can offer
four kinds of things:

- **tools** for the AI: the chat, and AI apps through `meshgrove-mcp`
- **panels**: tabs in Meshgrove's window, drawn from a widget tree the plugin sends
- **jobs**: background work a panel starts
- **file handlers**: "Open with <plugin>" for the plugin's file types

Plugins never write into the library directly. They hand over a **bundle**: files plus tags,
notes and print lists, which Meshgrove applies as one change the user can undo. A plugin may
read the library only when the user allows it, and then by file ids, never by paths.

The interface is the crate [`meshgrove-plugin-api`](../crates/meshgrove-plugin-api). It is MIT
OR Apache-2.0: **plugins may use any licence**, open or closed (Meshgrove itself is GPL-3.0).
[`examples/plugins/hello-box`](../examples/plugins/hello-box) is a complete example: a tool, a
panel with a form and a 3D view, a job, a file handler and a bundle.

## Contents

- [Building a plugin](#building-a-plugin)
- [The manifest](#the-manifest)
- [Running in the sandbox](#running-in-the-sandbox)
- [Talking to Meshgrove](#talking-to-meshgrove)
- [Tools for the AI](#tools-for-the-ai)
- [Bundles: files for the library](#bundles-files-for-the-library)
- [Panels](#panels)
- [Widgets and when they send events](#widgets-and-when-they-send-events)
- [Effects](#effects)
- [Jobs](#jobs)
- [Opening files](#opening-files)
- [Testing with meshgrove-plugin-run](#testing-with-meshgrove-plugin-run)
- [Installing and granting](#installing-and-granting)
- [API versions](#api-versions)

## Building a plugin

```toml
# Cargo.toml
[package]
name = "my-plugin"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
# Not on crates.io yet: from the Meshgrove repository (or `path = …` to a checkout).
meshgrove-plugin-api = { git = "https://github.com/DonDavis-vibe/MeshGrove", features = ["guest"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

[profile.release]
opt-level = "s"
lto = true
strip = true
```

```rust
// src/lib.rs
use meshgrove_plugin_api::guest::{self, Guest, GuestPanel, PluginError, ToolOutput};
use meshgrove_plugin_api::ui::{Node, View};
use meshgrove_plugin_api::{API_VERSION, Manifest, ToolDef};

struct MyPlugin;
struct MyPanel;

impl Guest for MyPlugin {
    type Panel = MyPanel;

    fn manifest() -> String {
        guest::json(&Manifest {
            id: "my-plugin".into(),
            name: "My plugin".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api: API_VERSION.into(),
            ..Manifest::default()
        })
    }

    fn list_tools() -> String {
        guest::json(&Vec::<ToolDef>::new())
    }

    fn call_tool(name: String, _args: String) -> Result<ToolOutput, PluginError> {
        Err(PluginError::Invalid(format!("no tool {name}")))
    }

    fn run_job(kind: String, _params: String) -> Result<String, PluginError> {
        Err(PluginError::Invalid(format!("no job {kind}")))
    }

    fn open_file(_name: String, _bytes: Vec<u8>) -> Result<String, PluginError> {
        Ok(guest::effects(&[]))
    }
}

impl GuestPanel for MyPanel {
    fn new(_id: String, _state: Option<String>) -> MyPanel {
        MyPanel
    }

    fn render(&self) -> String {
        guest::json(&View::new(Node::label("Hello")))
    }

    fn event(&self, _event: String) -> String {
        guest::effects(&[])
    }

    fn mesh(&self, _key: String) -> Option<guest::MeshData> {
        None
    }

    fn snapshot(&self) -> String {
        String::new()
    }
}

meshgrove_plugin_api::export_plugin!(MyPlugin);
```

```sh
rustup target add wasm32-wasip2
cargo build --target wasm32-wasip2 --release
# → target/wasm32-wasip2/release/my_plugin.wasm
```

**Distribution Rust (Fedora and others):** when `/usr/bin/cargo` comes first in `PATH`, it uses
the distribution's `rustc`, which has no `wasm32-wasip2` standard library. `rustup run stable
cargo` alone is not enough, because that cargo still finds `/usr/bin/rustc`. Use rustup's
toolchain for both:

```sh
PATH="$(dirname "$(rustup which --toolchain stable cargo)"):$PATH" cargo build --target wasm32-wasip2 --release
```

`scripts/build-plugins.sh` in the Meshgrove repository does this for the examples.

**Helpers in `meshgrove_plugin_api::guest`:**

- `json` and `parse`: to and from JSON.
- `effects`: a list of effects as JSON.
- `tool_output(&result, Option<&Bundle>)`
- `write_stl(path, &MeshData, name)`: a binary STL.
- `info()`: what the host tells about itself.
- `log`, plus the modules `host` and `library`.

`MeshData::cuboid`, `bounds`, `triangles` and `to_stl` are in the API crate.

## The manifest

`manifest()` returns a JSON `Manifest`:

| Field | Meaning |
|---|---|
| `id` | `[a-z][a-z0-9-]{1,30}`, unique among installed plugins; the AI sees tools as `<id>_<tool>` |
| `name`, `version`, `description`, `authors`, `license`, `homepage` | shown in Settings → Plugins |
| `api` | the API version it was built for: `meshgrove_plugin_api::API_VERSION` |
| `permissions` | what it asks for (below); the user grants each one |
| `panels` | `{id, title}` of each panel it can open |
| `file_types` | `{pattern, description}`, e.g. `*.gridfinity.json`; `*` alone is refused |
| `memory_mb` | memory one instance needs; default 1024, between 16 and 4096 (the user's setting wins) |

**Permissions:**

| Permission | Lets the plugin | The user grants |
|---|---|---|
| `ai_tools` | offer its tools to the chat and to AI apps | "Tools for the AI" |
| `bundles` | hand files over to the library | "Put files into the library", and optionally "Accept without review" (auto-accept) |
| `library_read` | `library.search`, `describe`, `triangles` | "Read the library": no / what local AI sees / everything |

Without a grant, the matching call fails with a message the plugin can pass on, for example
"the user has not allowed bingrove to read the library (Meshgrove → Settings → Plugins)".
Grants are checked on every call.

## Running in the sandbox

- **Folders:**
  - `/data` is the plugin's own folder. It is writable for panels and jobs, read-only for tool calls.
  - `/out` is a staging folder for the files of a bundle. A panel and the jobs it starts share one.
  - `/tmp` is fresh for each instance (a panel keeps its own; picked files land there).
- **No network, no environment variables.** Stderr goes to Meshgrove's log (for panics and debug output).
- **Instances:** every tool call and every job gets a fresh instance. An open panel keeps its
  instance on a thread of its own as long as its tab is open.
- **Deadlines:**
  - A panel event, including render, mesh and snapshot, gets **2 s**.
  - A tool call gets **60 s**.
  - Jobs have no deadline; the user cancels them.
  - Time Meshgrove spends in `library.*` calls on the plugin's behalf does not count.
  - Past the deadline the instance is stopped. A panel then shows the error and offers to open again.
- **Cancel:** check `host::cancelled()` in long loops. If the plugin does not look, it is
  interrupted within ~10 ms. Cancelling happens when the user stops a job, stops the chat's
  answer, or an AI app cancels its request.
- **Memory:** an instance that grows past its limit is stopped with "needed more than its N MiB".

## Talking to Meshgrove

**`host` (always there):**

- `log(level, message)`
- `progress(fraction, message)`: for jobs.
- `cancelled()`
- `info()`: JSON `HostInfo`, with the language (`de`/`en`), dark theme, printers with their
  build volumes (main one first), and Meshgrove's and the API's version. Panels get
  `Event::HostChanged` when any of it changes, and once right after they open.

**`library` (with `library_read`):**

- `search(query, limit)`: a page of matches for a search as typed in Meshgrove, for example
  `rose tag:Garten ext:stl`, as JSON with ids, names, sizes and tags.
- `describe(id)`: facts of one file as JSON (size in mm, tags, note, lists, printability).
- `triangles(id, max_triangles)`: the model's triangles in millimetres (STL, 3MF, OBJ, PLY,
  glTF, STEP, also inside archives). It is simplified to the budget by vertex clustering:
  outlines and proportions stay, small details may go.

What the plugin can see depends on the grant and on who is calling:

- **"What local AI sees":** a panel or a job reads as the user, locally. A tool call reads as the
  AI that called it: a cloud AI does not see local-only libraries.
- **"Everything":** every library.

Answers never contain paths. Ids look like `Garten/Rankhilfe/rose.stl`.

## Tools for the AI

**Defining tools:**

- `list_tools()` returns at most **8** `ToolDef { name, description, input_schema, bundles }`.
- `name` is `[a-z][a-z0-9_]*`, and `<id>_<name>` must stay within 64 characters.
- `input_schema` is a JSON-schema object of at most 8 KB.
- `bundles: true` marks a tool that may return files. It counts as "writes something" for the chat and AI apps.

**Answering:** `call_tool(name, args)` returns `ToolOutput`:

- `result`: JSON for the AI, at most 1 MiB.
- `images`: up to 4 PNG or JPEG of up to 8 MiB each. They go to MCP clients as pictures and
  show in Meshgrove's chat under the answer.
- `bundle`: optional, see below.

Errors are `PluginError::{Invalid, Failed, Denied, Cancelled}` with a text for the AI.

**Who sees the tools:** the chat sees the tools of plugins with the `ai_tools` grant;
`meshgrove-mcp` lists them beside its own. When a plugin is switched on or off, MCP clients
receive `notifications/tools/list_changed`. Tool names must not clash with Meshgrove's own tools.

Meshgrove tells the AI that a plugin's answers are data, not instructions.

## Bundles: files for the library

Write the files below `/out`, then describe them:

```json
{
  "title": "Gridfinity Schublade",
  "summary": "11 parts (14 pieces) for a 250 × 305 mm drawer",
  "folder": "Gridfinity/Schublade",
  "files": [{"path": "bin_2x1_6u.stl", "tags": ["Gridfinity"], "note": "…"}],
  "tags": ["Bingrove"],
  "lists": [{"name": "Schublade – Teile", "kind": "print", "items": [{"path": "bin_2x1_6u.stl", "quantity": 3}]}]
}
```

**Rules:**

- **Paths** are relative to `/out` and `/`-separated. No absolute paths, no `..`, no links.
- **Every file below `/out` goes along**, not only those named in the bundle.
- **Where it lands:** `folder` inside the library imports go to. Without one, it lands in the
  import folder under the title. A taken name gets " (2)". Nothing is ever replaced.
- **Limits:** at most 2000 files and 2 GiB; 8 tags per file; 8 lists; quantities 1–999.
- **One change:** the copy, its indexing (the files count as seen, not new in the inbox), tags,
  notes and lists form a single change in Meshgrove's history. Undo removes the files (but keeps
  the ones the user edited since) and the lists. Redo copies again from the staging folder,
  which Meshgrove keeps while anything refers to it.

**How a bundle is applied:**

| Coming from | What happens |
|---|---|
| A tool, chat in review mode | waits in the review as a card (files, folder, size, lists) |
| A tool, chat in automatic mode | applied at once, unless the plugin lacks "Accept without review" |
| A tool, AI app over MCP | waits in the review; accepted by itself only when the AI's auto-accept is on **and** the plugin has "Accept without review" |
| A panel, `Effect::ApplyBundle` | applied at once (the user clicked for it), with a toast; undo in the history |
| A panel, `Effect::ProposeBundle` | goes to the review, never accepted by itself |

**What became of it:** a panel hears back with `Event::BundleFinished { title, outcome }`, where
`title` is the bundle's. Show this to the user rather than assuming that handing the bundle over
worked.

| `outcome` | Meaning |
|---|---|
| `Applied { steps, skipped }` | the change went through and is in the history; `skipped` steps did nothing (a file already had that tag, say). Sent once it is done, not when it starts |
| `Queued` | waits in the review for the user to decide |
| `Refused` | the user has not allowed "Put files into the library" (Settings → Plugins). Nothing was tried and the files are gone |
| `Failed { error }` | it did not work, and the library is as it was |

A panel closed in the meantime hears nothing, and neither does a panel after the user decides in
the review. Meshgrove 0.8.1 and older do not send this event, so a panel must keep working
without it.

## Panels

A panel is Elm-like:

1. `render()` returns a `View { v: 1, root: Node }`.
2. When the user does something, Meshgrove sends an `Event` to `event()`.
3. `event()` answers with a list of `Effect`s, and Meshgrove renders the panel again.

Right after a panel opens, before its first `render()`, it gets `Event::HostChanged` with the
current host info. Its effects count like any other's: a panel that needs longer than an event
may (2 s) to build its first preview starts a job there and shows the result once the job has
finished.

While a panel works, its tab shows ⏳. The window never waits for a plugin.

- **3D views:** `mesh(key)` is asked when a `MeshView`'s `revision` changes. Bump the revision
  whenever the model changes; the camera stays where the user put it.
- **State:** `snapshot()` returns the panel's state as a string. When the tab is closed or
  Meshgrove quits, Meshgrove keeps it and passes it to `new(id, state)` next time. Panels open
  in the last session open again on start.

## Widgets and when they send events

Values the user is editing stay on screen until the plugin renders again. After that, the
plugin's values count: it may correct or refuse them.

| Node | Sends |
|---|---|
| `column`, `row`, `group` (title, collapsible), `scroll` (max_height), `split` (vertical, ratio; the user can drag the border) | — |
| `tabs` | `Changed {value: index}` when another tab is picked |
| `label` (style: body, heading, strong, small, weak, mono), `markdown`, `separator`, `notice` (info, success, warn, error), `progress` (`fraction`; none = busy), `image` (base64 PNG/JPEG) | — |
| `button` (primary, enabled) | `Click` |
| `text_input` | `Changed {value: text}` on Enter or when the field loses focus, not per keystroke |
| `number_input` (min, max, step, decimals, unit) | `Changed {value: number}` once a typed number is done (Enter, Tab or leaving the field; Escape drops it), or when a drag is let go. With `unit: mm` it shows the user's unit (inches when imperial) but **always sends millimetres** |
| `slider` | like `number_input` |
| `checkbox`, `select`, `radio`, `color` (`#rrggbb[aa]`) | `Changed` at once |
| `form` (rows of label + node; optional `submit` button) | the rows' own events; `Submit {values}` with every input by id |
| `table` (columns: title, width, numeric; rows: id + cells; selected; max_height) | `TableSelect {row}`; clicking the selected row again sends `row: null`. Columns without a `width` can be dragged wider by the user; a cell cut short shows its whole text on hover |
| `mesh_view` (key, revision, height, `bed: [x, y]`, `frame: [min, max]`) | — the bed is drawn under the model, centred; the frame is the space the model is meant to fit (a drawer, a shelf), drawn in blue round it and kept in the picture; the view's *Frame* button hides it (from 0.9.0; older versions ignore it) |
| `grid_editor` | `Grid {change}`, see below |
| `canvas` | `Canvas {change}` when `interactive`, see below |
| anything newer | shown as a placeholder |

**Grid editor** (cells × rows, `pitch` mm per cell, `snap_div`, zones, items):

- **Coordinates** are in cells, with y running down the widget.
- **Selection:** click selects; Shift/Ctrl-click adds or removes. Each change of the selection
  sends `Selected {items}`.
- **Moving:** dragging an item sends `Moved {item, x, y}`, its new top-left snapped to
  `1/snap_div` of a cell.
  - It is sent even when the item lands on another one (the ghost turns red): the plugin decides.
  - Moves stay within the grid and the `accepts_items` zones.
- **Resizing:** dragging an edge or corner sends `Resized {item, x, y, w, h}`.
- **Drawing:** dragging on free cells sends `Created {x, y, w, h}`, only when it fits.
- **Keys:** arrows send one `Moved` per selected item; Delete/Backspace sends `Deleted {items}`.
- **Context menu:**
  - the item's `actions`, each sending `Action {item, action}`
  - "Merge (w × h)" when several items are selected → `Merge {items}`
  - "Split" for items larger than a cell → `Split {item}`
  - "Delete" → `Deleted`
- **Locked items** cannot be dragged, resized or deleted.
- **Item ids** are the plugin's. If they are indices, re-number the selection after edits.

**Canvas** (`width` × `height` in its own units, y down, shapes):

- **Shapes:** `line`, `polygon` (concave is fine), `rect` (optional `id`), `circle`, `text`.
  `text.size` is in canvas units (no size: 12 pt); stroke widths are in screen pixels.
- **Looking around:** wheel zooms, right or middle button pans, double click shows everything.
  All of this stays in Meshgrove.
- **Events** (only when `interactive`):
  - `Click {x, y, shape}`, where `shape` is the topmost rect with an `id` under the pointer.
  - `Drag {from, to}` in canvas units, at most ~30 per second plus a last one on release;
    `from` is where the button went down.

## Effects

| Effect | Meshgrove |
|---|---|
| `toast {text, level}` | shows the text in the status bar, prefixed with the plugin's name |
| `start_jobs {jobs: [{id, kind, params, label}]}` | runs them in parallel (see Jobs) |
| `cancel_job {job}` | cancels it |
| `apply_bundle {bundle}` | applies what the panel and its jobs wrote to `/out` (needs `bundles`) |
| `propose_bundle {bundle}` | the same, into the review |
| `save_file {path, name}` | offers a file below `/out` or `/tmp` for saving (asks for the place) |
| `open_file_dialog {id, title, extensions}` | asks for a file, copies it into the panel's `/tmp`, sends `FilePicked {id, name, path}` |
| `open_panel {panel, state}` | opens (or, with a state, re-opens) a panel |
| `select_model {id}` | shows that library file in the gallery |
| `repaint_after {ms}` | renders the panel again after that long |

## Jobs

`start_jobs` runs each job in a fresh instance on a pool of threads, several at once.

- **Running:** `run_job(kind, params)` writes into the panel's `/out`, reports
  `host::progress(fraction, message)` and returns a JSON result.
- **Events back to the panel:**
  - `JobProgress {job, fraction, message}`: only the newest per job and frame.
  - `JobFinished {job, result | error}`.
- **Showing progress:** Meshgrove shows a progress bar with a cancel button above the panel
  while jobs run.
- **A common pattern:** start the jobs, then `apply_bundle` when the last one finishes.

## Opening files

For a file matching one of the manifest's `file_types`, the gallery's context menu offers
"Open with <plugin>". Meshgrove calls `open_file(name, bytes)` (up to 64 MiB), and the plugin
answers with effects, usually `open_panel` with the file's content as state.

## Testing with meshgrove-plugin-run

`meshgrove-plugin-run` runs a plugin without Meshgrove and without a library. Build it with
`cargo build -p meshgrove-plugins --bin meshgrove-plugin-run`, then:

```sh
meshgrove-plugin-run my_plugin.wasm manifest            # the manifest, checked
meshgrove-plugin-run my_plugin.wasm tools               # the tool list, checked
meshgrove-plugin-run my_plugin.wasm call make_box '{"width": 30}' --out out/
meshgrove-plugin-run my_plugin.wasm job box '{}' --out out/
meshgrove-plugin-run my_plugin.wasm open drawer.gridfinity.json
meshgrove-plugin-run my_plugin.wasm mcp --out out/      # an MCP server with the plugin's tools
```

Options: `--memory MB`, `--no-cache`. The `mcp` mode can be added to an AI app like any MCP
server, to try the tools from there. Meshgrove's own tests run `examples/plugins/hello-box`
(`scripts/build-plugins.sh --test-hooks`; `MESHGROVE_TEST_PLUGIN` names another file).

## Installing and granting

1. **Install:** Settings → Plugins. Meshgrove looks for plugins by itself: `.wasm` files directly
   in its plugins folder ("Open plugins folder"), in Downloads, in the import folders and in the
   folders plugins were installed from before. It checks each file the way an install would and
   offers the plugins it finds with "Install". "Browse …" opens the system's file dialog, and a
   `.wasm` file dropped on the window is installed as well. Meshgrove copies the plugin to
   `~/.local/share/meshgrove/plugins/<id>/<id>.wasm` (the data folder on Windows and macOS) and
   switches it on with **nothing granted**.
2. **Grant:** tick what it may do. Its panels appear under "Plugins ▾" next to "View".
3. **Update:** a file of an installed plugin with the same or a newer version is offered with
   "Update"; so after installing once from your build folder, each new build shows up there.
   The grants stay, but a plugin that asks for more stays off until it is switched on again.
4. **Outside changes:** a file changed outside Meshgrove (another hash than the one enabled)
   does not run until the user switches it on again.
5. **Remove:** "Remove" deletes the installed file. The plugin's data and staging folders
   stay, so undo keeps working. Meshgrove cleans up staging folders nothing refers to any more
   when it starts.

In `config.toml`:

```toml
[[plugins]]
id = "bingrove"
file = "/home/me/.local/share/meshgrove/plugins/bingrove/bingrove.wasm"
blake3 = "7426098f…"
enabled = true

[plugins.grants]
ai_tools = true
bundles = true
auto_accept = false
library_read = "none"   # "ai-scope" or "all"
```

## API versions

The WIT package is `meshgrove:plugin@<API_VERSION>`. Meshgrove runs a plugin when it was built for
**the same major version and a minor version no newer than its own**: a Meshgrove with API 0.2
runs plugins built for 0.1. A plugin built for a newer minor asks the user to update Meshgrove.
One built for another major version is refused, with a message saying which Meshgrove it needs.
Widgets Meshgrove does not know show as placeholders, so a panel still works on a slightly older
Meshgrove.

Events go the other way, so a newer Meshgrove can send a kind your plugin has never heard of. It
reads as `Event::Unknown`; with an older `meshgrove-plugin-api`, parsing it fails instead. Either
way, answer with no effects, as hello-box does, and never `unwrap()` the parse: a panic ends the
panel.

```rust
let Ok(event) = serde_json::from_str::<Event>(&event) else { return guest::effects(&[]) };
```
