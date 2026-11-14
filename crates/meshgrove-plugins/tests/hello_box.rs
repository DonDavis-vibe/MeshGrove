//! The host against the example plugin `examples/plugins/hello-box`, built with its test hooks
//! (`scripts/build-plugins.sh --test-hooks`, or the file named by MESHGROVE_TEST_PLUGIN).
//! Without the file each test passes with a note on stderr, unless MESHGROVE_REQUIRE_PLUGIN=1
//! (as in CI).

mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use meshgrove_core::bundle::{BundleError, BundleItem, BundleList, BundleListKind, Limits};
use meshgrove_core::changes::ApplyOp;
use meshgrove_core::config::{AccessClass, LibraryRead, PluginConfig};
use meshgrove_core::db::Library;
use meshgrove_plugin_api::manifest::Receive;
use meshgrove_plugin_api::ui::{BundleOutcome, Effect, Event, JobSpec, Node};
use meshgrove_plugin_api::{MeshData, Permission};
use meshgrove_plugins::jobs::{JobEvent, JobPool};
use meshgrove_plugins::{
    Caller, CancelToken, Error, LibraryAccess, Located, Opened, Plugin, PluginGrants, Settings, Staging,
};
use serde_json::{Value, json};

fn wasm() -> Option<Vec<u8>> {
    let path = std::env::var_os("MESHGROVE_TEST_PLUGIN").map(PathBuf::from).unwrap_or_else(|| {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/plugins-test/wasm32-wasip2/release/hello_box.wasm")
    });
    match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(e) if std::env::var("MESHGROVE_REQUIRE_PLUGIN").as_deref() == Ok("1") => {
            panic!("{}: {e} (build it with scripts/build-plugins.sh --test-hooks)", path.display())
        }
        Err(_) => {
            eprintln!("skipped: no {} (scripts/build-plugins.sh --test-hooks)", path.display());
            None
        }
    }
}

/// Tools and bundles granted, nothing else.
fn granted(base: &Path) -> Settings {
    Settings {
        grants: PluginGrants { ai_tools: true, bundles: true, ..PluginGrants::default() },
        ..Settings::new(base)
    }
}

fn load(bytes: &[u8], settings: Settings) -> Plugin {
    Plugin::from_bytes(bytes, settings).unwrap()
}

#[test]
fn compiles_once_then_comes_from_the_cache() {
    let Some(bytes) = wasm() else { return };
    let base = tempfile::tempdir().unwrap();
    let first = load(&bytes, granted(base.path()));
    assert!(!first.from_cache());
    let cached: Vec<_> = std::fs::read_dir(base.path().join(".cache")).unwrap().flatten().collect();
    assert_eq!(cached.len(), 1, "one compiled file");
    assert!(cached[0].file_name().to_string_lossy().starts_with(first.hash()));
    let second = load(&bytes, granted(base.path()));
    assert!(second.from_cache(), "the second load takes the compiled code");
    assert_eq!(second.hash(), first.hash());
    assert!(!base.path().join(".load").read_dir().unwrap().any(|_| true), "nothing left of the introduction");

    let m = second.manifest();
    assert_eq!(
        (m.id.as_str(), m.name.as_str(), m.api.as_str()),
        ("hello-box", "Hello Box", meshgrove_plugin_api::API_VERSION)
    );
    assert_eq!(m.permissions, [Permission::AiTools, Permission::Bundles, Permission::LibraryRead]);
    assert_eq!(m.panels[0].id, "box");
    assert!(m.file_types[0].matches("Deckel.box.json"));
    assert_eq!(
        m.opener(&["a.stl", "b.STL"]).map(|t| (t.receive, t.panel.as_deref())),
        Some((Receive::Ids, Some("box")))
    );
    let tools: Vec<&str> = second.tools().iter().map(|t| t.name.as_str()).collect();
    assert_eq!(tools, ["make_box", "make_box_later", "measure", "spin", "trap", "describe"]);
    assert!(second.tools()[0].bundles && second.tools()[0].input_schema["properties"]["width"].is_object());
    assert_eq!(second.memory_mb(), 64, "the manifest's wish");

    let config = |blake3: &str, enabled| PluginConfig {
        id: "hello-box".into(),
        file: "hello_box.wasm".into(),
        blake3: blake3.into(),
        enabled,
        grants: PluginGrants::default(),
        memory_mb: None,
    };
    assert!(second.check_config(&config(second.hash(), true)).is_ok());
    assert!(matches!(second.check_config(&config("0000", true)), Err(Error::Denied(_))), "a changed file waits");
    assert!(matches!(second.check_config(&config(second.hash(), false)), Err(Error::Denied(_))));
}

#[test]
fn other_files_are_no_plugins() {
    let base = tempfile::tempdir().unwrap();
    let err = Plugin::from_bytes(b"\0asm\x01\0\0\0", Settings::new(base.path())).err().unwrap();
    assert!(matches!(err, Error::Load(_)), "{err}");
    let err = Plugin::from_bytes(b"solid box", Settings::new(base.path())).err().unwrap();
    assert!(matches!(err, Error::Load(_)), "{err}");
}

fn quantities(lib: &Library, name: &str) -> Vec<(String, u32)> {
    let list = lib.lists().unwrap().into_iter().find(|l| l.name == name).unwrap();
    lib.list_entries(list.id).unwrap().into_iter().map(|e| (e.entry.name, e.quantity)).collect()
}

#[test]
fn make_box_writes_an_stl_and_a_bundle_that_lands_in_the_library() {
    let Some(bytes) = wasm() else { return };
    let base = tempfile::tempdir().unwrap();
    let plugin = load(&bytes, granted(base.path()));
    let args = json!({"width": 25, "depth": 30, "height": 10, "name": "lid", "copies": 3});
    let outcome = plugin.call_tool("make_box", &args, &CancelToken::new()).unwrap();
    assert_eq!(outcome.result, json!({"file": "lid_25x30x10.stl", "triangles": 12, "copies": 3}));
    let staged = outcome.bundle.expect("a bundle");
    let stl = staged.staging.out().join("lid_25x30x10.stl");
    assert_eq!(std::fs::metadata(&stl).unwrap().len(), 84 + 12 * 50, "binary STL with 12 triangles");
    assert!(staged.staging.out().starts_with(base.path().join("hello-box").join("staging")));
    assert_eq!(staged.bundle.folder.as_deref(), Some("Hello Box/lid"));

    // Into a library: one change, the file tagged and on the print list three times.
    let lib_dir = tempfile::tempdir().unwrap();
    let root = lib_dir.path().join("lib");
    std::fs::create_dir_all(&root).unwrap();
    let resolved = staged.resolve(&root, &root.join("Import")).unwrap();
    assert_eq!(resolved.to, root.join("Hello Box").join("lid"));
    let mut lib = Library::open_in_memory().unwrap();
    lib.finish_baseline(&root).unwrap();
    let ops = resolved.ops();
    assert!(matches!(ops[1], ApplyOp::Index { seen: true, .. }));
    let report = lib.apply_change("plugin:hello-box", &resolved.title, &ops).unwrap();
    assert_eq!(report.skipped, 0, "{report:?}");
    let landed = resolved.path_of("lid_25x30x10.stl");
    assert_eq!(std::fs::read(&landed).unwrap(), std::fs::read(&stl).unwrap());
    assert_eq!(quantities(&lib, "Hello Box"), [("lid_25x30x10.stl".to_owned(), 3)]);
    assert_eq!(lib.tags_of(std::slice::from_ref(&landed)).unwrap(), [("Hello Box".to_owned(), 1)]);
    lib.undo_change(report.id).unwrap();
    assert!(!landed.exists() && stl.exists(), "undone; the staging folder stays for a redo");

    // Arguments the plugin refuses, and permissions not granted: no staging folder is left.
    let err = plugin.call_tool("make_box", &json!({"width": 0}), &CancelToken::new()).unwrap_err();
    assert!(matches!(&err, Error::Invalid(m) if m.contains("between 1 and 500")), "{err}");
    let no_bundles = load(
        &bytes,
        Settings { grants: PluginGrants { ai_tools: true, ..PluginGrants::default() }, ..Settings::new(base.path()) },
    );
    assert!(matches!(no_bundles.call_tool("make_box", &args, &CancelToken::new()), Err(Error::Denied(_))));
    let nothing = load(&bytes, Settings::new(base.path()));
    assert!(matches!(nothing.call_tool("make_box", &args, &CancelToken::new()), Err(Error::Denied(_))));
    assert!(matches!(plugin.call_tool("teleport", &json!({}), &CancelToken::new()), Err(Error::Invalid(_))));
    assert_eq!(Staging::list(base.path(), "hello-box").len(), 1, "only the bundle's staging folder");
}

