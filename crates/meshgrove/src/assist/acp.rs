//! The client side of the Agent Client Protocol (ACP, <https://agentclientprotocol.com>), version 1:
//! JSON-RPC 2.0, one message per line over the agent's stdin/stdout. This file only builds and
//! reads messages; the process lives in `process.rs` and the conversation in `acp_client.rs`, so
//! everything here is plain data and tested without an agent.
//!
//! What Meshgrove says it can do (`clientCapabilities`): no file system and no terminal (the agent
//! reaches the library through the `meshgrove` MCP server instead), and terminal login, so an agent
//! that needs one describes a command for the user to run rather than asking for a key.
//!
//! Adapted from SolidCraft's `crates/ui-egui/src/agent_chat/acp.rs` (MIT license; see
//! `meshgrove_core::ai::agent_setup` for the notice).

use serde_json::{Value, json};

/// One advertised value, with an optional human-readable group label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigChoice {
    pub value: String,
    pub name: String,
    pub description: String,
    pub group: Option<String>,
}

/// A select option of the agent's session (its model, its reasoning effort …).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigOption {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub current_value: String,
    pub options: Vec<ConfigChoice>,
}

/// The select options of a `session/new` reply or a `config_option_update`, bounded.
#[derive(Debug, PartialEq, Eq)]
pub enum ConfigParse {
    Absent,
    Invalid,
    Snapshot(Vec<ConfigOption>),
}

pub fn config_options(v: &Value) -> ConfigParse {
    let Some(raw_list) = v.get("configOptions") else { return ConfigParse::Absent };
    let Some(list) = raw_list.as_array() else { return ConfigParse::Invalid };
    let mut out = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for x in list.iter().take(64) {
        match x.get("type").and_then(Value::as_str) {
            Some("select") => {}
            Some(_) => continue,
            None => return ConfigParse::Invalid,
        }
        let Some(id) = x.get("id").and_then(Value::as_str).filter(|s| !s.is_empty() && s.len() <= 256) else {
            return ConfigParse::Invalid;
        };
        let Some(current_value) = x.get("currentValue").and_then(Value::as_str).filter(|s| s.len() <= 256) else {
            return ConfigParse::Invalid;
        };
        let Some(raw) = x.get("options").and_then(Value::as_array) else { return ConfigParse::Invalid };
        if !ids.insert(id.to_string()) {
            continue;
        }
        let mut choices = Vec::new();
        let mut values = std::collections::HashSet::new();
        'items: for item in raw.iter().take(128) {
            if choices.len() >= 128 {
                break;
            }
            let group = item
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| item.get("group").and_then(Value::as_str))
                .map(|s| s.chars().take(256).collect());
            if let Some(cs) = item.get("options").and_then(Value::as_array) {
                for c in cs.iter().take(128) {
                    if choices.len() >= 128 {
                        break 'items;
                    }
                    let Some(value) =
                        c.get("value").and_then(Value::as_str).filter(|s| !s.is_empty() && s.len() <= 256)
                    else {
                        continue;
                    };
                    if !values.insert(value.to_string()) {
                        continue;
                    }
                    choices.push(ConfigChoice {
                        value: value.into(),
                        name: c.get("name").and_then(Value::as_str).unwrap_or(value).chars().take(256).collect(),
                        description: c
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .chars()
                            .take(1024)
                            .collect(),
                        group: group.clone(),
                    });
                }
            } else {
                let Some(value) = item.get("value").and_then(Value::as_str).filter(|s| !s.is_empty() && s.len() <= 256)
                else {
                    continue;
                };
                if values.insert(value.to_string()) {
                    choices.push(ConfigChoice {
                        value: value.into(),
                        name: item.get("name").and_then(Value::as_str).unwrap_or(value).chars().take(256).collect(),
                        description: item
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .chars()
                            .take(1024)
                            .collect(),
                        group: None,
                    });
                }
            }
        }
        if choices.is_empty() {
            return ConfigParse::Invalid;
        }
        out.push(ConfigOption {
            id: id.into(),
            name: x.get("name").and_then(Value::as_str).unwrap_or(id).chars().take(256).collect(),
            description: x.get("description").and_then(Value::as_str).unwrap_or("").chars().take(1024).collect(),
            category: x.get("category").and_then(Value::as_str).unwrap_or("").chars().take(128).collect(),
            current_value: current_value.into(),
            options: choices,
        });
    }
    ConfigParse::Snapshot(out)
}

