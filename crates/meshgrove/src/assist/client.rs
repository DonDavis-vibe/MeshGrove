//! A client for the user's OpenAI-compatible endpoint (Ollama, LM Studio, llama.cpp, OpenAI,
//! OpenRouter …): `/models`, and `/chat/completions` streamed, with tools and pictures.

use std::io::{BufRead, BufReader};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use meshgrove_core::config::EndpointConfig;
use serde_json::{Value, json};
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

#[derive(Debug, Clone, PartialEq)]
pub enum ClientError {
    /// 401/403: the key is missing or wrong.
    Unauthorized,
    /// The endpoint rejected the tools (the model cannot call any).
    ToolsUnsupported(String),
    /// It answered with this status and message.
    Http(u16, String),
    /// No answer in time.
    Timeout,
    /// Not reachable (not running, wrong address).
    Network(String),
    /// The answer was not what an OpenAI-compatible endpoint sends.
    BadResponse(String),
    Cancelled,
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Unauthorized => write!(f, "{}", t!("endpoint.error_key")),
            ClientError::ToolsUnsupported(m) => write!(f, "{}", t!("endpoint.error_tools", message = m)),
            ClientError::Http(code, m) => write!(f, "{}", t!("endpoint.error_http", code = code, message = m)),
            ClientError::Timeout => write!(f, "{}", t!("endpoint.error_timeout")),
            ClientError::Network(m) => write!(f, "{}", t!("endpoint.error_network", message = m)),
            ClientError::BadResponse(m) => write!(f, "{}", t!("endpoint.error_response", message = m)),
            ClientError::Cancelled => write!(f, "{}", t!("endpoint.error_cancelled")),
        }
    }
}

/// A tool call the model asked for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// JSON as the model wrote it.
    pub arguments: String,
}

/// What one completion brought: text, and tools to call.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reply {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Clone)]
pub struct Endpoint {
    base: String,
    model: String,
    key: Option<String>,
    agent: ureq::Agent,
}

