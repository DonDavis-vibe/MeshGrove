//! Hello Box, an example Meshgrove plugin: a box of the size you give.
//!
//! - Tool `make_box` for the AI: writes the box as STL below `/out` and hands it over as a
//!   bundle (folder "Hello Box/<name>", tag "Hello Box", print list "Hello Box").
//! - Tool `measure`: the size of a library model, through `library.triangles` (permission
//!   `library_read`).
//! - Panel "box": a form, a 3D view of the box and two ways to add it to the library: at once,
//!   or through a background job.
//! - Opens `*.box.json` files (a saved box) in its panel: it gets the file's bytes.
//! - Opens `*.stl` files with "Box around this …": it gets their library ids in its panel
//!   (`Event::ModelsOpened`), measures the first one and sizes the box to hold it. Those files it
//!   may read without the `library_read` permission, because the user opened them with it.
//!
//! With the feature `test-hooks` it has two more tools for Meshgrove's tests: `spin` never ends,
//! `trap` eats all memory. Its panel then also answers `HostChanged` with a toast naming the
//! language, and a click on "future" with an effect no Meshgrove knows.

use std::cell::RefCell;

use meshgrove_plugin_api::guest::{self, Guest, GuestPanel, PluginError, ToolOutput, library};
use meshgrove_plugin_api::ui::{Effect, Event, FormRow, JobSpec, Level, Node, TextStyle, Unit, View};
use meshgrove_plugin_api::{
    API_VERSION, Bundle, BundleFile, BundleItem, BundleList, FileType, ListKind, Manifest, MeshData, PanelDef,
    Permission, Receive, ToolDef,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

struct HelloBox;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct BoxSpec {
    width: f32,
    depth: f32,
    height: f32,
    name: String,
    /// How many go on the print list.
    copies: u32,
}

impl Default for BoxSpec {
    fn default() -> BoxSpec {
        BoxSpec { width: 20.0, depth: 30.0, height: 40.0, name: "box".into(), copies: 1 }
    }
}

impl BoxSpec {
    fn check(&self) -> Result<(), PluginError> {
        if [self.width, self.depth, self.height].iter().any(|v| !(1.0..=500.0).contains(v)) {
            return Err(PluginError::Invalid("width, depth and height must be between 1 and 500 mm".into()));
        }
        let name_ok = |c: char| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-');
        if self.name.trim().is_empty() || self.name.len() > 40 || !self.name.chars().all(name_ok) {
            return Err(PluginError::Invalid("the name: 1 to 40 letters, digits, spaces, '_' or '-'".into()));
        }
        if !(1..=99).contains(&self.copies) {
            return Err(PluginError::Invalid("copies must be between 1 and 99".into()));
        }
        Ok(())
    }

    fn file_name(&self) -> String {
        format!("{}_{}x{}x{}.stl", self.name.trim(), self.width, self.depth, self.height)
    }

    fn mesh(&self) -> MeshData {
        MeshData::cuboid([self.width, self.depth, self.height])
    }

    /// Writes the STL below `/out`; its path there.
    fn write(&self) -> Result<String, PluginError> {
        self.check()?;
        let rel = self.file_name();
        guest::write_stl(&format!("/out/{rel}"), &self.mesh(), &self.name)?;
        Ok(rel)
    }

    fn bundle(&self, rel: &str) -> Bundle {
        let size = format!("{} × {} × {} mm", self.width, self.depth, self.height);
        Bundle {
            title: format!("Box {size}"),
            folder: Some(format!("Hello Box/{}", self.name.trim())),
            files: vec![BundleFile {
                path: rel.into(),
                tags: Vec::new(),
                note: Some(format!("{size}, made by Hello Box")),
            }],
            tags: vec!["Hello Box".into()],
            lists: vec![BundleList {
                name: "Hello Box".into(),
                kind: ListKind::Print,
                items: vec![BundleItem { path: rel.into(), quantity: self.copies }],
            }],
            ..Bundle::default()
        }
    }
}

/// The size of library model `id` in mm (x, y, z) and its triangles.
fn measure(id: &str) -> Result<([f32; 3], usize), PluginError> {
    let mesh: MeshData = library::triangles(id, 50_000).map_err(PluginError::Denied)?.into();
    let (lo, hi) = mesh.bounds().ok_or_else(|| PluginError::Failed("the model has no triangles".into()))?;
    Ok(([hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]], mesh.triangles()))
}