/// The option among `options` that picks the model: by its category, else by its id or name.
pub fn model_option(options: &[ConfigOption]) -> Option<&ConfigOption> {
    options.iter().find(|o| o.category == "model").or_else(|| {
        options.iter().find(|o| o.id.to_lowercase().contains("model") || o.name.to_lowercase().contains("model"))
    })
}

/// The protocol version we speak.
pub const PROTOCOL_VERSION: u64 = 1;

/// JSON-RPC "method not found", our answer to every agent request we don't serve.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// The error code ACP agents use for "authentication required".
pub const AUTH_REQUIRED: i64 = -32000;

/// One request or notification to send.
pub fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}
pub fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}
pub fn reply(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}
pub fn reply_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

pub fn initialize_params(version: &str) -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "clientCapabilities": {
            "fs": {"readTextFile": false, "writeTextFile": false},
            "terminal": false,
            "auth": {"terminal": true},
            "_meta": {"terminal-auth": true},
        },
        "clientInfo": {"name": "meshgrove", "title": "Meshgrove", "version": version},
    })
}

/// The name of the MCP server a session gets: not `meshgrove`, the entry Meshgrove writes into
/// the assistants' own settings (an agent that keeps its own entry on a clash would then suggest
/// as an AI app, and nothing would show in the chat).
pub const SERVER_NAME: &str = "meshgrove-chat";

/// The MCP server entry of a session: `meshgrove-mcp` (or the AppImage's `mcp` subcommand) with
/// `--source chat --client <agent>`, so what it suggests shows in the chat and belongs to it, and
/// `--local` for an agent whose model runs here.
pub fn mcp_server(program: &str, lead: &[String], agent: &str, local: bool) -> Value {
    let mut args: Vec<String> = lead.to_vec();
    if local {
        args.push("--local".into());
    }
    args.extend(["--source".to_owned(), "chat".into(), "--client".into(), agent.to_owned()]);
    json!({"name": SERVER_NAME, "command": program, "args": args, "env": []})
}

/// The Claude Code adapter's own tools that reach the computer rather than the library: turned
/// off for the session (`_meta.claudeCode.options.disallowedTools`), so it works through Meshgrove's.
pub const CLAUDE_DISALLOWED_TOOLS: &[&str] = &[
    "Bash",
    "BashOutput",
    "KillShell",
    "Read",
    "Write",
    "Edit",
    "MultiEdit",
    "NotebookEdit",
    "Glob",
    "Grep",
    "WebFetch",
    "WebSearch",
    "Task",
];

/// The `session/new` parameters. `init` is the agent's `initialize` reply: agents read the system
/// note from different places, and the standard field may only go to an agent offering it.
pub fn session_new_params(cwd: &std::path::Path, server: Option<&Value>, init: &Value, note: &str) -> Value {
    let servers: Vec<Value> = server.cloned().into_iter().collect();
    let mut meta = json!({
        // The Claude adapter's own key, for its versions without the standard field.
        "systemPrompt": {"append": note},
        // Every tool call the agent asks about reaches the chat; its own shell and file tools
        // are off: the agent works through Meshgrove's tools.
        "claudeCode": {"options": {"allowDangerouslySkipPermissions": false, "disallowedTools": CLAUDE_DISALLOWED_TOOLS}},
    });
    if takes_antigravity_note(init)
        && let Some(meta) = meta.as_object_mut()
    {
        meta.insert("antigravity".into(), json!({"systemPrompt": {"append": note}}));
    }
    let mut params = json!({
        "cwd": cwd.to_string_lossy(),
        "mcpServers": servers,
        "_meta": meta,
    });
    if takes_standard_note(init)
        && let Some(params) = params.as_object_mut()
    {
        params.insert("systemPrompt".into(), json!({"mode": "append", "content": [{"type": "text", "text": note}]}));
    }
    params
}

