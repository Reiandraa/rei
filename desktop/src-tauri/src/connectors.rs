//! Connectors (P02 email, P03 Teams chats + channels, P04 calendar, manual import),
//! docs/INTERFACES.md §7. This module owns the connection state machine, durable
//! cursors and ingestion of Microsoft Graph wire-format pages. It performs no network
//! I/O and no login: an HTTP transport (not yet built, needs live consent) feeds pages
//! here. Source content is stored as untrusted data and is only ever handed to the
//! model wrapped by `guard::wrap_untrusted`.
use crate::guard::{self, Actor, AuditEntry};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

pub const MAX_BODY_CHARS: usize = 100_000;
pub const MAX_SUBJECT_CHARS: usize = 500;
/// Chat/channel APIs have no universal delta cursor: re-read this window each poll.
pub const OVERLAP_SECS: i64 = 600;
pub const MAX_ATTEMPTS: u32 = 5;
const GRAPH_PREFIX: &str = "https://graph.microsoft.com/";

#[derive(Debug)]
pub enum ConnectorError {
    Db(rusqlite::Error),
    NotFound,
    Invalid(String),
    /// Illegal state transition or wrong connector kind.
    State(String),
}

impl std::fmt::Display for ConnectorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectorError::Db(e) => write!(f, "database error: {e}"),
            ConnectorError::NotFound => write!(f, "connector account not found"),
            ConnectorError::Invalid(m) => write!(f, "invalid input: {m}"),
            ConnectorError::State(m) => write!(f, "invalid state: {m}"),
        }
    }
}

impl std::error::Error for ConnectorError {}

impl From<rusqlite::Error> for ConnectorError {
    fn from(e: rusqlite::Error) -> Self {
        ConnectorError::Db(e)
    }
}

type Res<T> = Result<T, ConnectorError>;

