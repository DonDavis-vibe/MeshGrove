//! Connecting Meshgrove to the AI assistants installed on this computer: each one gets a
//! `meshgrove` MCP server entry that starts `meshgrove-mcp`, so nobody has to copy anything.
//!
//! The assistants' own files are edited in place, not rewritten: only the `meshgrove` entry
//! changes, every other byte (other servers, other settings, comments, key order) stays as it
//! was. A file is copied to `<name>.bak` before its first change. A file that cannot be read as
//! JSON (with comments) or TOML is left alone, and so is an entry the user changed (it starts
//! another program, or other arguments); an entry that only has an old path to `meshgrove-mcp`
//! (after an update or a move) gets the new one. An assistant counts as installed when its
//! configuration folder exists; no assistant program is started.
//!
//! Adapted from SolidCraft's `crates/engine/src/agent_setup.rs`
//! (<https://github.com/tangency-labs/solidcraft>), used under its MIT license:
//!
//! > Copyright (c) 2026 SolidCraft contributors
//! >
//! > Permission is hereby granted, free of charge, to any person obtaining a copy of this software
//! > and associated documentation files (the "Software"), to deal in the Software without
//! > restriction, including without limitation the rights to use, copy, modify, merge, publish,
//! > distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the
//! > Software is furnished to do so, subject to the following conditions:
//! >
//! > The above copyright notice and this permission notice shall be included in all copies or
//! > substantial portions of the Software.
//! >
//! > THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING
//! > BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
//! > NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
//! > DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! > OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// The name of the server entry in every assistant.
pub const SERVER_NAME: &str = "meshgrove";

/// The assistants Meshgrove connects to: id (`meshgrove-mcp --client`) and name.
pub const CLIENTS: &[(&str, &str)] = &[
    ("claude-desktop", "Claude Desktop"),
    ("claude-code", "Claude Code"),
    ("lm-studio", "LM Studio"),
    ("cursor", "Cursor"),
    ("vscode", "VS Code"),
    ("codex", "Codex"),
    ("gemini", "Gemini CLI"),
    ("copilot", "Copilot CLI"),
    ("windsurf", "Windsurf"),
    ("zed", "Zed"),
    ("opencode", "OpenCode"),
];

/// How an assistant is named to the user.
pub fn client_name(id: &str) -> &str {
    CLIENTS.iter().find(|(i, _)| *i == id).map_or(id, |(_, name)| name)
}

/// Assistants whose models run on this computer: their entry gets `--local`, so they see the
/// libraries opened to local AI only, too.
fn runs_locally(client: &str) -> bool {
    client == "lm-studio"
}

/// Where the assistants keep their configuration, read from the environment so tests can point
/// it at a temporary folder.
pub struct Env {
    pub home: Option<PathBuf>,
    pub appdata: Option<PathBuf>,
    pub local_appdata: Option<PathBuf>,
    pub xdg_config: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
}

impl Env {
    pub fn from_process() -> Env {
        let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let home = var("HOME").or_else(|| var("USERPROFILE"));
        Env {
            home,
            appdata: var("APPDATA"),
            local_appdata: var("LOCALAPPDATA"),
            xdg_config: var("XDG_CONFIG_HOME"),
            codex_home: var("CODEX_HOME"),
        }
    }

    /// `$XDG_CONFIG_HOME`, else `~/.config`.
    fn config_home(&self) -> Option<PathBuf> {
        self.xdg_config.clone().or_else(|| self.home.as_ref().map(|h| h.join(".config")))
    }

    fn in_home(&self, rel: &str) -> Option<PathBuf> {
        self.home.as_ref().map(|h| h.join(rel))
    }
}

/// What starts the server: the program and the arguments before Meshgrove's own (`mcp` for an
/// AppImage, which runs `meshgrove-mcp` as a subcommand since the binary inside has another path
/// on every run).
#[derive(Debug, Clone, PartialEq)]
pub struct Server {
    pub program: String,
    pub lead: Vec<String>,
}

impl Server {
    pub fn new(exe: &Path, appimage: Option<&Path>) -> Server {
        match appimage {
            Some(image) => Server { program: plain_path(image), lead: vec!["mcp".into()] },
            None => Server { program: plain_path(exe), lead: Vec::new() },
        }
    }

    /// The arguments for one assistant: `[--local] --client ID`.
    pub fn args(&self, client: &str, local: bool) -> Vec<String> {
        let mut args = self.lead.clone();
        if local {
            args.push("--local".into());
        }
        args.extend(["--client".to_owned(), client.to_owned()]);
        args
    }
}

/// A path as text, without the `\\?\` prefix of a canonicalized Windows path (some clients
/// mishandle it).
pub fn plain_path(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => rest.to_owned(),
        _ => s,
    }
}

