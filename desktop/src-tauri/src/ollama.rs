//! Local Ollama inference adapter (docs/INTERFACES.md §5, ADR-007).
//! Loopback only; plain HTTP/1.0 over std TcpStream so responses are not chunked and
//! no extra dependency is needed. Model output is untrusted: tool calls it emits are
//! returned as raw JSON strings for `tools::parse_call` validation, never executed here.
use serde::Serialize;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const DEFAULT_PORT: u16 = 11434;
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PROMPT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OllamaStatus {
    NotRunning,
    ModelMissing,
    Ready { model: String, digest: String },
    Error(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum OllamaError {
    NonLoopback,
    BadUrl,
    NotRunning,
    Timeout,
    Cancelled,
    TooLarge,
    Protocol(String),
}

#[derive(Debug, Clone)]
pub struct OllamaClient {
    addr: SocketAddr,
    timeout: Duration,
}

/// Parses `http://127.0.0.1:port` / `http://[::1]:port`; anything else is rejected.
pub fn parse_loopback_url(url: &str) -> Result<SocketAddr, OllamaError> {
    let rest = url.strip_prefix("http://").ok_or(OllamaError::BadUrl)?;
    let hostport = rest.trim_end_matches('/');
    if hostport.contains('/') || hostport.contains('@') {
        return Err(OllamaError::BadUrl);
    }
    let addr: SocketAddr = if hostport.starts_with('[') || hostport.contains(':') {
        hostport.parse().map_err(|_| OllamaError::BadUrl)?
    } else {
        let ip: IpAddr = hostport.parse().map_err(|_| OllamaError::NonLoopback)?;
        SocketAddr::new(ip, 80)
    };
    if !addr.ip().is_loopback() {
        return Err(OllamaError::NonLoopback);
    }
    Ok(addr)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatOutput {
    pub content: String,
    /// Raw `{"name":..,"arguments":..}` JSON for each tool call; validate before use.
    pub tool_calls: Vec<String>,
}

impl OllamaClient {
    pub fn new(url: &str, timeout_s: u64) -> Result<Self, OllamaError> {
        Ok(Self {
            addr: parse_loopback_url(url)?,
            timeout: Duration::from_secs(timeout_s.clamp(1, 600)),
        })
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> Result<BufReader<TcpStream>, OllamaError> {
        let mut s = TcpStream::connect_timeout(&self.addr, Duration::from_secs(3))
            .map_err(|_| OllamaError::NotRunning)?;
        s.set_read_timeout(Some(self.timeout)).ok();
        s.set_write_timeout(Some(self.timeout)).ok();
        let body = body.unwrap_or("");
        let req = format!(
            "{method} {path} HTTP/1.0\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            self.addr,
            body.len()
        );
        s.write_all(req.as_bytes()).map_err(io_err)?;
        let mut r = BufReader::new(s);
        let mut status = String::new();
        r.read_line(&mut status).map_err(io_err)?;
        let code = status.split_whitespace().nth(1).unwrap_or("");
        loop {
            let mut h = String::new();
            if r.read_line(&mut h).map_err(io_err)? == 0 || h == "\r\n" || h == "\n" {
                break;
            }
        }
        if code != "200" {
            return Err(OllamaError::Protocol(format!("http status {code}")));
        }
        Ok(r)
    }

    /// `GET /api/tags`: decides running / model presence and reports the digest.
    pub fn status(&self, model: &str) -> OllamaStatus {
        let r = match self.request("GET", "/api/tags", None) {
            Ok(r) => r,
            Err(OllamaError::NotRunning) => return OllamaStatus::NotRunning,
            Err(e) => return OllamaStatus::Error(format!("{e:?}")),
        };
        let mut body = Vec::new();
        if r.take(MAX_RESPONSE_BYTES as u64)
            .read_to_end(&mut body)
            .is_err()
        {
            return OllamaStatus::Error("read".into());
        }
        status_from_tags(&body, model)
    }

    /// `POST /api/chat` with `stream: true`; reads NDJSON until `done`, honoring a
    /// cancellation flag and response size cap.
    pub fn chat(
        &self,
        model: &str,
        messages: &serde_json::Value,
        tools: &serde_json::Value,
        num_ctx: u32,
        cancel: &AtomicBool,
    ) -> Result<ChatOutput, OllamaError> {
        let body = chat_body(model, messages, tools, num_ctx, None)?;
        self.chat_request(&body, cancel)
    }

    /// Schema-constrained, no-tools output for the informational Assistant briefing.
    /// Ordinary chat/academic analysis retain their existing request options.
    pub fn chat_structured(
        &self,
        model: &str,
        messages: &serde_json::Value,
        schema: &serde_json::Value,
        cancel: &AtomicBool,
    ) -> Result<ChatOutput, OllamaError> {
        let body = chat_body(model, messages, &serde_json::json!([]), 8192, Some(schema))?;
        self.chat_request(&body, cancel)
    }

    fn chat_request(&self, body: &str, cancel: &AtomicBool) -> Result<ChatOutput, OllamaError> {
        let mut r = self.request("POST", "/api/chat", Some(body))?;
        let mut out = ChatOutput {
            content: String::new(),
            tool_calls: Vec::new(),
        };
        let mut total = 0usize;
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err(OllamaError::Cancelled);
            }
            let mut line = String::new();
            let n = r.read_line(&mut line).map_err(io_err)?;
            if n == 0 {
                return Err(OllamaError::Protocol("stream ended before done".into()));
            }
            total += n;
            if total > MAX_RESPONSE_BYTES {
                return Err(OllamaError::TooLarge);
            }
            if line.trim().is_empty() {
                continue;
            }
            let v: serde_json::Value =
                serde_json::from_str(&line).map_err(|e| OllamaError::Protocol(e.to_string()))?;
            if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
                return Err(OllamaError::Protocol(err.chars().take(200).collect()));
            }
            if let Some(c) = v.pointer("/message/content").and_then(|c| c.as_str()) {
                out.content.push_str(c);
            }
            if let Some(calls) = v.pointer("/message/tool_calls").and_then(|c| c.as_array()) {
                for c in calls {
                    if let Some(f) = c.get("function") {
                        out.tool_calls.push(f.to_string());
                    }
                }
            }
            if v.get("done").and_then(|d| d.as_bool()) == Some(true) {
                return Ok(out);
            }
        }
    }
}

fn chat_body(
    model: &str,
    messages: &serde_json::Value,
    tools: &serde_json::Value,
    num_ctx: u32,
    schema: Option<&serde_json::Value>,
) -> Result<String, OllamaError> {
    let mut body = serde_json::json!({
        "model": model, "messages": messages, "tools": tools, "stream": true,
        "options": { "num_ctx": num_ctx }
    });
    if let Some(schema) = schema {
        body["format"] = schema.clone();
        body["options"]["temperature"] = serde_json::json!(0);
    }
    let body = body.to_string();
    if body.len() > MAX_PROMPT_BYTES {
        return Err(OllamaError::TooLarge);
    }
    Ok(body)
}

fn io_err(e: std::io::Error) -> OllamaError {
    match e.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => OllamaError::Timeout,
        _ => OllamaError::Protocol(e.kind().to_string()),
    }
}