#[test]
fn resolve_refuses_bad_bundles() {
    let Some(bytes) = wasm() else { return };
    let base = tempfile::tempdir().unwrap();
    let plugin = load(&bytes, granted(base.path()));
    let mut staged =
        plugin.call_tool("make_box", &json!({"name": "box"}), &CancelToken::new()).unwrap().bundle.unwrap();
    let lib = tempfile::tempdir().unwrap();
    let (root, landing) = (lib.path().to_path_buf(), lib.path().join("Import"));
    assert!(staged.resolve(&root, &landing).is_ok());
    let item = |path: &str| BundleList {
        name: "Boxes".into(),
        kind: BundleListKind::Print,
        items: vec![BundleItem { path: path.into(), quantity: 1 }],
    };
    for path in ["../box_20x30x40.stl", "/out/box_20x30x40.stl", "sub/../../x.stl"] {
        staged.bundle.lists = vec![item(path)];
        let err = staged.resolve(&root, &landing).unwrap_err();
        assert!(matches!(err, Error::Bundle(BundleError::BadPath(_))), "{path}: {err}");
    }
    staged.bundle.lists.clear();
    staged.bundle.folder = Some("../../etc".into());
    assert!(matches!(staged.resolve(&root, &landing), Err(Error::Bundle(BundleError::BadPath(_)))));
    staged.bundle.folder = None;
    let small = Limits { files: 10, bytes: 100 };
    let too_big = meshgrove_core::bundle::resolve_with(&staged.bundle, staged.staging.out(), &root, &landing, small);
    assert!(matches!(too_big, Err(BundleError::TooBig(..))));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc/passwd", staged.staging.out().join("passwd")).unwrap();
        assert!(matches!(staged.resolve(&root, &landing), Err(Error::Bundle(BundleError::Link(_)))));
    }
}

/// The value of number input `id` in a view.
fn number(root: &Node, id: &str) -> f64 {
    match root.find(id) {
        Some(Node::NumberInput { value, .. }) => *value,
        other => panic!("{id}: {other:?}"),
    }
}

fn revision(root: &Node) -> u64 {
    match root.find("preview") {
        Some(Node::MeshView { revision, .. }) => *revision,
        other => panic!("{other:?}"),
    }
}

