//! The host against the example plugin `examples/plugins/hello-box`, built with its test hooks
//! (`scripts/build-plugins.sh --test-hooks`, or the file named by MESHGROVE_TEST_PLUGIN).
//! Without the file each test passes with a note on stderr, unless MESHGROVE_REQUIRE_PLUGIN=1
//! (as in CI).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use meshgrove_core::bundle::{BundleError, BundleItem, BundleList, BundleListKind, Limits};
use meshgrove_core::changes::ApplyOp;
use meshgrove_core::config::{LibraryRead, PluginConfig};
use meshgrove_core::db::Library;
use meshgrove_plugin_api::ui::{Effect, Event, JobSpec, Node};
use meshgrove_plugin_api::{MeshData, Permission};
use meshgrove_plugins::jobs::{JobEvent, JobPool};
use meshgrove_plugins::{CancelToken, Error, LibraryAccess, Plugin, PluginGrants, Settings, Staging};
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
    let tools: Vec<&str> = second.tools().iter().map(|t| t.name.as_str()).collect();
    assert_eq!(tools, ["make_box", "measure", "spin", "trap"]);
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
    let tokens = pool.start(&plugin, jobs, panel.out(), &events);
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

/// A library of one box, for `measure`.
struct OneBox;

impl LibraryAccess for OneBox {
    fn search(&self, _query: &str, _limit: u32, _scope: LibraryRead) -> Result<Value, String> {
        Ok(json!([{"id": "Eigene/box.stl"}]))
    }

    fn describe(&self, id: &str, _scope: LibraryRead) -> Result<Value, String> {
        Ok(json!({"id": id}))
    }

    fn triangles(&self, id: &str, _max: u32, scope: LibraryRead) -> Result<MeshData, String> {
        assert_eq!(scope, LibraryRead::AiScope, "the host passes on what the user granted");
        if id == "Eigene/box.stl" { Ok(MeshData::cuboid([10.0, 20.0, 30.0])) } else { Err(format!("unknown id: {id}")) }
    }
}

#[test]
fn reading_the_library_needs_the_grant() {
    let Some(bytes) = wasm() else { return };
    let base = tempfile::tempdir().unwrap();
    let library: Option<Arc<dyn LibraryAccess>> = Some(Arc::new(OneBox));
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
