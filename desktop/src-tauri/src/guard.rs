//! Untrusted source content handling and audit redaction (docs/THREAT-MODEL.md).
//! Email/Teams text is data, never instructions: it is wrapped in a per-run random
//! boundary the source cannot forge, control characters are stripped and size is
//! capped. Enforcement does not rely on the model obeying the wrapper: the tool
//! allow-list (`tools`) and approval binding (`proposals`) hold regardless.
use crate::tools::{truncate_bytes, MAX_EXCERPT_BYTES};
use rusqlite::{params, Connection};

/// Random boundary for one inference run.
pub fn new_boundary() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

pub fn wrap_untrusted(boundary: &str, source_id: &str, text: &str) -> String {
    let clean: String = text
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    // A source cannot close the block early: any occurrence of the boundary is removed
    // (repeat until stable so nested splices cannot reassemble it).
    let mut body = clean;
    while body.contains(boundary) {
        body = body.replace(boundary, "");
    }
    let body = truncate_bytes(&body, MAX_EXCERPT_BYTES);
    format!(
        "<untrusted_source id=\"{source_id}\" boundary=\"{boundary}\">\n{body}\n</untrusted_source boundary=\"{boundary}\">"
    )
}

const SENSITIVE_KEYS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "code=",
    "assertion",
    "cookie",
    "apikey",
    "api_key",
];

pub fn looks_like_secret(word: &str) -> bool {
    let lower = word.to_ascii_lowercase();
    if lower.contains("eyj") || SENSITIVE_KEYS.iter().any(|k| lower.contains(k)) {
        return true;
    }
    let run = word
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+' | '/' | '='))
        .count();
    run >= 32 && run * 10 >= word.chars().count() * 9
}

/// Redacts bearer tokens, JWTs, key=value secrets and long opaque strings.
pub fn redact(detail: &str) -> String {
    let mut out = Vec::new();
    let mut redact_next = false;
    for word in detail.split_whitespace() {
        if redact_next || looks_like_secret(word) {
            out.push("[REDACTED]");
            redact_next = false;
        } else {
            redact_next = word.eq_ignore_ascii_case("bearer") || word.eq_ignore_ascii_case("basic");
            out.push(word);
        }
    }
    out.join(" ")
}

#[derive(Clone, Copy, Debug)]
pub enum Actor {
    User,
    Ai,
    Sync,
    Worker,
}

impl Actor {
    fn as_str(self) -> &'static str {
        match self {
            Actor::User => "User",
            Actor::Ai => "Ai",
            Actor::Sync => "Sync",
            Actor::Worker => "Worker",
        }
    }
}

pub struct AuditEntry<'a> {
    pub at: &'a str,
    pub actor: Actor,
    pub action: &'a str,
    pub target_type: &'a str,
    pub target_id: Option<&'a str>,
    pub outcome: &'a str,
    pub detail: &'a str,
}

/// Appends a security audit event; detail is always redacted before storage.
pub fn audit(conn: &Connection, e: &AuditEntry<'_>) -> rusqlite::Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO audit_events(id, at, actor, action, target_type, target_id, outcome, detail_redacted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            id,
            e.at,
            e.actor.as_str(),
            e.action,
            e.target_type,
            e.target_id,
            e.outcome,
            redact(e.detail)
        ],
    )?;
    Ok(id)
}