#[test]
fn panels_render_change_and_show_their_mesh() {
    let Some(bytes) = wasm() else { return };
    let base = tempfile::tempdir().unwrap();
    let plugin = Arc::new(load(&bytes, granted(base.path())));
    assert!(matches!(plugin.open_panel("nope", None), Err(Error::Invalid(_))));
    let panel = plugin.open_panel("box", None).unwrap();
    let view = panel.render().unwrap();
    assert_eq!(view.v, 1);
    assert_eq!((number(&view.root, "width"), revision(&view.root)), (20.0, 0));

    let info = meshgrove_plugin_api::HostInfo { language: "de".into(), dark: true, ..Default::default() };
    assert!(panel.event(&Event::HostChanged { info }).is_ok(), "the panel hears that the host changed");
    let finished = Event::BundleFinished { title: "Box".into(), outcome: BundleOutcome::Refused };
    assert_eq!(panel.event(&finished).unwrap(), [], "and what became of its bundle");
    let effects = panel.event(&Event::Changed { id: "width".into(), value: json!(33.5) }).unwrap();
    assert!(effects.is_empty());
    let view = panel.render().unwrap();
    assert_eq!((number(&view.root, "width"), revision(&view.root)), (33.5, 1), "the tree follows the change");
    let mesh = panel.mesh("box").unwrap().expect("triangles");
    assert_eq!(mesh.triangles(), 12);
    assert_eq!(mesh.bounds().unwrap().1, [33.5, 30.0, 40.0]);
    assert_eq!(panel.mesh("other").unwrap(), None);

    // A snapshot opens the panel again as it was.
    let snapshot = panel.snapshot().unwrap();
    let again = plugin.open_panel("box", Some(snapshot)).unwrap();
    assert_eq!(number(&again.render().unwrap().root, "width"), 33.5);
    drop(again);

    // "Add to library": the STL in the panel's /out and a bundle naming it.
    let effects = panel.event(&Event::Click { id: "apply".into() }).unwrap();
    let [Effect::ApplyBundle { bundle }] = effects.as_slice() else { panic!("{effects:?}") };
    assert_eq!(bundle.files[0].path, "box_33.5x30x40.stl");
    let staging = panel.take_output().unwrap();
    assert!(staging.out().join("box_33.5x30x40.stl").is_file());
    assert!(panel.out().read_dir().unwrap().next().is_none(), "the panel's /out is empty again");
    let lib = tempfile::tempdir().unwrap();
    assert!(meshgrove_core::bundle::resolve(bundle, staging.out(), lib.path(), &lib.path().join("Import")).is_ok());

    // Opening a saved box: the plugin asks to open its panel with it.
    let effects = plugin.open_file("lid.box.json", br#"{"width": 50, "name": "lid"}"#).unwrap();
    let [Effect::OpenPanel { panel: id, state: Some(state) }] = effects.as_slice() else { panic!("{effects:?}") };
    assert_eq!(id, "box");
    let opened = plugin.open_panel("box", Some(state.clone())).unwrap();
    assert_eq!(number(&opened.render().unwrap().root, "width"), 50.0);
    assert!(matches!(plugin.open_file("lid.stl", b""), Err(Error::Invalid(_))));
    let out = panel.out().to_path_buf();
    drop(panel);
    assert!(!out.exists(), "a closed panel's folder goes");
}

/// The 3D view's overlay and click tool: a click tints a face, Shift-click clears it.
#[test]
fn a_click_on_the_box_tints_a_face_through_the_overlay() {
    use meshgrove_plugin_api::ui::{MeshHit, PaintTool};
    let Some(bytes) = wasm() else { return };
    let base = tempfile::tempdir().unwrap();
    let plugin = Arc::new(load(&bytes, granted(base.path())));
    let panel = plugin.open_panel("box", None).unwrap();
    let preview = |panel: &meshgrove_plugins::panel::PanelHandle| match panel.render().unwrap().root.find("preview") {
        Some(Node::MeshView { paint, overlay: Some(overlay), .. }) => (paint.clone(), overlay.clone()),
        other => panic!("{other:?}"),
    };
    let (paint, overlay) = preview(&panel);
    assert_eq!((paint, overlay.key.as_str(), overlay.revision), (None, "tint", 0));
    let classes = |panel: &meshgrove_plugins::panel::PanelHandle| {
        let mesh = panel.mesh("tint").unwrap().expect("the overlay");
        assert!(mesh.positions.is_empty(), "an overlay has no positions");
        mesh.indices
    };
    assert_eq!(classes(&panel), [0; 12]);
    // Ticking "Tint faces" gives the view the click tool.
    panel.event(&Event::Changed { id: "tint".into(), value: json!(true) }).unwrap();
    let (paint, _) = preview(&panel);
    assert_eq!(paint.map(|p| p.tool), Some(PaintTool::Click));
    let hit = |triangle| MeshHit { triangle, point: [0.0; 3], normal: [0.0, 0.0, 1.0] };
    let pick = |triangle, shift| Event::MeshPick { id: "preview".into(), hit: Some(hit(triangle)), shift };
    panel.event(&pick(5, false)).unwrap();
    panel.event(&pick(2, false)).unwrap();
    let (_, overlay) = preview(&panel);
    assert_eq!(overlay.revision, 2);
    assert_eq!(classes(&panel), [0, 0, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0], "the top and the front");
    panel.event(&pick(3, true)).unwrap();
    assert_eq!(classes(&panel), [0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0], "Shift-click cleared the top");
    // A click beside the box, and a stroke, change nothing.
    panel.event(&Event::MeshPick { id: "preview".into(), hit: None, shift: false }).unwrap();
    let stroke = Event::MeshStroke {
        id: "preview".into(),
        stroke: 1,
        phase: meshgrove_plugin_api::ui::StrokePhase::Start,
        erase: false,
        hits: vec![hit(0)],
    };
    assert_eq!(panel.event(&stroke).unwrap(), []);
    assert_eq!(preview(&panel).1.revision, 3);
}

#[test]
fn jobs_run_in_parallel_into_the_panels_folder() {
    let Some(bytes) = wasm() else { return };
    let base = tempfile::tempdir().unwrap();
    let plugin = Arc::new(load(&bytes, granted(base.path())));
    let panel = plugin.open_panel("box", None).unwrap();
    let pool = JobPool::new(2);
    let (events, received) = crossbeam_channel::unbounded();
    let spec = |id: &str, name: &str| JobSpec {
        id: id.into(),
        kind: "box".into(),
        params: json!({"name": name, "width": 10}),
        label: None,
    };
    let jobs = vec![spec("a", "left"), spec("b", "right"), JobSpec { kind: "nope".into(), ..spec("c", "x") }];
    let tokens = pool.start(&plugin, jobs, panel.out(), panel.opened(), &events);
    assert_eq!(tokens.len(), 3);
    let (mut finished, mut progress) = (Vec::new(), 0);
    while finished.len() < 3 {
        match received.recv_timeout(Duration::from_secs(30)).expect("all jobs finish") {
            JobEvent::Progress { .. } => progress += 1,
            JobEvent::Finished { job, result } => finished.push((job, result)),
        }
    }
    finished.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(finished[0].1.as_ref().unwrap()["file"], "left_10x30x40.stl");
    assert_eq!(finished[1].1.as_ref().unwrap()["file"], "right_10x30x40.stl");
    assert!(matches!(finished[2].1, Err(Error::Invalid(_))));
    assert_eq!(progress, 2, "each box job reports once");
    for name in ["left_10x30x40.stl", "right_10x30x40.stl"] {
        assert!(panel.out().join(name).is_file(), "{name} in the panel's /out");
    }
}

/// A library of one box, for `measure`, which cloud AI may not see; it takes `slow` to answer,
/// and notes who asked for triangles, and how far they may read.
#[derive(Default)]
struct OneBox {
    slow: Duration,
    asked: Mutex<Vec<(String, LibraryRead, Caller)>>,
}

impl OneBox {
    fn shared(slow: Duration) -> Arc<OneBox> {
        Arc::new(OneBox { slow, ..OneBox::default() })
    }

    /// Who asked since the last look.
    fn asked(&self) -> Vec<(String, LibraryRead, Caller)> {
        std::mem::take(&mut self.asked.lock().unwrap())
    }
}

impl LibraryAccess for OneBox {
    fn search(&self, _query: &str, _limit: u32, _read: LibraryRead, _caller: Caller) -> Result<Value, String> {
        Ok(json!([{"id": "Eigene/box.stl"}]))
    }

    fn describe(&self, id: &str, _read: LibraryRead, _caller: Caller) -> Result<Value, String> {
        Ok(json!({"id": id, "copies": ["Kunden/geheim.stl"], "tags": ["Box"]}))
    }

    fn triangles(
        &self,
        id: &str,
        _part: Option<usize>,
        _max: u32,
        read: LibraryRead,
        caller: Caller,
    ) -> Result<MeshData, String> {
        self.asked.lock().unwrap().push((id.to_owned(), read, caller));
        std::thread::sleep(self.slow);
        // The box is in a library only local AI may see; "Kunden" is closed to all AI.
        let seen = match (read, caller) {
            (LibraryRead::All, _) => true,
            (LibraryRead::AiScope, caller) => id.starts_with("Eigene/") && caller.class() == AccessClass::Local,
            (LibraryRead::None, _) => false,
        };
        match id {
            "Eigene/box.stl" | "Kunden/geheim.stl" if seen => Ok(MeshData::cuboid([10.0, 20.0, 30.0])),
            _ => Err(format!("unknown id: {id}")),
        }
    }
}

#[test]
fn reading_the_library_needs_the_grant() {
    let Some(bytes) = wasm() else { return };
    let base = tempfile::tempdir().unwrap();
    let one_box = OneBox::shared(Duration::ZERO);
    let library: Option<Arc<dyn LibraryAccess>> = Some(one_box.clone());
    let args = json!({"id": "Eigene/box.stl"});
    let without = load(&bytes, Settings { library: library.clone(), ..granted(base.path()) });
    let err = without.call_tool("measure", &args, &CancelToken::new()).unwrap_err();
    assert!(matches!(&err, Error::Denied(m) if m.contains("read the library")), "{err}");

    let mut settings = granted(base.path());
    settings.grants.library_read = LibraryRead::AiScope;
    settings.library = library;
    let with = load(&bytes, settings);
    let outcome = with.call_tool("measure", &args, &CancelToken::new()).unwrap();
    assert_eq!(outcome.result, json!({"size_mm": [10.0, 20.0, 30.0], "triangles": 12}));
    assert!(outcome.bundle.is_none());
    let err = with.call_tool("measure", &json!({"id": "x"}), &CancelToken::new()).unwrap_err();
    assert!(matches!(&err, Error::Denied(m) if m.contains("unknown id")), "{err}");
    // Called by a cloud AI, it reads what that AI may see.
    let err = with.call_tool_as("measure", &args, AccessClass::Cloud, &CancelToken::new()).unwrap_err();
    assert!(matches!(&err, Error::Denied(m) if m.contains("unknown id")), "{err}");
    let asked: Vec<(LibraryRead, Caller)> =
        one_box.asked().into_iter().map(|(_, read, caller)| (read, caller)).collect();
    let (local, cloud) = (Caller::Ai(AccessClass::Local), Caller::Ai(AccessClass::Cloud));
    assert_eq!(asked, [(LibraryRead::AiScope, local), (LibraryRead::AiScope, local), (LibraryRead::AiScope, cloud)]);
}

/// The width of the box in the panel's view after it was told about `ids`, or what it says instead.
fn opened_width(panel: &meshgrove_plugins::panel::PanelHandle, ids: &[&str]) -> Result<f64, String> {
    let ids = ids.iter().map(|id| (*id).to_owned()).collect();
    match panel.event(&Event::ModelsOpened { ids }).unwrap().as_slice() {
        [Effect::Toast { text, .. }] if !text.contains("files:") => Err(text.clone()),
        _ => Ok(number(&panel.render().unwrap().root, "width")),
    }
}

/// Files the user opened with a panel are readable there without "read the library", and
/// nothing else is: not another file, not a search, not the same file through a tool.
#[test]
fn a_panel_reads_what_was_opened_with_it_and_nothing_else() {
    let Some(bytes) = wasm() else { return };
    let base = tempfile::tempdir().unwrap();
    let one_box = OneBox::shared(Duration::ZERO);
    // Tools and bundles granted, reading the library not.
    let plugin = Arc::new(load(&bytes, Settings { library: Some(one_box.clone()), ..granted(base.path()) }));
    let opened = Opened::default();
    let panel = plugin.open_panel_with("box", None, opened.clone()).unwrap();

    // Not opened: the panel is refused, and the library is not even asked.
    let err = opened_width(&panel, &["Eigene/box.stl"]).unwrap_err();
    assert!(err.contains("has not allowed hello-box to read the library"), "{err}");
    assert_eq!(one_box.asked(), []);

    // The user opens the box with the panel (what "Box around this …" does): readable, as the
    // user sees it, whatever AI may see. The box takes its size (10 mm and 2 mm of air a side).
    opened.add(&["Eigene/box.stl".into()]);
    assert_eq!(opened_width(&panel, &["Eigene/box.stl"]), Ok(14.0));
    assert_eq!(one_box.asked(), [("Eigene/box.stl".to_owned(), LibraryRead::All, Caller::User)]);
    assert_eq!(opened_width(&panel, &["Eigene/box.stl", "Eigene/other.stl"]), Ok(14.0), "the first of several");

    // Any other file stays closed, a neighbour and a look-alike included.
    for other in ["Kunden/geheim.stl", "Eigene/box.stl/../../Kunden/geheim.stl", "Eigene"] {
        let err = opened_width(&panel, &[other]).unwrap_err();
        assert!(err.contains("has not allowed hello-box to read the library"), "{other:?}: {err}");
    }
    assert_eq!(one_box.asked().len(), 1, "only the opened file was read, once more");

    // The jobs of the panel read what it reads; a job of another panel does not.
    let pool = JobPool::new(1);
    let (events, received) = crossbeam_channel::unbounded();
    let job = |id: &str| JobSpec {
        id: id.into(),
        kind: "measure".into(),
        params: json!({"id": "Eigene/box.stl"}),
        label: None,
    };
    pool.start(&plugin, vec![job("mine")], panel.out(), panel.opened(), &events);
    pool.start(&plugin, vec![job("other")], panel.out(), &Opened::default(), &events);
    let mut finished = Vec::new();
    while finished.len() < 2 {
        if let JobEvent::Finished { job, result } = received.recv_timeout(Duration::from_secs(30)).expect("jobs finish")
        {
            finished.push((job, result));
        }
    }
    finished.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(finished[0].1.as_ref().unwrap()["size_mm"], json!([10.0, 20.0, 30.0]));
    assert!(matches!(&finished[1].1, Err(Error::Denied(m)) if m.contains("has not allowed")), "{:?}", finished[1].1);

    // A tool call is the AI's, not the panel's: what was opened there does not count.
    let args = json!({"id": "Eigene/box.stl"});
    let err = plugin.call_tool("measure", &args, &CancelToken::new()).unwrap_err();
    assert!(matches!(&err, Error::Denied(m) if m.contains("read the library")), "{err}");
}

/// A plugin reads models in its own folders through the host, which needs no library and no
/// grant: what it could read itself. Paths that leave the folders are refused.
#[test]
fn a_plugin_reads_models_in_its_own_folders_and_only_there() {
    let Some(bytes) = wasm() else { return };
    let base = tempfile::tempdir().unwrap();
    let plugin = Arc::new(load(&bytes, granted(base.path())));
    let picked = plugin.data_dir().join("picked");
    std::fs::create_dir_all(&picked).unwrap();
    std::fs::write(picked.join("box.stl"), support::stl([10.0, 20.0, 30.0])).unwrap();
    std::fs::write(picked.join("dose.3mf"), support::dose_3mf()).unwrap();
    // Beside the plugin's folders, where it has no business.
    std::fs::write(base.path().join("hello-box").join("secret.stl"), support::stl([1.0, 2.0, 3.0])).unwrap();
    let ask = |tool: &str, reference: &str| {
        plugin.call_tool(tool, &json!({"id": reference}), &CancelToken::new()).map(|o| o.result)
    };

    assert_eq!(ask("measure", "/data/picked/box.stl").unwrap()["size_mm"], json!([10.0, 20.0, 30.0]));
    let facts = ask("describe", "/data/picked/box.stl").unwrap();
    assert_eq!(
        (facts["name"].as_str(), facts["kind"].as_str(), facts["triangles"].as_u64()),
        (Some("box.stl"), Some("stl"), Some(12))
    );
    assert_eq!(facts["size_mm"], json!([10.0, 20.0, 30.0]));

    // A 3MF says its parts; a part's token gives the part by itself.
    let facts = ask("describe", "/data/picked/dose.3mf").unwrap();
    let parts = facts["parts"].as_array().expect("parts");
    assert_eq!(parts.iter().map(|p| p["name"].as_str().unwrap()).collect::<Vec<_>>(), ["Dose", "Deckel"]);
    assert_eq!(ask("measure", "/data/picked/dose.3mf").unwrap()["size_mm"], json!([140.0, 30.0, 20.0]));
    let lid = parts[1]["token"].as_str().unwrap();
    assert_eq!(ask("measure", lid).unwrap(), json!({"size_mm": [40.0, 30.0, 2.0], "triangles": 12}));
    assert_eq!(ask("describe", lid).unwrap()["parts"], facts["parts"], "a part is described with its file");
    let err = ask("measure", &lid.replace("part=1", "part=7")).unwrap_err();
    assert!(matches!(&err, Error::Denied(m) if m.contains("has 2 parts")), "{err}");

    for outside in
        ["/data/../secret.stl", "/data/picked/../../secret.stl", "/tmp/../secret.stl", "/data//picked/box.stl"]
    {
        let err = ask("measure", outside).unwrap_err();
        assert!(matches!(&err, Error::Denied(m) if m.contains("plain path")), "{outside}: {err}");
    }
    for elsewhere in ["/out/box.stl", "/etc/passwd", "/"] {
        let err = ask("measure", elsewhere).unwrap_err();
        assert!(matches!(&err, Error::Denied(m) if m.contains("neither a library id")), "{elsewhere}: {err}");
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("../../secret.stl", picked.join("link.stl")).unwrap();
        let err = ask("measure", "/data/picked/link.stl").unwrap_err();
        assert!(matches!(&err, Error::Denied(m) if m.contains("leads through a link")), "{err}");
    }
    // A library id is still what it was: nothing without the grant.
    let err = ask("measure", "Eigene/box.stl").unwrap_err();
    assert!(matches!(&err, Error::Denied(m) if m.contains("has not allowed")), "{err}");

    // A panel's /tmp, where picked files land.
    let panel = plugin.open_panel("box", None).unwrap();
    std::fs::write(panel.tmp().join("a.stl"), support::stl([10.0, 20.0, 30.0])).unwrap();
    assert_eq!(opened_width(&panel, &["/tmp/a.stl"]), Ok(14.0));
    assert!(opened_width(&panel, &["/tmp/../out/a.stl"]).unwrap_err().contains("plain path"));
}

/// A library "lib" on disk whose files are all a box of 10 × 20 × 30 mm, and which says where
/// they are.
struct Shelf {
    root: PathBuf,
}

impl LibraryAccess for Shelf {
    fn search(&self, _query: &str, _limit: u32, _read: LibraryRead, _caller: Caller) -> Result<Value, String> {
        Ok(json!([]))
    }

    fn describe(&self, id: &str, _read: LibraryRead, _caller: Caller) -> Result<Value, String> {
        Ok(json!({"id": id}))
    }

    fn triangles(
        &self,
        id: &str,
        _part: Option<usize>,
        _max: u32,
        read: LibraryRead,
        caller: Caller,
    ) -> Result<MeshData, String> {
        self.locate(id, read, caller).map(|_| MeshData::cuboid([10.0, 20.0, 30.0]))
    }

    fn locate(&self, id: &str, _read: LibraryRead, _caller: Caller) -> Result<Located, String> {
        let unknown = || format!("unknown id: {id}");
        let path =
            id.strip_prefix("lib/").ok_or_else(unknown)?.split('/').fold(self.root.clone(), |p, part| p.join(part));
        if !path.is_file() {
            return Err(unknown());
        }
        Ok(Located { root: self.root.clone(), path })
    }
}

/// "Box around this …": the box made for a library model lands beside it, as a loose file
/// that never replaces one, in one change that takes only the box away again. The plugin
/// needs no grant to read the library: the user opened the model with it.
#[test]
fn a_box_made_for_a_model_lands_beside_it() {
    let Some(bytes) = wasm() else { return };
    let (base, dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = dir.path().join("lib");
    let garden = root.join("Garten");
    std::fs::create_dir_all(&garden).unwrap();
    std::fs::write(garden.join("rose.stl"), support::stl([10.0, 20.0, 30.0])).unwrap();
    // The name the box will ask for is taken by a file of the user's.
    std::fs::write(garden.join("box_14x24x34.stl"), "mine").unwrap();
    let library: Arc<dyn LibraryAccess> = Arc::new(Shelf { root: root.clone() });
    let plugin = Arc::new(load(&bytes, Settings { library: Some(library), ..granted(base.path()) }));

    let panel = plugin.open_panel("box", None).unwrap();
    panel.opened().add(&["lib/Garten/rose.stl".into()]);
    assert_eq!(opened_width(&panel, &["lib/Garten/rose.stl"]), Ok(14.0));
    let effects = panel.event(&Event::Click { id: "apply".into() }).unwrap();
    let [Effect::ApplyBundle { bundle }] = effects.as_slice() else { panic!("{effects:?}") };
    assert_eq!((bundle.next_to.as_deref(), bundle.folder.as_deref()), (Some("lib/Garten/rose.stl"), None));

    // Handed over: the host finds the model, as the panel may read it.
    let staged = panel.stage(bundle.clone()).unwrap();
    assert_eq!(staged.sources.len(), 1);
    assert_eq!(staged.sources[0].path, garden.join("rose.stl"));
    // No library is set up for imports, and none is needed: the model's library takes the file.
    let resolved = staged.resolve_for(&meshgrove_core::config::Config::default()).unwrap();
    assert!(resolved.is_loose());
    assert_eq!(resolved.plugin, "hello-box");
    let landed = resolved.path_of("box_14x24x34.stl");
    assert_eq!(landed, garden.join("box_14x24x34 (2).stl"), "beside the rose, under a free name");
    assert_eq!(resolved.files[0].beside.as_deref(), Some("rose.stl"));

    let mut lib = Library::open_in_memory().unwrap();
    lib.finish_baseline(&root).unwrap();
    let ops = resolved.ops();
    assert!(
        matches!(&ops[..2], [ApplyOp::Copy { folder: false, .. }, ApplyOp::Index { only, .. }] if only == std::slice::from_ref(&landed))
    );
    let report = lib.apply_change("plugin:hello-box", &resolved.title, &ops).unwrap();
    assert_eq!(report.skipped, 0, "{report:?}");
    assert_eq!(std::fs::metadata(&landed).unwrap().len(), 84 + 12 * 50);
    assert_eq!(std::fs::read(garden.join("box_14x24x34.stl")).unwrap(), b"mine", "never replaced");
    assert_eq!(lib.tags_of(std::slice::from_ref(&landed)).unwrap(), [("Hello Box".to_owned(), 1)]);
    assert_eq!(quantities(&lib, "Hello Box"), [("box_14x24x34 (2).stl".to_owned(), 1)]);
    lib.undo_change(report.id).unwrap();
    assert!(!landed.exists() && garden.join("rose.stl").is_file() && garden.join("box_14x24x34.stl").is_file());

    // Beside a file the panel may not read, nothing lands: not one that was not opened with
    // it, not one that is not there, and no path instead of an id.
    let beside = |reference: &str| {
        std::fs::write(panel.out().join("box.stl"), support::stl([1.0, 2.0, 3.0])).unwrap();
        let bundle =
            meshgrove_plugin_api::Bundle { title: "Box".into(), next_to: Some(reference.into()), ..Default::default() };
        let staged = panel.stage(bundle);
        assert!(panel.out().read_dir().unwrap().next().is_none(), "the files are taken either way");
        staged
    };
    std::fs::write(garden.join("topf.stl"), "topf").unwrap();
    let err = beside("lib/Garten/topf.stl").unwrap_err();
    assert!(matches!(&err, Error::Denied(m) if m.contains("has not allowed hello-box to read the library")), "{err}");
    assert_eq!(Staging::list(base.path(), "hello-box").len(), 1, "only the first bundle's staging folder is left");
    panel.opened().add(&["lib/Garten/weg.stl".into()]);
    assert!(matches!(beside("lib/Garten/weg.stl"), Err(Error::Denied(m)) if m.contains("unknown id")));
    assert!(matches!(beside("/data/picked/box.stl"), Err(Error::Invalid(m)) if m.contains("not a path")));
    // Beside a part of a file is beside the file.
    let token = meshgrove_plugins::library::part_token("lib/Garten/rose.stl", 0);
    assert_eq!(beside(&token).unwrap().sources[0].path, garden.join("rose.stl"));

    // A tool is the AI's: with the grant it may put its files beside what that AI may read.
    let mut settings = granted(base.path());
    settings.grants.library_read = LibraryRead::All;
    settings.library = Some(Arc::new(Shelf { root: root.clone() }));
    let reader = Arc::new(load(&bytes, settings));
    let other = reader.open_panel("box", None).unwrap();
    assert_eq!(opened_width(&other, &["lib/Garten/topf.stl"]), Ok(14.0), "through the grant");
    let effects = other.event(&Event::Click { id: "apply".into() }).unwrap();
    let [Effect::ApplyBundle { bundle }] = effects.as_slice() else { panic!("{effects:?}") };
    let resolved = other.stage(bundle.clone()).unwrap().resolve(Path::new(""), Path::new("")).unwrap();
    assert_eq!(resolved.files[0].beside.as_deref(), Some("topf.stl"));
}

/// "Folder …": the panel asks for Meshgrove's folder chooser, gets a token for the folder the
/// user chose and names it in its bundle; the box lands there, in a library that is not the one
/// imports go to. Tokens the host did not give, and tools, put nothing anywhere.
#[test]
fn a_box_lands_in_the_folder_the_user_chose() {
    use meshgrove_plugins::{Target, Targets};
    let Some(bytes) = wasm() else { return };
    let (base, dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (imports, workshop) = (dir.path().join("Eigene"), dir.path().join("Werkstatt"));
    std::fs::create_dir_all(&imports).unwrap();
    std::fs::create_dir_all(workshop.join("Boxen").join("box")).unwrap();
    let config =
        meshgrove_core::config::Config { library_roots: vec![imports.clone(), workshop.clone()], ..Default::default() };
    let plugin = Arc::new(load(&bytes, granted(base.path())));
    let targets = Targets::default();
    let panel = plugin.open_panel("box", None).unwrap().with_targets(targets.clone());

    let effects = panel.event(&Event::Click { id: "folder".into() }).unwrap();
    let [Effect::PickFolder { id, title: Some(_) }] = effects.as_slice() else { panic!("{effects:?}") };
    // The user chose Werkstatt/Boxen: the host keeps it and tells the panel its token.
    let chosen = Target { root: workshop.clone(), dir: workshop.join("Boxen"), label: "Werkstatt/Boxen".into() };
    let token = targets.add(chosen);
    let picked = Event::FolderPicked { id: id.clone(), target: token.clone(), label: "Werkstatt/Boxen".into() };
    assert!(panel.event(&picked).unwrap().is_empty());
    let view = serde_json::to_string(&panel.render().unwrap()).unwrap();
    assert!(view.contains("Into Werkstatt/Boxen"), "{view}");

    let effects = panel.event(&Event::Click { id: "apply".into() }).unwrap();
    let [Effect::ApplyBundle { bundle }] = effects.as_slice() else { panic!("{effects:?}") };
    assert_eq!((bundle.target.as_deref(), bundle.folder.as_deref()), (Some(token.as_str()), Some("box")));
    let staged = panel.stage(bundle.clone()).unwrap();
    assert!(staged.is_placed());
    let resolved = staged.resolve_for(&config).unwrap();
    assert_eq!(resolved.root, workshop);
    assert_eq!(resolved.to, workshop.join("Boxen").join("box (2)"), "a folder of that name is there already");
    let mut lib = Library::open_in_memory().unwrap();
    lib.finish_baseline(&workshop).unwrap();
    let report = lib.apply_change("plugin:hello-box", &resolved.title, &resolved.ops()).unwrap();
    assert_eq!(report.skipped, 0, "{report:?}");
    assert!(resolved.path_of("box_20x30x40.stl").is_file());
    assert!(std::fs::read_dir(&imports).unwrap().next().is_none(), "nothing in the import library");

    // The library left Meshgrove meanwhile: the bundle has no place.
    std::fs::write(panel.out().join("box.stl"), support::stl([1.0, 2.0, 3.0])).unwrap();
    let again = panel.stage(bundle.clone()).unwrap();
    let only_imports = meshgrove_core::config::Config { library_roots: vec![imports.clone()], ..Default::default() };
    let err = again.resolve_for(&only_imports).unwrap_err();
    assert!(err.to_string().contains("Werkstatt/Boxen"), "{err}");

    // A token the host did not give: refused before the files are taken.
    std::fs::write(panel.out().join("box.stl"), support::stl([1.0, 2.0, 3.0])).unwrap();
    let forged = meshgrove_plugin_api::Bundle { target: Some("0123456789abcdef".into()), ..bundle.clone() };
    let err = panel.stage(forged).unwrap_err();
    assert!(matches!(&err, Error::Invalid(m) if m.contains("pick_folder")), "{err}");
    assert!(panel.out().join("box.stl").is_file(), "still there for the next try");

    // A tool's bundle cannot name one: tools have no folders chosen for them.
    let before = Staging::list(base.path(), "hello-box").len();
    let err = plugin.call_tool("make_box", &json!({"target": token}), &CancelToken::new()).unwrap_err();
    assert!(matches!(&err, Error::Invalid(m) if m.contains("pick_folder")), "{err}");
    assert_eq!(Staging::list(base.path(), "hello-box").len(), before, "its staging folder went");
}

/// A bundle file that is a copy of a 3MF with one part swapped: the host makes it when the
/// bundle is handed over, from the mesh the plugin wrote, which does not go into the library.
#[test]
fn a_3mf_is_copied_with_the_mesh_of_one_part_swapped() {
    use meshgrove_core::loaders::threemf::parse_parts;
    use meshgrove_plugin_api::{Bundle, BundleFile, Derive3mf, PartMesh};
    use meshgrove_plugins::library::part_token;

    let Some(bytes) = wasm() else { return };
    let (base, dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = dir.path().join("lib");
    std::fs::create_dir_all(root.join("Dosen")).unwrap();
    std::fs::write(root.join("Dosen").join("dose.3mf"), support::dose_3mf()).unwrap();
    std::fs::write(root.join("Dosen").join("zweite.3mf"), support::dose_3mf()).unwrap();
    std::fs::write(root.join("box.stl"), support::stl([1.0, 2.0, 3.0])).unwrap();
    let library: Arc<dyn LibraryAccess> = Arc::new(Shelf { root: root.clone() });
    let plugin = Arc::new(load(&bytes, Settings { library: Some(library), ..granted(base.path()) }));
    let picked = plugin.data_dir().join("picked");
    std::fs::create_dir_all(&picked).unwrap();
    std::fs::write(picked.join("dose.3mf"), support::dose_3mf()).unwrap();
    let panel = plugin.open_panel("box", None).unwrap();
    panel.opened().add(&["lib/Dosen/dose.3mf".into(), "lib/box.stl".into()]);

    // A lid of 5 mm instead of 2, as the plugin would write it below /out.
    let write = |rel: &[&str]| {
        let path = rel.iter().fold(panel.out().to_path_buf(), |p, part| p.join(part));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, support::stl([40.0, 30.0, 5.0])).unwrap();
    };
    let swapped = |source: &str, part: String, mesh: &str| Bundle {
        title: "Deckel mit Rillen".into(),
        files: vec![BundleFile {
            path: "dose – Rillen.3mf".into(),
            tags: vec!["Texturiert".into()],
            derive_3mf: Some(Derive3mf { source: source.into(), replace: vec![PartMesh { part, mesh: mesh.into() }] }),
            ..BundleFile::default()
        }],
        ..Bundle::default()
    };
    let lid_of = |source: &str| part_token(source, 1);
    let files_in = |out: &Path| -> Vec<String> {
        let mut names: Vec<String> =
            out.read_dir().unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    };

    // From the library, beside its source.
    let id = "lib/Dosen/dose.3mf";
    write(&["parts", "deckel.stl"]);
    let bundle = Bundle { next_to: Some(id.into()), ..swapped(id, lid_of(id), "parts/deckel.stl") };
    let staged = panel.stage(bundle).unwrap();
    assert_eq!(files_in(staged.staging.out()), ["dose – Rillen.3mf"], "the mesh is used up, its folder gone");
    let derived = std::fs::read(staged.staging.out().join("dose – Rillen.3mf")).unwrap();
    let (before, after) = (parse_parts(&support::dose_3mf()).unwrap(), parse_parts(&derived).unwrap());
    assert_eq!(after.len(), 2);
    assert_eq!(after[0].mesh.positions, before[0].mesh.positions, "the box is as it was");
    assert_eq!(after[0].name.as_deref(), Some("Dose"));
    assert_eq!(after[1].mesh.bounds().unwrap().size().to_array(), [40.0, 30.0, 5.0], "the lid is the new one");
    assert_eq!((after[1].name.as_deref(), after[1].instances), (Some("Deckel"), 2), "placed twice as before");
    assert_eq!(after[1].transform, before[1].transform);
    let resolved = staged.resolve_for(&meshgrove_core::config::Config::default()).unwrap();
    assert_eq!(resolved.path_of("dose – Rillen.3mf"), root.join("Dosen").join("dose – Rillen.3mf"));
    assert_eq!((resolved.files.len(), resolved.files[0].tags.as_slice()), (1, &["Texturiert".to_owned()][..]));

    // From the plugin's own folder (a file the user picked), into a folder of the library.
    let own = "/data/picked/dose.3mf";
    write(&["deckel.stl"]);
    let bundle = Bundle { folder: Some("Texturiert".into()), ..swapped(own, lid_of(own), "deckel.stl") };
    let staged = panel.stage(bundle).unwrap();
    assert_eq!(files_in(staged.staging.out()), ["dose – Rillen.3mf"]);
    let resolved = staged.resolve(&root, &root.join("Import")).unwrap();
    assert_eq!(resolved.path_of("dose – Rillen.3mf"), root.join("Texturiert").join("dose – Rillen.3mf"));
    assert_eq!(Staging::list(base.path(), "hello-box").len(), 2);

    // What does not fit is refused, and leaves nothing behind.
    let refused = |bundle: Bundle| {
        write(&["parts", "deckel.stl"]);
        let err = panel.stage(bundle).unwrap_err();
        assert!(panel.out().read_dir().unwrap().next().is_none(), "the panel's /out is empty again");
        assert_eq!(Staging::list(base.path(), "hello-box").len(), 2, "no staging folder is left of it: {err}");
        err
    };
    let mesh = "parts/deckel.stl";
    let err = refused(swapped(id, lid_of("lib/Dosen/zweite.3mf"), mesh));
    assert!(matches!(&err, Error::Invalid(m) if m.contains("not one of dose.3mf")), "a part of another file: {err}");
    let err = refused(swapped(id, id.into(), mesh));
    assert!(matches!(&err, Error::Invalid(m) if m.contains("not one of dose.3mf")), "a file is no part: {err}");
    let err = refused(swapped(id, part_token(id, 7), mesh));
    assert!(matches!(&err, Error::Failed(m) if m.contains("has 2 parts")), "{err}");
    let err = refused(swapped("lib/Dosen/zweite.3mf", lid_of("lib/Dosen/zweite.3mf"), mesh));
    assert!(matches!(&err, Error::Denied(m) if m.contains("has not allowed")), "not opened with the panel: {err}");
    let err = refused(swapped("lib/box.stl", lid_of("lib/box.stl"), mesh));
    assert!(matches!(&err, Error::Invalid(m) if m.contains("is not a 3MF")), "{err}");
    let err = refused(swapped(&lid_of(id), lid_of(id), mesh));
    assert!(matches!(&err, Error::Invalid(m) if m.contains("a file, not a part")), "{err}");
    for outside in ["../deckel.stl", "/out/parts/deckel.stl", "parts/../../x.stl", "parts/missing.stl"] {
        let err = refused(swapped(id, lid_of(id), outside));
        assert!(
            matches!(&err, Error::Invalid(m) if m.contains("plain path") || m.contains("not a file")),
            "{outside}: {err}"
        );
    }
    let mut taken = swapped(id, lid_of(id), mesh);
    taken.files[0].path = "parts/deckel.stl".into();
    let err = refused(taken);
    assert!(matches!(&err, Error::Invalid(m) if m.contains("is there already")), "{err}");
    #[cfg(unix)]
    {
        let secret = base.path().join("secret.stl");
        std::fs::write(&secret, support::stl([1.0, 1.0, 1.0])).unwrap();
        std::os::unix::fs::symlink(&secret, panel.out().join("link.stl")).unwrap();
        let err = refused(swapped(id, lid_of(id), "link.stl"));
        assert!(matches!(&err, Error::Invalid(m) if m.contains("leads through a link")), "{err}");
    }
}

/// What `meshgrove-plugin-run --library DIR --data DIR` sets up: a plain folder as the
/// library, another as the plugin's `/data`.
#[test]
fn a_plugin_runs_on_a_folder_as_its_library() {
    use meshgrove_plugins::folder::FolderLibrary;
    let Some(bytes) = wasm() else { return };
    let (base, dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (models, data) = (dir.path().join("models"), dir.path().join("data"));
    std::fs::create_dir_all(models.join("Dosen")).unwrap();
    std::fs::create_dir_all(data.join("picked")).unwrap();
    std::fs::write(models.join("Dosen").join("dose.3mf"), support::dose_3mf()).unwrap();
    std::fs::write(data.join("picked").join("box.stl"), support::stl([10.0, 20.0, 30.0])).unwrap();
    let mut settings = granted(base.path());
    settings.library = Some(Arc::new(FolderLibrary::new(&models).unwrap()));
    settings.grants.library_read = LibraryRead::All;
    settings.data = Some(data.clone());
    let plugin = load(&bytes, settings);
    assert_eq!(plugin.data_dir(), data);
    let ask = |tool: &str, reference: &str| {
        plugin.call_tool(tool, &json!({"id": reference}), &CancelToken::new()).map(|o| o.result)
    };
    assert_eq!(ask("measure", "models/Dosen/dose.3mf").unwrap()["size_mm"], json!([140.0, 30.0, 20.0]));
    let facts = ask("describe", "models/Dosen/dose.3mf").unwrap();
    let lid = facts["parts"][1]["token"].as_str().unwrap();
    assert_eq!(ask("measure", lid).unwrap()["size_mm"], json!([40.0, 30.0, 2.0]));
    assert_eq!(
        ask("measure", "/data/picked/box.stl").unwrap()["size_mm"],
        json!([10.0, 20.0, 30.0]),
        "the given /data"
    );
    assert!(
        matches!(ask("measure", "models/../data/picked/box.stl"), Err(Error::Denied(m)) if m.contains("unknown id"))
    );
    assert!(!base.path().join("hello-box").join("data").exists(), "its usual folder is not made");
}

/// With the grant, a file outside what the grant shows still reads when the user opened it.
#[test]
fn an_opened_file_reads_past_what_ai_may_see() {
    let Some(bytes) = wasm() else { return };
    let base = tempfile::tempdir().unwrap();
    let one_box = OneBox::shared(Duration::ZERO);
    let mut settings = granted(base.path());
    settings.grants.library_read = LibraryRead::AiScope;
    settings.library = Some(one_box.clone());
    let plugin = Arc::new(load(&bytes, settings));
    let panel = plugin.open_panel("box", None).unwrap();
    assert_eq!(opened_width(&panel, &["Eigene/box.stl"]), Ok(14.0), "through the grant");
    assert_eq!(one_box.asked(), [("Eigene/box.stl".to_owned(), LibraryRead::AiScope, Caller::User)]);
    assert!(opened_width(&panel, &["Kunden/geheim.stl"]).unwrap_err().contains("unknown id"), "closed to AI");
    panel.opened().add(&["Kunden/geheim.stl".into()]);
    assert_eq!(opened_width(&panel, &["Kunden/geheim.stl"]), Ok(14.0), "opened by the user");
    let asked: Vec<LibraryRead> = one_box.asked().into_iter().map(|(_, read, _)| read).collect();
    assert_eq!(asked, [LibraryRead::AiScope, LibraryRead::AiScope, LibraryRead::All], "the grant first");
}

#[test]
fn time_spent_reading_the_library_is_not_the_plugins() {
    let Some(bytes) = wasm() else { return };
    let base = tempfile::tempdir().unwrap();
    let mut settings = granted(base.path());
    settings.grants.library_read = LibraryRead::AiScope;
    settings.library = Some(OneBox::shared(Duration::from_millis(600)));
    settings.tool_deadline = Duration::from_millis(400);
    let plugin = load(&bytes, settings);
    let outcome = plugin.call_tool("measure", &json!({"id": "Eigene/box.stl"}), &CancelToken::new());
    assert!(outcome.is_ok(), "the 600 ms of loading do not count against 400 ms: {outcome:?}");
}

#[test]
fn endless_loops_run_into_the_deadline() {
    let Some(bytes) = wasm() else { return };
    let base = tempfile::tempdir().unwrap();
    let plugin = load(&bytes, Settings { tool_deadline: Duration::from_millis(300), ..granted(base.path()) });
    let started = Instant::now();
    let err = plugin.call_tool("spin", &json!({}), &CancelToken::new()).unwrap_err();
    assert!(matches!(err, Error::Timeout(d) if d == Duration::from_millis(300)), "{err}");
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    // The plugin is fine for the next call (a fresh instance).
    assert!(plugin.call_tool("make_box", &json!({}), &CancelToken::new()).is_ok());
}

#[test]
fn memory_is_limited() {
    let Some(bytes) = wasm() else { return };
    let base = tempfile::tempdir().unwrap();
    let plugin = load(&bytes, Settings { memory_mb: Some(64), ..granted(base.path()) });
    let err = plugin.call_tool("trap", &json!({}), &CancelToken::new()).unwrap_err();
    assert!(matches!(err, Error::Memory(64)), "{err}");
}

#[test]
fn cancel_stops_a_running_call() {
    let Some(bytes) = wasm() else { return };
    let base = tempfile::tempdir().unwrap();
    let plugin = load(&bytes, granted(base.path()));
    let cancel = CancelToken::new();
    let later = cancel.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        later.cancel();
    });
    let started = Instant::now();
    let err = plugin.call_tool("spin", &json!({}), &cancel).unwrap_err();
    canceller.join().unwrap();
    assert!(matches!(err, Error::Cancelled), "{err}");
    assert!(started.elapsed() < Duration::from_secs(10), "long before the 60 s deadline");
}

/// hello-box as Meshgrove 0.8.3 shipped it: built against plugin API 0.1 (see fixtures/README.md).
const API_0_1: &[u8] = include_bytes!("fixtures/hello_box_api_0_1.wasm");

/// The world a plugin links against did not change with API 0.2, so what was installed before
/// keeps running: its manifest, tools, panel, job and file handler.
#[test]
fn a_plugin_built_for_api_0_1_still_runs() {
    let base = tempfile::tempdir().unwrap();
    let plugin = Arc::new(load(API_0_1, granted(base.path())));
    assert_eq!(plugin.manifest().api, "0.1.0");
    assert_ne!(plugin.manifest().api, meshgrove_plugin_api::API_VERSION, "the fixture is the old one");
    let file_type = &plugin.manifest().file_types[0];
    assert_eq!((file_type.receive, &file_type.label, &file_type.panel), (Receive::Bytes, &None, &None));
    let outcome = plugin.call_tool("make_box", &json!({"width": 25, "name": "lid"}), &CancelToken::new()).unwrap();
    let staged = outcome.bundle.expect("a bundle");
    assert!(staged.staging.out().join("lid_25x30x40.stl").is_file());
    assert_eq!((staged.bundle.next_to.as_ref(), staged.bundle.job.as_ref()), (None, None));
    assert!(staged.sources.is_empty());
    let panel = plugin.open_panel("box", None).unwrap();
    assert_eq!(number(&panel.render().unwrap().root, "width"), 20.0);
    // It hears of events it has never heard of, and of a host that says more than it knows.
    assert_eq!(panel.event(&Event::ModelsOpened { ids: vec!["Eigene/box.stl".into()] }).unwrap(), []);
    let info = meshgrove_plugin_api::HostInfo { language: "de".into(), imperial: true, ..Default::default() };
    assert!(panel.event(&Event::HostChanged { info }).is_ok());
    assert_eq!(panel.mesh("box").unwrap().expect("triangles").triangles(), 12);
    let job = plugin.run_job("box", &json!({"name": "left"}), panel.out(), &CancelToken::new(), None).unwrap();
    assert_eq!(job["file"], "left_20x30x40.stl");
    let effects = plugin.open_file("lid.box.json", br#"{"width": 50}"#).unwrap();
    assert!(matches!(effects.as_slice(), [Effect::OpenPanel { .. }]), "{effects:?}");
}

/// The manifest names the API of the JSON shapes: a host one minor version on (0.3) still loads
/// hello-box (0.2); one of the version before (0.1, Meshgrove 0.8) asks for an update, and a
/// host of another major version says which Meshgrove the plugin needs.
#[test]
fn the_manifests_api_decides_not_the_worlds() {
    let Some(bytes) = wasm() else { return };
    let base = tempfile::tempdir().unwrap();
    let later = load(&bytes, Settings { host_api: "0.4.0".into(), ..granted(base.path()) });
    assert!(later.call_tool("make_box", &json!({}), &CancelToken::new()).is_ok());
    let older = Plugin::from_bytes(&bytes, Settings { host_api: "0.2.0".into(), ..granted(base.path()) });
    let Err(Error::Load(why)) = older else { panic!("loaded by a 0.2 host") };
    assert!(why.contains("built for plugin API 0.3.0") && why.contains("update Meshgrove"), "{why}");
    let other = Plugin::from_bytes(&bytes, Settings { host_api: "1.0.0".into(), ..granted(base.path()) });
    let Err(Error::Load(why)) = other else { panic!("loaded by a 1.0 host") };
    assert!(why.contains("built for plugin API 0.3.0") && why.contains("an older Meshgrove"), "{why}");
    // The old plugin on a host that says it is the old one, as on Meshgrove 0.8.
    assert!(Plugin::from_bytes(API_0_1, Settings { host_api: "0.1.0".into(), ..granted(base.path()) }).is_ok());
}

/// An effect of a newer API is left out; the rest of the answer is done.
#[test]
fn an_unknown_effect_is_skipped() {
    let Some(bytes) = wasm() else { return };
    let base = tempfile::tempdir().unwrap();
    let plugin = Arc::new(load(&bytes, granted(base.path())));
    let panel = plugin.open_panel("box", None).unwrap();
    let effects = panel.event(&Event::Click { id: "future".into() }).unwrap();
    assert!(matches!(effects.as_slice(), [Effect::Toast { text, .. }] if text == "still here"), "{effects:?}");
    assert!(panel.render().is_ok(), "and the panel goes on");
}

#[test]
fn discovery_offers_the_plugin_once_and_not_when_installed() {
    use meshgrove_plugins::discovery::{Folder, discover};
    let Some(bytes) = wasm() else { return };
    let base = tempfile::tempdir().unwrap();
    let (put, downloads) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(put.path().join("hello_box.wasm"), &bytes).unwrap();
    std::fs::write(downloads.path().join("hello-box-copy.wasm"), &bytes).unwrap();
    let folders = [
        Folder { path: put.path().to_path_buf(), report_problems: true },
        Folder { path: downloads.path().to_path_buf(), report_problems: false },
    ];
    let found = discover(&folders, &Settings::new(base.path()), &[]);
    assert_eq!(found.len(), 1, "the copy has the same content: {found:?}");
    let offer = found[0].plugin.as_ref().unwrap();
    assert_eq!(offer.id, "hello-box");
    assert!(!offer.name.is_empty() && !offer.version.is_empty());
    assert_eq!(found[0].path, put.path().join("hello_box.wasm"));
    let installed = [found[0].hash.clone()];
    assert!(discover(&folders, &Settings::new(base.path()), &installed).is_empty());
}

/// A registry of hello-box as the window or `meshgrove-mcp` loads it, reading `library` with
/// "what local AI sees", and what a tool call needs: a library to put files into, a
/// suggestion store in a file.
struct Deferred {
    dir: tempfile::TempDir,
    config: meshgrove_core::config::Config,
    registry: meshgrove_plugins::Registry,
    store: meshgrove_core::ai::proposals::ProposalStore,
}

fn deferred(bytes: &[u8], library: Arc<dyn LibraryAccess>) -> Deferred {
    use meshgrove_core::config::{Config, PluginGrants};
    let dir = tempfile::tempdir().unwrap();
    let (root, base, file) = (dir.path().join("lib"), dir.path().join("plugins"), dir.path().join("hello_box.wasm"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(&file, bytes).unwrap();
    let hash = Plugin::from_bytes(bytes, Settings::new(&base)).unwrap().hash().to_owned();
    let grants = PluginGrants { ai_tools: true, bundles: true, auto_accept: true, library_read: LibraryRead::AiScope };
    let entry = PluginConfig { id: "hello-box".into(), file, blake3: hash, enabled: true, grants, memory_mb: None };
    let mut config = Config { library_roots: vec![root], plugins: vec![entry], ..Config::default() };
    // The user lets suggestions be accepted by themselves: a job's files wait all the same.
    config.ai.enabled = true;
    config.ai.auto_accept_since = Some(1);
    let info = Settings::new(&base).info;
    let registry = meshgrove_plugins::Registry::load(&config, &base, &info, Some(library));
    assert_eq!(registry.problems(), [], "loads");
    let store = meshgrove_core::ai::proposals::ProposalStore::open(&dir.path().join("proposals.db")).unwrap();
    Deferred { dir, config, registry, store }
}

impl Deferred {
    /// Calls `make_box_later` as an AI of `class`; the answer.
    fn call(&mut self, class: AccessClass, args: Value) -> Result<Value, String> {
        let lib = Library::open_in_memory().unwrap();
        let cx = meshgrove_core::ai::Context {
            lib: &lib,
            scope: meshgrove_core::ai::Scope::whole(&self.config),
            config: &self.config,
            thumbs: None,
            printers: Vec::new(),
            now: std::time::SystemTime::now(),
        };
        let origin = meshgrove_core::ai::proposals::Origin { source: "mcp".into(), client: Some("test".into()), class };
        let name = "hello-box_make_box_later";
        let (answer, _) = self
            .registry
            .call_ai(&cx, &mut self.store, &origin, name, args, &CancelToken::new())
            .map_err(|e| e.to_string())?;
        Ok(answer)
    }

    /// The batches waiting in the review, and their bundles.
    fn waiting(&self) -> Vec<meshgrove_core::bundle::ResolvedBundle> {
        let batches = self.store.batches(None, None, &["pending"], 100).unwrap();
        batches
            .iter()
            .flat_map(|b| self.store.proposals(b.id).unwrap())
            .map(|p| serde_json::from_value(p.payload).unwrap())
            .collect()
    }

    fn staging_folders(&self) -> usize {
        Staging::list(&self.dir.path().join("plugins"), "hello-box").len()
    }
}

/// A tool hands its work to a job: the AI hears at once that it started; the files arrive in
/// the review when the job is done, and always wait there.
#[test]
fn a_tools_job_runs_in_the_background_and_its_files_wait_in_the_review() {
    let Some(bytes) = wasm() else { return };
    let mut d = deferred(&bytes, OneBox::shared(Duration::ZERO));
    let answer = d.call(AccessClass::Local, json!({"name": "later", "width": 25})).unwrap();
    assert_eq!((answer["status"].as_str(), answer["arrives"].as_str()), (Some("started"), Some("review")), "{answer}");
    assert_eq!(answer["result"], json!({"box": "later"}));
    let job = answer["job"].as_str().unwrap().to_owned();
    assert!(d.registry.jobs().wait_idle(Duration::from_secs(60)));
    let finished = d.registry.jobs().take_finished();
    assert_eq!(finished.len(), 1);
    assert_eq!((finished[0].id.as_str(), finished[0].plugin_name.as_str()), (job.as_str(), "Hello Box"));
    assert_eq!(finished[0].outcome, Ok("Box 25 × 30 × 40 mm".to_owned()), "the job's own bundle");
    let waiting = d.waiting();
    assert_eq!(waiting.len(), 1);
    assert!(waiting[0].review, "never accepted by itself, though the user lets the plugin's be");
    assert_eq!(waiting[0].files[0].rel, "later_25x30x40.stl");
    assert!(waiting[0].from.join("later_25x30x40.stl").is_file(), "the files the job wrote");
    assert!(!waiting[0].from.parent().unwrap().join(meshgrove_plugins::deferred::MARKER).exists(), "marker gone");
    assert!(d.registry.jobs().running().is_empty());
}

/// The job reads the library as the AI that called the tool: a cloud AI's job does not see a
/// library only local AI may see. It then proposes nothing and leaves no staging folder.
#[test]
fn a_tools_job_reads_as_its_caller() {
    let Some(bytes) = wasm() else { return };
    let one_box = OneBox::shared(Duration::ZERO);
    let mut d = deferred(&bytes, one_box.clone());
    let args = json!({"name": "around", "around": "Eigene/box.stl"});
    d.call(AccessClass::Cloud, args.clone()).unwrap();
    assert!(d.registry.jobs().wait_idle(Duration::from_secs(60)));
    let failed = d.registry.jobs().take_finished();
    assert!(failed[0].outcome.as_ref().is_err_and(|e| e.contains("unknown id")), "{failed:?}");
    assert!(d.waiting().is_empty() && d.staging_folders() == 0);
    assert_eq!(
        one_box.asked().into_iter().map(|(_, r, c)| (r, c)).collect::<Vec<_>>(),
        [(LibraryRead::AiScope, Caller::Ai(AccessClass::Cloud))]
    );
    // A local AI's job does: the box takes the model's size.
    d.call(AccessClass::Local, args).unwrap();
    assert!(d.registry.jobs().wait_idle(Duration::from_secs(60)));
    assert_eq!(d.registry.jobs().take_finished()[0].outcome, Ok("Box 14 × 24 × 34 mm".to_owned()));
    assert_eq!(d.waiting()[0].files[0].rel, "around_14x24x34.stl");
    assert_eq!(one_box.asked()[0].2, Caller::Ai(AccessClass::Local));
}

/// A job can be cancelled: nothing is proposed and its staging folder goes. While it runs,
/// its folder carries the marker that keeps cleaning up away.
#[test]
fn a_cancelled_tool_job_leaves_nothing() {
    let Some(bytes) = wasm() else { return };
    let mut d = deferred(&bytes, OneBox::shared(Duration::ZERO));
    let answer = d.call(AccessClass::Local, json!({"name": "slow", "slow": true})).unwrap();
    let job = answer["job"].as_str().unwrap().to_owned();
    let until = Instant::now() + Duration::from_secs(30);
    while d.registry.jobs().running().first().is_none_or(|j| j.fraction == 0.0) {
        assert!(Instant::now() < until, "the job reports");
        std::thread::sleep(Duration::from_millis(10));
    }
    let running = d.registry.jobs().running();
    assert_eq!(
        (running[0].id.as_str(), running[0].message.as_str(), running[0].tool.as_str()),
        (job.as_str(), "taking its time", "make_box_later")
    );
    let staging = Staging::list(&d.dir.path().join("plugins"), "hello-box");
    let dir = staging[0].1.parent().unwrap().to_path_buf();
    assert!(meshgrove_plugins::deferred::running_in(&dir));
    let swept = meshgrove_plugins::staging::sweep(&d.dir.path().join("plugins"), &|_| false, Duration::ZERO);
    assert_eq!(swept.staging, 0, "a running job's folder stays");
    assert!(d.registry.jobs().cancel(&job));
    assert!(d.registry.jobs().wait_idle(Duration::from_secs(30)));
    let finished = d.registry.jobs().take_finished();
    assert_eq!(finished[0].outcome, Err("cancelled".to_owned()));
    assert!(d.waiting().is_empty() && d.staging_folders() == 0 && !dir.exists());
    assert!(!d.registry.jobs().cancel(&job), "not running any more");
}
