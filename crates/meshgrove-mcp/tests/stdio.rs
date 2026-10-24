//! Drives the real `meshgrove-mcp` binary over stdin/stdout with plain JSON-RPC, as MCP clients
//! do, against a small library in a temporary folder.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, SystemTime};

use image::RgbaImage;
use meshgrove_core::ai::proposals::ProposalStore;
use meshgrove_core::cache::{ThumbCache, ThumbMeta, Thumbnail};
use meshgrove_core::config::{AiAccess, Config, LibraryAi};
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
    let mut c = Config { library_roots: vec![eigene.clone(), kunden.clone()], language: Some("de".into()), ..Config::default() };
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
        cache.store(&key, &Thumbnail { image: RgbaImage::from_pixel(64, 64, image::Rgba([200, 100, 50, 255])), meta }).unwrap();
    }
    lib.tag(&[eigene.join("rose.stl")], "Garten").unwrap();
    Fixture { _dir: dir, root, config, db, proposals, thumbs }
}

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Server {
    fn start(f: &Fixture, extra: &[&str]) -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_meshgrove-mcp"))
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
        let stdout = BufReader::new(child.stdout.take().unwrap());
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
            let mut line = String::new();
            assert!(self.stdout.read_line(&mut line).unwrap() > 0, "server closed stdout");
            let message: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON-RPC on stdout: {line:?} ({e})"));
            if message["id"] == id {
                assert!(message.get("error").is_none(), "{method}: {message}");
                return message["result"].clone();
            }
        }
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
    let mut ids: Vec<String> =
        result["structuredContent"]["items"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap().to_owned()).collect();
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
    for name in ["library_overview", "search_models", "get_model", "get_thumbnails", "list_inbox", "propose_tags", "propose_list"] {
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
    let prompts: Vec<&str> = prompts["prompts"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
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
    let garden = local.call("search_models", json!({"q": "tag:garten", "fits_printer": "any"}));
    assert_eq!(garden["structuredContent"]["items"].as_array().unwrap().len(), 0, "no printers configured");
    let pictures = local.call("get_thumbnails", json!({"ids": ["eigene/rose.stl", "kunden/halter.stl"]}));
    assert_eq!(pictures["structuredContent"]["denied"], json!(["kunden/halter.stl"]), "no pictures of customer files");
    let images: Vec<&Value> = pictures["content"].as_array().unwrap().iter().filter(|c| c["type"] == "image").collect();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0]["mimeType"], "image/png");
    let model = local.call("get_model", json!({"id": "eigene/rose.stl", "include_thumbnail": true}));
    assert_eq!(model["structuredContent"]["path"], f.root.join("eigene/rose.stl").display().to_string());
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
    assert!(log.iter().any(|e| e.tool == "propose_tags" && e.client.as_deref() == Some("test") && e.access_class == "local"));
    let mine = s.call("list_proposals", json!({}));
    assert_eq!(mine["structuredContent"][0]["title"], "Kunde einordnen");
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
