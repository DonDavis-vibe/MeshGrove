//! Drives the real `meshgrove-mcp` binary over stdin/stdout with plain JSON-RPC, as MCP clients
//! do, against a small library in a temporary folder.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, SystemTime};

use image::RgbaImage;
use meshgrove_core::ai::proposals::{ProposalKind, ProposalStore};
use meshgrove_core::cache::{ThumbCache, ThumbMeta, Thumbnail};
use meshgrove_core::config::{AiAccess, Config, LibraryAi, LibraryRead, PluginConfig, PluginGrants};
use meshgrove_core::db::Library;
use meshgrove_core::listing::Entry;
use meshgrove_core::{Kind, Mesh};
use serde_json::{Value, json};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    config: PathBuf,
    db: PathBuf,
    proposals: PathBuf,
    thumbs: PathBuf,
}

fn entry(dir: &Path, name: &str) -> Entry {
    Entry {
        name: name.into(),
        path: dir.join(name),
        kind: Kind::from_file_name(name),
        size: 684,
        modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1000)),
    }
}

/// eigene (cloud, pictures): rose.stl tagged "Garten"; kunden (local only): halter.stl.
fn fixture(ai_enabled: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("lib");
    let (eigene, kunden) = (root.join("eigene"), root.join("kunden"));
    let config = dir.path().join("config.toml");
    let db = dir.path().join("library.db");
    let proposals = dir.path().join("proposals.db");
    let thumbs = dir.path().join("thumbs");
    let mut c = Config {
        library_roots: vec![eigene.clone(), kunden.clone()],
        language: Some("de".into()),
        ..Config::default()
    };
    c.ai.enabled = ai_enabled;
    c.ai.libraries = vec![
        LibraryAi { root: eigene.clone(), access: AiAccess::Cloud, images: true },
        LibraryAi { root: kunden.clone(), access: AiAccess::LocalOnly, images: false },
    ];
    c.save_to(&config).unwrap();
    let mut lib = Library::open(&db).unwrap();
    let cache = ThumbCache::new(thumbs.clone());
    for (root, name, hash) in [(&eigene, "rose.stl", "h-rose"), (&kunden, "halter.stl", "h-halter")] {
        lib.finish_baseline(root).unwrap();
        lib.update_dir(root, root, 1, &[entry(root, name)], &[]).unwrap();
        let key = format!("key-{hash}");
        let meta = ThumbMeta {
            size_mm: Some([40.0, 20.0, 10.0]),
            content_hash: Some(hash.into()),
            checks: Some(Box::new(meshgrove_core::checks::check(&Mesh::cube(10.0)))),
            ..Default::default()
        };
        lib.store_meta(&root.join(name), &key, &meta).unwrap();
        cache
            .store(&key, &Thumbnail { image: RgbaImage::from_pixel(64, 64, image::Rgba([200, 100, 50, 255])), meta })
            .unwrap();
    }
    lib.tag(&[eigene.join("rose.stl")], "Garten").unwrap();
    Fixture { _dir: dir, root, config, db, proposals, thumbs }
}

struct Server {
    child: Child,
    stdin: ChildStdin,
    /// Lines from stdout, read on a thread of their own.
    stdout: std::sync::mpsc::Receiver<String>,
    next_id: i64,
}

/// Every line of `stdout`, as it comes.
fn lines(stdout: ChildStdout) -> std::sync::mpsc::Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

