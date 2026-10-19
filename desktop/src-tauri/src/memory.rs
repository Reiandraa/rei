//! P06 local memory: explicit consent, provenance, edit/export/forget, TTL retention,
//! FTS search and embedding version tracking (docs/INTERFACES.md §2 memory row).
//! Deleting a memory removes its FTS row (trigger) and embeddings (FK cascade).
use crate::guard::{audit, looks_like_secret, Actor, AuditEntry};
use crate::time::format_unix;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

pub const MAX_TEXT_CHARS: usize = 2000;
pub const MAX_TTL_DAYS: i64 = 3650;
pub const MAX_SEARCH_LIMIT: u32 = 20;

#[derive(Debug)]
pub enum MemoryError {
    /// Memory writes require an explicit user consent flag.
    ConsentRequired,
    /// Passwords, tokens and other secrets are never stored as memories.
    SecretRejected,
    Bounds(&'static str),
    NotFound,
    Db(rusqlite::Error),
}

impl From<rusqlite::Error> for MemoryError {
    fn from(e: rusqlite::Error) -> Self {
        MemoryError::Db(e)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Provenance {
    User,
    AiProposal,
    Import,
}

impl Provenance {
    fn as_str(self) -> &'static str {
        match self {
            Provenance::User => "User",
            Provenance::AiProposal => "AiProposal",
            Provenance::Import => "Import",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Memory {
    pub id: String,
    pub text: String,
    pub provenance: String,
    pub source_ids: Vec<String>,
    pub consent_at: String,
    pub created_at: String,
    pub updated_at: String,
    pub expires_at: Option<String>,
}

pub struct NewMemory<'a> {
    pub text: &'a str,
    pub provenance: Provenance,
    pub source_ids: &'a [String],
    /// Must be true: set only by an explicit user action (consent dialog/approval).
    pub user_consented: bool,
    pub ttl_days: Option<i64>,
}

fn validate_text(text: &str) -> Result<String, MemoryError> {
    let t = text.trim();
    if t.is_empty() || t.chars().count() > MAX_TEXT_CHARS {
        return Err(MemoryError::Bounds("text"));
    }
    if t.split_whitespace().any(looks_like_secret) {
        return Err(MemoryError::SecretRejected);
    }
    Ok(t.to_string())
}

fn row_to_memory(r: &rusqlite::Row<'_>) -> rusqlite::Result<Memory> {
    let src: String = r.get(3)?;
    Ok(Memory {
        id: r.get(0)?,
        text: r.get(1)?,
        provenance: r.get(2)?,
        source_ids: serde_json::from_str(&src).unwrap_or_default(),
        consent_at: r.get(4)?,
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
        expires_at: r.get(7)?,
    })
}

const COLS: &str = "m.id, m.text, m.provenance, m.source_ids, m.consent_at, m.created_at, m.updated_at, m.expires_at";

fn log(conn: &Connection, now: i64, action: &str, id: &str, outcome: &str) -> rusqlite::Result<()> {
    audit(
        conn,
        &AuditEntry {
            at: &format_unix(now),
            actor: Actor::User,
            action,
            target_type: "memory",
            target_id: Some(id),
            outcome,
            detail: "",
        },
    )
    .map(|_| ())
}

pub fn create(conn: &Connection, m: &NewMemory<'_>, now: i64) -> Result<Memory, MemoryError> {
    if !m.user_consented {
        return Err(MemoryError::ConsentRequired);
    }
    let text = validate_text(m.text)?;
    if m.source_ids.len() > 16 {
        return Err(MemoryError::Bounds("source_ids"));
    }
    let expires_at = match m.ttl_days {
        None => None,
        Some(d) if (1..=MAX_TTL_DAYS).contains(&d) => Some(format_unix(now + d * 86_400)),
        Some(_) => return Err(MemoryError::Bounds("ttl_days")),
    };
    let id = uuid::Uuid::new_v4().to_string();
    let ts = format_unix(now);
    let sources = serde_json::to_string(m.source_ids).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "INSERT INTO memories(id, text, provenance, source_ids, consent_at, created_at, updated_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?5, ?6)",
        params![id, text, m.provenance.as_str(), sources, ts, expires_at],
    )?;
    log(conn, now, "memory.create", &id, "ok")?;
    get(conn, &id, now)?.ok_or(MemoryError::NotFound)
}

/// Returns a live (non-expired) memory.
pub fn get(conn: &Connection, id: &str, now: i64) -> Result<Option<Memory>, MemoryError> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLS} FROM memories m WHERE m.id = ?1 AND (m.expires_at IS NULL OR m.expires_at > ?2)"),
            params![id, format_unix(now)],
            row_to_memory,
        )
        .optional()?)
}

