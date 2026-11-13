# Writing plugins for Meshgrove

A Meshgrove plugin is a WebAssembly component (`wasm32-wasip2`): one `.wasm` file runs on Linux,
Windows and macOS. Meshgrove runs it in a sandbox with no network and no environment. The
plugin can see only its own folders, and it has a memory limit and deadlines. Plugins can offer
four kinds of things:

- **tools** for the AI: the chat, and AI apps through `meshgrove-mcp`
- **panels**: tabs in Meshgrove's window, drawn from a widget tree the plugin sends
- **jobs**: background work a panel starts
- **file handlers**: "Open with <plugin>" for the plugin's file types, one file or a whole
  selection

Plugins never write into the library directly. They hand over a **bundle**: files plus tags,
notes and print lists, which Meshgrove applies as one change the user can undo. A plugin may
read the library only when the user allows it, or the very files the user opened with it, and
always by file ids, never by paths.

The interface is the crate [`meshgrove-plugin-api`](../crates/meshgrove-plugin-api). It is MIT
OR Apache-2.0: **plugins may use any licence**, open or closed (Meshgrove itself is GPL-3.0).
[`examples/plugins/hello-box`](../examples/plugins/hello-box) is a complete example: a tool, a
panel with a form and a 3D view, a job, two file handlers (one for bytes, one for library ids)
and bundles: into a folder of their own, beside a library file, and into a folder the user
chose.