/// How an assistant spells a server entry and where its list is.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Shape {
    /// `"mcpServers": {name: {"command": exe, "args": [...]}}` (Claude Desktop, LM Studio, Cursor,
    /// Gemini CLI, Windsurf)
    McpServers,
    /// Claude Code's `~/.claude.json`: the same with `"type": "stdio"`.
    ClaudeCode,
    /// GitHub Copilot CLI: the same with `"type": "local"` and `"tools": ["*"]`.
    Copilot,
    /// VS Code's user `mcp.json`: `"servers": {name: {"type": "stdio", ...}}`.
    VsCode,
    /// Zed's `settings.json`: `"context_servers": {name: {"command": exe, "args": [...]}}`.
    Zed,
    /// OpenCode: `"mcp": {name: {"type": "local", "command": [exe, ...], "enabled": true}}`.
    OpenCode,
    /// Codex: `[mcp_servers.name]` with `command` and `args`, in `config.toml`.
    Codex,
}

impl Shape {
    fn list_key(self) -> &'static str {
        match self {
            Shape::McpServers | Shape::ClaudeCode | Shape::Copilot => "mcpServers",
            Shape::VsCode => "servers",
            Shape::Zed => "context_servers",
            Shape::OpenCode => "mcp",
            Shape::Codex => "mcp_servers",
        }
    }

    fn entry(self, program: &str, args: &[String]) -> Value {
        match self {
            Shape::McpServers | Shape::Zed | Shape::Codex => json!({"command": program, "args": args}),
            Shape::ClaudeCode | Shape::VsCode => json!({"type": "stdio", "command": program, "args": args}),
            Shape::Copilot => json!({"type": "local", "command": program, "args": args, "tools": ["*"]}),
            Shape::OpenCode => {
                let mut cmd = vec![program.to_owned()];
                cmd.extend_from_slice(args);
                json!({"type": "local", "command": cmd, "enabled": true})
            }
        }
    }
}

/// One assistant's configuration file found on this computer.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub client: &'static str,
    pub file: PathBuf,
    shape: Shape,
}

/// Claude Desktop's configuration folders: the regular one, and the Microsoft Store (MSIX)
/// package's redirected copy on Windows.
fn claude_desktop_dirs(env: &Env) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(a) = &env.appdata {
        dirs.push(a.join("Claude"));
    }
    if cfg!(target_os = "macos")
        && let Some(d) = env.in_home("Library/Application Support/Claude")
    {
        dirs.push(d);
    }
    if cfg!(target_os = "linux")
        && let Some(c) = env.config_home()
    {
        dirs.push(c.join("Claude"));
    }
    if let Some(l) = &env.local_appdata
        && let Ok(rd) = std::fs::read_dir(l.join("Packages"))
    {
        let mut pk: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("Claude_"))
            .map(|e| e.path().join("LocalCache/Roaming/Claude"))
            .collect();
        pk.sort();
        dirs.extend(pk);
    }
    dirs
}

/// The folder VS Code keeps its user settings in.
fn vscode_dir(env: &Env) -> Option<PathBuf> {
    if cfg!(windows) {
        env.appdata.as_ref().map(|a| a.join("Code/User"))
    } else if cfg!(target_os = "macos") {
        env.in_home("Library/Application Support/Code/User")
    } else {
        env.config_home().map(|c| c.join("Code/User"))
    }
}

/// The assistants found on this computer: those whose configuration folder exists.
pub fn detect(env: &Env) -> Vec<Target> {
    let mut v = Vec::new();
    fn add(v: &mut Vec<Target>, client: &'static str, dir: Option<PathBuf>, file: &str, shape: Shape) {
        if let Some(dir) = dir.filter(|d| d.is_dir()) {
            v.push(Target { client, file: dir.join(file), shape });
        }
    }
    for dir in claude_desktop_dirs(env) {
        add(&mut v, "claude-desktop", Some(dir), "claude_desktop_config.json", Shape::McpServers);
    }
    if let Some(h) = &env.home
        && (h.join(".claude.json").is_file() || h.join(".claude").is_dir())
    {
        v.push(Target { client: "claude-code", file: h.join(".claude.json"), shape: Shape::ClaudeCode });
    }
    add(&mut v, "lm-studio", env.in_home(".lmstudio"), "mcp.json", Shape::McpServers);
    add(&mut v, "cursor", env.in_home(".cursor"), "mcp.json", Shape::McpServers);
    add(&mut v, "vscode", vscode_dir(env), "mcp.json", Shape::VsCode);
    add(&mut v, "codex", env.codex_home.clone().or_else(|| env.in_home(".codex")), "config.toml", Shape::Codex);
    add(&mut v, "gemini", env.in_home(".gemini"), "settings.json", Shape::McpServers);
    add(&mut v, "copilot", env.in_home(".copilot"), "mcp-config.json", Shape::Copilot);
    add(&mut v, "windsurf", env.in_home(".codeium/windsurf"), "mcp_config.json", Shape::McpServers);
    let zed = if cfg!(windows) {
        env.appdata.as_ref().map(|a| a.join("Zed"))
    } else {
        env.config_home().map(|c| c.join("zed"))
    };
    add(&mut v, "zed", zed, "settings.json", Shape::Zed);
    let opencode = env.config_home().map(|c| c.join("opencode"));
    // opencode.jsonc is read too (comments allowed); keep to the one the user has.
    let file =
        if opencode.as_ref().is_some_and(|d| !d.join("opencode.json").exists() && d.join("opencode.jsonc").exists()) {
            "opencode.jsonc"
        } else {
            "opencode.json"
        };
    add(&mut v, "opencode", opencode, file, Shape::OpenCode);
    v
}