/// The agent offers ACP's `systemPrompt` field on its sessions, and takes an appended note
/// through it: an offer that names only `override` (the Claude adapter's) gets the note through
/// its own key instead, so the agent's own prompt stays.
fn takes_standard_note(init: &Value) -> bool {
    let Some(offer) = init.pointer("/agentCapabilities/sessionCapabilities/systemPrompt").filter(|o| !o.is_null())
    else {
        return false;
    };
    match offer.get("mode") {
        Some(modes) => modes.get("append").is_some_and(|a| !a.is_null()),
        None => true,
    }
}

/// Antigravity offers its own key under `_meta` and doesn't read the Claude adapter's.
fn takes_antigravity_note(init: &Value) -> bool {
    init.pointer("/agentCapabilities/_meta/antigravity/systemPrompt/append").and_then(Value::as_bool).unwrap_or(false)
}

/// One part of a message to the agent.
#[derive(Clone, Debug, PartialEq)]
pub enum PromptPart {
    Text(String),
    /// A picture (PNG), sent only to agents that take images.
    Image {
        mime: String,
        base64: String,
    },
}

/// A message's parts as ACP content blocks.
pub fn content_blocks(parts: &[PromptPart]) -> Vec<Value> {
    parts
        .iter()
        .map(|p| match p {
            PromptPart::Text(t) => json!({"type": "text", "text": t}),
            PromptPart::Image { mime, base64 } => json!({"type": "image", "mimeType": mime, "data": base64}),
        })
        .collect()
}

pub fn prompt_params(session: &str, parts: &[PromptPart]) -> Value {
    json!({"sessionId": session, "prompt": content_blocks(parts)})
}

/// One model an agent offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
}

/// The agent's models in a `session/new` reply (`models`: `currentModelId` and
/// `availableModels: [{modelId, name, description?}]`): the current one and each model. `None`
/// when the agent reports none.
pub fn models(session: &Value) -> Option<(String, Vec<ModelInfo>)> {
    let m = session.get("models")?;
    let current = m.get("currentModelId").and_then(Value::as_str).unwrap_or("").to_string();
    let available: Vec<ModelInfo> = m
        .get("availableModels")?
        .as_array()?
        .iter()
        .take(200)
        .filter_map(|x| {
            let id = x.get("modelId").or_else(|| x.get("id"))?.as_str()?.to_string();
            let name = x.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).unwrap_or(&id).to_string();
            let description = x.get("description").and_then(Value::as_str).unwrap_or("").to_string();
            Some(ModelInfo { id, name, description })
        })
        .collect();
    (!available.is_empty()).then_some((current, available))
}

pub fn set_model_params(session: &str, model: &str) -> Value {
    json!({"sessionId": session, "modelId": model})
}
pub fn set_config_option_params(session: &str, id: &str, value: &str) -> Value {
    json!({"sessionId": session, "configId": id, "value": value})
}

/// Whether the agent takes pictures in a message (`agentCapabilities.promptCapabilities.image`).
pub fn takes_images(init: &Value) -> bool {
    init.pointer("/agentCapabilities/promptCapabilities/image").and_then(Value::as_bool).unwrap_or(false)
}