impl Server {
    fn start(f: &Fixture, extra: &[&str]) -> Server {
        // A home of its own: printers are not looked for in the slicers of whoever runs the tests.
        let home = f.config.parent().unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_meshgrove-mcp"))
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join("xdg-config"))
            .args(["--config", f.config.to_str().unwrap(), "--db", f.db.to_str().unwrap()])
            .args(["--proposals", f.proposals.to_str().unwrap(), "--thumbs", f.thumbs.to_str().unwrap()])
            .args(["--client", "test"])
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = lines(child.stdout.take().unwrap());
        let mut server = Server { child, stdin, stdout, next_id: 1 };
        // What Claude Desktop and others send today: an older protocol version with a handshake.
        let init = server.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}),
        );
        assert_eq!(init["serverInfo"]["name"], "meshgrove");
        assert!(init["instructions"].as_str().unwrap().contains("until the user accepts"));
        server.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        server
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.stdin, "{message}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// Sends a request and returns its result; every line on stdout must be a JSON-RPC message.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let message = self.next_message(Duration::from_secs(60)).expect("server closed stdout or timed out");
            if message["id"] == id {
                assert!(message.get("error").is_none(), "{method}: {message}");
                return message["result"].clone();
            }
        }
    }

    /// The next JSON-RPC message (every line on stdout must be one).
    fn next_message(&mut self, timeout: Duration) -> Option<Value> {
        let line = self.stdout.recv_timeout(timeout).ok()?;
        Some(serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON-RPC on stdout: {line:?} ({e})")))
    }

    /// Waits up to `timeout` for a notification `method`.
    fn notified(&mut self, method: &str, timeout: Duration) -> bool {
        let until = std::time::Instant::now() + timeout;
        while let Some(left) = until.checked_duration_since(std::time::Instant::now()) {
            match self.next_message(left) {
                Some(m) if m["method"] == method => return true,
                Some(_) => {}
                None => return false,
            }
        }
        false
    }

    fn call(&mut self, tool: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": tool, "arguments": arguments}))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn ids(result: &Value) -> Vec<String> {
    let mut ids: Vec<String> = result["structuredContent"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_owned())
        .collect();
    ids.sort();
    ids
}

#[test]
fn lists_tools_with_annotations_and_prompts() {
    let f = fixture(true);
    let mut s = Server::start(&f, &[]);
    let tools = s.request("tools/list", json!({}));
    let tools = tools["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for name in [
        "library_overview",
        "search_models",
        "get_model",
        "get_thumbnails",
        "list_inbox",
        "propose_tags",
        "propose_list",
    ] {
        assert!(names.contains(&name), "{name} in {names:?}");
    }
    for t in tools {
        let name = t["name"].as_str().unwrap();
        let read_only = t["annotations"]["readOnlyHint"].as_bool().unwrap();
        assert_eq!(read_only, !name.starts_with("propose_"), "{name}");
        assert_eq!(t["annotations"]["destructiveHint"], false, "{name}: suggestions destroy nothing");
    }
    let search = tools.iter().find(|t| t["name"] == "search_models").unwrap();
    assert!(search["inputSchema"]["properties"]["fits_printer"].is_object(), "filter fields have a schema");
    let prompts = s.request("prompts/list", json!({}));
    let prompts: Vec<&str> =
        prompts["prompts"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert!(prompts.contains(&"eingang-einordnen") && prompts.contains(&"druckliste-zusammenstellen"));
    let prompt = s.request("prompts/get", json!({"name": "eingang-einordnen", "arguments": {}}));
    assert!(prompt["messages"][0]["content"]["text"].as_str().unwrap().starts_with("Ordne die neuen Dateien"));
}

#[test]
fn each_access_class_sees_what_it_was_given() {
    let f = fixture(true);
    let mut cloud = Server::start(&f, &[]);
    assert_eq!(ids(&cloud.call("search_models", json!({}))), ["eigene/rose.stl"]);
    let hidden = cloud.call("get_model", json!({"id": "kunden/halter.stl"}));
    assert_eq!(hidden["isError"], true);
    assert!(hidden["content"][0]["text"].as_str().unwrap().contains("unknown id"));
    let mut local = Server::start(&f, &["--local"]);
    assert_eq!(ids(&local.call("search_models", json!({}))), ["eigene/rose.stl", "kunden/halter.stl"]);
    // Every answer's structured content is an object, also where the tool has a list to give:
    // clients that check it refuse an array (and the assistant sees nothing).
    for tool in ["find_duplicates", "list_inbox", "list_lists", "list_proposals"] {
        let answer = local.call(tool, json!({}));
        assert!(answer["structuredContent"]["items"].is_array(), "{tool}: {answer}");
    }
    let garden = local.call("search_models", json!({"q": "tag:garten", "fits_printer": "any"}));
    assert_eq!(garden["structuredContent"]["items"].as_array().unwrap().len(), 0, "no printers configured");
    let pictures = local.call("get_thumbnails", json!({"ids": ["eigene/rose.stl", "kunden/halter.stl"]}));
    assert_eq!(pictures["structuredContent"]["denied"], json!(["kunden/halter.stl"]), "no pictures of customer files");
    let images: Vec<&Value> = pictures["content"].as_array().unwrap().iter().filter(|c| c["type"] == "image").collect();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0]["mimeType"], "image/png");
    let model = local.call("get_model", json!({"id": "eigene/rose.stl", "include_thumbnail": true}));
    assert_eq!(model["structuredContent"]["path"], f.root.join("eigene").join("rose.stl").display().to_string());
    assert_eq!(model["structuredContent"]["checks"]["shells"], 1);
}