fn invalid(m: impl Into<String>) -> ConnectorError {
    ConnectorError::Invalid(m.into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Email,
    Calendar,
    TeamsChat,
    TeamsChannel,
    Manual,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Email => "email",
            Kind::Calendar => "calendar",
            Kind::TeamsChat => "teams_chat",
            Kind::TeamsChannel => "teams_channel",
            Kind::Manual => "manual",
        }
    }
    fn parse(s: &str) -> Res<Kind> {
        Ok(match s {
            "email" => Kind::Email,
            "calendar" => Kind::Calendar,
            "teams_chat" => Kind::TeamsChat,
            "teams_channel" => Kind::TeamsChannel,
            "manual" => Kind::Manual,
            _ => return Err(invalid(format!("unknown connector kind {s}"))),
        })
    }
    /// Least-privilege delegated scopes per connector (no Send/ReadWrite in v1).
    pub fn scopes(self) -> &'static [&'static str] {
        match self {
            Kind::Email => &["Mail.Read"],
            Kind::Calendar => &["Calendars.Read"],
            Kind::TeamsChat => &["Chat.Read"],
            Kind::TeamsChannel => &[
                "ChannelMessage.Read.All",
                "Team.ReadBasic.All",
                "Channel.ReadBasic.All",
            ],
            Kind::Manual => &[],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnState {
    NotConfigured,
    ConsentNeeded,
    Testing,
    Connected,
    Degraded,
    Expired,
    PermissionDenied,
}

impl ConnState {
    pub fn as_str(self) -> &'static str {
        match self {
            ConnState::NotConfigured => "not_configured",
            ConnState::ConsentNeeded => "consent_needed",
            ConnState::Testing => "testing",
            ConnState::Connected => "connected",
            ConnState::Degraded => "degraded",
            ConnState::Expired => "expired",
            ConnState::PermissionDenied => "permission_denied",
        }
    }
    fn parse(s: &str) -> Res<ConnState> {
        use ConnState::*;
        Ok(match s {
            "not_configured" => NotConfigured,
            "consent_needed" => ConsentNeeded,
            "testing" => Testing,
            "connected" => Connected,
            "degraded" => Degraded,
            "expired" => Expired,
            "permission_denied" => PermissionDenied,
            _ => return Err(invalid(format!("unknown state {s}"))),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    None,
    Fixture,
    Live,
}

impl Evidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Evidence::None => "none",
            Evidence::Fixture => "fixture",
            Evidence::Live => "live",
        }
    }
    fn parse(s: &str) -> Res<Evidence> {
        Ok(match s {
            "none" => Evidence::None,
            "fixture" => Evidence::Fixture,
            "live" => Evidence::Live,
            _ => return Err(invalid(format!("unknown evidence {s}"))),
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Account {
    pub id: String,
    pub kind: Kind,
    pub provider: String,
    pub display_name: String,
    pub state: ConnState,
    pub evidence: Evidence,
    pub last_error: Option<String>,
    pub updated_at: String,
}

/// Allowed edges of the documented state machine. `connected` is reachable only
/// from `testing`, and only with live evidence (also enforced by a DB CHECK).
fn transition_allowed(from: ConnState, to: ConnState) -> bool {
    use ConnState::*;
    if to == NotConfigured {
        return true; // disconnect is always allowed
    }
    matches!(
        (from, to),
        (NotConfigured, ConsentNeeded)
            | (ConsentNeeded, Testing)
            | (ConsentNeeded, PermissionDenied)
            | (Testing, Connected)
            | (Testing, Degraded)
            | (Testing, PermissionDenied)
            | (Testing, Expired)
            | (Connected, Degraded)
            | (Connected, Expired)
            | (Connected, PermissionDenied)
            | (Connected, Testing)
            | (Degraded, Testing)
            | (Expired, ConsentNeeded)
            | (Expired, Testing)
            | (PermissionDenied, ConsentNeeded)
    )
}

fn row_to_account(r: &rusqlite::Row<'_>) -> rusqlite::Result<(Account, String, String, String)> {
    let kind: String = r.get(1)?;
    let state: String = r.get(4)?;
    let evidence: String = r.get(5)?;
    Ok((
        Account {
            id: r.get(0)?,
            kind: Kind::Manual,
            provider: r.get(2)?,
            display_name: r.get(3)?,
            state: ConnState::NotConfigured,
            evidence: Evidence::None,
            last_error: r.get(6)?,
            updated_at: r.get(7)?,
        },
        kind,
        state,
        evidence,
    ))
}

fn finish(raw: (Account, String, String, String)) -> Res<Account> {
    let (mut a, k, s, e) = raw;
    a.kind = Kind::parse(&k)?;
    a.state = ConnState::parse(&s)?;
    a.evidence = Evidence::parse(&e)?;
    Ok(a)
}

const ACCOUNT_SELECT: &str =
    "SELECT id, kind, provider, display_name, state, evidence, last_error, updated_at FROM connector_accounts";

pub fn get_account(conn: &Connection, id: &str) -> Res<Account> {
    let raw = conn
        .query_row(
            &format!("{ACCOUNT_SELECT} WHERE id = ?1"),
            [id],
            row_to_account,
        )
        .optional()?
        .ok_or(ConnectorError::NotFound)?;
    finish(raw)
}

pub fn list_accounts(conn: &Connection) -> Res<Vec<Account>> {
    let mut stmt = conn.prepare(&format!("{ACCOUNT_SELECT} ORDER BY kind, display_name, id"))?;
    let rows = stmt.query_map([], row_to_account)?;
    rows.map(|r| finish(r?)).collect()
}

pub fn create_account(
    conn: &Connection,
    kind: Kind,
    provider: &str,
    display_name: &str,
    now: &str,
) -> Res<Account> {
    let name = display_name.trim();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(invalid("display name must be 1-200 characters"));
    }
    if provider.trim().is_empty() {
        return Err(invalid("provider is required"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO connector_accounts(id, kind, provider, display_name, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![id, kind.as_str(), provider.trim(), name, now],
    )?;
    get_account(conn, &id)
}

/// Moves an account through the state machine. Fixture evidence can never mark an
/// account `connected`; only a real provider read (Evidence::Live) can.
pub fn transition(
    conn: &Connection,
    id: &str,
    to: ConnState,
    evidence: Evidence,
    error: Option<&str>,
    now: &str,
) -> Res<Account> {
    let cur = get_account(conn, id)?;
    if !transition_allowed(cur.state, to) {
        return Err(ConnectorError::State(format!(
            "{} -> {} is not allowed",
            cur.state.as_str(),
            to.as_str()
        )));
    }
    if to == ConnState::Connected && evidence != Evidence::Live {
        return Err(ConnectorError::State(
            "connected requires a successful live provider read".into(),
        ));
    }
    let err = error.map(guard::redact);
    conn.execute(
        "UPDATE connector_accounts SET state=?1, evidence=?2, last_error=?3, updated_at=?4 WHERE id=?5",
        params![to.as_str(), evidence.as_str(), err, now, id],
    )?;
    guard::audit(
        conn,
        &AuditEntry {
            at: now,
            actor: Actor::Worker,
            action: "connector_state",
            target_type: "connector_account",
            target_id: Some(id),
            outcome: to.as_str(),
            detail: error.unwrap_or(""),
        },
    )?;
    get_account(conn, id)
}

/// Disconnects locally: cursors are always cleared; cached source content is purged
/// only if the user chose so. Provider-side revoke must be done by the user at the
/// provider (no token is revoked from here).
pub fn disconnect(conn: &mut Connection, id: &str, purge_cache: bool, now: &str) -> Res<Account> {
    get_account(conn, id)?;
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM source_cursors WHERE account_id = ?1", [id])?;
    if purge_cache {
        tx.execute("DELETE FROM source_messages WHERE account_id = ?1", [id])?;
        tx.execute(
            "DELETE FROM events WHERE account_id = ?1 AND provider IS NOT NULL",
            [id],
        )?;
    }
    tx.execute(
        "UPDATE connector_accounts SET state='not_configured', evidence='none', last_error=NULL, updated_at=?1 WHERE id=?2",
        params![now, id],
    )?;
    guard::audit(
        &tx,
        &AuditEntry {
            at: now,
            actor: Actor::User,
            action: "connector_disconnect",
            target_type: "connector_account",
            target_id: Some(id),
            outcome: if purge_cache { "purged" } else { "kept_cache" },
            detail: "",
        },
    )?;
    tx.commit()?;
    get_account(conn, id)
}

pub fn get_cursor(conn: &Connection, account_id: &str, stream: &str) -> Res<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT cursor FROM source_cursors WHERE account_id=?1 AND stream=?2",
            params![account_id, stream],
            |r| r.get(0),
        )
        .optional()?)
}

fn set_cursor(
    tx: &Transaction<'_>,
    account_id: &str,
    stream: &str,
    cursor: &str,
    now: &str,
) -> Res<()> {
    tx.execute(
        "INSERT INTO source_cursors(account_id, stream, cursor, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(account_id, stream) DO UPDATE SET cursor=excluded.cursor, updated_at=excluded.updated_at",
        params![account_id, stream, cursor, now],
    )?;
    Ok(())
}

/// Graph paging/delta links are followed by the transport, so they must point at
/// Graph over HTTPS; anything else is rejected (cursor injection / SSRF guard).
fn check_graph_link(link: &str) -> Res<()> {
    if link.starts_with(GRAPH_PREFIX) && !link.contains('@') && !link.contains(char::is_whitespace)
    {
        Ok(())
    } else {
        Err(invalid("paging link is not a Microsoft Graph HTTPS URL"))
    }
}

#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct SyncSummary {
    pub inserted: u32,
    pub updated: u32,
    pub unchanged: u32,
    pub removed: u32,
    pub skipped: u32,
    /// Next page to fetch, if the provider returned one.
    pub next_link: Option<String>,
    /// True when the provider signalled the end of the delta round.
    pub complete: bool,
}

fn require(conn: &Connection, account_id: &str, kind: Kind) -> Res<Account> {
    let a = get_account(conn, account_id)?;
    if a.kind != kind {
        return Err(ConnectorError::State(format!(
            "account is {}, expected {}",
            a.kind.as_str(),
            kind.as_str()
        )));
    }
    if a.state == ConnState::NotConfigured {
        return Err(ConnectorError::State("account is not configured".into()));
    }
    Ok(a)
}

fn label_for(a: &Account) -> &'static str {
    match a.evidence {
        Evidence::Live => "provider",
        Evidence::Fixture | Evidence::None => "SYNTHETIC_FIXTURE",
    }
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Reduces Graph HTML bodies to plain text. Markup is dropped, never interpreted.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn body_text(item: &Value) -> String {
    let content = item["body"]["content"].as_str();
    let text = match (content, item["body"]["contentType"].as_str()) {
        (Some(c), Some(t)) if t.eq_ignore_ascii_case("html") => html_to_text(c),
        (Some(c), _) => c.to_string(),
        (None, _) => item["bodyPreview"].as_str().unwrap_or("").to_string(),
    };
    truncate(&text, MAX_BODY_CHARS)
}

fn items(page: &Value) -> Res<&Vec<Value>> {
    page["value"]
        .as_array()
        .ok_or_else(|| invalid("page has no value array"))
}

struct Msg<'a> {
    kind: Kind,
    object_id: &'a str,
    container: Option<&'a str>,
    parent: Option<&'a str>,
    sender: Option<String>,
    subject: Option<String>,
    body: String,
    received_at: &'a str,
    modified_at: Option<&'a str>,
}

enum Upsert {
    Inserted,
    Updated,
    Unchanged,
}

fn upsert_message(tx: &Transaction<'_>, a: &Account, m: &Msg<'_>, now: &str) -> Res<Upsert> {
    let existing: Option<(Option<String>, Option<String>)> = tx
        .query_row(
            "SELECT provider_modified_at, deleted_at FROM source_messages
             WHERE account_id=?1 AND kind=?2 AND provider_object_id=?3",
            params![a.id, m.kind.as_str(), m.object_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match existing {
        // Dedupe by stable ID + lastModifiedDateTime (overlap windows re-deliver).
        Some((prev, None)) if prev.as_deref() == m.modified_at && m.modified_at.is_some() => {
            Ok(Upsert::Unchanged)
        }
        Some(_) => {
            tx.execute(
                "UPDATE source_messages SET sender=?1, subject=?2, body_text=?3, received_at=?4,
                   provider_modified_at=?5, container_id=?6, parent_id=?7, deleted_at=NULL, ingested_at=?8
                 WHERE account_id=?9 AND kind=?10 AND provider_object_id=?11",
                params![
                    m.sender, m.subject, m.body, m.received_at, m.modified_at, m.container,
                    m.parent, now, a.id, m.kind.as_str(), m.object_id
                ],
            )?;
            Ok(Upsert::Updated)
        }
        None => {
            tx.execute(
                "INSERT INTO source_messages(id, account_id, kind, provider_object_id, container_id, parent_id,
                   sender, subject, body_text, received_at, provider_modified_at, label, ingested_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    a.id,
                    m.kind.as_str(),
                    m.object_id,
                    m.container,
                    m.parent,
                    m.sender,
                    m.subject,
                    m.body,
                    m.received_at,
                    m.modified_at,
                    label_for(a),
                    now
                ],
            )?;
            Ok(Upsert::Inserted)
        }
    }
}

fn tally(s: &mut SyncSummary, u: Upsert) {
    match u {
        Upsert::Inserted => s.inserted += 1,
        Upsert::Updated => s.updated += 1,
        Upsert::Unchanged => s.unchanged += 1,
    }
}

fn mark_removed(
    tx: &Transaction<'_>,
    a: &Account,
    kind: Kind,
    object_id: &str,
    now: &str,
) -> Res<bool> {
    let n = tx.execute(
        "UPDATE source_messages SET deleted_at=?1, body_text='', subject=NULL
         WHERE account_id=?2 AND kind=?3 AND provider_object_id=?4 AND deleted_at IS NULL",
        params![now, a.id, kind.as_str(), object_id],
    )?;
    Ok(n > 0)
}

/// Stores the paging state of a Graph delta round in the same transaction as rows.
fn store_delta_links(
    tx: &Transaction<'_>,
    a: &Account,
    stream: &str,
    page: &Value,
    s: &mut SyncSummary,
    now: &str,
) -> Res<()> {
    if let Some(next) = page["@odata.nextLink"].as_str() {
        check_graph_link(next)?;
        set_cursor(tx, &a.id, &format!("{stream}:next"), next, now)?;
        s.next_link = Some(next.to_string());
    } else if let Some(delta) = page["@odata.deltaLink"].as_str() {
        check_graph_link(delta)?;
        set_cursor(tx, &a.id, stream, delta, now)?;
        tx.execute(
            "DELETE FROM source_cursors WHERE account_id=?1 AND stream=?2",
            params![a.id, format!("{stream}:next")],
        )?;
        s.complete = true;
    }
    Ok(())
}

pub const MAIL_STREAM: &str = "mail:inbox";
pub const MAIL_DELTA_START: &str =
    "https://graph.microsoft.com/v1.0/me/mailFolders/inbox/messages/delta";

/// Applies one page of `/me/mailFolders/inbox/messages/delta`. Rows and the
/// nextLink/deltaLink cursor are committed atomically; a failure stores nothing.
pub fn apply_mail_delta_page(
    conn: &mut Connection,
    account_id: &str,
    page: &Value,
    now: &str,
) -> Res<SyncSummary> {
    let a = require(conn, account_id, Kind::Email)?;
    let tx = conn.transaction()?;
    let mut s = SyncSummary::default();
    for it in items(page)? {
        let Some(id) = it["id"].as_str() else {
            s.skipped += 1;
            continue;
        };
        if !it["@removed"].is_null() {
            if mark_removed(&tx, &a, Kind::Email, id, now)? {
                s.removed += 1;
            }
            continue;
        }
        let Some(received) = it["receivedDateTime"].as_str() else {
            s.skipped += 1;
            continue;
        };
        let m = Msg {
            kind: Kind::Email,
            object_id: id,
            container: it["conversationId"].as_str(),
            parent: None,
            sender: it["from"]["emailAddress"]["address"]
                .as_str()
                .map(|x| truncate(x, 320)),
            subject: it["subject"]
                .as_str()
                .map(|x| truncate(x, MAX_SUBJECT_CHARS)),
            body: body_text(it),
            received_at: received,
            modified_at: it["lastModifiedDateTime"].as_str(),
        };
        let u = upsert_message(&tx, &a, &m, now)?;
        tally(&mut s, u);
    }
    store_delta_links(&tx, &a, MAIL_STREAM, page, &mut s, now)?;
    tx.commit()?;
    Ok(s)
}

/// Next URL the transport should fetch for mail: pending page, stored deltaLink,
/// or the initial delta request.
pub fn mail_next_request(conn: &Connection, account_id: &str) -> Res<String> {
    if let Some(n) = get_cursor(conn, account_id, &format!("{MAIL_STREAM}:next"))? {
        return Ok(n);
    }
    Ok(get_cursor(conn, account_id, MAIL_STREAM)?.unwrap_or_else(|| MAIL_DELTA_START.to_string()))
}

pub const CALENDAR_STREAM: &str = "calendar:view";
pub const CALENDAR_PROVIDER: &str = "GraphCalendar";

/// Converts a Graph `dateTimeTimeZone` to the planner's stored instant (UTC `Z`).
/// The transport requests `Prefer: outlook.timezone="UTC"`; IANA zones are also
/// accepted. Windows zone names are rejected rather than guessed.
fn graph_instant(dt: &Value) -> Res<String> {
    let raw = dt["dateTime"]
        .as_str()
        .ok_or_else(|| invalid("missing dateTime"))?;
    let tz = dt["timeZone"].as_str().unwrap_or("UTC");
    let civil_str = raw.get(..19).ok_or_else(|| invalid("bad dateTime"))?;
    let civil: jiff::civil::DateTime = civil_str.parse().map_err(|_| invalid("bad dateTime"))?;
    let zone = if tz.eq_ignore_ascii_case("UTC") {
        jiff::tz::TimeZone::UTC
    } else {
        jiff::tz::TimeZone::get(tz).map_err(|_| invalid(format!("unsupported time zone {tz}")))?
    };
    let ts = civil
        .to_zoned(zone)
        .map_err(|_| invalid("unrepresentable time"))?
        .timestamp();
    Ok(crate::time::format_unix(ts.as_second()))
}

/// Applies one page of `/me/calendarView/delta`. Provider events are stored
/// read-only (P04: no external writes without separate consent + approval).
pub fn apply_calendar_delta_page(
    conn: &mut Connection,
    account_id: &str,
    device_id: &str,
    page: &Value,
    now: &str,
) -> Res<SyncSummary> {
    let a = require(conn, account_id, Kind::Calendar)?;
    let tx = conn.transaction()?;
    let mut s = SyncSummary::default();
    for it in items(page)? {
        let Some(id) = it["id"].as_str() else {
            s.skipped += 1;
            continue;
        };
        if !it["@removed"].is_null() {
            let n = tx.execute(
                "UPDATE events SET deleted_at=?1, revision=revision+1, updated_at=?1
                 WHERE provider=?2 AND account_id=?3 AND provider_object_id=?4 AND deleted_at IS NULL",
                params![now, CALENDAR_PROVIDER, a.id, id],
            )?;
            s.removed += n as u32;
            continue;
        }
        let parsed = (|| -> Res<(String, String)> {
            Ok((graph_instant(&it["start"])?, graph_instant(&it["end"])?))
        })();
        let Ok((start, end)) = parsed else {
            s.skipped += 1;
            continue;
        };
        if end < start {
            s.skipped += 1;
            continue;
        }
        let title = truncate(
            it["subject"].as_str().unwrap_or("(no title)"),
            MAX_SUBJECT_CHARS,
        );
        let all_day = it["isAllDay"].as_bool().unwrap_or(false) as i64;
        let version = it["changeKey"].as_str();
        let prev: Option<(Option<String>, Option<String>)> = tx
            .query_row(
                "SELECT source_version, deleted_at FROM events WHERE provider=?1 AND account_id=?2 AND provider_object_id=?3",
                params![CALENDAR_PROVIDER, a.id, id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match prev {
            Some((v, None)) if v.as_deref() == version && version.is_some() => s.unchanged += 1,
            Some(_) => {
                tx.execute(
                    "UPDATE events SET title=?1, start_value=?2, end_value=?3, all_day=?4, tz='UTC',
                       source_version=?5, deleted_at=NULL, read_only=1, revision=revision+1, updated_at=?6
                     WHERE provider=?7 AND account_id=?8 AND provider_object_id=?9",
                    params![title, start, end, all_day, version, now, CALENDAR_PROVIDER, a.id, id],
                )?;
                s.updated += 1;
            }
            None => {
                tx.execute(
                    "INSERT INTO events(id, created_at, updated_at, origin_device_id, title, provider, account_id,
                       provider_object_id, start_value, end_value, all_day, tz, source_version, read_only)
                     VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'UTC', ?11, 1)",
                    params![
                        uuid::Uuid::new_v4().to_string(),
                        now,
                        device_id,
                        title,
                        CALENDAR_PROVIDER,
                        a.id,
                        id,
                        start,
                        end,
                        all_day,
                        version
                    ],
                )?;
                s.inserted += 1;
            }
        }
    }
    store_delta_links(&tx, &a, CALENDAR_STREAM, page, &mut s, now)?;
    tx.commit()?;
    Ok(s)
}

fn ts(s: &str) -> Res<jiff::Timestamp> {
    s.parse().map_err(|_| invalid(format!("bad timestamp {s}")))
}

fn sender_name(it: &Value) -> Option<String> {
    it["from"]["user"]["displayName"]
        .as_str()
        .or_else(|| it["from"]["application"]["displayName"].as_str())
        .map(|x| truncate(x, 200))
}

/// Shared Teams ingestion. Teams has no universal delta cursor: the cursor is the
/// highest `lastModifiedDateTime` seen, and the next poll re-reads an overlap window.
struct TeamsTarget<'a> {
    kind: Kind,
    container: &'a str,
    parent: Option<&'a str>,
    stream: &'a str,
}

fn apply_teams_page(
    conn: &mut Connection,
    a: &Account,
    t: TeamsTarget<'_>,
    page: &Value,
    now: &str,
) -> Res<SyncSummary> {
    let TeamsTarget {
        kind,
        container,
        parent,
        stream,
    } = t;
    let tx = conn.transaction()?;
    let mut s = SyncSummary::default();
    let mut high = get_cursor_tx(&tx, &a.id, stream)?;
    for it in items(page)? {
        let (Some(id), Some(created)) = (it["id"].as_str(), it["createdDateTime"].as_str()) else {
            s.skipped += 1;
            continue;
        };
        // System events (member added, call ended, …) are not conversation content.
        if it["messageType"].as_str().is_some_and(|t| t != "message") {
            s.skipped += 1;
            continue;
        }
        let modified = it["lastModifiedDateTime"].as_str().unwrap_or(created);
        let modified_ts = ts(modified)?;
        if high
            .as_deref()
            .map(ts)
            .transpose()?
            .is_none_or(|h| modified_ts > h)
        {
            high = Some(modified.to_string());
        }
        if !it["deletedDateTime"].is_null() {
            if mark_removed(&tx, a, kind, id, now)? {
                s.removed += 1;
            }
            continue;
        }
        let m = Msg {
            kind,
            object_id: id,
            container: Some(container),
            parent: parent.or_else(|| it["replyToId"].as_str()),
            sender: sender_name(it),
            subject: it["subject"]
                .as_str()
                .map(|x| truncate(x, MAX_SUBJECT_CHARS)),
            body: body_text(it),
            received_at: created,
            modified_at: Some(modified),
        };
        let u = upsert_message(&tx, a, &m, now)?;
        tally(&mut s, u);
    }
    if let Some(next) = page["@odata.nextLink"].as_str() {
        check_graph_link(next)?;
        s.next_link = Some(next.to_string());
    } else {
        s.complete = true;
    }
    // Reply streams record that the thread was enumerated even when it has no replies.
    if let Some(h) = high.or_else(|| parent.map(|_| now.to_string())) {
        set_cursor(&tx, &a.id, stream, &h, now)?;
    }
    tx.commit()?;
    Ok(s)
}

fn get_cursor_tx(tx: &Transaction<'_>, account_id: &str, stream: &str) -> Res<Option<String>> {
    Ok(tx
        .query_row(
            "SELECT cursor FROM source_cursors WHERE account_id=?1 AND stream=?2",
            params![account_id, stream],
            |r| r.get(0),
        )
        .optional()?)
}

fn check_graph_id(id: &str) -> Res<()> {
    let ok = !id.is_empty()
        && id.len() <= 400
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '@' | '.' | '_' | '-' | '='));
    if ok {
        Ok(())
    } else {
        Err(invalid("invalid Graph identifier"))
    }
}