// -------------------------------------------------------------------------------------------------
// What an existing entry is.

/// What connecting does about the entry.
#[derive(Debug, PartialEq)]
enum Verdict {
    /// No entry: add it.
    Add,
    /// The entry is what would be written.
    Same,
    /// Ours, with an older path to `meshgrove-mcp`: update it.
    Stale,
    /// Something the user wrote or changed: leave it.
    Custom,
}

/// Whether `command` (with `args`) starts Meshgrove's MCP server: `meshgrove-mcp`, or a Meshgrove
/// AppImage given `mcp`.
fn is_meshgrove_server(command: &str, args: &[String]) -> bool {
    let name = command.rsplit(['/', '\\']).next().unwrap_or(command).to_ascii_lowercase();
    let stem = name.strip_suffix(".exe").unwrap_or(&name);
    stem == "meshgrove-mcp"
        || (stem.ends_with(".appimage") && stem.contains("meshgrove") && args.first().is_some_and(|a| a == "mcp"))
}

/// The command and arguments of an entry, whether it spells them as `command` + `args` or as
/// OpenCode's one `command` array.
fn command_and_args(entry: &Value) -> Option<(String, Vec<String>)> {
    let strings =
        |v: &Value| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect::<Vec<_>>());
    match entry.get("command")? {
        Value::String(c) => Some((c.clone(), entry.get("args").and_then(strings).unwrap_or_default())),
        Value::Array(_) => {
            let mut all = strings(entry.get("command")?)?;
            (!all.is_empty()).then(|| (all.remove(0), all))
        }
        _ => None,
    }
}

fn judge(existing: Option<&Value>, program: &str, args: &[String]) -> Verdict {
    let Some(existing) = existing else { return Verdict::Add };
    let Some((cmd, got)) = command_and_args(existing) else { return Verdict::Custom };
    if got != args {
        return Verdict::Custom;
    }
    if cmd == program {
        Verdict::Same
    } else if is_meshgrove_server(&cmd, &got) {
        Verdict::Stale
    } else {
        Verdict::Custom
    }
}

/// `existing` with its command replaced by `program`, every other field kept.
fn with_command(existing: &Value, program: &str) -> Value {
    let mut v = existing.clone();
    match v.get_mut("command") {
        Some(Value::Array(a)) if !a.is_empty() => a[0] = json!(program),
        Some(c) => *c = json!(program),
        None => {}
    }
    v
}

// -------------------------------------------------------------------------------------------------
// Editing JSON (with comments and trailing commas, as VS Code, Zed and OpenCode accept) in place.

/// One member of an object in the text: its key and the byte range of its value.
struct Member {
    key: String,
    key_at: usize,
    val: (usize, usize),
}

/// An object in the text: the offsets of its braces and its members.
struct Obj {
    open: usize,
    close: usize,
    members: Vec<Member>,
}

fn skip_ws(b: &[u8], mut i: usize) -> Result<usize, String> {
    loop {
        match b.get(i) {
            Some(c) if c.is_ascii_whitespace() => i += 1,
            Some(b'/') if b.get(i + 1) == Some(&b'/') => {
                while b.get(i).is_some_and(|&c| c != b'\n') {
                    i += 1;
                }
            }
            Some(b'/') if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while !(b.get(i) == Some(&b'*') && b.get(i + 1) == Some(&b'/')) {
                    if i >= b.len() {
                        return Err("an unterminated comment".into());
                    }
                    i += 1;
                }
                i += 2;
            }
            _ => return Ok(i),
        }
    }
}

