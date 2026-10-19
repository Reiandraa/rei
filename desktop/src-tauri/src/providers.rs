//! Shared provider-connector state (docs/PROVIDER-CONNECTORS-NEXT.md): non-secret
//! configuration, explicit source allow-lists, honest run status, cancellation and
//! opt-in polling. Network transports live in `hiz` and `teams`; neither holds the
//! database lock across network I/O.
use crate::connectors::{self, ConnectorError, Evidence, Kind};
use crate::guard;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

pub const MAX_SOURCES: usize = 500;
pub const MIN_INTERVAL_MIN: u32 = 15;
pub const MAX_INTERVAL_MIN: u32 = 1440;

/// Transport-independent connector failure. Messages are fixed English text and
/// never contain credentials, tokens, URLs with cursors or message content.
#[derive(Debug, PartialEq, Eq)]
pub enum ProviderError {
    NotConfigured(&'static str),
    SecretMissing,
    VaultUnavailable,
    Tls,
    Network,
    Auth,
    /// Provider refused (e.g. HTTP 403); may be missing tenant consent, not a proven denial.
    Forbidden,
    /// The owner/tenant explicitly denied the sign-in or consent.
    Denied,
    Protocol(&'static str),
    Destination,
    Cancelled,
    TimedOut,
    Busy,
    Invalid(String),
    Db,
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use ProviderError::*;
        match self {
            NotConfigured(m) => write!(f, "Setup incomplete: {m}"),
            SecretMissing => f.write_str("No saved sign-in for this account. Sign in again."),
            VaultUnavailable => f.write_str(
                "The operating-system credential store is unavailable; nothing was saved.",
            ),
            Tls => f.write_str("Secure connection failed (TLS certificate or handshake rejected)."),
            Network => f.write_str("Network error while contacting the provider."),
            Auth => f.write_str("The provider rejected the sign-in."),
            Forbidden => f.write_str(
                "The provider refused access (403). The tenant may require admin consent; this is not confirmed as a denial.",
            ),
            Denied => f.write_str("Sign-in or consent was declined."),
            Protocol(m) => write!(f, "Unexpected provider response: {m}"),
            Destination => f.write_str("Blocked: the destination is not an allowed provider host."),
            Cancelled => f.write_str("Cancelled by you."),
            TimedOut => f.write_str("Timed out."),
            Busy => f.write_str("A fetch for this account is already running."),
            Invalid(m) => write!(f, "Invalid input: {m}"),
            Db => f.write_str("Local database error."),
        }
    }
}

impl From<ConnectorError> for ProviderError {
    fn from(e: ConnectorError) -> Self {
        match e {
            ConnectorError::Invalid(m) => ProviderError::Invalid(m),
            ConnectorError::State(m) => ProviderError::Invalid(m),
            ConnectorError::NotFound => ProviderError::Invalid("unknown account".into()),
            ConnectorError::Db(_) => ProviderError::Db,
        }
    }
}

impl From<rusqlite::Error> for ProviderError {
    fn from(_: rusqlite::Error) -> Self {
        ProviderError::Db
    }
}

pub type PRes<T> = Result<T, ProviderError>;

pub fn lock(db: &Mutex<Connection>) -> PRes<std::sync::MutexGuard<'_, Connection>> {
    db.lock().map_err(|_| ProviderError::Db)
}

// ---------- configuration ----------

pub fn save_config(
    conn: &Connection,
    account_id: &str,
    provider: &str,
    config: &serde_json::Value,
    now: &str,
) -> PRes<()> {
    conn.execute(
        "INSERT INTO provider_config(account_id, provider, config, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(account_id) DO UPDATE SET config=excluded.config, updated_at=excluded.updated_at",
        params![account_id, provider, config.to_string(), now],
    )?;
    Ok(())
}