pub fn chat_stream(chat_id: &str) -> String {
    format!("chat:{chat_id}")
}

/// Applies a page of `/chats/{chat-id}/messages` (Chat.Read, delegated).
pub fn apply_chat_messages_page(
    conn: &mut Connection,
    account_id: &str,
    chat_id: &str,
    page: &Value,
    now: &str,
) -> Res<SyncSummary> {
    check_graph_id(chat_id)?;
    let a = require(conn, account_id, Kind::TeamsChat)?;
    apply_teams_page(
        conn,
        &a,
        TeamsTarget {
            kind: Kind::TeamsChat,
            container: chat_id,
            parent: None,
            stream: &chat_stream(chat_id),
        },
        page,
        now,
    )
}

/// Lower bound for the next chat poll (`lastModifiedDateTime gt …` filter):
/// last high-water mark minus the overlap window; None means full first read.
pub fn chat_poll_since(conn: &Connection, account_id: &str, chat_id: &str) -> Res<Option<String>> {
    let Some(c) = get_cursor(conn, account_id, &chat_stream(chat_id))? else {
        return Ok(None);
    };
    let t = ts(&c)?.as_second() - OVERLAP_SECS;
    Ok(Some(crate::time::format_unix(t)))
}

/// The `/messages` request for a chat, ordered as the API requires for filtering.
pub fn chat_messages_url(chat_id: &str, since: Option<&str>) -> Res<String> {
    check_graph_id(chat_id)?;
    let mut u = format!(
        "{GRAPH_PREFIX}v1.0/chats/{chat_id}/messages?$top=50&$orderby=lastModifiedDateTime%20desc"
    );
    if let Some(s) = since {
        ts(s)?;
        u.push_str(&format!("&$filter=lastModifiedDateTime%20gt%20{s}"));
    }
    Ok(u)
}