fn scan_string(b: &[u8], mut i: usize) -> Result<usize, String> {
    i += 1;
    while let Some(&c) = b.get(i) {
        match c {
            b'\\' => i += 2,
            b'"' => return Ok(i + 1),
            _ => i += 1,
        }
    }
    Err("an unterminated string".into())
}

/// The end of the value starting at `i`.
fn scan_value(b: &[u8], i: usize, depth: usize) -> Result<usize, String> {
    if depth > 64 {
        return Err("nesting too deep".into());
    }
    match b.get(i) {
        Some(b'"') => scan_string(b, i),
        Some(b'{') => Ok(scan_object(b, i, depth)?.close + 1),
        Some(b'[') => {
            let mut j = skip_ws(b, i + 1)?;
            loop {
                match b.get(j) {
                    Some(b']') => return Ok(j + 1),
                    Some(b',') => j = skip_ws(b, j + 1)?,
                    Some(_) => j = skip_ws(b, scan_value(b, j, depth + 1)?)?,
                    None => return Err("an unterminated array".into()),
                }
            }
        }
        Some(c) if !matches!(c, b',' | b'}' | b']' | b':') => {
            let mut j = i;
            while b.get(j).is_some_and(|c| !c.is_ascii_whitespace() && !matches!(c, b',' | b'}' | b']' | b'/')) {
                j += 1;
            }
            Ok(j)
        }
        _ => Err(format!("a value was expected at byte {i}")),
    }
}

fn scan_object(b: &[u8], open: usize, depth: usize) -> Result<Obj, String> {
    let mut members = Vec::new();
    let mut i = skip_ws(b, open + 1)?;
    loop {
        match b.get(i) {
            Some(b'}') => return Ok(Obj { open, close: i, members }),
            Some(b',') => i = skip_ws(b, i + 1)?,
            Some(b'"') => {
                let end = scan_string(b, i)?;
                let key: String =
                    serde_json::from_slice(b.get(i..end).unwrap_or_default()).map_err(|e| e.to_string())?;
                let colon = skip_ws(b, end)?;
                if b.get(colon) != Some(&b':') {
                    return Err(format!("a `:` was expected at byte {colon}"));
                }
                let vs = skip_ws(b, colon + 1)?;
                let ve = scan_value(b, vs, depth + 1)?;
                members.push(Member { key, key_at: i, val: (vs, ve) });
                i = skip_ws(b, ve)?;
            }
            _ => return Err(format!("a key or `}}` was expected at byte {i}")),
        }
    }
}

/// The top-level object of a JSON document, with the byte after it checked to hold nothing else.
fn root_object(text: &str) -> Result<Obj, String> {
    let b = text.as_bytes();
    let i = skip_ws(b, 0)?;
    if b.get(i) != Some(&b'{') {
        return Err("the top level is not a JSON object".into());
    }
    let obj = scan_object(b, i, 0)?;
    if skip_ws(b, obj.close + 1)? != b.len() {
        return Err("text after the top-level object".into());
    }
    Ok(obj)
}

/// The indentation of the line holding byte `at`.
fn line_indent(text: &str, at: usize) -> String {
    let start = text.get(..at).and_then(|t| t.rfind('\n')).map_or(0, |n| n + 1);
    text.get(start..).unwrap_or_default().chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

/// `value` pretty-printed, its lines after the first indented by `indent`.
fn pretty_at(value: &Value, indent: &str) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default().replace('\n', &format!("\n{indent}"))
}

/// Insert `"key": value` as the last member of `obj`.
fn insert_member(text: &mut String, obj: &Obj, key: &str, value: &Value) {
    let closing_indent = line_indent(text, obj.open);
    let member_indent = match obj.members.first() {
        Some(m) => line_indent(text, m.key_at),
        None => format!("{closing_indent}  "),
    };
    let k = serde_json::to_string(key).unwrap_or_default();
    let member = format!("{k}: {}", pretty_at(value, &member_indent));
    match obj.members.last() {
        Some(last) => text.insert_str(last.val.1, &format!(",\n{member_indent}{member}")),
        None => {
            let inner = format!("\n{member_indent}{member}\n{closing_indent}");
            text.replace_range(obj.open + 1..obj.close, &inner);
        }
    }
}

/// Delete member `i` of `obj` from the text, with its comma.
fn remove_member(text: &mut String, obj: &Obj, i: usize) {
    let Some(m) = obj.members.get(i) else { return };
    if let Some(next) = obj.members.get(i + 1) {
        text.replace_range(m.key_at..next.key_at, "");
    } else if i > 0
        && let Some(prev) = obj.members.get(i - 1)
    {
        text.replace_range(prev.val.1..m.val.1, "");
    } else {
        text.replace_range(obj.open + 1..obj.close, "");
    }
}