/// Installed model names from an `/api/tags` body (bounded, valid names only).
pub fn model_names(body: &[u8]) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
    let mut out: Vec<String> = v
        .get("models")
        .and_then(|m| m.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|m| m.get("name").and_then(|n| n.as_str()))
                .filter(|n| {
                    !n.is_empty()
                        && n.len() <= 100
                        && n.chars().all(|c| {
                            c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '-' | '_' | '/')
                        })
                })
                .take(100)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out.dedup();
    out
}

impl OllamaClient {
    /// `GET /api/tags` on the fixed loopback address: names of installed models.
    pub fn list_models(&self) -> Result<Vec<String>, OllamaError> {
        let r = self.request("GET", "/api/tags", None)?;
        let mut body = Vec::new();
        r.take(MAX_RESPONSE_BYTES as u64)
            .read_to_end(&mut body)
            .map_err(|_| OllamaError::Protocol("read".into()))?;
        Ok(model_names(&body))
    }
}

/// Pure parser for the `/api/tags` body; tag matching accepts an implicit `:latest`.
pub fn status_from_tags(body: &[u8], model: &str) -> OllamaStatus {
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return OllamaStatus::Error("invalid tags json".into()),
    };
    let want = if model.contains(':') {
        model.to_string()
    } else {
        format!("{model}:latest")
    };
    let models = v
        .get("models")
        .and_then(|m| m.as_array())
        .cloned()
        .unwrap_or_default();
    for m in models {
        let name = m.get("name").and_then(|n| n.as_str()).unwrap_or("");
        if name == want || name == model {
            let digest = m
                .get("digest")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            return OllamaStatus::Ready {
                model: name.to_string(),
                digest,
            };
        }
    }
    OllamaStatus::ModelMissing
}

#[cfg(test)]
mod request_tests {
    use super::*;
    use serde_json::{json, Value};

    #[test]
    fn structured_request_sends_schema_without_changing_ordinary_chat_options() {
        let messages = json!([{"role":"user","content":"Synthetic notice"}]);
        let schema = json!({"type":"object","properties":{"points":{"type":"array"}}});
        let structured: Value = serde_json::from_str(
            &chat_body("test", &messages, &json!([]), 8192, Some(&schema)).unwrap(),
        )
        .unwrap();
        assert_eq!(structured["format"], schema);
        assert_eq!(structured["messages"], messages);
        assert_eq!(structured["tools"], json!([]));
        assert_eq!(
            structured["options"],
            json!({"num_ctx":8192,"temperature":0})
        );
        assert_eq!(structured["stream"], true);
        assert!(structured.get("think").is_none());
        let ordinary: Value =
            serde_json::from_str(&chat_body("test", &messages, &json!([]), 4096, None).unwrap())
                .unwrap();
        assert!(ordinary.get("format").is_none());
        assert_eq!(ordinary["options"], json!({"num_ctx":4096}));
    }

    #[test]
    fn structured_schema_and_source_share_the_existing_request_size_limit() {
        assert_eq!(
            chat_body(
                "test",
                &json!([]),
                &json!([]),
                8192,
                Some(&json!({"description":"x".repeat(MAX_PROMPT_BYTES)}))
            ),
            Err(OllamaError::TooLarge)
        );
    }
}