pub fn channel_stream(team_id: &str, channel_id: &str) -> String {
    format!("channel:{team_id}/{channel_id}")
}

/// Applies a page of `/teams/{team-id}/channels/{channel-id}/messages`, or of
/// `/messages/{message-id}/replies` when `reply_to` is set
/// (ChannelMessage.Read.All, tenant consent checked separately).
pub fn apply_channel_messages_page(
    conn: &mut Connection,
    account_id: &str,
    team_id: &str,
    channel_id: &str,
    reply_to: Option<&str>,
    page: &Value,
    now: &str,
) -> Res<SyncSummary> {
    check_graph_id(team_id)?;
    check_graph_id(channel_id)?;
    if let Some(r) = reply_to {
        check_graph_id(r)?;
    }
    let a = require(conn, account_id, Kind::TeamsChannel)?;
    let container = format!("{team_id}/{channel_id}");
    let stream = match reply_to {
        Some(r) => format!("{}:replies:{r}", channel_stream(team_id, channel_id)),
        None => channel_stream(team_id, channel_id),
    };
    apply_teams_page(
        conn,
        &a,
        TeamsTarget {
            kind: Kind::TeamsChannel,
            container: &container,
            parent: reply_to,
            stream: &stream,
        },
        page,
        now,
    )
}