#[test]
fn suggestions_are_queued_not_applied() {
    let f = fixture(true);
    let mut s = Server::start(&f, &["--local"]);
    let receipt = s.call(
        "propose_tags",
        json!({"title": "Kunde einordnen", "items": [{"id": "kunden/halter.stl", "add": ["garden", "Halterung"]}], "mark_inbox_done": true}),
    );
    let r = &receipt["structuredContent"];
    assert_eq!(r["items"], 1, "{receipt}");
    assert_eq!(r["tags"], json!([{"name": "Garten", "status": "existing"}, {"name": "Halterung", "status": "new"}]));
    let lib = Library::open_reader(&f.db).unwrap();
    assert_eq!(lib.all_tags().unwrap(), [("Garten".to_owned(), 1)], "the library is unchanged");
    let store = ProposalStore::open(&f.proposals).unwrap();
    assert_eq!(store.pending_count().unwrap(), 1);
    let log = store.access_log(10).unwrap();
    assert!(
        log.iter()
            .any(|e| e.tool == "propose_tags" && e.client.as_deref() == Some("test") && e.access_class == "local")
    );
    let mine = s.call("list_proposals", json!({}));
    assert_eq!(mine["structuredContent"]["items"][0]["title"], "Kunde einordnen", "a list goes out as an object");
}

#[test]
fn says_what_is_wrong_instead_of_failing() {
    let off = fixture(false);
    let mut s = Server::start(&off, &[]);
    let r = s.call("library_overview", json!({}));
    assert_eq!(r["isError"], true);
    assert!(r["content"][0]["text"].as_str().unwrap().contains("switched off"));
    drop(s);

    let other = fixture(true);
    rusqlite_version(&other.db, 99);
    let mut s = Server::start(&other, &[]);
    let r = s.call("library_overview", json!({}));
    assert!(r["content"][0]["text"].as_str().unwrap().contains("another Meshgrove version"), "{r}");

    let empty = fixture(true);
    std::fs::remove_file(&empty.db).unwrap();
    let mut s = Server::start(&empty, &[]);
    let r = s.call("library_overview", json!({}));
    assert!(r["content"][0]["text"].as_str().unwrap().contains("not indexed yet"), "{r}");
}

/// Sets the index's layout version as another Meshgrove would have.
fn rusqlite_version(db: &Path, version: i32) {
    rusqlite::Connection::open(db).unwrap().pragma_update(None, "user_version", version).unwrap();
}

/// The example plugin (`scripts/build-plugins.sh --test-hooks`, or MESHGROVE_TEST_PLUGIN); without
/// it the test passes with a note, unless MESHGROVE_REQUIRE_PLUGIN=1 (as in CI).
fn test_plugin() -> Option<PathBuf> {
    let path = std::env::var_os("MESHGROVE_TEST_PLUGIN").map(PathBuf::from).unwrap_or_else(|| {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/plugins-test/wasm32-wasip2/release/hello_box.wasm")
    });
    if path.is_file() {
        return Some(path);
    }
    assert_ne!(std::env::var("MESHGROVE_REQUIRE_PLUGIN").as_deref(), Ok("1"), "{} is missing", path.display());
    eprintln!("skipped: no {} (scripts/build-plugins.sh --test-hooks)", path.display());
    None
}

/// Sets the plugin up in the fixture's config.toml, newer than before (the server loads
/// plugins again when the file changed).
fn set_plugin(f: &Fixture, wasm: &Path, enabled: bool) {
    set_plugin_with(f, wasm, enabled, LibraryRead::None, if enabled { 1 } else { 2 });
}

/// [`set_plugin`] with what it may read of the library; `later` seconds newer than now.
fn set_plugin_with(f: &Fixture, wasm: &Path, enabled: bool, library_read: LibraryRead, later: u64) {
    let mut config = Config::load_from(&f.config).unwrap();
    config.plugins = vec![PluginConfig {
        id: "hello-box".into(),
        file: wasm.to_path_buf(),
        blake3: blake3::hash(&std::fs::read(wasm).unwrap()).to_hex().to_string(),
        enabled,
        grants: PluginGrants { ai_tools: true, bundles: true, library_read, ..PluginGrants::default() },
        memory_mb: None,
    }];
    config.save_to(&f.config).unwrap();
    let later = SystemTime::now() + Duration::from_secs(later);
    std::fs::File::options().write(true).open(&f.config).unwrap().set_modified(later).unwrap();
}

