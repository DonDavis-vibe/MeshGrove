//! An AI agent of this computer answering the chat: started as a child process, spoken to over
//! the Agent Client Protocol (initialize, a session with the `meshgrove` MCP server, prompts,
//! permission questions, sign-in). The app sees only [`Event`]s, polled every frame; nothing
//! here blocks.
//!
//! Adapted from SolidCraft's `crates/ui-egui/src/agent_chat/acp_client.rs` (MIT license; see
//! `meshgrove_core::ai::agent_setup` for the notice).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::acp::{self, AuthMethod, ConfigOption, Incoming, ModelInfo, PermissionOption, PromptPart, ToolStatus};
use super::process::{AgentProcess, FromAgent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    Initialize,
    SessionNew,
    Prompt,
    Authenticate,
    SetModel,
    SetConfig,
}

/// What the agent reports to the chat.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Ready for a message. `agent`: its name and version; `images`: it takes pictures.
    Ready { agent: String, images: bool },
    /// Text it streams (`thought`: its reasoning).
    Text { thought: bool, text: String },
    /// A new tool call, or news about one.
    Tool { id: String, title: Option<String>, kind: Option<String>, status: Option<ToolStatus>, output: Option<String> },
    /// It asks before running something of its own; answered with [`AcpBackend::answer_permission`].
    /// `name` and `kind` are the tool call's, from the request or the call it announced before.
    Permission { key: u64, name: Option<String>, kind: Option<String>, title: String, options: Vec<PermissionOption> },
    /// It needs the user to sign in; `methods` are its own ways to do so.
    AuthRequired { methods: Vec<AuthMethod> },
    /// What it says about its login.
    AuthStatus(String),
    /// Its models (empty `available`: only the current one changed), from an agent without
    /// session config options.
    Models { current: String, available: Vec<ModelInfo> },
    /// Its session options (the model among them); each replaces the last.
    ConfigOptions(Vec<ConfigOption>),
    /// The agent refused the model it was just sent, and says why.
    ModelRejected(String),
    /// The message is answered (`reason`: why it stopped, empty for a normal end).
    TurnEnded { reason: String },
    /// Something failed; the agent is still running unless [`Event::Exited`] follows.
    Error(String),
    /// It ended; its last error output.
    Exited { tail: Vec<String> },
}

pub struct AcpBackend {
    proc: AgentProcess,
    cwd: PathBuf,
    server: Option<Value>,
    note: String,
    /// The agent's `initialize` reply: which routes the system note can take into a session.
    init: Value,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    session: Option<String>,
    agent: String,
    images: bool,
    auth: Vec<AuthMethod>,
    /// Open permission questions: our key → the agent's request id.
    permissions: HashMap<u64, Value>,
    next_key: u64,
    /// What the tool calls of this turn said of themselves: id → (name, kind).
    tool_calls: HashMap<String, (Option<String>, Option<String>)>,
    exited: bool,
    config_pending: Option<(u64, String)>,
    modern_config: bool,
}