/// Thread roots in a channel whose replies must be enumerated (reconciliation):
/// roots never reconciled, or modified after their replies were last read.
pub fn channel_threads_to_reconcile(
    conn: &Connection,
    account_id: &str,
    team_id: &str,
    channel_id: &str,
) -> Res<Vec<String>> {
    let container = format!("{team_id}/{channel_id}");
    let prefix = format!("{}:replies:", channel_stream(team_id, channel_id));
    let mut stmt = conn.prepare(
        "SELECT m.provider_object_id FROM source_messages m
         LEFT JOIN source_cursors c ON c.account_id = m.account_id AND c.stream = ?3 || m.provider_object_id
         WHERE m.account_id=?1 AND m.kind='teams_channel' AND m.container_id=?2 AND m.parent_id IS NULL
           AND m.deleted_at IS NULL AND (c.cursor IS NULL OR c.updated_at < m.ingested_at)
         ORDER BY m.received_at, m.provider_object_id",
    )?;
    let rows = stmt.query_map(params![account_id, container, prefix], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<String>>>()?)
}

/// Known channel roots in least-recently-checked order. A new reply need not
/// modify its root message, so periodic polling cannot rely on root edits alone.
pub fn channel_threads_to_poll(
    conn: &Connection,
    account_id: &str,
    team_id: &str,
    channel_id: &str,
    limit: u32,
) -> Res<Vec<String>> {
    let container = format!("{team_id}/{channel_id}");
    let prefix = format!("{}:replies:", channel_stream(team_id, channel_id));
    let mut stmt = conn.prepare(
        "SELECT m.provider_object_id FROM source_messages m
         LEFT JOIN source_cursors c ON c.account_id=m.account_id AND c.stream=?3 || m.provider_object_id
         WHERE m.account_id=?1 AND m.kind='teams_channel' AND m.container_id=?2
           AND m.parent_id IS NULL AND m.deleted_at IS NULL
         ORDER BY COALESCE(c.updated_at,''), m.received_at DESC, m.provider_object_id LIMIT ?4",
    )?;
    let rows = stmt.query_map(
        params![account_id, container, prefix, limit.clamp(1, 1000)],
        |r| r.get(0),
    )?;
    Ok(rows.collect::<rusqlite::Result<Vec<String>>>()?)
}