pub fn load_config(conn: &Connection, account_id: &str, provider: &str) -> PRes<serde_json::Value> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT config FROM provider_config WHERE account_id=?1 AND provider=?2",
            params![account_id, provider],
            |r| r.get(0),
        )
        .optional()?;
    let raw = raw.ok_or(ProviderError::NotConfigured("account settings are missing"))?;
    serde_json::from_str(&raw).map_err(|_| ProviderError::Db)
}

pub fn set_secret_saved(conn: &Connection, account_id: &str, saved: bool, now: &str) -> PRes<()> {
    conn.execute(
        "UPDATE provider_config SET secret_saved=?1, updated_at=?2 WHERE account_id=?3",
        params![saved as i32, now, account_id],
    )?;
    Ok(())
}

/// Records a fetch/test attempt. `ok` sets last success and last actual read.
pub fn record_outcome(
    conn: &Connection,
    account_id: &str,
    ok: bool,
    outcome: &str,
    now: &str,
) -> PRes<()> {
    let outcome: String = guard::redact(outcome).chars().take(300).collect();
    conn.execute(
        "UPDATE provider_config SET last_attempt_at=?1, last_outcome=?2,
           last_success_at=CASE WHEN ?3 THEN ?1 ELSE last_success_at END,
           last_read_ok_at=CASE WHEN ?3 THEN ?1 ELSE last_read_ok_at END,
           authenticated_at=CASE WHEN ?3 THEN ?1 ELSE authenticated_at END,
           updated_at=?1 WHERE account_id=?4",
        params![now, outcome, ok, account_id],
    )?;
    Ok(())
}

pub fn record_authenticated(conn: &Connection, account_id: &str, now: &str) -> PRes<()> {
    conn.execute(
        "UPDATE provider_config SET authenticated_at=?1, updated_at=?1 WHERE account_id=?2",
        params![now, account_id],
    )?;
    Ok(())
}

/// After a successful real read, moves the account to `connected` only when the
/// transport is the production one (Evidence::Live). Synthetic transports stay
/// `testing` with fixture evidence and can never appear connected.
pub fn mark_read_ok(
    conn: &Connection,
    account_id: &str,
    evidence: Evidence,
    now: &str,
) -> PRes<()> {
    let a = connectors::get_account(conn, account_id)?;
    use connectors::ConnState::*;
    if a.state == ConsentNeeded || a.state == Expired || a.state == Degraded {
        connectors::transition(conn, account_id, Testing, evidence, None, now)?;
    }
    if evidence == Evidence::Live {
        let a = connectors::get_account(conn, account_id)?;
        if a.state == Testing {
            connectors::transition(conn, account_id, Connected, Evidence::Live, None, now)?;
        }
    } else {
        conn.execute(
            "UPDATE connector_accounts SET evidence='fixture', updated_at=?1 WHERE id=?2 AND state <> 'connected'",
            params![now, account_id],
        )?;
    }
    Ok(())
}

/// Moves the account to a failure state where the state machine allows it.
pub fn mark_failed(conn: &Connection, account_id: &str, e: &ProviderError, now: &str) -> PRes<()> {
    use connectors::ConnState::*;
    let a = connectors::get_account(conn, account_id)?;
    let to = match e {
        ProviderError::Auth | ProviderError::SecretMissing => Expired,
        ProviderError::Denied => PermissionDenied,
        ProviderError::Cancelled => return Ok(()),
        _ => Degraded,
    };
    let msg = e.to_string();
    let path: &[connectors::ConnState] = match (a.state, to) {
        (ConsentNeeded, PermissionDenied) => &[PermissionDenied],
        (ConsentNeeded, _) | (Expired, Degraded) => &[Testing, to],
        (Testing, _) | (Connected, _) => &[to],
        (Degraded, _) => &[Testing, to],
        _ => &[],
    };
    for s in path {
        connectors::transition(conn, account_id, *s, a.evidence, Some(&msg), now)?;
    }
    Ok(())
}