impl AcpBackend {
    /// Start `argv` in `cwd` and say hello (`initialize`); the session, with `server` as its MCP
    /// server and `note` as its system prompt, follows when the agent answers.
    pub fn start(
        argv: &[String],
        cwd: &Path,
        server: Option<Value>,
        note: String,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<AcpBackend, String> {
        let proc = AgentProcess::spawn(argv, cwd, wake)?;
        let mut b = AcpBackend {
            proc,
            cwd: cwd.to_path_buf(),
            server,
            note,
            init: Value::Null,
            next_id: 0,
            pending: HashMap::new(),
            session: None,
            agent: String::new(),
            images: false,
            auth: Vec::new(),
            permissions: HashMap::new(),
            next_key: 0,
            tool_calls: HashMap::new(),
            exited: false,
            config_pending: None,
            modern_config: false,
        };
        b.request(Pending::Initialize, "initialize", acp::initialize_params(env!("CARGO_PKG_VERSION")));
        Ok(b)
    }

    fn request(&mut self, what: Pending, method: &str, params: Value) {
        self.next_id += 1;
        self.pending.insert(self.next_id, what);
        self.proc.send(&acp::request(self.next_id, method, params));
    }

    fn open_session(&mut self) {
        self.config_pending = None;
        self.pending.retain(|_, request| *request != Pending::SetConfig);
        let params = acp::session_new_params(&self.cwd, self.server.as_ref(), &self.init, &self.note);
        self.request(Pending::SessionNew, "session/new", params);
    }

    /// Whether a session is open and no message is being answered.
    pub fn ready(&self) -> bool {
        self.session.is_some() && !self.pending.values().any(|p| *p == Pending::Prompt)
    }

    /// A message is being answered.
    pub fn busy(&self) -> bool {
        self.pending.values().any(|p| *p == Pending::Prompt)
    }

    pub fn takes_images(&self) -> bool {
        self.images
    }

    fn handle(&mut self, msg: Incoming, out: &mut Vec<Event>) {
        match msg {
            Incoming::Reply { id, result } => {
                let Some(what) = self.pending.remove(&id) else { return };
                match (what, result) {
                    (Pending::Initialize, Ok(v)) => {
                        self.agent = acp::agent_info(&v);
                        self.images = acp::takes_images(&v);
                        self.auth = acp::auth_methods(&v);
                        self.init = v;
                        self.open_session();
                    }
                    (Pending::SessionNew, Ok(v)) => match v.get("sessionId").and_then(Value::as_str) {
                        Some(s) => {
                            self.session = Some(s.to_string());
                            match acp::config_options(&v) {
                                acp::ConfigParse::Snapshot(options) => {
                                    self.modern_config = true;
                                    out.push(Event::ConfigOptions(options));
                                }
                                _ => {
                                    self.modern_config = false;
                                    if let Some((current, available)) = acp::models(&v) {
                                        out.push(Event::Models { current, available });
                                    }
                                }
                            }
                            out.push(Event::Ready { agent: self.agent.clone(), images: self.images });
                        }
                        None => out.push(Event::Error("session/new: no sessionId".into())),
                    },
                    (Pending::Prompt, Ok(v)) => {
                        self.tool_calls.clear();
                        let reason = v.get("stopReason").and_then(Value::as_str).unwrap_or("");
                        out.push(Event::TurnEnded {
                            reason: if reason == "end_turn" { String::new() } else { reason.to_string() },
                        });
                    }
                    // Signed in: try the session again.
                    (Pending::Authenticate, Ok(_)) => self.open_session(),
                    (Pending::SetModel, Ok(_)) => {}
                    (Pending::SetConfig, Ok(v)) => {
                        if let Some((request_id, _)) = self.config_pending.take()
                            && request_id == id
                            && let acp::ConfigParse::Snapshot(o) = acp::config_options(&v)
                        {
                            out.push(Event::ConfigOptions(o));
                        }
                    }
                    // `session/set_model` is still unstable in ACP: an agent can list `models` and
                    // still not serve it. It keeps its own model; that is no error to show.
                    (Pending::SetModel, Err((acp::METHOD_NOT_FOUND, message))) => {
                        log::debug!("agent does not serve session/set_model: {message}");
                    }
                    (what, Err((code, message))) => {
                        if what == Pending::SetConfig
                            && let Some((request_id, _)) = self.config_pending.take()
                            && request_id == id
                        {
                            out.push(Event::ModelRejected(message.clone()));
                            return;
                        }
                        let auth = code == acp::AUTH_REQUIRED || message.to_ascii_lowercase().contains("auth");
                        if auth {
                            self.session = None;
                            self.modern_config = false;
                            self.config_pending = None;
                            self.pending.retain(|_, request| *request != Pending::SetConfig);
                            if what == Pending::Prompt {
                                out.push(Event::TurnEnded { reason: "auth".into() });
                            }
                            out.push(Event::AuthRequired { methods: self.auth.clone() });
                        } else if what == Pending::SetModel {
                            out.push(Event::ModelRejected(message));
                        } else {
                            if what == Pending::Prompt {
                                out.push(Event::TurnEnded { reason: "error".into() });
                            }
                            out.push(Event::Error(message));
                        }
                    }
                }
            }
            Incoming::Text { thought, text } => out.push(Event::Text { thought, text }),
            Incoming::Tool { id, name, title, kind, status, output } => {
                // A permission request names its call; what the call said of itself is kept.
                let known = self.tool_calls.entry(id.clone()).or_default();
                if name.is_some() {
                    known.0 = name;
                }
                if kind.is_some() {
                    known.1 = kind.clone();
                }
                out.push(Event::Tool { id, title, kind, status, output });
            }
            Incoming::Permission { rpc, id, name, kind, title, options } => {
                self.next_key += 1;
                self.permissions.insert(self.next_key, rpc);
                let known = id.as_ref().and_then(|id| self.tool_calls.get(id)).cloned().unwrap_or_default();
                let name = name.or(known.0);
                let kind = kind.or(known.1);
                out.push(Event::Permission { key: self.next_key, name, kind, title, options });
            }
            Incoming::AuthStatus(s) => out.push(Event::AuthStatus(s)),
            Incoming::ModelChanged(m) if !self.modern_config => {
                out.push(Event::Models { current: m, available: Vec::new() });
            }
            Incoming::ConfigOptions { session, options } if self.session.as_deref() == Some(session.as_str()) => {
                out.push(Event::ConfigOptions(options));
            }
            Incoming::ModelChanged(_) | Incoming::ConfigOptions { .. } => {}
            Incoming::Unsupported { rpc, method } => {
                self.proc.send(&acp::reply_error(
                    rpc,
                    acp::METHOD_NOT_FOUND,
                    &format!("Meshgrove does not serve `{method}`"),
                ));
            }
            Incoming::Ignored => {}
        }
    }

    /// What arrived since the last call.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        if self.exited {
            return out;
        }
        for _ in 0..512 {
            match self.proc.from_agent.try_recv() {
                Ok(FromAgent::Line(l)) => {
                    if let Some(m) = acp::parse(&l) {
                        self.handle(m, &mut out);
                    }
                }
                Ok(FromAgent::Exited(tail)) => {
                    self.exited = true;
                    self.session = None;
                    self.pending.clear();
                    self.config_pending = None;
                    out.push(Event::Exited { tail });
                    break;
                }
                Err(_) => break,
            }
        }
        out
    }

    /// Send a message (only after [`Event::Ready`] and while no turn runs).
    pub fn prompt(&mut self, parts: &[PromptPart]) {
        if let Some(s) = self.session.clone() {
            self.request(Pending::Prompt, "session/prompt", acp::prompt_params(&s, parts));
        }
    }

    /// Stop the message being answered; open permission questions count as cancelled.
    pub fn cancel(&mut self) {
        for (_, rpc) in self.permissions.drain() {
            self.proc.send(&acp::permission_reply(rpc, None));
        }
        if let Some(s) = &self.session {
            self.proc.send(&acp::notification("session/cancel", json!({"sessionId": s})));
        }
    }

    /// Start a fresh conversation on the running agent (no restart); [`Event::Ready`] follows.
    /// A message being answered is cancelled first.
    pub fn new_session(&mut self) {
        if self.busy() {
            self.cancel();
        }
        self.session = None;
        self.modern_config = false;
        self.open_session();
    }

    /// Answer an [`Event::Permission`] with one of its options (`None`: cancelled).
    pub fn answer_permission(&mut self, key: u64, option: Option<&str>) {
        if let Some(rpc) = self.permissions.remove(&key) {
            self.proc.send(&acp::permission_reply(rpc, option));
        }
    }

    /// Ask the agent to sign in by itself with one of its methods (it opens the browser).
    pub fn authenticate(&mut self, method: &str) {
        self.request(Pending::Authenticate, "authenticate", json!({"methodId": method}));
    }

    /// Use one of the models it reported in [`Event::Models`] (or one typed by the user).
    pub fn set_model(&mut self, model: &str) {
        if let Some(s) = self.session.clone() {
            self.request(Pending::SetModel, "session/set_model", acp::set_model_params(&s, model));
        }
    }

    /// Set one of its session options (the model among them).
    pub fn set_config_option(&mut self, id: &str, value: &str) {
        if self.config_pending.is_some() {
            return;
        }
        if let Some(s) = self.session.clone() {
            self.request(Pending::SetConfig, "session/set_config_option", acp::set_config_option_params(&s, id, value));
            self.config_pending = Some((self.next_id, id.to_string()));
        }
    }

    /// End it and everything it started. `wait`: block until done (quitting).
    pub fn shutdown(&mut self, wait: bool) {
        self.proc.shutdown(wait);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn backend() -> AcpBackend {
        AcpBackend::start(
            &["sh".into(), "-c".into(), "cat >/dev/null".into()],
            &std::env::temp_dir(),
            None,
            "note".into(),
            || {},
        )
        .unwrap()
    }

    fn snapshot(session: &str, value: &str) -> Value {
        json!({"sessionId":session,"configOptions":[{"id":"provider.model","category":"model","name":"Model","type":"select","currentValue":value,"options":[{"value":"fast","name":"Fast"},{"value":"smart","name":"Smart"}]}]})
    }

    #[test]
    fn a_session_opens_after_the_hello_and_its_options_are_reported() {
        let mut b = backend();
        let mut events = Vec::new();
        b.handle(Incoming::Reply { id: 1, result: Ok(json!({"agentInfo":{"name":"fake","version":"1"}, "agentCapabilities": {"promptCapabilities": {"image": true}}})) }, &mut events);
        assert!(events.is_empty() && b.pending.contains_key(&2), "session/new was asked");
        b.handle(Incoming::Reply { id: 2, result: Ok(snapshot("s1", "fast")) }, &mut events);
        assert!(b.ready());
        assert!(
            matches!(&events[..], [Event::ConfigOptions(o), Event::Ready { agent, images: true }] if o[0].current_value == "fast" && agent == "fake 1"),
            "{events:?}"
        );
        events.clear();
        b.set_config_option("provider.model", "zzz");
        let request = b.config_pending.as_ref().map(|p| p.0).unwrap();
        b.handle(Incoming::Reply { id: request, result: Err((-32602, "no such model".into())) }, &mut events);
        assert_eq!(events, [Event::ModelRejected("no such model".into())]);
        events.clear();
        b.prompt(&[PromptPart::Text("hi".into())]);
        assert!(b.busy() && !b.ready());
        b.handle(Incoming::Reply { id: b.next_id, result: Ok(json!({"stopReason": "end_turn"})) }, &mut events);
        assert_eq!(events, [Event::TurnEnded { reason: String::new() }]);
        b.shutdown(true);
    }

    #[test]
    fn a_permission_request_is_told_what_its_tool_call_said_of_itself() {
        let mut b = backend();
        let mut events = Vec::new();
        b.handle(
            Incoming::Tool {
                id: "t1".into(),
                name: Some("mcp__meshgrove-chat__search_models".into()),
                title: Some("search_models".into()),
                kind: Some("other".into()),
                status: None,
                output: None,
            },
            &mut events,
        );
        let ask = |id: &str| Incoming::Permission {
            rpc: json!(5),
            id: Some(id.into()),
            name: None,
            kind: None,
            title: "search_models".into(),
            options: vec![],
        };
        events.clear();
        b.handle(ask("t1"), &mut events);
        assert!(
            matches!(&events[..], [Event::Permission { name: Some(n), kind: Some(k), .. }] if n == "mcp__meshgrove-chat__search_models" && k == "other"),
            "{events:?}"
        );
        events.clear();
        b.handle(ask("t9"), &mut events);
        assert!(
            matches!(&events[..], [Event::Permission { name: None, kind: None, .. }]),
            "an unknown call: {events:?}"
        );
        b.shutdown(true);
    }

    #[test]
    fn a_sign_in_is_asked_for_and_the_session_tried_again() {
        let mut b = backend();
        let mut events = Vec::new();
        b.handle(
            Incoming::Reply { id: 1, result: Ok(json!({"authMethods": [{"id": "login", "name": "Log in"}]})) },
            &mut events,
        );
        b.handle(
            Incoming::Reply { id: 2, result: Err((acp::AUTH_REQUIRED, "Authentication required".into())) },
            &mut events,
        );
        assert!(matches!(&events[..], [Event::AuthRequired { methods }] if methods[0].id == "login"), "{events:?}");
        events.clear();
        b.authenticate("login");
        b.handle(Incoming::Reply { id: b.next_id, result: Ok(json!({})) }, &mut events);
        assert_eq!(b.pending.get(&b.next_id), Some(&Pending::SessionNew), "the session is opened again");
        b.handle(Incoming::Reply { id: b.next_id, result: Ok(json!({"sessionId": "s2", "models": {"currentModelId": "a", "availableModels": [{"modelId": "a"}]}})) }, &mut events);
        assert!(
            matches!(&events[..], [Event::Models { current, .. }, Event::Ready { .. }] if current == "a"),
            "{events:?}"
        );
        b.shutdown(true);
    }
}