fn message(e: &PluginError) -> String {
    match e {
        PluginError::Invalid(m) | PluginError::Failed(m) | PluginError::Denied(m) => m.clone(),
        PluginError::Cancelled => "cancelled".into(),
    }
}

fn box_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "width": {"type": "number", "description": "mm, 1 to 500", "default": 20},
            "depth": {"type": "number", "description": "mm, 1 to 500", "default": 30},
            "height": {"type": "number", "description": "mm, 1 to 500", "default": 40},
            "name": {"type": "string", "description": "file and folder name", "default": "box"},
            "copies": {"type": "integer", "description": "how many go on the print list", "default": 1}
        }
    })
}

impl Guest for HelloBox {
    type Panel = BoxPanel;

    fn manifest() -> String {
        guest::json(&Manifest {
            id: "hello-box".into(),
            name: "Hello Box".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            api: API_VERSION.into(),
            description: "Makes a box of the size you give; an example of a Meshgrove plugin.".into(),
            license: Some("MIT OR Apache-2.0".into()),
            permissions: vec![Permission::AiTools, Permission::Bundles, Permission::LibraryRead],
            panels: vec![PanelDef { id: "box".into(), title: "Hello Box".into() }],
            file_types: vec![
                FileType {
                    pattern: "*.box.json".into(),
                    description: "A saved box".into(),
                    ..FileType::default()
                },
                // Library ids instead of bytes: one model or a whole selection, opened in the panel.
                FileType {
                    pattern: "*.stl".into(),
                    description: "A model to make a box for".into(),
                    label: Some("Box around this …".into()),
                    receive: Receive::Ids,
                    panel: Some("box".into()),
                },
            ],
            memory_mb: Some(64),
            ..Manifest::default()
        })
    }