pub fn list(conn: &Connection, now: i64) -> Result<Vec<Memory>, MemoryError> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLS} FROM memories m WHERE m.expires_at IS NULL OR m.expires_at > ?1 ORDER BY m.created_at DESC, m.id"
    ))?;
    let rows = st.query_map([format_unix(now)], row_to_memory)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Edits text; the FTS row is reindexed and stale embeddings dropped by trigger.
pub fn update(conn: &Connection, id: &str, text: &str, now: i64) -> Result<Memory, MemoryError> {
    let text = validate_text(text)?;
    if get(conn, id, now)?.is_none() {
        return Err(MemoryError::NotFound);
    }
    conn.execute(
        "UPDATE memories SET text = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, text, format_unix(now)],
    )?;
    log(conn, now, "memory.update", id, "ok")?;
    get(conn, id, now)?.ok_or(MemoryError::NotFound)
}

/// Forgets a memory: row, FTS entry and embeddings are removed in one transaction.
pub fn delete(conn: &mut Connection, id: &str, now: i64) -> Result<(), MemoryError> {
    let tx = conn.transaction()?;
    let n = tx.execute("DELETE FROM memories WHERE id = ?1", [id])?;
    if n == 0 {
        return Err(MemoryError::NotFound);
    }
    log(&tx, now, "memory.delete", id, "ok")?;
    tx.commit()?;
    Ok(())
}

/// Retention: permanently removes expired memories (and their FTS/embeddings).
pub fn purge_expired(conn: &mut Connection, now: i64) -> Result<usize, MemoryError> {
    let tx = conn.transaction()?;
    let ids: Vec<String> = {
        let mut st = tx
            .prepare("SELECT id FROM memories WHERE expires_at IS NOT NULL AND expires_at <= ?1")?;
        let rows = st.query_map([format_unix(now)], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    for id in &ids {
        tx.execute("DELETE FROM memories WHERE id = ?1", [id])?;
        audit(
            &tx,
            &AuditEntry {
                at: &format_unix(now),
                actor: Actor::Worker,
                action: "memory.expire",
                target_type: "memory",
                target_id: Some(id),
                outcome: "ok",
                detail: "",
            },
        )?;
    }
    tx.commit()?;
    Ok(ids.len())
}

/// Converts free text into an FTS5 query of quoted terms so user or model input
/// cannot inject FTS operators (NEAR, column filters, `*`, etc.).
pub fn fts_query(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .take(16)
        .map(|t| format!("\"{t}\""))
        .collect();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

pub fn search(
    conn: &Connection,
    query: &str,
    limit: u32,
    now: i64,
) -> Result<Vec<Memory>, MemoryError> {
    if limit == 0 || limit > MAX_SEARCH_LIMIT || query.chars().count() > 500 {
        return Err(MemoryError::Bounds("search"));
    }
    let Some(q) = fts_query(query) else {
        return Ok(Vec::new());
    };
    let mut st = conn.prepare(&format!(
        "SELECT {COLS} FROM memory_fts f JOIN memories m ON m.id = f.id
         WHERE memory_fts MATCH ?1 AND (m.expires_at IS NULL OR m.expires_at > ?2)
         ORDER BY bm25(memory_fts) LIMIT ?3"
    ))?;
    let rows = st.query_map(params![q, format_unix(now), limit], row_to_memory)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Portable user export of all live memories (JSON). Writing to a file requires a
/// renderer-issued file grant and is handled by the command layer.
pub fn export_json(conn: &Connection, now: i64) -> Result<String, MemoryError> {
    let all = list(conn, now)?;
    Ok(
        serde_json::json!({ "format": "rei-os-memory-export", "version": 1, "memories": all })
            .to_string(),
    )
}

pub fn put_embedding(
    conn: &Connection,
    id: &str,
    model: &str,
    version: i64,
    vector: &[f32],
) -> Result<(), MemoryError> {
    let blob: Vec<u8> = vector.iter().flat_map(|f| f.to_le_bytes()).collect();
    conn.execute(
        "INSERT INTO memory_embeddings(memory_id, model, version, dims, vector) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(memory_id, model) DO UPDATE SET version = excluded.version, dims = excluded.dims, vector = excluded.vector",
        params![id, model, version, vector.len() as i64, blob],
    )?;
    Ok(())
}

/// Memories lacking an embedding for (model, version): the rebuild work list after an
/// embedding model change or text edit.
pub fn needs_embedding(
    conn: &Connection,
    model: &str,
    version: i64,
) -> Result<Vec<String>, MemoryError> {
    let mut st = conn.prepare(
        "SELECT m.id FROM memories m WHERE NOT EXISTS (
           SELECT 1 FROM memory_embeddings e WHERE e.memory_id = m.id AND e.model = ?1 AND e.version = ?2)
         ORDER BY m.id",
    )?;
    let rows = st.query_map(params![model, version], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}