impl Endpoint {
    pub fn new(base_url: &str, model: &str, key: Option<String>, timeout: Duration) -> Endpoint {
        let crypto = Arc::new(rustls::crypto::ring::default_provider());
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::PlatformVerifier)
            .unversioned_rustls_crypto_provider(crypto)
            .build();
        let agent = ureq::Agent::config_builder()
            .tls_config(tls)
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(10)))
            // Waiting for the first token can take long on a local model; streaming then goes on.
            .timeout_recv_response(Some(timeout))
            .timeout_per_read(Some(timeout))
            .build()
            .new_agent();
        Endpoint { base: base_url.trim_end_matches('/').to_owned(), model: model.to_owned(), key, agent }
    }

    pub fn from_config(cfg: &EndpointConfig, key: Option<String>) -> Endpoint {
        Endpoint::new(&cfg.base_url, &cfg.model, key, Duration::from_secs(cfg.timeout_s.max(10) as u64))
    }

    fn post(&self, path: &str) -> ureq::RequestBuilder<ureq::typestate::WithBody> {
        let request = self.agent.post(format!("{}{path}", self.base)).header("Content-Type", "application/json");
        match &self.key {
            Some(key) => request.header("Authorization", format!("Bearer {key}")),
            None => request,
        }
    }

    /// The models the endpoint offers.
    pub fn models(&self) -> Result<Vec<String>, ClientError> {
        let mut request = self.agent.get(format!("{}/models", self.base));
        if let Some(key) = &self.key {
            request = request.header("Authorization", format!("Bearer {key}"));
        }
        let mut response = request.call().map_err(network)?;
        check(&mut response)?;
        let body: Value = response.body_mut().read_json().map_err(|e| ClientError::BadResponse(e.to_string()))?;
        let mut models: Vec<String> = body["data"]
            .as_array()
            .map(|a| a.iter().filter_map(|m| m["id"].as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        models.sort();
        Ok(models)
    }

    /// A completion, streamed: `on_text` gets the text as it comes. With `tools` the model may
    /// answer with tool calls instead.
    pub fn chat(
        &self,
        messages: &[Value],
        tools: Option<&[Value]>,
        on_text: &mut dyn FnMut(&str),
        cancel: &AtomicBool,
    ) -> Result<Reply, ClientError> {
        let mut body = json!({"model": self.model, "messages": messages, "stream": true});
        if let Some(tools) = tools.filter(|t| !t.is_empty()) {
            body["tools"] = json!(tools);
        }
        let mut response = self.post("/chat/completions").send_json(&body).map_err(network)?;
        if let Err(e) = check(&mut response) {
            return Err(match e {
                ClientError::Http(400 | 404 | 422, m) if tools.is_some() && m.to_lowercase().contains("tool") => {
                    ClientError::ToolsUnsupported(m)
                }
                e => e,
            });
        }
        let streamed = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.contains("event-stream"));
        let mut reply = Reply::default();
        if !streamed {
            // Some servers ignore "stream": the whole answer at once.
            let value: Value = response.body_mut().read_json().map_err(|e| ClientError::BadResponse(e.to_string()))?;
            let message = &value["choices"][0]["message"];
            reply.text = message["content"].as_str().unwrap_or_default().to_owned();
            on_text(&reply.text);
            for (i, call) in message["tool_calls"].as_array().into_iter().flatten().enumerate() {
                reply.tool_calls.push(ToolCall {
                    id: call["id"].as_str().map_or_else(|| format!("call_{i}"), str::to_owned),
                    name: call["function"]["name"].as_str().unwrap_or_default().to_owned(),
                    arguments: arguments_text(&call["function"]["arguments"]),
                });
            }
            return Ok(reply);
        }
        let reader = BufReader::new(response.into_body().into_reader());
        for line in reader.lines() {
            if cancel.load(Ordering::Relaxed) {
                return Err(ClientError::Cancelled);
            }
            let line = line.map_err(|e| {
                if e.kind() == std::io::ErrorKind::TimedOut {
                    ClientError::Timeout
                } else {
                    ClientError::Network(e.to_string())
                }
            })?;
            let Some(data) = line.strip_prefix("data:").map(str::trim) else { continue };
            if data == "[DONE]" {
                break;
            }
            let Ok(chunk) = serde_json::from_str::<Value>(data) else { continue };
            if let Some(message) = chunk["error"]["message"].as_str() {
                return Err(ClientError::Http(500, message.to_owned()));
            }
            let delta = &chunk["choices"][0]["delta"];
            if let Some(text) = delta["content"].as_str().filter(|t| !t.is_empty()) {
                reply.text.push_str(text);
                on_text(text);
            }
            for call in delta["tool_calls"].as_array().into_iter().flatten() {
                let index = call["index"].as_u64().unwrap_or(reply.tool_calls.len() as u64) as usize;
                while reply.tool_calls.len() <= index {
                    reply.tool_calls.push(ToolCall::default());
                }
                let entry = &mut reply.tool_calls[index];
                if let Some(id) = call["id"].as_str() {
                    entry.id = id.to_owned();
                }
                if let Some(name) = call["function"]["name"].as_str() {
                    entry.name.push_str(name);
                }
                entry.arguments.push_str(&arguments_text(&call["function"]["arguments"]));
            }
        }
        for (i, call) in reply.tool_calls.iter_mut().enumerate() {
            if call.id.is_empty() {
                call.id = format!("call_{i}");
            }
        }
        reply.tool_calls.retain(|c| !c.name.is_empty());
        Ok(reply)
    }

    /// A completion as a whole, asking for JSON (the one-click actions): the schema first, then
    /// plain JSON mode, then only the prompt, as far as the endpoint goes along.
    pub fn complete_json(&self, messages: &[Value], schema: &Value) -> Result<String, ClientError> {
        let formats = [
            Some(json!({"type": "json_schema", "json_schema": {"name": "answer", "schema": schema}})),
            Some(json!({"type": "json_object"})),
            None,
        ];
        let mut last = ClientError::BadResponse("no answer".into());
        for format in formats {
            let mut body = json!({"model": self.model, "messages": messages, "stream": false, "temperature": 0.2});
            if let Some(f) = &format {
                body["response_format"] = f.clone();
            }
            let mut response = self.post("/chat/completions").send_json(&body).map_err(network)?;
            match check(&mut response) {
                Ok(()) => {
                    let value: Value =
                        response.body_mut().read_json().map_err(|e| ClientError::BadResponse(e.to_string()))?;
                    return Ok(value["choices"][0]["message"]["content"].as_str().unwrap_or_default().to_owned());
                }
                // This format is not supported: try the next one.
                Err(ClientError::Http(400 | 422, m)) => last = ClientError::Http(400, m),
                Err(e) => return Err(e),
            }
        }
        Err(last)
    }
}

/// Arguments arrive as a JSON string, or (some servers) as an object.
fn arguments_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn network(e: ureq::Error) -> ClientError {
    match e {
        ureq::Error::Timeout(_) => ClientError::Timeout,
        e => ClientError::Network(e.to_string()),
    }
}