/// User-supplied text or link pasted into REI. Labeled as a manual import and
/// still treated as untrusted content.
pub fn import_manual(
    conn: &mut Connection,
    account_id: &str,
    title: &str,
    text: &str,
    link: Option<&str>,
    now: &str,
) -> Res<String> {
    let a = get_account(conn, account_id)?;
    if a.kind != Kind::Manual {
        return Err(ConnectorError::State(
            "manual imports need a manual source".into(),
        ));
    }
    let title = title.trim();
    if title.is_empty() || title.chars().count() > MAX_SUBJECT_CHARS {
        return Err(invalid("title must be 1-500 characters"));
    }
    if text.trim().is_empty() && link.is_none() {
        return Err(invalid("text or link is required"));
    }
    if text.chars().count() > MAX_BODY_CHARS {
        return Err(invalid("text is too long"));
    }
    if let Some(l) = link {
        if !(l.starts_with("https://") || l.starts_with("http://"))
            || l.contains(char::is_whitespace)
        {
            return Err(invalid("link must be an http(s) URL"));
        }
    }
    let body = match link {
        Some(l) => format!("{text}\n\nLink: {l}").trim().to_string(),
        None => text.to_string(),
    };
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO source_messages(id, account_id, kind, provider_object_id, subject, body_text, received_at, label, ingested_at)
         VALUES (?1, ?2, 'manual', ?1, ?3, ?4, ?5, 'MANUAL_IMPORT', ?5)",
        params![id, a.id, title, body, now],
    )?;
    Ok(id)
}