/// One way to log in that the agent offers (`authMethods` in its `initialize` reply).
#[derive(Clone, Debug, PartialEq)]
pub struct AuthMethod {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: AuthKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AuthKind {
    /// The agent signs in by itself when asked (`authenticate`), e.g. by opening a browser.
    Agent,
    /// The user runs the agent's own login in a terminal: the agent's command plus `args`.
    Terminal { args: Vec<String>, command: Option<(String, Vec<String>)> },
    /// The agent reads a key from an environment variable the user sets themselves.
    EnvVar { vars: Vec<String> },
}

pub fn auth_methods(init: &Value) -> Vec<AuthMethod> {
    let strings = |v: Option<&Value>| -> Vec<String> {
        v.and_then(Value::as_array)
            .map(|a| a.iter().take(64).filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    };
    init.get("authMethods")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .take(32)
                .filter_map(|m| {
                    let s = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                    let id = s("id");
                    if id.is_empty() {
                        return None;
                    }
                    let kind = match m.get("type").and_then(Value::as_str) {
                        Some("terminal") => {
                            let meta = m.get("_meta").and_then(|x| x.get("terminal-auth"));
                            let command = meta
                                .and_then(|t| Some((t.get("command")?.as_str()?.to_string(), strings(t.get("args")))));
                            AuthKind::Terminal { args: strings(m.get("args")), command }
                        }
                        Some("env_var") => AuthKind::EnvVar {
                            vars: m
                                .get("vars")
                                .and_then(Value::as_array)
                                .map(|v| {
                                    v.iter()
                                        .take(16)
                                        .filter_map(|x| x.get("name").and_then(Value::as_str).map(str::to_string))
                                        .collect()
                                })
                                .unwrap_or_default(),
                        },
                        // A key the agent would take from the client: Meshgrove never handles
                        // agents' keys, so this becomes "set it in your environment" instead.
                        _ if m.get("_meta").and_then(|x| x.get("api-key")).is_some() => {
                            let provider = m
                                .get("_meta")
                                .and_then(|x| x.get("api-key"))
                                .and_then(|k| k.get("provider"))
                                .and_then(Value::as_str)
                                .unwrap_or("agent");
                            AuthKind::EnvVar { vars: vec![format!("{}_API_KEY", provider.to_ascii_uppercase())] }
                        }
                        _ => AuthKind::Agent,
                    };
                    Some(AuthMethod { id, name: s("name"), description: s("description"), kind })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The agent's name and version from its `initialize` reply.
pub fn agent_info(init: &Value) -> String {
    let i = init.get("agentInfo");
    let s = |k: &str| i.and_then(|i| i.get(k)).and_then(Value::as_str);
    match (s("title").or(s("name")), s("version")) {
        (Some(n), Some(v)) => format!("{n} {v}"),
        (Some(n), None) => n.to_string(),
        _ => String::new(),
    }
}

/// A tool call's state as the agent reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

impl ToolStatus {
    pub fn parse(s: &str) -> Option<ToolStatus> {
        Some(match s {
            "pending" => ToolStatus::Pending,
            "in_progress" => ToolStatus::InProgress,
            "completed" => ToolStatus::Completed,
            "failed" => ToolStatus::Failed,
            _ => return None,
        })
    }
}

/// One choice of a permission request.
#[derive(Clone, Debug, PartialEq)]
pub struct PermissionOption {
    pub id: String,
    pub name: String,
    /// `allow_once`, `allow_always`, `reject_once` or `reject_always`.
    pub kind: String,
}

/// What a message from the agent means for the chat.
#[derive(Clone, Debug, PartialEq)]
pub enum Incoming {
    /// The reply to one of our requests.
    Reply {
        id: u64,
        result: Result<Value, (i64, String)>,
    },
    /// Text the agent streams (`thought`: its reasoning).
    Text {
        thought: bool,
        text: String,
    },
    /// A new tool call, or news about one (`title`/`status` when they changed).
    Tool {
        id: String,
        title: Option<String>,
        kind: Option<String>,
        status: Option<ToolStatus>,
        output: Option<String>,
    },
    /// The agent asks before running a tool call; answered with [`permission_reply`].
    Permission {
        rpc: Value,
        title: String,
        options: Vec<PermissionOption>,
    },
    /// The agent says whether it is logged in (an extension some adapters send).
    AuthStatus(String),
    /// The agent switched to another model.
    ModelChanged(String),
    ConfigOptions {
        session: String,
        options: Vec<ConfigOption>,
    },
    /// A request we don't serve (file system, terminal, …): answer "method not found".
    Unsupported {
        rpc: Value,
        method: String,
    },
    /// Anything else (mode and command lists, plans, usage): nothing to show.
    Ignored,
}

/// The longest text kept from one tool call's output.
const MAX_OUTPUT: usize = 2000;

fn content_text(c: &Value) -> Option<String> {
    // A content block, or a tool call content item wrapping one (`{type: "content", content}`).
    let block = if c.get("type").and_then(Value::as_str) == Some("content") { c.get("content")? } else { c };
    match block.get("type").and_then(Value::as_str) {
        Some("text") => block.get("text").and_then(Value::as_str).map(str::to_string),
        Some("image") => Some("[image]".into()),
        Some("resource_link") => block.get("uri").and_then(Value::as_str).map(|u| format!("[{u}]")),
        Some("diff") => block.get("path").and_then(Value::as_str).map(|p| format!("[diff {p}]")),
        Some("terminal") => Some("[terminal]".into()),
        _ => None,
    }
}

fn tool_output(u: &Value) -> Option<String> {
    let parts: Vec<String> = u.get("content")?.as_array()?.iter().take(16).filter_map(content_text).collect();
    if parts.is_empty() {
        return None;
    }
    let mut s = parts.join("\n");
    if s.len() > MAX_OUTPUT {
        let mut cut = MAX_OUTPUT;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push('…');
    }
    Some(s)
}

/// Read one line from the agent.
pub fn parse(line: &str) -> Option<Incoming> {
    let msg: Value = serde_json::from_str(line.trim()).ok()?;
    let method = msg.get("method").and_then(Value::as_str);
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    match (method, msg.get("id")) {
        (None, Some(id)) => {
            let id = id.as_u64()?;
            let result = match msg.get("error") {
                Some(e) => Err((
                    e.get("code").and_then(Value::as_i64).unwrap_or(0),
                    e.get("message").and_then(Value::as_str).unwrap_or("error").to_string(),
                )),
                None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
            };
            Some(Incoming::Reply { id, result })
        }
        (Some("session/request_permission"), Some(id)) => {
            let call = params.get("toolCall");
            let title = call.and_then(|c| c.get("title")).and_then(Value::as_str).unwrap_or("").to_string();
            let options = params
                .get("options")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .take(8)
                        .filter_map(|o| {
                            Some(PermissionOption {
                                id: o.get("optionId")?.as_str()?.to_string(),
                                name: o.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                                kind: o.get("kind").and_then(Value::as_str).unwrap_or("").to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(Incoming::Permission { rpc: id.clone(), title, options })
        }
        (Some(m), Some(id)) => Some(Incoming::Unsupported { rpc: id.clone(), method: m.to_string() }),
        (Some("session/update"), None) => {
            Some(update(params.get("update")?, params.get("sessionId").and_then(Value::as_str).unwrap_or("")))
        }
        (Some("_auth/status_update"), None) => params
            .get("authStatus")
            .and_then(|s| s.get("label"))
            .and_then(Value::as_str)
            .map(|l| Incoming::AuthStatus(l.to_string())),
        _ => Some(Incoming::Ignored),
    }
}

fn update(u: &Value, session: &str) -> Incoming {
    let s = |k: &str| u.get(k).and_then(Value::as_str).map(str::to_string);
    match u.get("sessionUpdate").and_then(Value::as_str) {
        Some(kind @ ("agent_message_chunk" | "agent_thought_chunk")) => match u.get("content").and_then(content_text) {
            Some(text) => Incoming::Text { thought: kind == "agent_thought_chunk", text },
            None => Incoming::Ignored,
        },
        Some("tool_call" | "tool_call_update") => match s("toolCallId") {
            Some(id) => Incoming::Tool {
                id,
                title: s("title"),
                kind: s("kind"),
                status: u.get("status").and_then(Value::as_str).and_then(ToolStatus::parse),
                output: tool_output(u),
            },
            None => Incoming::Ignored,
        },
        Some("current_model_update") => match s("currentModelId").or_else(|| s("modelId")) {
            Some(m) => Incoming::ModelChanged(m),
            None => Incoming::Ignored,
        },
        Some("config_option_update") => match config_options(u) {
            ConfigParse::Snapshot(options) if !session.is_empty() => {
                Incoming::ConfigOptions { session: session.into(), options }
            }
            _ => Incoming::Ignored,
        },
        _ => Incoming::Ignored,
    }
}

/// The answer to a permission request: the option picked, or `None` for "cancelled".
pub fn permission_reply(rpc: Value, option: Option<&str>) -> Value {
    let outcome = match option {
        Some(o) => json!({"outcome": "selected", "optionId": o}),
        None => json!({"outcome": "cancelled"}),
    };
    reply(rpc, json!({"outcome": outcome}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_gets_the_mcp_server_and_the_note_takes_every_route_offered() {
        let server = mcp_server("/opt/meshgrove/meshgrove-mcp", &[], "claude", false);
        assert_eq!(server["args"], json!(["--source", "chat", "--client", "claude"]));
        let image = mcp_server("/x/Meshgrove.AppImage", &["mcp".into()], "agent-1", true);
        assert_eq!(image["args"], json!(["mcp", "--local", "--source", "chat", "--client", "agent-1"]));
        let params = |init: Value| session_new_params(std::path::Path::new("/tmp"), Some(&server), &init, "NOTE");
        let standard = json!({"mode": "append", "content": [{"type": "text", "text": "NOTE"}]});
        let offers_standard = params(json!({"agentCapabilities": {"sessionCapabilities": {"systemPrompt": {}}}}));
        assert_eq!(offers_standard["systemPrompt"], standard);
        let appends = params(
            json!({"agentCapabilities": {"sessionCapabilities": {"systemPrompt": {"mode": {"append": {}, "override": {}}}}}}),
        );
        assert_eq!(appends["systemPrompt"], standard);
        // The Claude adapter offers only `override`: the note goes through its own key alone.
        let overrides =
            params(json!({"agentCapabilities": {"sessionCapabilities": {"systemPrompt": {"mode": {"override": {}}}}}}));
        assert!(overrides.get("systemPrompt").is_none());
        assert_eq!(overrides["_meta"]["systemPrompt"]["append"], "NOTE");
        assert_eq!(offers_standard["mcpServers"][0]["name"], "meshgrove-chat");
        assert!(offers_standard["_meta"].get("antigravity").is_none());
        let offers_antigravity =
            params(json!({"agentCapabilities": {"_meta": {"antigravity": {"systemPrompt": {"append": true}}}}}));
        assert_eq!(offers_antigravity["_meta"]["antigravity"]["systemPrompt"]["append"], "NOTE");
        assert!(offers_antigravity.get("systemPrompt").is_none());
        let plain = params(json!({}));
        assert!(plain.get("systemPrompt").is_none());
        assert_eq!(plain["_meta"]["systemPrompt"]["append"], "NOTE");
        assert!(
            plain["_meta"]["claudeCode"]["options"]["disallowedTools"]
                .as_array()
                .is_some_and(|t| t.iter().any(|x| x == "Bash"))
        );
        assert_eq!(session_new_params(std::path::Path::new("/tmp"), None, &json!({}), "")["mcpServers"], json!([]));
    }

    #[test]
    fn updates_permissions_and_replies_are_told_apart() {
        let text = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Hi"}}}}"#;
        assert_eq!(parse(text), Some(Incoming::Text { thought: false, text: "Hi".into() }));
        let tool = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"tool_call_update","toolCallId":"t1","status":"completed","content":[{"type":"content","content":{"type":"text","text":"ok"}}]}}}"#;
        assert_eq!(
            parse(tool),
            Some(Incoming::Tool {
                id: "t1".into(),
                title: None,
                kind: None,
                status: Some(ToolStatus::Completed),
                output: Some("ok".into())
            })
        );
        let perm = r#"{"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{"sessionId":"s","toolCall":{"toolCallId":"t1","title":"search_models"},"options":[{"optionId":"allow","name":"Allow","kind":"allow_once"},{"optionId":"reject","name":"Reject","kind":"reject_once"}]}}"#;
        let Some(Incoming::Permission { rpc, title, options }) = parse(perm) else { panic!() };
        assert_eq!((title.as_str(), options.len(), options[1].kind.as_str()), ("search_models", 2, "reject_once"));
        assert_eq!(
            permission_reply(rpc.clone(), Some("allow"))["result"]["outcome"],
            json!({"outcome": "selected", "optionId": "allow"})
        );
        assert_eq!(permission_reply(rpc, None)["result"]["outcome"]["outcome"], "cancelled");
        let fs = r#"{"jsonrpc":"2.0","id":8,"method":"fs/read_text_file","params":{}}"#;
        assert!(matches!(parse(fs), Some(Incoming::Unsupported { .. })));
        let err = r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32000,"message":"Authentication required"}}"#;
        assert_eq!(
            parse(err),
            Some(Incoming::Reply { id: 3, result: Err((AUTH_REQUIRED, "Authentication required".into())) })
        );
        assert_eq!(parse("not json"), None);
        let changed = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"current_model_update","currentModelId":"haiku"}}}"#;
        assert_eq!(parse(changed), Some(Incoming::ModelChanged("haiku".into())));
    }

    #[test]
    fn models_and_config_options_of_a_session_are_read() {
        let v = json!({"sessionId": "s", "models": {"currentModelId": "sonnet", "availableModels": [
            {"modelId": "opus", "name": "Opus", "description": "Most capable"},
            {"modelId": "sonnet", "name": "Sonnet"},
            {"modelId": "haiku"},
            {"name": "no id"}]}});
        let (current, list) = models(&v).unwrap();
        assert_eq!(current, "sonnet");
        let names: Vec<_> = list.iter().map(|m| (m.id.as_str(), m.name.as_str(), m.description.as_str())).collect();
        assert_eq!(names, [("opus", "Opus", "Most capable"), ("sonnet", "Sonnet", ""), ("haiku", "haiku", "")]);
        assert!(models(&json!({"sessionId": "s"})).is_none());
        let c = json!({"configOptions":[
            {"id":"effort","category":"thought_level","type":"select","currentValue":"high","options":[{"group":"Reasoning","options":[{"value":"high","name":"High"}]}]},
            {"id":"arbitrary","name":"Model","category":"model","type":"select","currentValue":"a","options":[{"value":"a","name":"A"}]},
            {"id":"bad","type":"boolean"}]});
        let ConfigParse::Snapshot(options) = config_options(&c) else { panic!() };
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].options[0].group.as_deref(), Some("Reasoning"));
        assert_eq!(model_option(&options).map(|o| o.id.as_str()), Some("arbitrary"));
        assert!(matches!(
            config_options(&json!({"configOptions":[{"type":"select","id":"broken","currentValue":"x","options":[]}]})),
            ConfigParse::Invalid
        ));
        assert_eq!(config_options(&json!({})), ConfigParse::Absent);
        assert_eq!(
            set_config_option_params("s1", "vendor.model", "smart"),
            json!({"sessionId":"s1","configId":"vendor.model","value":"smart"})
        );
        assert_eq!(set_model_params("s", "opus"), json!({"sessionId": "s", "modelId": "opus"}));
    }

    #[test]
    fn auth_methods_of_the_adapters_are_read() {
        let claude = json!({"authMethods": [{"description": "Use Claude subscription ", "name": "Claude Subscription", "id": "claude-ai-login",
            "type": "terminal", "args": ["--cli", "auth", "login", "--claudeai"],
            "_meta": {"terminal-auth": {"command": "/usr/bin/node", "args": ["/x/dist/index.js", "--cli", "auth", "login", "--claudeai"], "label": "Claude Login"}}}]});
        let m = auth_methods(&claude);
        assert_eq!(m.len(), 1);
        let AuthKind::Terminal { args, command } = &m[0].kind else { panic!("{m:?}") };
        assert_eq!(args.last().map(String::as_str), Some("--claudeai"));
        assert_eq!(command.as_ref().map(|c| c.0.as_str()), Some("/usr/bin/node"));
        let codex = json!({"authMethods": [{"id": "chatgpt", "name": "Login with ChatGPT"},
            {"type": "env_var", "id": "openai-api-key", "name": "Use OPENAI_API_KEY", "vars": [{"name": "OPENAI_API_KEY"}]}]});
        let m = auth_methods(&codex);
        assert_eq!(m[0].kind, AuthKind::Agent);
        assert_eq!(m[1].kind, AuthKind::EnvVar { vars: vec!["OPENAI_API_KEY".into()] });
        assert!(takes_images(&json!({"agentCapabilities": {"promptCapabilities": {"image": true}}})));
        assert!(!takes_images(&json!({})));
        assert_eq!(agent_info(&json!({"agentInfo": {"name": "codex-acp", "version": "1.2"}})), "codex-acp 1.2");
    }

    #[test]
    fn pictures_go_as_image_blocks_and_long_output_is_cut_on_a_character() {
        let b = content_blocks(&[
            PromptPart::Text("hi".into()),
            PromptPart::Image { mime: "image/png".into(), base64: "AA==".into() },
        ]);
        assert_eq!(b[1]["type"], "image");
        assert_eq!(b[1]["mimeType"], "image/png");
        let long = "é".repeat(3000);
        let u = json!({"sessionUpdate": "tool_call", "toolCallId": "t", "content": [{"type": "content", "content": {"type": "text", "text": long}}]});
        let Incoming::Tool { output: Some(o), .. } = update(&u, "s") else { panic!() };
        assert!(o.len() <= MAX_OUTPUT + 3 && o.ends_with('…'));
    }
}