/// Turns an error status into a [`ClientError`] with the endpoint's message.
fn check(response: &mut ureq::http::Response<ureq::Body>) -> Result<(), ClientError> {
    let status = response.status().as_u16();
    if status < 400 {
        return Ok(());
    }
    if status == 401 || status == 403 {
        return Err(ClientError::Unauthorized);
    }
    let text = response.body_mut().read_to_string().unwrap_or_default();
    let message = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().or(v["error"].as_str()).map(str::to_owned))
        .unwrap_or(text);
    Err(ClientError::Http(status, message.chars().take(300).collect()))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;

    /// A tiny HTTP server: answers each request with the next of `answers` (status, content
    /// type, body) and keeps the request bodies.
    pub fn serve(answers: Vec<(u16, &'static str, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for (status, kind, body) in answers {
                let Ok((mut stream, _)) = listener.accept() else { return };
                let mut request = Vec::new();
                let mut buf = [0u8; 8192];
                // Headers, then as much body as Content-Length says.
                loop {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    request.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&request).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length || n == 0 {
                            break;
                        }
                    } else if n == 0 {
                        break;
                    }
                }
                log.lock().unwrap().push(String::from_utf8_lossy(&request).to_string());
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body.as_bytes());
            }
        });
        (url, seen)
    }

    pub fn sse(chunks: &[Value]) -> String {
        let mut out: String = chunks.iter().map(|c| format!("data: {c}\n\n")).collect();
        out.push_str("data: [DONE]\n\n");
        out
    }

    fn endpoint(url: &str, key: Option<&str>) -> Endpoint {
        Endpoint::new(url, "test-model", key.map(str::to_owned), Duration::from_secs(5))
    }

    #[test]
    fn streams_text_and_tool_calls() {
        let body = sse(&[
            json!({"choices": [{"delta": {"content": "Ich "}}]}),
            json!({"choices": [{"delta": {"content": "suche."}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c1", "function": {"name": "search_models", "arguments": "{\"q\":"}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "\"tag:garten\"}"}}]}}]}),
        ]);
        let (url, seen) = serve(vec![(200, "text/event-stream", body)]);
        let mut streamed = String::new();
        let reply = endpoint(&url, Some("geheim"))
            .chat(
                &[json!({"role": "user", "content": "Garten?"})],
                Some(&[json!({"type": "function"})]),
                &mut |t| streamed.push_str(t),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!((reply.text.as_str(), streamed.as_str()), ("Ich suche.", "Ich suche."));
        assert_eq!(
            reply.tool_calls,
            [ToolCall { id: "c1".into(), name: "search_models".into(), arguments: "{\"q\":\"tag:garten\"}".into() }]
        );
        let request = seen.lock().unwrap()[0].clone();
        assert!(request.contains("authorization: Bearer geheim"), "{request}");
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!((body["stream"].as_bool(), body["model"].as_str()), (Some(true), Some("test-model")));
        assert!(body["tools"].is_array());
    }

    #[test]
    fn says_what_went_wrong() {
        let (url, _) = serve(vec![
            (401, "application/json", "{}".into()),
            (400, "application/json", json!({"error": {"message": "model does not support tools"}}).to_string()),
            (200, "application/json", json!({"data": [{"id": "qwen3"}, {"id": "gemma3"}]}).to_string()),
        ]);
        let e = endpoint(&url, None);
        let msgs = [json!({"role": "user", "content": "x"})];
        assert_eq!(e.chat(&msgs, None, &mut |_| {}, &AtomicBool::new(false)), Err(ClientError::Unauthorized));
        assert!(matches!(
            e.chat(&msgs, Some(&[json!({})]), &mut |_| {}, &AtomicBool::new(false)),
            Err(ClientError::ToolsUnsupported(_))
        ));
        assert_eq!(e.models().unwrap(), ["gemma3", "qwen3"]);
        let nowhere = endpoint("http://127.0.0.1:9/v1", None);
        assert!(matches!(nowhere.models(), Err(ClientError::Network(_))));
    }

    #[test]
    fn json_answers_fall_back_to_plainer_formats() {
        let (url, seen) = serve(vec![
            (
                400,
                "application/json",
                json!({"error": {"message": "response_format json_schema not supported"}}).to_string(),
            ),
            (200, "application/json", json!({"choices": [{"message": {"content": "{\"ok\":true}"}}]}).to_string()),
        ]);
        let text = endpoint(&url, None)
            .complete_json(&[json!({"role": "user", "content": "x"})], &json!({"type": "object"}))
            .unwrap();
        assert_eq!(text, "{\"ok\":true}");
        let requests = seen.lock().unwrap();
        assert!(requests[0].contains("json_schema") && requests[1].contains("json_object"));
    }
}