/// What happened (or, trying it out, would happen) to the entry in one assistant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Added,
    /// Ours, with the path updated.
    Updated,
    /// Already set up.
    Unchanged,
    Removed,
    /// The user's own entry, left as it is.
    Custom,
}

/// What a run does with the entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Add,
    /// Take out the entries Meshgrove wrote (the user's own stay).
    Remove,
}

/// Set the entry in the JSON text, in the object under the top-level list key. `None`: nothing to
/// do (removing, and there was no entry).
fn edit_json(
    text: &mut String,
    shape: Shape,
    program: &str,
    args: &[String],
    mode: Mode,
) -> Result<Option<Change>, String> {
    let key = shape.list_key();
    if text.trim().is_empty() {
        if mode == Mode::Remove {
            return Ok(None);
        }
        *text = "{}\n".into();
    }
    let root = root_object(text)?;
    // The list object: found, or created holding nothing yet.
    let list_open = match root.members.iter().find(|m| m.key == key) {
        Some(m) if text.as_bytes().get(m.val.0) == Some(&b'{') => m.val.0,
        Some(_) => return Err(format!("`{key}` is not a JSON object")),
        None if mode == Mode::Remove => return Ok(None),
        None => {
            insert_member(text, &root, key, &json!({}));
            let root = root_object(text)?;
            root.members.iter().find(|m| m.key == key).map_or(0, |m| m.val.0)
        }
    };
    let new = shape.entry(program, args);
    let obj = scan_object(text.as_bytes(), list_open, 0)?;
    let existing = obj.members.iter().find(|m| m.key == SERVER_NAME);
    let parsed =
        existing.and_then(|m| serde_json::from_str::<Value>(text.get(m.val.0..m.val.1).unwrap_or_default()).ok());
    // An entry that is not even valid JSON (a comment inside it) is the user's.
    let verdict = match (existing, &parsed) {
        (Some(_), None) => Verdict::Custom,
        _ => judge(parsed.as_ref(), program, args),
    };
    let change = match (verdict, existing) {
        (Verdict::Add, _) if mode == Mode::Remove => return Ok(None),
        (Verdict::Same | Verdict::Stale, Some(_)) if mode == Mode::Remove => {
            let at = obj.members.iter().position(|m| m.key == SERVER_NAME).unwrap_or(0);
            remove_member(text, &obj, at);
            Change::Removed
        }
        (Verdict::Add, _) => {
            insert_member(text, &obj, SERVER_NAME, &new);
            Change::Added
        }
        (Verdict::Stale, Some(m)) => {
            let updated = with_command(parsed.as_ref().unwrap_or(&new), program);
            let indent = line_indent(text, m.val.0);
            text.replace_range(m.val.0..m.val.1, &pretty_at(&updated, &indent));
            Change::Updated
        }
        (Verdict::Same, _) => Change::Unchanged,
        _ => Change::Custom,
    };
    if !text.ends_with('\n') {
        text.push('\n');
    }
    Ok(Some(change))
}

// -------------------------------------------------------------------------------------------------
// Editing Codex's TOML in place.

fn edit_toml(text: &mut String, program: &str, args: &[String], mode: Mode) -> Result<Option<Change>, String> {
    use toml_edit::{Array, DocumentMut, Item, Table, value};
    let mut doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    if doc.get("mcp_servers").is_some_and(|i| !i.is_table()) {
        return Err("`mcp_servers` is not a table".into());
    }
    let existing = doc.get("mcp_servers").and_then(|t| t.get(SERVER_NAME)).map(|i| {
        i.as_table().map(toml_to_json).or_else(|| i.as_inline_table().map(|t| toml_to_json(&t.clone().into_table())))
    });
    let existing = match existing {
        Some(Some(v)) => Some(v),
        Some(None) => return Ok(Some(Change::Custom)),
        None => None,
    };
    let change = match judge(existing.as_ref(), program, args) {
        Verdict::Add if mode == Mode::Remove => return Ok(None),
        Verdict::Same | Verdict::Stale if mode == Mode::Remove => {
            if let Some(t) = doc.get_mut("mcp_servers").and_then(Item::as_table_mut) {
                t.remove(SERVER_NAME);
            }
            Change::Removed
        }
        Verdict::Add => {
            let mut t = Table::new();
            t["command"] = value(program);
            let mut a = Array::new();
            for x in args {
                a.push(x.as_str());
            }
            t["args"] = value(a);
            let servers = doc.entry("mcp_servers").or_insert_with(|| {
                let mut t = Table::new();
                t.set_implicit(true);
                Item::Table(t)
            });
            if let Some(s) = servers.as_table_mut() {
                s.insert(SERVER_NAME, Item::Table(t));
            }
            Change::Added
        }
        Verdict::Stale => {
            if let Some(entry) = doc.get_mut("mcp_servers").and_then(|t| t.get_mut(SERVER_NAME)) {
                entry["command"] = value(program);
            }
            Change::Updated
        }
        Verdict::Same => Change::Unchanged,
        Verdict::Custom => Change::Custom,
    };
    *text = doc.to_string();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    Ok(Some(change))
}