#[derive(Clone, Debug, Serialize)]
pub struct SourceMessage {
    pub id: String,
    pub account_id: String,
    pub kind: String,
    pub container_id: Option<String>,
    pub parent_id: Option<String>,
    pub sender: Option<String>,
    pub subject: Option<String>,
    pub body_text: String,
    pub received_at: String,
    pub label: String,
}

pub fn list_messages(
    conn: &Connection,
    account_id: Option<&str>,
    limit: u32,
) -> Res<Vec<SourceMessage>> {
    let limit = limit.clamp(1, 200);
    let mut stmt = conn.prepare(
        "SELECT id, account_id, kind, container_id, parent_id, sender, subject, body_text, received_at, label
         FROM source_messages WHERE deleted_at IS NULL AND (?1 IS NULL OR account_id = ?1)
         ORDER BY received_at DESC, id LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![account_id, limit], |r| {
        Ok(SourceMessage {
            id: r.get(0)?,
            account_id: r.get(1)?,
            kind: r.get(2)?,
            container_id: r.get(3)?,
            parent_id: r.get(4)?,
            sender: r.get(5)?,
            subject: r.get(6)?,
            body_text: r.get(7)?,
            received_at: r.get(8)?,
            label: r.get(9)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// The only way source content reaches the model: inside an untrusted boundary.
pub fn model_context(conn: &Connection, message_id: &str, boundary: &str) -> Res<String> {
    let (subject, body): (Option<String>, String) = conn
        .query_row(
            "SELECT subject, body_text FROM source_messages WHERE id=?1 AND deleted_at IS NULL",
            [message_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or(ConnectorError::NotFound)?;
    let text = match subject {
        Some(s) => format!("Subject: {s}\n{body}"),
        None => body,
    };
    Ok(guard::wrap_untrusted(boundary, message_id, &text))
}

/// Retry policy for 429/503: honour Retry-After (capped), else exponential backoff
/// with caller-supplied jitter in [0,1). None once attempts are exhausted.
pub fn retry_delay(attempt: u32, retry_after_secs: Option<u64>, jitter: f64) -> Option<Duration> {
    if attempt >= MAX_ATTEMPTS {
        return None;
    }
    if let Some(s) = retry_after_secs {
        return Some(Duration::from_secs(s.min(300)));
    }
    let base = 1u64 << attempt.min(6);
    let j = jitter.clamp(0.0, 0.999);
    Some(Duration::from_millis(base * 1000 + (j * 1000.0) as u64))
}

/// Maps a provider HTTP status to the state an account should move to.
pub fn state_for_status(status: u16) -> Option<ConnState> {
    match status {
        200..=299 => None,
        401 => Some(ConnState::Expired),
        403 => Some(ConnState::PermissionDenied),
        _ => Some(ConnState::Degraded),
    }
}

// ---------- HIZ IMAP ingestion (docs/PROVIDER-CONNECTORS-NEXT.md) ----------

/// One message parsed from IMAP by the vetted MIME parser (crate::hiz).
#[derive(Debug, Clone)]
pub struct ImapMessage {
    pub uid: u32,
    /// Stable identity: hashed Message-ID when present, else folder/UIDVALIDITY/UID.
    pub object_id: String,
    pub sender: Option<String>,
    pub subject: Option<String>,
    pub body: String,
    pub received_at: String,
}

pub fn imap_stream(folder: &str) -> String {
    format!("imap:{folder}")
}

/// Cursor value `"{uidvalidity}:{last_uid}"` for a folder, if any.
pub fn imap_cursor(conn: &Connection, account_id: &str, folder: &str) -> Res<Option<(u32, u32)>> {
    let Some(c) = get_cursor(conn, account_id, &imap_stream(folder))? else {
        return Ok(None);
    };
    let (v, u) = c
        .split_once(':')
        .ok_or_else(|| invalid("corrupt IMAP cursor"))?;
    Ok(Some((
        v.parse().map_err(|_| invalid("corrupt IMAP cursor"))?,
        u.parse().map_err(|_| invalid("corrupt IMAP cursor"))?,
    )))
}

/// Applies one IMAP batch (UIDs up to `through_uid` were examined) and advances the folder cursor in the same transaction,
/// so a failed or cancelled fetch never skips messages. A changed UIDVALIDITY drops
/// only the old UID mapping; stored sources (and tasks approved from them) are kept
/// and re-matched by their Message-ID identity on re-ingestion.
pub fn apply_imap_batch(
    conn: &mut Connection,
    account_id: &str,
    folder: &str,
    uidvalidity: u32,
    batch: &[ImapMessage],
    through_uid: u32,
    now: &str,
) -> Res<SyncSummary> {
    let a = require(conn, account_id, Kind::Email)?;
    let tx = conn.transaction()?;
    let stream = imap_stream(folder);
    let prev = get_cursor_tx(&tx, &a.id, &stream)?;
    let mut last = match prev.as_deref().and_then(|c| c.split_once(':')) {
        Some((v, u)) if v == uidvalidity.to_string() => u.parse::<u32>().unwrap_or(0),
        _ => {
            tx.execute(
                "DELETE FROM imap_uids WHERE account_id=?1 AND folder=?2 AND uidvalidity<>?3",
                params![a.id, folder, uidvalidity],
            )?;
            0
        }
    };
    let mut s = SyncSummary::default();
    for m in batch {
        ts(&m.received_at)?;
        let msg = Msg {
            kind: Kind::Email,
            object_id: &m.object_id,
            container: Some(folder),
            parent: None,
            sender: m.sender.as_deref().map(|x| truncate(x, MAX_SUBJECT_CHARS)),
            subject: m.subject.as_deref().map(|x| truncate(x, MAX_SUBJECT_CHARS)),
            body: truncate(&m.body, MAX_BODY_CHARS),
            received_at: &m.received_at,
            // A message's content is immutable for a given identity; re-delivery
            // (overlap, UIDVALIDITY reset, same mail in two folders) is unchanged.
            modified_at: Some(&m.received_at),
        };
        let u = upsert_message(&tx, &a, &msg, now)?;
        tally(&mut s, u);
        tx.execute(
            "INSERT OR REPLACE INTO imap_uids(account_id, folder, uidvalidity, uid, provider_object_id)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![a.id, folder, uidvalidity, m.uid, m.object_id],
        )?;
        last = last.max(m.uid);
    }
    // Skipped (malformed) UIDs in the batch are passed too, so they are not refetched.
    let last = last.max(through_uid);
    set_cursor(&tx, &a.id, &stream, &format!("{uidvalidity}:{last}"), now)?;
    s.complete = true;
    tx.commit()?;
    Ok(s)
}

/// Marks sources whose UIDs disappeared from the folder (server-side deletion or
/// move) as removed. `present` is the full UID set reported by `UID SEARCH ALL`.
pub fn reconcile_imap_deletions(
    conn: &mut Connection,
    account_id: &str,
    folder: &str,
    uidvalidity: u32,
    present: &std::collections::HashSet<u32>,
    now: &str,
) -> Res<u32> {
    let a = require(conn, account_id, Kind::Email)?;
    let tx = conn.transaction()?;
    let known: Vec<(u32, String)> = {
        let mut st = tx.prepare(
            "SELECT uid, provider_object_id FROM imap_uids WHERE account_id=?1 AND folder=?2 AND uidvalidity=?3",
        )?;
        let rows = st.query_map(params![a.id, folder, uidvalidity], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut removed = 0;
    for (uid, oid) in known.into_iter().filter(|(u, _)| !present.contains(u)) {
        tx.execute(
            "DELETE FROM imap_uids WHERE account_id=?1 AND folder=?2 AND uidvalidity=?3 AND uid=?4",
            params![a.id, folder, uidvalidity, uid],
        )?;
        // The same message may still exist in another selected folder.
        let elsewhere: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM imap_uids WHERE account_id=?1 AND provider_object_id=?2)",
            params![a.id, oid],
            |r| r.get(0),
        )?;
        if !elsewhere && mark_removed(&tx, &a, Kind::Email, &oid, now)? {
            removed += 1;
        }
    }
    tx.commit()?;
    Ok(removed)
}