    fn list_tools() -> String {
        #[allow(unused_mut)]
        let mut tools = vec![
            ToolDef {
                name: "make_box".into(),
                description: "Makes a box (an STL file) and puts it into the library, on the print list \"Hello Box\"."
                    .into(),
                input_schema: box_schema(),
                bundles: true,
            },
            ToolDef {
                name: "measure".into(),
                description: "The size of a model in the library, in mm.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {"id": {"type": "string", "description": "a file id from search_models"}},
                    "required": ["id"]
                }),
                bundles: false,
            },
        ];
        #[cfg(feature = "test-hooks")]
        for name in ["spin", "trap"] {
            let description = format!("Test hook: {name}");
            tools.push(ToolDef {
                name: name.into(),
                description,
                input_schema: json!({"type": "object"}),
                bundles: false,
            });
        }
        guest::json(&tools)
    }

    fn call_tool(name: String, args: String) -> Result<ToolOutput, PluginError> {
        match name.as_str() {
            "make_box" => {
                let spec: BoxSpec = guest::parse(&args)?;
                guest::host::progress(0.5, "writing the STL");
                let rel = spec.write()?;
                let result = json!({"file": rel, "triangles": spec.mesh().triangles(), "copies": spec.copies});
                Ok(guest::tool_output(&result, Some(&spec.bundle(&rel))))
            }
            "measure" => {
                #[derive(Deserialize)]
                struct Args {
                    id: String,
                }
                let args: Args = guest::parse(&args)?;
                let (size, triangles) = measure(&args.id)?;
                Ok(guest::tool_output(&json!({"size_mm": size, "triangles": triangles}), None))
            }
            #[cfg(feature = "test-hooks")]
            "spin" => {
                let mut n = 0u64;
                loop {
                    n = std::hint::black_box(n.wrapping_add(1));
                }
            }
            #[cfg(feature = "test-hooks")]
            "trap" => {
                let mut hog: Vec<Vec<u8>> = Vec::new();
                loop {
                    hog.push(vec![1u8; 16 << 20]);
                    std::hint::black_box(&hog);
                }
            }
            _ => Err(PluginError::Invalid(format!("no tool {name:?}"))),
        }
    }

    fn run_job(kind: String, params: String) -> Result<String, PluginError> {
        match kind.as_str() {
            "box" => {
                let spec: BoxSpec = guest::parse(&params)?;
                let rel = spec.write()?;
                guest::host::progress(1.0, "done");
                Ok(guest::json(&json!({"file": rel, "spec": spec})))
            }
            // A job reads the files the user opened with its panel, as the panel does.
            #[cfg(feature = "test-hooks")]
            "measure" => {
                let id = serde_json::from_str::<serde_json::Value>(&params)?["id"].as_str().unwrap_or_default().to_owned();
                let (size, _) = measure(&id)?;
                Ok(guest::json(&json!({"size_mm": size})))
            }
            _ => Err(PluginError::Invalid(format!("no job {kind:?}"))),
        }
    }

    fn open_file(_name: String, bytes: Vec<u8>) -> Result<String, PluginError> {
        let spec: BoxSpec = serde_json::from_slice(&bytes)?;
        let state = PanelState { spec, ..PanelState::default() };
        Ok(guest::effects(&[Effect::OpenPanel { panel: "box".into(), state: Some(guest::json(&state)) }]))
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct PanelState {
    spec: BoxSpec,
    /// Grows with every change, so the 3D view asks for the box anew.
    revision: u64,
    busy: bool,
    /// The library model the box was sized for ("Box around this …"), by its id.
    source: Option<String>,
}

struct BoxPanel {
    state: RefCell<PanelState>,
}

impl GuestPanel for BoxPanel {
    fn new(_id: String, state: Option<String>) -> BoxPanel {
        let state = state.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        BoxPanel { state: RefCell::new(state) }
    }

    fn render(&self) -> String {
        let s = self.state.borrow();
        let row = |label: &str, node: Node| FormRow { label: label.into(), node };
        let copies = Node::NumberInput {
            id: "copies".into(),
            value: f64::from(s.spec.copies),
            min: Some(1.0),
            max: Some(99.0),
            step: Some(1.0),
            unit: Unit::None,
            decimals: Some(0),
            enabled: true,
        };
        let mut children = vec![Node::heading("Hello Box")];
        if let Some(source) = &s.source {
            children.push(Node::Label { text: format!("Sized to hold {source}"), style: TextStyle::Weak });
        }
        children.extend([
            Node::Form {
                id: "size".into(),
                rows: vec![
                    row("Width", Node::mm("width", f64::from(s.spec.width), 1.0, 500.0)),
                    row("Depth", Node::mm("depth", f64::from(s.spec.depth), 1.0, 500.0)),
                    row("Height", Node::mm("height", f64::from(s.spec.height), 1.0, 500.0)),
                    row("Name", Node::text_input("name", s.spec.name.clone())),
                    row("Copies", copies),
                ],
                submit: None,
            },
            Node::MeshView {
                id: "preview".into(),
                key: "box".into(),
                revision: s.revision,
                height: Some(240.0),
                bed: None,
                frame: None,
            },
            Node::row(vec![
                Node::Button { id: "apply".into(), label: "Add to library".into(), primary: true, enabled: !s.busy },
                Node::Button {
                    id: "job".into(),
                    label: "Make in the background".into(),
                    primary: false,
                    enabled: !s.busy,
                },
            ]),
        ]);
        if s.busy {
            children.push(Node::Progress { fraction: None, text: Some("Making the box …".into()) });
        }
        guest::json(&View::new(Node::column(children)))
    }

    fn event(&self, event: String) -> String {
        let Ok(event) = serde_json::from_str::<Event>(&event) else { return guest::effects(&[]) };
        // An effect no Meshgrove knows yet, then one every Meshgrove knows: the first is skipped.
        #[cfg(feature = "test-hooks")]
        if matches!(&event, Event::Click { id } if id == "future") {
            return r#"[{"type": "hologram", "depth": 3}, {"type": "toast", "text": "still here"}]"#.into();
        }
        let mut s = self.state.borrow_mut();
        let effects = match event {
            Event::Changed { id, value } => {
                let number = value.as_f64().map(|v| v as f32);
                match (id.as_str(), number) {
                    ("width", Some(v)) => s.spec.width = v,
                    ("depth", Some(v)) => s.spec.depth = v,
                    ("height", Some(v)) => s.spec.height = v,
                    ("copies", Some(v)) => s.spec.copies = v.round().clamp(1.0, 99.0) as u32,
                    ("name", _) => s.spec.name = value.as_str().unwrap_or_default().to_owned(),
                    _ => {}
                }
                s.revision += 1;
                Vec::new()
            }
            Event::Click { id } if id == "apply" => match s.spec.write() {
                Ok(rel) => vec![Effect::ApplyBundle { bundle: s.spec.bundle(&rel) }],
                Err(e) => vec![Effect::Toast { text: message(&e), level: Level::Error }],
            },
            Event::Click { id } if id == "job" => match s.spec.check() {
                Ok(()) => {
                    s.busy = true;
                    let params = serde_json::to_value(&s.spec).unwrap_or_default();
                    vec![Effect::StartJobs {
                        jobs: vec![JobSpec { id: "box".into(), kind: "box".into(), params, label: Some("Box".into()) }],
                    }]
                }
                Err(e) => vec![Effect::Toast { text: message(&e), level: Level::Error }],
            },
            Event::JobFinished { result: Some(result), .. } => {
                s.busy = false;
                match result.get("file").and_then(|f| f.as_str()) {
                    Some(rel) => vec![Effect::ApplyBundle { bundle: s.spec.bundle(rel) }],
                    None => Vec::new(),
                }
            }
            Event::JobFinished { error: Some(error), .. } => {
                s.busy = false;
                vec![Effect::Toast { text: error, level: Level::Error }]
            }
            // "Box around this …" in the gallery: the ids of the files the user picked. The box
            // takes the size of the first one, with 2 mm of air around it.
            Event::ModelsOpened { ids } => match ids.first().map(|id| (id, measure(id))) {
                Some((id, Ok((size, _)))) => {
                    let fit = |mm: f32| (mm + 4.0).clamp(1.0, 500.0);
                    (s.spec.width, s.spec.depth, s.spec.height) = (fit(size[0]), fit(size[1]), fit(size[2]));
                    s.source = Some(id.clone());
                    s.revision += 1;
                    match ids.len() {
                        1 => Vec::new(),
                        n => vec![Effect::Toast { text: format!("{n} files: sized for the first"), level: Level::Info }],
                    }
                }
                Some((_, Err(e))) => vec![Effect::Toast { text: message(&e), level: Level::Error }],
                None => Vec::new(),
            },
            #[cfg(feature = "test-hooks")]
            Event::HostChanged { info } => {
                vec![Effect::Toast { text: format!("host: {}", info.language), level: Level::Info }]
            }
            _ => Vec::new(),
        };
        guest::effects(&effects)
    }

    fn mesh(&self, key: String) -> Option<guest::MeshData> {
        (key == "box").then(|| self.state.borrow().spec.mesh().into())
    }

    fn snapshot(&self) -> String {
        guest::json(&*self.state.borrow())
    }
}

meshgrove_plugin_api::export_plugin!(HelloBox);