/// The `command` and `args` of a TOML table, as JSON for `judge`.
fn toml_to_json(t: &toml_edit::Table) -> Value {
    let mut m = serde_json::Map::new();
    if let Some(c) = t.get("command").and_then(|c| c.as_str()) {
        m.insert("command".into(), json!(c));
    }
    if let Some(a) = t.get("args").and_then(|a| a.as_array()) {
        m.insert("args".into(), Value::Array(a.iter().filter_map(|x| x.as_str().map(|s| json!(s))).collect()));
    }
    Value::Object(m)
}

// -------------------------------------------------------------------------------------------------

/// One assistant's outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub client: &'static str,
    pub file: PathBuf,
    /// `None`: nothing to do (removing where there was no entry).
    pub result: Result<Option<Change>, String>,
}

impl Outcome {
    pub fn name(&self) -> &str {
        client_name(self.client)
    }
}

fn configure(t: &Target, server: &Server, mode: Mode, dry_run: bool) -> Result<Option<Change>, String> {
    let existing = match std::fs::read_to_string(&t.file) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let args = server.args(t.client, runs_locally(t.client));
    let mut text = existing.clone().unwrap_or_default();
    let change = if t.shape == Shape::Codex {
        edit_toml(&mut text, &server.program, &args, mode)
    } else {
        edit_json(&mut text, t.shape, &server.program, &args, mode)
    }?;
    if dry_run || existing.as_deref() == Some(text.as_str()) || change.is_none() {
        return Ok(change);
    }
    if existing.is_some() {
        let mut bak = t.file.clone().into_os_string();
        bak.push(".bak");
        // The first backup is the user's original: later runs keep it.
        if !Path::new(&bak).exists() {
            std::fs::copy(&t.file, &bak).map_err(|e| format!("cannot back it up: {e}"))?;
        }
    } else if let Some(dir) = t.file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    write_atomic(&t.file, &text)?;
    Ok(change)
}