#[test]
fn plugin_tools_are_offered_and_their_files_wait_as_a_suggestion() {
    let Some(wasm) = test_plugin() else { return };
    let f = fixture(true);
    set_plugin(&f, &wasm, true);
    let plugins = f.config.parent().unwrap().join("plugins");
    let mut s = Server::start(&f, &["--local", "--plugins", plugins.to_str().unwrap()]);
    let tools = s.request("tools/list", json!({}));
    let tools = tools["tools"].as_array().unwrap();
    let make = tools.iter().find(|t| t["name"] == "hello-box_make_box").expect("the plugin's tool is listed");
    assert_eq!(make["annotations"]["readOnlyHint"], false, "it makes files");
    assert!(make["inputSchema"]["properties"]["width"].is_object());
    let measure = tools.iter().find(|t| t["name"] == "hello-box_measure").unwrap();
    assert_eq!(measure["annotations"]["readOnlyHint"], true);

    let r = s.call("hello-box_make_box", json!({"width": 30, "depth": 20, "height": 10, "name": "Kiste", "copies": 2}));
    assert_ne!(r["isError"], true, "{r}");
    let answer = &r["structuredContent"];
    assert_eq!(answer["plugin"], "hello-box");
    let batch = answer["batch"].as_str().expect("a suggestion");
    let lib = Library::open_reader(&f.db).unwrap();
    assert_eq!(lib.all_tags().unwrap(), [("Garten".to_owned(), 1)], "the library is unchanged");
    let store = ProposalStore::open(&f.proposals).unwrap();
    let id = store.batch_by_uid(batch).unwrap().unwrap();
    let b = store.batches(None, None, &["pending"], 10).unwrap().into_iter().find(|b| b.id == id).unwrap();
    assert_eq!((b.kind.as_str(), b.source.as_str()), ("bundle", "mcp"));
    let proposals = store.proposals(id).unwrap();
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].kind, ProposalKind::Bundle);
    assert_eq!(proposals[0].payload["plugin"], "hello-box");
    let files = proposals[0].payload["files"].as_array().unwrap();
    assert!(files.iter().any(|f| f["rel"].as_str().unwrap().ends_with(".stl")), "{files:?}");
    assert!(store.access_log(10).unwrap().iter().any(|e| e.tool == "hello-box_make_box"));

    // Switched off in Meshgrove: the client is told, and the tool is gone.
    set_plugin(&f, &wasm, false);
    assert!(s.notified("notifications/tools/list_changed", Duration::from_secs(10)), "no list_changed");
    let tools = s.request("tools/list", json!({}));
    assert!(!tools["tools"].as_array().unwrap().iter().any(|t| t["name"] == "hello-box_make_box"), "{tools}");
    let r = s.call("hello-box_make_box", json!({"width": 30, "depth": 20, "height": 10, "name": "Kiste"}));
    assert_eq!(r["isError"], true);
}

/// A tetrahedron 40 × 20 × 10 mm, as ASCII STL.
const TETRA: &str = "solid t
facet normal 0 0 0
outer loop
vertex 0 0 0
vertex 40 0 0
vertex 0 20 0
endloop
endfacet
facet normal 0 0 0
outer loop
vertex 0 0 0
vertex 0 20 0
vertex 0 0 10
endloop
endfacet
facet normal 0 0 0
outer loop
vertex 0 0 0
vertex 0 0 10
vertex 40 0 0
endloop
endfacet
facet normal 0 0 0
outer loop
vertex 40 0 0
vertex 0 0 10
vertex 0 20 0
endloop
endfacet
endsolid t
";

#[test]
fn plugins_read_the_library_only_with_the_grant_and_within_the_scope() {
    let Some(wasm) = test_plugin() else { return };
    let f = fixture(true);
    std::fs::create_dir_all(f.root.join("eigene")).unwrap();
    std::fs::create_dir_all(f.root.join("kunden")).unwrap();
    std::fs::write(f.root.join("eigene").join("rose.stl"), TETRA).unwrap();
    std::fs::write(f.root.join("kunden").join("halter.stl"), TETRA).unwrap();
    set_plugin_with(&f, &wasm, true, LibraryRead::AiScope, 1);
    let plugins = f.config.parent().unwrap().join("plugins");
    let mut cloud = Server::start(&f, &["--plugins", plugins.to_str().unwrap()]);
    let r = cloud.call("hello-box_measure", json!({"id": "eigene/rose.stl"}));
    assert_ne!(r["isError"], true, "{r}");
    assert_eq!(r["structuredContent"]["result"], json!({"size_mm": [40.0, 20.0, 10.0], "triangles": 4}));
    let hidden = cloud.call("hello-box_measure", json!({"id": "kunden/halter.stl"}));
    assert_eq!(hidden["isError"], true, "customer files are local only: {hidden}");
    assert!(hidden["content"][0]["text"].as_str().unwrap().contains("unknown id"), "{hidden}");
    let mut local = Server::start(&f, &["--local", "--plugins", plugins.to_str().unwrap()]);
    let r = local.call("hello-box_measure", json!({"id": "kunden/halter.stl"}));
    assert_ne!(r["isError"], true, "a local AI sees them: {r}");

    set_plugin_with(&f, &wasm, true, LibraryRead::None, 3);
    let denied = local.call("hello-box_measure", json!({"id": "eigene/rose.stl"}));
    assert_eq!(denied["isError"], true);
    assert!(denied["content"][0]["text"].as_str().unwrap().contains("not allowed"), "{denied}");
}