// ---------- explicit allow-lists ----------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderSource {
    pub source_kind: String,
    pub source_ref: String,
    pub display_name: String,
    pub selected: bool,
}

/// Stores discovered sources without selecting any (owner must opt in).
pub fn upsert_discovered(
    conn: &Connection,
    account_id: &str,
    found: &[(String, String, String)],
    now: &str,
) -> PRes<()> {
    for (kind, r, name) in found.iter().take(MAX_SOURCES) {
        conn.execute(
            "INSERT INTO provider_sources(account_id, source_kind, source_ref, display_name, discovered_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(account_id, source_kind, source_ref) DO UPDATE SET display_name=excluded.display_name",
            params![account_id, kind, r, name.chars().take(300).collect::<String>(), now],
        )?;
    }
    Ok(())
}

pub fn list_sources(conn: &Connection, account_id: &str) -> PRes<Vec<ProviderSource>> {
    let mut st = conn.prepare(
        "SELECT source_kind, source_ref, display_name, selected FROM provider_sources
         WHERE account_id=?1 ORDER BY source_kind, display_name, source_ref",
    )?;
    let rows = st.query_map([account_id], |r| {
        Ok(ProviderSource {
            source_kind: r.get(0)?,
            source_ref: r.get(1)?,
            display_name: r.get(2)?,
            selected: r.get::<_, i32>(3)? == 1,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Selection may only name sources that were actually discovered for this account.
pub fn set_selected(
    conn: &mut Connection,
    account_id: &str,
    selected: &[(String, String)],
) -> PRes<u32> {
    let _run = operation_guard(conn, account_id)?;
    let provider: String = conn.query_row(
        "SELECT provider FROM provider_config WHERE account_id=?1",
        [account_id],
        |r| r.get(0),
    )?;
    let limit = if provider == crate::hiz::PROVIDER {
        crate::hiz::MAX_FOLDERS
    } else {
        MAX_SOURCES
    };
    if selected.len() > limit {
        return Err(ProviderError::Invalid(format!(
            "select at most {limit} sources for this account"
        )));
    }
    let tx = conn.transaction()?;
    tx.execute(
        "UPDATE provider_sources SET selected=0 WHERE account_id=?1",
        [account_id],
    )?;
    let mut n = 0;
    for (kind, r) in selected {
        let c = tx.execute(
            "UPDATE provider_sources SET selected=1 WHERE account_id=?1 AND source_kind=?2 AND source_ref=?3",
            params![account_id, kind, r],
        )?;
        if c == 0 {
            return Err(ProviderError::Invalid(
                "source was not discovered for this account".into(),
            ));
        }
        n += 1;
    }
    tx.commit()?;
    Ok(n)
}

pub fn selected(conn: &Connection, account_id: &str, kind: &str) -> PRes<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT source_ref FROM provider_sources WHERE account_id=?1 AND source_kind=?2 AND selected=1 ORDER BY source_ref",
    )?;
    let rows = st.query_map(params![account_id, kind], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<String>>>()?)
}

// ---------- status ----------

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct ProviderAccountStatus {
    pub account_id: String,
    /// hiz_imap | teams_graph
    pub provider: String,
    pub kind: String,
    pub display_name: String,
    pub state: String,
    /// none | fixture | live — fixture data never counts as a connected account.
    pub evidence: String,
    pub settings_saved: bool,
    pub sign_in_saved: bool,
    pub authenticated_at: Option<String>,
    pub last_read_ok_at: Option<String>,
    pub last_attempt_at: Option<String>,
    pub last_success_at: Option<String>,
    pub last_outcome: Option<String>,
    pub last_error: Option<String>,
    pub config: serde_json::Value,
    pub sources: Vec<ProviderSource>,
    pub stored_messages: u32,
    pub fetch_running: bool,
}

pub fn status(conn: &Connection) -> PRes<Vec<ProviderAccountStatus>> {
    let mut st = conn.prepare(
        "SELECT a.id, p.provider, a.kind, a.display_name, a.state, a.evidence, p.secret_saved,
                p.authenticated_at, p.last_read_ok_at, p.last_attempt_at, p.last_success_at,
                p.last_outcome, a.last_error, p.config
         FROM provider_config p JOIN connector_accounts a ON a.id = p.account_id
         ORDER BY p.provider, a.kind, a.display_name",
    )?;
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        String,
        String,
        String,
        String,
        String,
        String,
        i32,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
    )> = st
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
                r.get(10)?,
                r.get(11)?,
                r.get(12)?,
                r.get(13)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = Vec::new();
    for r in rows {
        let stored: u32 = conn.query_row(
            "SELECT count(*) FROM source_messages WHERE account_id=?1 AND deleted_at IS NULL",
            [&r.0],
            |x| x.get(0),
        )?;
        out.push(ProviderAccountStatus {
            sources: list_sources(conn, &r.0)?,
            fetch_running: running(&operation_identity(conn, &r.0)?),
            account_id: r.0,
            provider: r.1,
            kind: r.2,
            display_name: r.3,
            state: r.4,
            evidence: r.5,
            settings_saved: true,
            sign_in_saved: r.6 == 1,
            authenticated_at: r.7,
            last_read_ok_at: r.8,
            last_attempt_at: r.9,
            last_success_at: r.10,
            last_outcome: r.11,
            last_error: r.12,
            config: serde_json::from_str(&r.13).unwrap_or(serde_json::Value::Null),
            stored_messages: stored,
        });
    }
    Ok(out)
}

// ---------- per-account run guard and cancellation ----------

static RUNS: Mutex<Vec<(String, std::sync::Arc<AtomicBool>)>> = Mutex::new(Vec::new());

/// Teams chat and channel accounts share credentials and must share one operation
/// lock. Settings, source selection and disconnect use this same lock.
pub fn operation_identity(conn: &Connection, account_id: &str) -> PRes<String> {
    let (provider, config): (String, String) = conn.query_row(
        "SELECT provider, config FROM provider_config WHERE account_id=?1",
        [account_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if provider == crate::teams::PROVIDER {
        let config: serde_json::Value =
            serde_json::from_str(&config).map_err(|_| ProviderError::Db)?;
        Ok(config["identity"]
            .as_str()
            .unwrap_or(account_id)
            .to_string())
    } else {
        Ok(account_id.to_string())
    }
}

pub fn operation_guard(conn: &Connection, account_id: &str) -> PRes<RunGuard> {
    RunGuard::start(&operation_identity(conn, account_id)?)
}

/// Held while a fetch runs; dropping it unregisters the account.
pub struct RunGuard {
    account_id: String,
    pub cancel: std::sync::Arc<AtomicBool>,
}

impl RunGuard {
    pub fn start(account_id: &str) -> PRes<RunGuard> {
        let mut r = RUNS.lock().map_err(|_| ProviderError::Db)?;
        if r.iter().any(|(id, _)| id == account_id) {
            return Err(ProviderError::Busy);
        }
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        r.push((account_id.to_string(), cancel.clone()));
        Ok(RunGuard {
            account_id: account_id.to_string(),
            cancel,
        })
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        if let Ok(mut r) = RUNS.lock() {
            r.retain(|(id, _)| id != &self.account_id);
        }
    }
}

pub fn running(account_id: &str) -> bool {
    RUNS.lock()
        .map(|r| r.iter().any(|(id, _)| id == account_id))
        .unwrap_or(false)
}

/// Requests cancellation; returns whether a run was active.
pub fn cancel(account_id: &str) -> bool {
    RUNS.lock()
        .map(|r| {
            r.iter()
                .filter(|(id, _)| id == account_id)
                .map(|(_, c)| c.store(true, Ordering::SeqCst))
                .count()
                > 0
        })
        .unwrap_or(false)
}

/// Cancellation-aware bounded sleep (retry-after/backoff).
pub fn sleep_cancellable(d: Duration, cancel: &AtomicBool) -> PRes<()> {
    let end = std::time::Instant::now() + d;
    while std::time::Instant::now() < end {
        if cancel.load(Ordering::SeqCst) {
            return Err(ProviderError::Cancelled);
        }
        std::thread::sleep(Duration::from_millis(50).min(end - std::time::Instant::now()));
    }
    Ok(())
}

// ---------- opt-in polling ----------

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct Polling {
    pub enabled: bool,
    pub interval_minutes: u32,
}

pub fn get_polling(conn: &Connection) -> PRes<Polling> {
    Ok(conn
        .query_row(
            "SELECT enabled, interval_minutes FROM provider_polling WHERE id=1",
            [],
            |r| {
                Ok(Polling {
                    enabled: r.get::<_, i32>(0)? == 1,
                    interval_minutes: r.get(1)?,
                })
            },
        )
        .optional()?
        .unwrap_or(Polling {
            enabled: false,
            interval_minutes: 30,
        }))
}

pub fn set_polling(conn: &Connection, p: &Polling, now: &str) -> PRes<Polling> {
    if !(MIN_INTERVAL_MIN..=MAX_INTERVAL_MIN).contains(&p.interval_minutes) {
        return Err(ProviderError::Invalid(format!(
            "interval must be {MIN_INTERVAL_MIN}-{MAX_INTERVAL_MIN} minutes"
        )));
    }
    conn.execute(
        "INSERT INTO provider_polling(id, enabled, interval_minutes, updated_at) VALUES (1, ?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET enabled=excluded.enabled, interval_minutes=excluded.interval_minutes, updated_at=excluded.updated_at",
        params![p.enabled as i32, p.interval_minutes, now],
    )?;
    get_polling(conn)
}

/// Backoff after consecutive failed polls: interval * 2^failures, capped at 4x / 24h.
pub fn poll_delay(p: &Polling, consecutive_failures: u32) -> Duration {
    let mult = 1u64 << consecutive_failures.min(2);
    Duration::from_secs((p.interval_minutes as u64 * 60 * mult).min(24 * 3600))
}

/// Accounts eligible for scheduled fetching: configured, signed-in, with selections.
pub fn pollable_accounts(conn: &Connection) -> PRes<Vec<(String, String)>> {
    let mut st = conn.prepare(
        "SELECT a.id, p.provider FROM provider_config p JOIN connector_accounts a ON a.id=p.account_id
         WHERE p.secret_saved=1 AND a.state NOT IN ('not_configured','permission_denied')
           AND EXISTS(SELECT 1 FROM provider_sources s WHERE s.account_id=a.id AND s.selected=1)
         ORDER BY a.id",
    )?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Kind for a provider account; used by IPC to reject cross-provider calls.
pub fn require_provider(conn: &Connection, account_id: &str, provider: &str) -> PRes<Kind> {
    let p: Option<String> = conn
        .query_row(
            "SELECT provider FROM provider_config WHERE account_id=?1",
            [account_id],
            |r| r.get(0),
        )
        .optional()?;
    if p.as_deref() != Some(provider) {
        return Err(ProviderError::Invalid(
            "account belongs to another provider".into(),
        ));
    }
    Ok(connectors::get_account(conn, account_id)?.kind)
}

/// Re-entering setup after a disconnect moves the account back to `consent_needed`.
pub fn reopen(conn: &Connection, account_id: &str, now: &str) -> PRes<()> {
    let a = connectors::get_account(conn, account_id)?;
    if a.state == connectors::ConnState::NotConfigured {
        connectors::transition(
            conn,
            account_id,
            connectors::ConnState::ConsentNeeded,
            Evidence::None,
            None,
            now,
        )?;
    }
    Ok(())
}