What is marked **from 0.9.0** needs Meshgrove 0.9.0 (plugin API 0.2) or newer, what is marked
**from 0.11.0** Meshgrove 0.11.0 (plugin API 0.3); see [API versions](#api-versions) for what an
older Meshgrove does with it.

## Contents

- [Building a plugin](#building-a-plugin)
- [The manifest](#the-manifest)
- [Running in the sandbox](#running-in-the-sandbox)
- [Talking to Meshgrove](#talking-to-meshgrove)
- [Tools for the AI](#tools-for-the-ai)
- [Bundles: files for the library](#bundles-files-for-the-library)
- [Panels](#panels)
- [Widgets and when they send events](#widgets-and-when-they-send-events)
- [Painting on a 3D view](#painting-on-a-3d-view)
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
| `api` | the API version it was built for: `meshgrove_plugin_api::API_VERSION` (see [API versions](#api-versions)) |
| `permissions` | what it asks for (below); the user grants each one |
| `panels` | `{id, title}` of each panel it can open |
| `file_types` | `{pattern, description}`, e.g. `*.gridfinity.json`; `*` alone is refused. From 0.9.0 also `label`, `receive` and `panel`, see [Opening files](#opening-files) |
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

Two things a plugin reads without `library_read` (from 0.9.0): the library files the user
opened with one of its panels ([Opening files](#opening-files)), and model files in its own
folders. See [Talking to Meshgrove](#talking-to-meshgrove).

## Running in the sandbox

- **Folders:**
  - `/data` is the plugin's own folder. It is writable for panels and jobs, read-only for tool calls.
  - `/out` is a staging folder for the files of a bundle. A panel and the jobs it starts share one.
  - `/tmp` is fresh for each instance (a panel keeps its own; picked files land there, or in
    `/data/picked` when the plugin asks for that).
- **No network, no environment variables.** Stderr goes to Meshgrove's log (for panics and debug output).
- **Instances:** every tool call and every job gets a fresh instance. An open panel keeps its
  instance on a thread of its own as long as its tab is open.
- **Deadlines:**
  - A panel event, including render, mesh and snapshot, gets **2 s**.
  - A tool call gets **60 s**. For more, a tool hands its work to a job (from 0.9.0, see
    [Work that takes longer](#work-that-takes-longer-tool-jobs)).
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
  From 0.9.0 it also says `imperial` (the user reads lengths in inches; lengths still cross
  the interface in millimetres) and each printer's `nozzle` diameter in mm, where one is set.

**`library`:**

- `search(query, limit)`: a page of matches for a search as typed in Meshgrove, for example
  `rose tag:Garten ext:stl`, as JSON with ids, names, sizes and tags. Needs `library_read`.
- `describe(reference)`: facts of one file as JSON (size in mm, tags, note, lists, printability).
- `triangles(reference, max_triangles)`: the model's triangles in millimetres (STL, 3MF, OBJ,
  PLY, glTF, STEP, also inside archives). It is simplified to the budget by vertex clustering:
  outlines and proportions stay, small details may go. A model within the budget comes whole,
  equal corners shared. The budget is at most 8 million triangles (2 million before 0.9.0).

A **reference** is one of these (the last three from 0.9.0; before, only library ids):

| Reference | Needs | Reads |
|---|---|---|
| a library id (`Garten/Rankhilfe/rose.stl`) | `library_read`, as far as it reaches | the file in the library |
| the id of a file opened with the panel | nothing: the user opened it | that file, as the user sees it |
| a path below `/tmp` or `/data` (`/data/picked/lid.stl`) | nothing: the plugin could read it itself | the plugin's own file, through Meshgrove's loaders |
| a part token (`<reference>\0part=<n>`, from `describe`) | what its reference needs | one object of a 3MF |

**What the grant shows** depends on who is calling:

- **"What local AI sees":** a panel or a job reads as the user: the libraries opened to local
  AI, from 0.9.0 also while AI is switched off (no AI is reading there). A tool call reads as
  the AI that called it: a cloud AI does not see local-only libraries, and nothing at all
  while AI is switched off.
- **"Everything":** every library.

**Files opened with a panel** (from 0.9.0) are the ids of `Event::ModelsOpened`. The panel
and the jobs it starts read exactly those, with or without `library_read`, also after a
restart; Meshgrove keeps the newest 512 for each panel. A tool call gets none of this. Their
facts come without what they say about other files (copies, G-code beside them, the folder's
readme). `select_model` shows them too.

**The plugin's own files** (from 0.9.0): a path must be plain. One with `..`, or one that
leads through a link, is refused, and nothing outside `/tmp` and `/data` is read (not `/out`).
`describe` of such a file gives its name, kind, size in bytes and mm, and triangles. Files a
model refers to (glTF buffers) are read from beside it by the same rules.

**The parts of a 3MF** (from 0.9.0): `describe` of a 3MF lists the objects it is put together
from, as `parts`:

```json
{"token": "Eigene/dose.3mf\u0000part=1", "name": "Deckel", "triangles": 4200,
 "min": [0, 0, 0], "max": [40, 30, 2],
 "transform": [1, 0, 0,  0, 1, 0,  0, 0, 1,  50, 0, 0], "instances": 2}
```

- A part is an object with triangles of its own; the same object placed several times is one
  part (`instances`). Of a slicer project with several plates, the parts are those of the
  first plate. Supports, modifiers and negative volumes are no parts.
- `triangles(token, …)` gives that object alone, **in its own coordinates** and millimetres.
  `min` and `max` are its box there.
- `transform` says where it stands (its first placement): twelve numbers as in 3MF, the three
  axes and then the shift, so a point `p` of the part is at `x·p.x + y·p.y + z·p.z + shift`.
- Treat the token as opaque: pass it back to `triangles`, or to a bundle's `derive_3mf`.

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
  Or beside a library file, or in a folder the user chose; see below.
- **Limits:** at most 2000 files and 2 GiB; 8 tags per file; 8 lists; quantities 1–999.
- **One change:** the copy, its indexing (the files count as seen, not new in the inbox), tags,
  notes and lists form a single change in Meshgrove's history. Undo removes the files (but keeps
  the ones the user edited since) and the lists. Redo copies again from the staging folder,
  which Meshgrove keeps while anything refers to it.

**Beside a library file (from 0.9.0).** For files made from a library file (a textured copy of
a model), name that file instead of a folder:

```json
{
  "title": "Wood on 2 models",
  "next_to": "Garten/Rankhilfe/rose.stl",
  "files": [
    {"path": "rose – wood.3mf", "note": "wood, 0.4 mm deep"},
    {"path": "topf – wood.3mf", "next_to": "Garten/Töpfe/topf.stl"}
  ],
  "tags": ["Textured"]
}
```

- Each file lands **loose** in the folder of its source, under its own name; a folder it had
  below `/out` is dropped. A taken name gets a number (`rose – wood (2).3mf`). The source is
  never touched.
- `next_to` on the bundle is the source of every file; a file's own `next_to` wins. So one
  bundle serves a whole batch and is one step in the history. Every file needs a source.
- The sources are library ids the plugin may read: opened with the panel, or within
  `library_read`. For a tool call, what the AI that called it may see. A source inside an
  archive puts the file beside the archive.
- All sources of one bundle are in one library, and `next_to` does not go with `folder`. Both
  are refused with a message.
- Undo removes exactly these files (not ones the user changed since). The files around them
  are not touched: Meshgrove indexes only the new ones.
- The review card says "beside <file>".

**Into a folder the user chose (from 0.11.0).** A panel can let the user say where its files
go, in any library, with Meshgrove's own folder chooser. The plugin learns neither paths nor
how the libraries are laid out, and needs no `library_read` for it:

1. The panel answers an event with `{"type": "pick_folder", "id": "dest", "title": "Where should the drawer go?"}`.
2. The user chooses a library and a folder in it, maybe naming a new folder below. The panel
   gets `{"type": "folder_picked", "id": "dest", "target": "3f9a0c11d2e4b567", "label": "Werkstatt/Boxen"}`.
   When the user cancels, it gets nothing.
3. Its bundles name the token:

```json
{"title": "Drawer 160 × 160", "target": "3f9a0c11d2e4b567", "folder": "Drawer 160",
 "files": [{"path": "front.stl"}, {"path": "back.stl"}]}
```

- With `folder`, the files land in that new folder below the chosen one, as a whole (a taken
  name gets " (2)"). Without it, they land **loose** in the chosen folder, each under its own
  name with a number where that is taken; a folder of their own below `/out` is dropped.
- `label` is for showing the user (library name, then the folders). Keep `target` in the
  panel's snapshot: the token stays good for this panel after a restart. Meshgrove keeps the
  64 folders chosen last for each panel; an older token is refused with a message.
- The chosen folder need not exist yet: it is made when the files land, and the undo takes it
  away again with them.
- `target` does not go with `next_to`. Only a panel's bundles can have one (`apply_bundle`,
  `propose_bundle`): a tool's bundle with `target` is refused, since nobody chose a folder for it.
- A library that is no longer one of Meshgrove's, or one on a read-only drive, is no place: the
  bundle fails with a message, or the chooser does not offer it.
- The review card names the library and folder ("3 files to Werkstatt/Boxen/Drawer 160").

**A 3MF with some objects swapped (from 0.9.0).** When the source is a slicer project with
several objects, plates and settings, a plugin that changed one object should not hand back a
bare mesh. It writes only the new mesh and lets Meshgrove make the file:

```json
{
  "title": "Lid with grooves",
  "next_to": "Eigene/dose.3mf",
  "files": [{
    "path": "dose – grooves.3mf",
    "derive_3mf": {
      "source": "Eigene/dose.3mf",
      "replace": [{"part": "Eigene/dose.3mf\u0000part=1", "mesh": "parts/lid.stl"}]
    }
  }]
}
```

- `source` is the 3MF: a library id or a path below `/tmp` or `/data`, as `describe` takes it.
- `replace` names parts by their tokens (from `describe` of that same source) and, for each, a
  mesh the plugin wrote below `/out` (STL, 3MF, OBJ or PLY): in millimetres and **in the
  part's own coordinates**, as `triangles(token)` gave it. Corners at the same place become
  one vertex, so an STL comes out closed.
- Meshgrove copies the 3MF to `path` and swaps only those objects' meshes. Everything else
  stays as the slicer saved it: the other objects, where each one stands, plates, profiles and
  settings (Bambu Studio, OrcaSlicer, Anycubic Slicer Next and PrusaSlicer projects included).
- What the slicer kept for each triangle of a swapped object goes with the old triangles:
  painted colours, supports, seams. The pictures of the plates are emptied, since they show
  the model as it was.
- The mesh files are used up and do not reach the library. The copy is a file of the bundle
  like any other: tags, notes, lists, `next_to`.
- An object PrusaSlicer cuts into several volumes within one mesh cannot be swapped.

### Work that takes longer: tool jobs

From 0.9.0 a tool whose work does not fit its 60 s hands it to a job. It answers at once with
a bundle that names the job instead of files:

```json
{"title": "Wood on rose.stl", "job": {"kind": "apply", "params": {"source": "Garten/rose.stl", "texture": "wood"}}}
```

- The AI gets the tool's `result` and `{"status": "started", "job": "<id>", "arrives": "review"}`
  right away; it does not wait.
- Meshgrove runs `run_job(kind, params)` in the background, one tool job at a time, writing
  into the bundle's staging folder (`/out`). The job runs **as the tool did**: it reads the
  library as the AI that called the tool may (a cloud AI's job does not see a local-only
  library), `/data` is read-only, it has no deadline, and the user can cancel it. Report
  `host::progress`; Meshgrove shows it in the status bar with a cancel button.
- The job's answer may carry the final bundle as its `bundle` member (files, folder or
  `next_to`, `derive_3mf`, tags, lists); without one, the tool's bundle is taken without its
  `job`. It is checked like any bundle; a job's bundle cannot start another job.
- What it made **always waits in the review**, also when the user lets the plugin's files be
  accepted by themselves and in the chat's automatic mode: the user did not watch it being made.
  A toast says so ("… waits in the review").
- A job that fails or is cancelled proposes nothing, and its files are gone.
- `meshgrove-mcp` runs these jobs itself; when the AI app closes the connection it waits up to
  15 minutes for running jobs before it exits.
- `meshgrove-plugin-run … call` waits for the job and prints the bundle it ended with.

hello-box's `make_box_later` is an example: its job writes the box and returns its bundle.
A Meshgrove before 0.9.0 refuses a bundle with `job`.

**How a bundle is applied:**

| Coming from | What happens |
|---|---|
| A tool, chat in review mode | waits in the review as a card (files, folder, size, lists) |
| A tool, chat in automatic mode | applied at once, unless the plugin lacks "Accept without review" |
| A tool, AI app over MCP | waits in the review; accepted by itself only when the AI's auto-accept is on **and** the plugin has "Accept without review" |
| A tool's job (`job`, from 0.9.0) | always waits in the review, whatever is let be accepted by itself |
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
  whenever the model changes; the camera stays where the user put it. Meshgrove prepares and
  uploads the triangles on the panel's own thread, so even a preview of millions of triangles
  does not hold up the window (from 0.9.0). From 0.9.0 a 3D view can also be painted on and
  coloured, see [Painting on a 3D view](#painting-on-a-3d-view).
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
| `label` (style: body, heading, strong, small, weak, mono), `markdown`, `separator`, `notice` (info, success, warn, error), `progress` (`fraction`; none = busy) | — |
| `image` (base64 PNG/JPEG, `width`: at most that wide, `alt`: shown on hover) | — ; with an `id` (from 0.9.0) `Click`, like a button. `selected` draws a frame round the one that is chosen: a gallery to pick from. Older versions show the picture and ignore clicks |
| `button` (primary, enabled) | `Click` |
| `text_input` | `Changed {value: text}` on Enter or when the field loses focus, not per keystroke |
| `number_input` (min, max, step, decimals, unit) | `Changed {value: number}` once a typed number is done (Enter, Tab or leaving the field; Escape drops it), or when a drag is let go. With `unit: mm` it shows the user's unit (inches when imperial) but **always sends millimetres** |
| `slider` | like `number_input` |
| `checkbox`, `select`, `radio`, `color` (`#rrggbb[aa]`) | `Changed` at once |
| `form` (rows of label + node; optional `submit` button) | the rows' own events; `Submit {values}` with every input by id |
| `table` (columns: title, width, numeric; rows: id + cells; selected; max_height) | `TableSelect {row}`; clicking the selected row again sends `row: null`. Columns without a `width` can be dragged wider by the user; a cell cut short shows its whole text on hover |
| `mesh_view` (key, revision, height, `bed: [x, y]`, `frame: [min, max]`, `paint`, `overlay`) | — the bed is drawn under the model, centred; the frame is the space the model is meant to fit (a drawer, a shelf), drawn in blue round it and kept in the picture; the view's *Frame* button hides it (from 0.9.0; older versions ignore it). With `paint` (from 0.9.0): `MeshStroke` and `MeshPick`, see [Painting on a 3D view](#painting-on-a-3d-view) |
| `grid_editor` | `Grid {change}`, see below |
| `canvas` | `Canvas {change}` when `interactive`, see below |
| anything newer | shown as a placeholder |

**Signs in labels.** Meshgrove draws text with the fonts it brings (Ubuntu, Noto Emoji, egui's
icon font and Hack), plus the system's UI font. A sign none of them has shows as an empty box on
the systems that lack it: Windows' Segoe UI has no ✧ (U+2727), ✓ (U+2713), ✕ (U+2715) or
＋ (U+FF0B), for example. Signs that show everywhere include ✔ ✖ ⚠ ★ ☆ ✨ ⚙ ← ↑ → ↓ ▲ ▼ ▾ ↺ ↻ •
and Noto Emoji's pictures (📁 📏 🔍 🔒 …); plain `+` and `×` are safe as well.

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

## Painting on a 3D view

From 0.9.0 a `mesh_view` can let the user paint on the model or pick points on it (`paint`),
and show colours over it (`overlay`). Meshgrove 0.8 ignores both and shows the model as usual.

```json
{"type": "mesh_view", "id": "preview", "key": "model", "revision": 4,
 "paint": {"tool": "brush", "radius_mm": 3.0, "tint": "#ff8000c0"},
 "overlay": {"key": "mask", "revision": 9, "palette": ["#00000000", "#ff8000", "#3ba3ff80"]}}
```

**`paint`:** `tool`, `radius_mm` (default 5), `tint` and `erase_tint` (`#rrggbb[aa]`).

| `tool` | The left button | Sends |
|---|---|---|
| `brush` (default) | paints dabs: one where the pointer has moved a quarter of the radius on the surface; a circle of the radius shows under the pointer | `MeshStroke`, one hit per dab (its centre); take the triangles within `radius_mm` yourself |
| `triangles` | paints the triangles it passes, each once per stroke | `MeshStroke`, one hit per triangle |
| `drag` | follows the pointer on the surface, nothing tinted | `MeshStroke`, a hit every few pixels |
| `click` | a click (a press that does not travel) | `MeshPick {hit, shift}`; `hit` is null beside the model |

The rules, so a stroke never does what the user did not mean:

- While `paint` is set, the **right** button turns the model, the **middle** one moves it, the
  wheel zooms. Measuring is off.
- What a press starts it finishes: another button, Shift or leaving the view change nothing
  until the button is let go. **Shift held at the press erases** (`erase: true`) for the whole
  stroke. A press outside the view starts nothing. A double click does not reset the camera.
- What is painted is tinted at once with `tint` (or `erase_tint`; default: a light orange and
  the model's grey) until your next overlay arrives. The click and drag tools tint nothing.

**Events:**

```json
{"type": "mesh_stroke", "id": "preview", "stroke": 3, "phase": "move", "erase": false,
 "hits": [{"triangle": 1234, "point": [12.5, 3.0, 40.1], "normal": [0.0, -1.0, 0.0]}]}
{"type": "mesh_pick", "id": "preview", "hit": {"triangle": 7, "point": [0, 0, 20], "normal": [0, 0, 1]}, "shift": true}
```

- `phase` is `start` (at the press), `move`, then `end` (let go); each brings the hits added
  since, the end too. `stroke` numbers the strokes of the view.
- A stroke goes out at most every 50 ms, and **nothing goes out while the panel is busy** with
  an event: the hits wait and come together. So keep `event` quick; a stroke still comes whole.
- `triangle` is the index in the triangles you gave (`indices[3 * triangle ..]`), `point` and
  `normal` are in your coordinates; the normal faces the camera.

**`overlay`:** when its `revision` changes (and whenever the mesh changes), Meshgrove asks
`mesh(overlay.key)`. Answer with `positions` empty and `indices` holding **one class per
triangle** of the view's mesh, in its order. Class `k` is drawn in `palette[k]` over the
model's grey: a transparent entry (or none for that class) shows the grey, a half transparent
one is mixed with it. An overlay whose length is not the mesh's triangle count (made for a
mesh that changed since) is left out, and the panel goes on.

hello-box shows the click tool and an overlay: with "Tint faces" ticked, a click tints the face
of the box it hits.

## Effects

| Effect | Meshgrove |
|---|---|
| `toast {text, level}` | shows the text in the status bar, prefixed with the plugin's name |
| `start_jobs {jobs: [{id, kind, params, label}]}` | runs them in parallel (see Jobs) |
| `cancel_job {job}` | cancels it |
| `apply_bundle {bundle}` | applies what the panel and its jobs wrote to `/out` (needs `bundles`) |
| `propose_bundle {bundle}` | the same, into the review |
| `save_file {path, name}` | offers a file below `/out` or `/tmp` for saving, with the system's file dialog (before 0.9.0: a field to type the path into) |
| `open_file_dialog {id, title, extensions, multiple, into}` | asks for a file with the system's file dialog, copies it into the panel's `/tmp` and sends `FilePicked {id, name, path}` (`path`: `/tmp/<name>`). From 0.9.0: `multiple: true` lets the user pick several, one `FilePicked` each; `into: "data"` copies to `/data/picked/<name>` instead, where the file stays and the panel's jobs can read it. A file of that name with other content is kept there and the new one gets a number, so use the `path` of the event |
| `pick_folder {id, title}` (from 0.11.0) | lets the user choose a folder in a library with Meshgrove's own chooser and sends `FolderPicked {id, target, label}`; nothing when they cancel. See [Into a folder the user chose](#bundles-files-for-the-library) |
| `open_panel {panel, state}` | opens (or, with a state, re-opens) a panel |
| `select_model {id}` | shows that library file in the gallery, if the plugin may read it |
| `repaint_after {ms}` | renders the panel again after that long |

An effect Meshgrove does not know (from a newer API) is skipped and the others are done (from
0.9.0; an older Meshgrove drops the whole answer, so do not send it effects it cannot know:
`host.info().api` says what it speaks).

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
"Open with <plugin>", and the Info pane shows the same as a button below "Open in <slicer>".
Meshgrove calls `open_file(name, bytes)` (up to 64 MiB), and the plugin
answers with effects, usually `open_panel` with the file's content as state.

**By id (from 0.9.0).** A plugin that works on models wants neither 64 MiB of bytes nor one
file at a time. A file type may say so:

```json
{"pattern": "*.stl", "description": "A model to texture",
 "label": "Apply a texture …", "receive": "ids", "panel": "texture"}
```

| Field | Meaning |
|---|---|
| `label` | the entry in the context menu and the button in the Info pane; without one it is "Open with <plugin>" |
| `receive` | `"bytes"` (the default): `open_file`, one file. `"ids"`: the files' library ids, to a panel |
| `panel` | with `"ids"`: the panel that takes them, one of the manifest's (required) |

- The entry shows for **one file or a whole selection**: every selected file has to match a
  file type of the plugin that receives ids in the same panel. Files outside the libraries
  have no ids and get no entry; files inside archives do.
- Meshgrove opens the panel (or brings it to the front) and sends it
  `Event::ModelsOpened {ids}`. `open_file` is not called.
- From then on the panel and its jobs may read exactly these files through `library`, whether
  or not the user granted `library_read` (see [Talking to Meshgrove](#talking-to-meshgrove)),
  and may put a bundle's files beside them (`next_to`).
- For several patterns, list several file types with the same `panel`.

## Testing with meshgrove-plugin-run

`meshgrove-plugin-run` runs a plugin without Meshgrove. Build it with
`cargo build -p meshgrove-plugins --bin meshgrove-plugin-run`, then:

```sh
meshgrove-plugin-run my_plugin.wasm manifest            # the manifest, checked
meshgrove-plugin-run my_plugin.wasm tools               # the tool list, checked
meshgrove-plugin-run my_plugin.wasm call make_box '{"width": 30}' --out out/
meshgrove-plugin-run my_plugin.wasm job box '{}' --out out/
meshgrove-plugin-run my_plugin.wasm open drawer.gridfinity.json
meshgrove-plugin-run my_plugin.wasm mcp --out out/      # an MCP server with the plugin's tools
meshgrove-plugin-run my_plugin.wasm call measure '{"id": "models/Garten/rose.stl"}' --library ~/models
```

Options:

- `--library DIR` (from 0.9.0): the plugin reads that folder as a library, all of it, without
  Meshgrove's index or settings. Ids start with the folder's name (`models/Garten/rose.stl`);
  `search` matches words against them (and `ext:stl`). A bundle with `next_to` is printed
  with the places its files would land (`would_land`); nothing is copied there.
- `--data DIR` (from 0.9.0): the folder the plugin sees as `/data`, in place of its own in
  Meshgrove's plugins folder. Put test files there to read them by path.
- `--memory MB`, `--no-cache`.

Without `--library` the plugin has no library. The `mcp` mode can be added to an AI app like
any MCP server, to try the tools from there. Meshgrove's own tests run `examples/plugins/hello-box`
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

The interface has two versions, in `meshgrove_plugin_api`:

| | Is | Now | Changes |
|---|---|---|---|
| `WIT_VERSION` | the WIT package `meshgrove:plugin@…`: the functions a plugin imports and exports, which it is **linked** against | `0.1.0` | not within 0.x |
| `API_VERSION` | the JSON shapes: manifest, widgets, events, effects, bundles, and what the host functions take | `0.3.0` (Meshgrove 0.11.0) | with new fields and variants |

**The rule: within 0.x nothing is added to the WIT world**, no import and no export. A
component that imports a function the host lacks does not load at all, so a new function
would stop every installed plugin on an older Meshgrove, and a changed package version would
stop every older plugin on a new one. Everything new is JSON: a field with a default, a new
variant of an enum, a new kind of reference a function takes. That is why a plugin built for
API 0.1 (Meshgrove 0.8) runs unchanged on 0.9.0.

| | Plugin API | Meshgrove |
|---|---|---|
| 0.1 | the first | 0.8.0 – 0.8.3 |
| 0.2 | everything marked "from 0.9.0" in this document | 0.9.0 – 0.10.0 |
| 0.3 | `pick_folder`, `FolderPicked`, `Bundle.target` ("from 0.11.0") | 0.11.0 |

**What Meshgrove checks.** The plugin's manifest names the API it was built for (`api`). It
runs when that is **the same major version and a minor version no newer than Meshgrove's**:

- A Meshgrove with API 0.3 runs plugins built for 0.1 and 0.2.
- A plugin built for a newer minor version is refused with "built for plugin API 0.2.0, newer
  than this Meshgrove's 0.1.0: update Meshgrove". A plugin that uses anything from 0.9.0 should
  say `api: "0.2.0"`, which `API_VERSION` of the crate it was built with does by itself.
- A plugin that should still run on an older Meshgrove may name the older API in its manifest
  (`api: "0.2.0"`) and use what is newer only where the host has it: `host.info().api` says
  which API Meshgrove speaks. Send `pick_folder` only when that is 0.3 or newer; a Meshgrove
  0.9 or 0.10 would skip the effect and ignore a bundle's `target` (the files would land in
  the import library).
- One built for another major version is refused, with a message saying which Meshgrove it
  needs.

**What each side does with what it does not know:**

| A newer … | meets an older … | which |
|---|---|---|
| widget | Meshgrove | shows a placeholder, the rest of the panel works |
| field of a widget, an effect or a bundle | Meshgrove | ignores it |
| effect | Meshgrove 0.9.0 or newer | skips it and does the others (0.8: drops the whole answer) |
| event | plugin | reads it as `Event::Unknown` (with `meshgrove-plugin-api` 0.1: fails to parse it) |
| field of `HostInfo` or of an event | plugin | ignores it |

So a newer Meshgrove can send a kind of event your plugin has never heard of. Answer with no
effects, as hello-box does, and never `unwrap()` the parse: a panic ends the panel.

```rust
let Ok(event) = serde_json::from_str::<Event>(&event) else { return guest::effects(&[]) };
```