/// Writes through a temporary file in the same folder, so an assistant reading the file at the
/// same moment never sees half of it.
fn write_atomic(file: &Path, text: &str) -> Result<(), String> {
    let mut tmp = file.as_os_str().to_owned();
    tmp.push(format!(".meshgrove-{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    // Keep the original's permissions (Claude Code's file is private to the user).
    if let Ok(meta) = std::fs::metadata(file) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, file).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

/// Connects (or, with [`Mode::Remove`], disconnects) every assistant found, or only `only`.
/// `dry_run` changes nothing and says what would happen. A failing assistant does not stop the
/// others.
pub fn run(env: &Env, server: &Server, mode: Mode, dry_run: bool, only: Option<&str>) -> Vec<Outcome> {
    detect(env)
        .into_iter()
        .filter(|t| only.is_none_or(|o| o == t.client))
        .map(|t| {
            let result = configure(&t, server, mode, dry_run);
            Outcome { client: t.client, file: t.file, result }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "/opt/meshgrove/meshgrove-mcp";

    fn server() -> Server {
        Server::new(Path::new(EXE), None)
    }

    fn args(client: &str) -> Vec<String> {
        server().args(client, false)
    }

    fn edit(text: &str, shape: Shape) -> (String, Option<Change>) {
        let mut t = text.to_owned();
        let c = edit_json(&mut t, shape, EXE, &args("cursor"), Mode::Add).unwrap();
        (t, c)
    }

    #[test]
    fn adds_to_an_empty_file_and_keeps_everything_else_byte_for_byte() {
        let (t, c) = edit("", Shape::McpServers);
        assert_eq!(c, Some(Change::Added));
        let v: Value = serde_json::from_str(&t).unwrap();
        assert_eq!(v["mcpServers"]["meshgrove"], json!({"command": EXE, "args": ["--client", "cursor"]}));

        let src = "{\n    \"zeta\": 1, // keep\n    \"mcpServers\": {\n        \"other\": {\"command\": \"o\"}\n    },\n    \"alpha\": [1,2]\n}\n";
        let (t, _) = edit(src, Shape::McpServers);
        assert!(
            t.starts_with("{\n    \"zeta\": 1, // keep\n    \"mcpServers\": {\n        \"other\": {\"command\": \"o\"},\n        \"meshgrove\": {"),
            "{t}"
        );
        assert!(t.ends_with("    },\n    \"alpha\": [1,2]\n}\n"), "{t}");
    }

    #[test]
    fn a_file_with_comments_and_a_missing_list_gets_the_list() {
        let (t, _) = edit("// my settings\n{\n  \"theme\": \"x\", /* c */\n}\n", Shape::Zed);
        assert!(t.contains("// my settings") && t.contains("/* c */"), "{t}");
        assert!(t.contains("\"context_servers\""), "{t}");
    }

    #[test]
    fn stale_path_is_updated_and_custom_entries_are_not() {
        let old = r#"{"mcpServers": {"meshgrove": {"command": "C:\\Old\\meshgrove-mcp.exe", "args": ["--client", "cursor"], "env": {"A": "1"}}}}"#;
        let (t, c) = edit(old, Shape::McpServers);
        assert_eq!(c, Some(Change::Updated));
        let v: Value = serde_json::from_str(&t).unwrap();
        assert_eq!(v["mcpServers"]["meshgrove"]["command"], EXE);
        assert_eq!(v["mcpServers"]["meshgrove"]["env"], json!({"A": "1"}), "other fields stay");

        // The user made it local: theirs.
        let custom = r#"{"mcpServers": {"meshgrove": {"command": "/x/meshgrove-mcp", "args": ["--local", "--client", "cursor"]}}}"#;
        let (t, c) = edit(custom, Shape::McpServers);
        assert_eq!(c, Some(Change::Custom));
        assert_eq!(t.trim_end(), custom);
        // Another program under that name: theirs.
        let other = r#"{"mcpServers": {"meshgrove": {"command": "node", "args": ["--client", "cursor"]}}}"#;
        assert_eq!(edit(other, Shape::McpServers).1, Some(Change::Custom));
        // Already right: the text is untouched.
        let same =
            format!(r#"{{"mcpServers": {{"meshgrove": {{"command": "{EXE}", "args": ["--client", "cursor"]}}}}}}"#);
        let (t, c) = edit(&same, Shape::McpServers);
        assert_eq!((t.trim_end(), c), (same.as_str(), Some(Change::Unchanged)));
    }

    #[test]
    fn appimages_start_the_server_as_a_subcommand_and_count_as_ours() {
        let image = Server::new(
            Path::new("/tmp/.mount_x/usr/bin/meshgrove-mcp"),
            Some(Path::new("/home/u/Meshgrove-0.5.2-x86_64.AppImage")),
        );
        assert_eq!(image.program, "/home/u/Meshgrove-0.5.2-x86_64.AppImage");
        assert_eq!(image.args("lm-studio", true), ["mcp", "--local", "--client", "lm-studio"]);
        // After an update the AppImage has another name: the entry follows it.
        let old = r#"{"mcpServers": {"meshgrove": {"command": "/home/u/Meshgrove-0.5.1-x86_64.AppImage", "args": ["mcp", "--client", "cursor"]}}}"#;
        let mut t = old.to_owned();
        let c = edit_json(&mut t, Shape::McpServers, &image.program, &image.args("cursor", false), Mode::Add).unwrap();
        assert_eq!(c, Some(Change::Updated));
        assert!(t.contains("Meshgrove-0.5.2"), "{t}");
    }

    #[test]
    fn each_shape_writes_what_its_assistant_reads() {
        let v = |shape| serde_json::from_str::<Value>(&edit("{}", shape).0).unwrap();
        assert_eq!(v(Shape::ClaudeCode)["mcpServers"]["meshgrove"]["type"], "stdio");
        assert_eq!(v(Shape::VsCode)["servers"]["meshgrove"]["type"], "stdio");
        assert_eq!(v(Shape::Copilot)["mcpServers"]["meshgrove"]["tools"], json!(["*"]));
        assert_eq!(v(Shape::OpenCode)["mcp"]["meshgrove"]["command"], json!([EXE, "--client", "cursor"]));
    }

    #[test]
    fn broken_json_is_an_error() {
        for bad in ["[1]", "{\"a\": ", "{\"a\": 1} x", "{\"mcpServers\": 3}"] {
            assert!(
                edit_json(&mut bad.to_owned(), Shape::McpServers, EXE, &args("cursor"), Mode::Add).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn codex_toml_keeps_comments_and_other_tables() {
        let src = "# my codex\nmodel = \"x\"\n\n[mcp_servers.other]\ncommand = \"o\" # mine\n";
        let mut t = src.to_owned();
        assert_eq!(edit_toml(&mut t, EXE, &args("codex"), Mode::Add).unwrap(), Some(Change::Added));
        assert!(t.starts_with(src), "{t}");
        assert!(t.contains("[mcp_servers.meshgrove]") && t.contains(&format!("command = \"{EXE}\"")), "{t}");
        let mut stale =
            "[mcp_servers.meshgrove]\ncommand = \"/old/meshgrove-mcp\"\nargs = [\"--client\", \"codex\"]\nstartup_timeout_sec = 30\n"
                .to_owned();
        assert_eq!(edit_toml(&mut stale, EXE, &args("codex"), Mode::Add).unwrap(), Some(Change::Updated));
        assert!(stale.contains("startup_timeout_sec = 30") && stale.contains(EXE), "{stale}");
        let mut custom = "[mcp_servers.meshgrove]\ncommand = \"/x/meshgrove-mcp\"\nargs = [\"--local\"]\n".to_owned();
        let before = custom.clone();
        assert_eq!(edit_toml(&mut custom, EXE, &args("codex"), Mode::Add).unwrap(), Some(Change::Custom));
        assert_eq!(custom, before);
    }

    #[test]
    fn removing_takes_out_only_our_entry_and_keeps_the_rest() {
        let src = "{\n  \"mcpServers\": {\n    \"a\": {\"command\": \"x\"},\n    \"meshgrove\": {\"command\": \"/old/meshgrove-mcp\", \"args\": [\"--client\", \"cursor\"]}\n  },\n  \"k\": 1\n}\n";
        let mut t = src.to_owned();
        assert_eq!(
            edit_json(&mut t, Shape::McpServers, EXE, &args("cursor"), Mode::Remove).unwrap(),
            Some(Change::Removed)
        );
        let v: Value = serde_json::from_str(&t).unwrap();
        assert_eq!(v, json!({"mcpServers": {"a": {"command": "x"}}, "k": 1}));
        let mut mine = r#"{"mcpServers": {"meshgrove": {"command": "node", "args": ["x"]}}}"#.to_owned();
        let before = mine.clone();
        assert_eq!(
            edit_json(&mut mine, Shape::McpServers, EXE, &args("cursor"), Mode::Remove).unwrap(),
            Some(Change::Custom)
        );
        assert_eq!(mine.trim_end(), before);
        let mut none = "{\"k\": 1}".to_owned();
        assert_eq!(edit_json(&mut none, Shape::McpServers, EXE, &args("cursor"), Mode::Remove).unwrap(), None);
        assert_eq!(none.trim_end(), "{\"k\": 1}");
    }

    #[test]
    fn running_against_a_home_finds_connects_backs_up_and_removes() {
        let h = tempfile::tempdir().unwrap();
        let h = h.path();
        std::fs::create_dir_all(h.join(".cursor")).unwrap();
        std::fs::create_dir_all(h.join(".lmstudio")).unwrap();
        std::fs::write(h.join(".cursor/mcp.json"), "{\n  // mine\n  \"mcpServers\": {}\n}\n").unwrap();
        let env = Env {
            home: Some(h.to_path_buf()),
            appdata: None,
            local_appdata: None,
            xdg_config: Some(h.join("xdg")),
            codex_home: None,
        };
        let s = server();
        let found: Vec<_> = detect(&env).iter().map(|t| t.client).collect();
        assert_eq!(found, ["lm-studio", "cursor"]);
        // Trying it out writes nothing.
        let dry = run(&env, &s, Mode::Add, true, None);
        assert!(dry.iter().all(|o| o.result == Ok(Some(Change::Added))));
        assert!(!h.join(".lmstudio/mcp.json").exists());
        let done = run(&env, &s, Mode::Add, false, None);
        assert!(done.iter().all(|o| o.result == Ok(Some(Change::Added))), "{done:?}");
        let lm: Value = serde_json::from_str(&std::fs::read_to_string(h.join(".lmstudio/mcp.json")).unwrap()).unwrap();
        assert_eq!(lm["mcpServers"]["meshgrove"]["args"], json!(["--local", "--client", "lm-studio"]));
        assert!(std::fs::read_to_string(h.join(".cursor/mcp.json")).unwrap().contains("// mine"));
        assert!(h.join(".cursor/mcp.json.bak").exists(), "the original is kept");
        // A second run changes nothing.
        assert!(run(&env, &s, Mode::Add, false, None).iter().all(|o| o.result == Ok(Some(Change::Unchanged))));
        // Turned off: the entries go again, the user's comment stays.
        assert!(run(&env, &s, Mode::Remove, false, None).iter().all(|o| o.result == Ok(Some(Change::Removed))));
        let cursor = std::fs::read_to_string(h.join(".cursor/mcp.json")).unwrap();
        assert!(cursor.contains("// mine") && !cursor.contains("meshgrove"), "{cursor}");
    }
}
