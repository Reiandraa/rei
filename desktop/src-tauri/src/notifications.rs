//! P10 notifications/automations: opt-in categories, quiet hours, dedupe, snooze, expiring worker
//! leases and restart catch-up. Everything runs only while an authorised device app is awake; no
//! 24/7 processing is implied. Briefing text is deterministic from local rows (no AI claim).
use jiff::{SignedDuration, Timestamp};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

pub const CATEGORIES: &[&str] = &[
    "morning_brief",
    "deadline",
    "weekly_recap",
    "inbox",
    "teams",
];
pub const LEASE_NAME: &str = "notifications";
const MAX_LEASE_SECS: i64 = 900;

#[derive(Debug, PartialEq)]
pub enum NotifError {
    Validation(String),
    Internal(String),
}

impl From<rusqlite::Error> for NotifError {
    fn from(e: rusqlite::Error) -> Self {
        NotifError::Internal(e.to_string())
    }
}

fn val<T>(m: impl Into<String>) -> Result<T, NotifError> {
    Err(NotifError::Validation(m.into()))
}

fn ts(s: &str) -> Result<Timestamp, NotifError> {
    match s.parse::<Timestamp>() {
        Ok(t) => Ok(t),
        Err(_) => val(format!("invalid timestamp: {s}")),
    }
}

fn plus(t: Timestamp, d: SignedDuration) -> Result<Timestamp, NotifError> {
    t.checked_add(d).or_else(|e| val(e.to_string()))
}

fn audit(
    conn: &Connection,
    now: &str,
    action: &str,
    target: Option<&str>,
    outcome: &str,
) -> Result<(), NotifError> {
    conn.execute(
        "INSERT INTO audit_events(id, at, actor, action, target_type, target_id, outcome)
         VALUES (?1, ?2, 'Worker', ?3, 'notification', ?4, ?5)",
        params![
            uuid::Uuid::new_v4().to_string(),
            now,
            action,
            target,
            outcome
        ],
    )?;
    Ok(())
}

// ---- opt-in preferences and quiet hours ------------------------------------------------------

pub fn set_category(
    conn: &Connection,
    category: &str,
    enabled: bool,
    now: &str,
) -> Result<(), NotifError> {
    if !CATEGORIES.contains(&category) {
        return val(format!("unknown category: {category}"));
    }
    conn.execute(
        "INSERT INTO notification_prefs(category, enabled, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(category) DO UPDATE SET enabled = ?2, updated_at = ?3",
        params![category, enabled as i64, now],
    )?;
    let action = if enabled {
        "category_enable"
    } else {
        "category_disable"
    };
    audit(conn, now, action, Some(category), "ok")
}

/// Categories are opt-in: absent means disabled.
pub fn category_enabled(conn: &Connection, category: &str) -> Result<bool, NotifError> {
    let v: Option<i64> = conn
        .query_row(
            "SELECT enabled FROM notification_prefs WHERE category = ?1",
            [category],
            |r| r.get(0),
        )
        .optional()?;
    Ok(v == Some(1))
}

fn parse_hm(s: &str) -> Result<u32, NotifError> {
    let bad = || val("time must be HH:MM");
    let Some((h, m)) = s.split_once(':') else {
        return bad();
    };
    if h.len() != 2 || m.len() != 2 {
        return bad();
    }
    match (h.parse::<u32>(), m.parse::<u32>()) {
        (Ok(h), Ok(m)) if h < 24 && m < 60 => Ok(h * 60 + m),
        _ => bad(),
    }
}

pub fn set_quiet_hours(
    conn: &Connection,
    enabled: bool,
    start: &str,
    end: &str,
    tz: &str,
    now: &str,
) -> Result<(), NotifError> {
    let (s, e) = (parse_hm(start)?, parse_hm(end)?);
    if enabled && s == e {
        return val("quiet hours start and end must differ");
    }
    if jiff::tz::TimeZone::get(tz).is_err() {
        return val(format!("unknown time zone: {tz}"));
    }
    conn.execute(
        "INSERT INTO notification_config(id, quiet_enabled, quiet_start, quiet_end, tz, updated_at)
         VALUES (1, ?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET quiet_enabled=?1, quiet_start=?2, quiet_end=?3, tz=?4, updated_at=?5",
        params![enabled as i64, start, end, tz, now],
    )?;
    audit(conn, now, "quiet_hours_set", None, "ok")
}

struct Config {
    quiet: bool,
    start: u32,
    end: u32,
    tz: String,
    window_min: i64,
}

fn config(conn: &Connection) -> Result<Config, NotifError> {
    type Row = (i64, String, String, String, i64);
    let row: Option<Row> = conn
        .query_row(
            "SELECT quiet_enabled, quiet_start, quiet_end, tz, catch_up_window_min
             FROM notification_config WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    Ok(match row {
        Some((q, s, e, tz, w)) => Config {
            quiet: q == 1,
            start: parse_hm(&s)?,
            end: parse_hm(&e)?,
            tz,
            window_min: w,
        },
        None => Config {
            quiet: false,
            start: 22 * 60,
            end: 7 * 60,
            tz: "UTC".into(),
            window_min: 720,
        },
    })
}

fn in_quiet(cfg: &Config, at: Timestamp) -> Result<bool, NotifError> {
    if !cfg.quiet {
        return Ok(false);
    }
    let z = match at.in_tz(&cfg.tz) {
        Ok(z) => z,
        Err(e) => return val(e.to_string()),
    };
    let m = z.hour() as u32 * 60 + z.minute() as u32;
    Ok(if cfg.start < cfg.end {
        m >= cfg.start && m < cfg.end
    } else {
        m >= cfg.start || m < cfg.end
    })
}

pub fn is_quiet_now(conn: &Connection, now: &str) -> Result<bool, NotifError> {
    in_quiet(&config(conn)?, ts(now)?)
}

// ---- leases ----------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Serialize)]
pub struct Lease {
    pub holder: String,
    pub epoch: i64,
    pub expires_at: String,
}

/// Acquires or renews the lease. A live lease held by another device is refused (Ok(None)); an
/// expired one is taken over with a bumped epoch.
pub fn lease_acquire(
    conn: &mut Connection,
    device: &str,
    now: &str,
    ttl_secs: i64,
) -> Result<Option<Lease>, NotifError> {
    if !(1..=MAX_LEASE_SECS).contains(&ttl_secs) {
        return val("lease ttl out of range");
    }
    let n = ts(now)?;
    let exp = plus(n, SignedDuration::from_secs(ttl_secs))?.to_string();
    let tx = conn.transaction()?;
    let cur: Option<(String, i64, String)> = tx
        .query_row(
            "SELECT device_id, epoch, expires_at FROM worker_leases WHERE name = ?1",
            [LEASE_NAME],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let lease = match cur {
        None => {
            tx.execute(
                "INSERT INTO worker_leases(name, device_id, epoch, acquired_at, expires_at)
                 VALUES (?1, ?2, 1, ?3, ?4)",
                params![LEASE_NAME, device, now, exp],
            )?;
            Lease {
                holder: device.into(),
                epoch: 1,
                expires_at: exp,
            }
        }
        Some((h, epoch, _)) if h == device => {
            tx.execute(
                "UPDATE worker_leases SET expires_at = ?2 WHERE name = ?1",
                params![LEASE_NAME, exp],
            )?;
            Lease {
                holder: h,
                epoch,
                expires_at: exp,
            }
        }
        Some((_, epoch, e)) if ts(&e)? <= n => {
            tx.execute(
                "UPDATE worker_leases SET device_id=?2, epoch=?3, acquired_at=?4, expires_at=?5
                 WHERE name=?1",
                params![LEASE_NAME, device, epoch + 1, now, exp],
            )?;
            audit(&tx, now, "lease_takeover", Some(device), "ok")?;
            Lease {
                holder: device.into(),
                epoch: epoch + 1,
                expires_at: exp,
            }
        }
        Some(_) => return Ok(None),
    };
    tx.commit()?;
    Ok(Some(lease))
}

pub fn lease_release(conn: &Connection, device: &str) -> Result<bool, NotifError> {
    let n = conn.execute(
        "DELETE FROM worker_leases WHERE name = ?1 AND device_id = ?2",
        params![LEASE_NAME, device],
    )?;
    Ok(n > 0)
}

/// The live (unexpired) lease holder, if any.
pub fn lease_holder(conn: &Connection, now: &str) -> Result<Option<Lease>, NotifError> {
    let n = ts(now)?;
    let l: Option<Lease> = conn
        .query_row(
            "SELECT device_id, epoch, expires_at FROM worker_leases WHERE name = ?1",
            [LEASE_NAME],
            |r| {
                Ok(Lease {
                    holder: r.get(0)?,
                    epoch: r.get(1)?,
                    expires_at: r.get(2)?,
                })
            },
        )
        .optional()?;
    Ok(l.filter(|l| ts(&l.expires_at).map(|e| e > n).unwrap_or(false)))
}

// ---- enqueue / snooze / dismiss ---------------------------------------------------------------

#[derive(Debug, PartialEq, Serialize)]
pub enum Enqueue {
    Created(String),
    Duplicate,
    /// Category is not opted in: no row, no effect.
    Disabled,
}

pub fn enqueue(
    conn: &Connection,
    dedupe_key: &str,
    category: &str,
    title: &str,
    body: &str,
    due_at: &str,
    now: &str,
) -> Result<Enqueue, NotifError> {
    if !CATEGORIES.contains(&category) {
        return val(format!("unknown category: {category}"));
    }
    ts(due_at)?;
    if !category_enabled(conn, category)? {
        return Ok(Enqueue::Disabled);
    }
    let id = uuid::Uuid::new_v4().to_string();
    let n = conn.execute(
        "INSERT OR IGNORE INTO notification_receipts(id, dedupe_key, category, title, body, due_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, dedupe_key, category, title, body, due_at, now],
    )?;
    Ok(if n == 1 {
        Enqueue::Created(id)
    } else {
        Enqueue::Duplicate
    })
}

pub fn snooze(conn: &Connection, id: &str, until: &str, now: &str) -> Result<(), NotifError> {
    if ts(until)? <= ts(now)? {
        return val("snooze must end in the future");
    }
    let n = conn.execute(
        "UPDATE notification_receipts SET snoozed_until = ?2
         WHERE id = ?1 AND status IN ('Pending','PermissionDenied')",
        params![id, until],
    )?;
    if n == 0 {
        return val("notification not found or already settled");
    }
    audit(conn, now, "snooze", Some(id), "ok")
}

pub fn dismiss(conn: &Connection, id: &str, now: &str) -> Result<(), NotifError> {
    let n = conn.execute(
        "UPDATE notification_receipts SET status='Dismissed'
         WHERE id = ?1 AND status IN ('Pending','PermissionDenied')",
        [id],
    )?;
    if n == 0 {
        return val("notification not found or already settled");
    }
    audit(conn, now, "dismiss", Some(id), "ok")
}

// ---- generators (all gated by opt-in) ----------------------------------------------------------

/// Enqueues deadline alerts for open tasks due within `horizon_hours` (including overdue); the
/// dedupe key is per task and due instant, so rescans and a second device never repeat an alert.
pub fn scan_deadlines(
    conn: &Connection,
    now: &str,
    horizon_hours: i64,
) -> Result<usize, NotifError> {
    if !category_enabled(conn, "deadline")? {
        return Ok(0);
    }
    let n = ts(now)?;
    let limit = plus(n, SignedDuration::from_hours(horizon_hours))?;
    let tz = config(conn)?.tz;
    type Row = (String, String, Option<String>, Option<String>);
    let rows: Vec<Row> = {
        let mut st = conn.prepare(
            "SELECT id, title, due_at_utc, due_date FROM tasks
             WHERE deleted_at IS NULL AND status IN ('todo','doing')
               AND (due_at_utc IS NOT NULL OR due_date IS NOT NULL)",
        )?;
        let v = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<Result<_, _>>()?;
        v
    };
    let mut created = 0;
    for (id, title, at, date) in rows {
        let due = match (at, date) {
            (Some(a), _) => ts(&a)?,
            // All-day: due at the start of that local day in the configured zone.
            (None, Some(d)) => {
                let z = jiff::tz::TimeZone::get(&tz)
                    .ok()
                    .and_then(|t| d.parse::<jiff::civil::Date>().ok()?.to_zoned(t).ok());
                match z {
                    Some(z) => z.timestamp(),
                    None => continue,
                }
            }
            _ => continue,
        };
        if due > limit {
            continue;
        }
        // Overdue items fire at once; upcoming ones fire at their due instant.
        let fire = if due < n { n } else { due };
        let key = format!("deadline:{id}:{due}");
        let r = enqueue(
            conn,
            &key,
            "deadline",
            &format!("Due: {title}"),
            &format!("Due {due}"),
            &fire.to_string(),
            now,
        )?;
        if matches!(r, Enqueue::Created(_)) {
            created += 1;
        }
    }
    Ok(created)
}

/// Builds the briefing from local rows only and stores it once per local date. Returns None when
/// the category is not opted in or the briefing already exists (UNIQUE makes two devices safe).
pub fn morning_brief(
    conn: &Connection,
    device: &str,
    local_date: &str,
    now: &str,
) -> Result<Option<String>, NotifError> {
    if !category_enabled(conn, "morning_brief")? {
        return Ok(None);
    }
    let tz = config(conn)?.tz;
    let Ok(day) = local_date.parse::<jiff::civil::Date>() else {
        return val("invalid local_date");
    };
    let bound = |d: jiff::civil::Date| -> Result<String, NotifError> {
        let t = jiff::tz::TimeZone::get(&tz).or_else(|e| val(e.to_string()))?;
        Ok(d.to_zoned(t)
            .or_else(|e| val(e.to_string()))?
            .timestamp()
            .to_string())
    };
    let start = bound(day)?;
    let end = bound(day.tomorrow().or_else(|e| val(e.to_string()))?)?;
    let mut lines = vec![format!("Briefing for {local_date}")];
    {
        let mut st = conn.prepare(
            "SELECT title FROM tasks WHERE deleted_at IS NULL AND status IN ('todo','doing')
               AND (due_date = ?1 OR (due_at_utc >= ?2 AND due_at_utc < ?3))
             ORDER BY priority DESC, title",
        )?;
        for t in st.query_map(params![local_date, start, end], |r| r.get::<_, String>(0))? {
            lines.push(format!("- Task due: {}", t?));
        }
        let mut st = conn.prepare(
            "SELECT title FROM events WHERE deleted_at IS NULL AND all_day = 0
               AND start_value >= ?1 AND start_value < ?2 ORDER BY start_value",
        )?;
        for t in st.query_map(params![start, end], |r| r.get::<_, String>(0))? {
            lines.push(format!("- Event: {}", t?));
        }
    }
    if lines.len() == 1 {
        lines.push("Nothing scheduled.".into());
    }
    let body = lines.join("\n");
    let n = conn.execute(
        "INSERT OR IGNORE INTO briefings(id, local_date, kind, body, generated_at, generated_by_device)
         VALUES (?1, ?2, 'morning', ?3, ?4, ?5)",
        params![uuid::Uuid::new_v4().to_string(), local_date, body, now, device],
    )?;
    if n == 0 {
        return Ok(None);
    }
    enqueue(
        conn,
        &format!("morning:{local_date}"),
        "morning_brief",
        "Morning briefing",
        &body,
        now,
        now,
    )?;
    audit(conn, now, "briefing_generated", Some(local_date), "ok")?;
    Ok(Some(body))
}

// ---- delivery tick / catch-up -------------------------------------------------------------------

#[derive(Debug, Default, PartialEq, Serialize)]
pub struct TickReport {
    pub delivered: usize,
    pub deferred_quiet: usize,
    pub deferred_snooze: usize,
    pub permission_denied: usize,
    pub missed: usize,
    /// Minutes since the previous tick on any device (None on first run).
    pub gap_minutes: Option<i64>,
    pub lease_held: bool,
}

#[derive(Debug, PartialEq)]
pub enum SinkError {
    PermissionDenied,
    Other(String),
}

/// One worker pass. Requires the lease; without it nothing is delivered. Non-deadline receipts
/// older than the catch-up window become `Missed` and collapse into one digest so waking after
/// sleep does not flood. `sink(category, title, body)` is the OS-notification call; failures are
/// recorded and the receipt stays retryable.
pub fn tick<F>(
    conn: &mut Connection,
    device: &str,
    now: &str,
    mut sink: F,
) -> Result<TickReport, NotifError>
where
    F: FnMut(&str, &str, &str) -> Result<(), SinkError>,
{
    let mut rep = TickReport::default();
    if lease_acquire(conn, device, now, 120)?.is_none() {
        return Ok(rep);
    }
    rep.lease_held = true;
    let n = ts(now)?;
    let cfg = config(conn)?;
    let last: Option<String> = conn
        .query_row(
            "SELECT value FROM app_meta WHERE key='notif_last_tick'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    rep.gap_minutes = match last {
        Some(l) => Some((n.as_second() - ts(&l)?.as_second()).max(0) / 60),
        None => None,
    };
    let cutoff = n
        .checked_sub(SignedDuration::from_mins(cfg.window_min))
        .or_else(|e| val(e.to_string()))?
        .to_string();
    let missed: Vec<String> = {
        let mut st = conn.prepare(
            "SELECT title FROM notification_receipts
             WHERE status='Pending' AND category != 'deadline' AND due_at < ?1",
        )?;
        let v = st
            .query_map([&cutoff], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        v
    };
    conn.execute(
        "UPDATE notification_receipts SET status='Missed'
         WHERE status='Pending' AND category != 'deadline' AND due_at < ?1",
        [&cutoff],
    )?;
    rep.missed = missed.len();
    if !missed.is_empty() {
        let body: String = missed.join("; ").chars().take(3900).collect();
        conn.execute(
            "INSERT OR IGNORE INTO notification_receipts(id, dedupe_key, category, title, body, due_at, created_at)
             VALUES (?1, ?2, 'deadline', ?3, ?4, ?5, ?5)",
            params![
                uuid::Uuid::new_v4().to_string(),
                format!("catchup:{now}"),
                format!("{} notifications missed while the app was asleep", missed.len()),
                body,
                now
            ],
        )?;
        audit(
            conn,
            now,
            "catch_up_missed",
            None,
            &missed.len().to_string(),
        )?;
    }
    let quiet = in_quiet(&cfg, n)?;
    type Row = (String, String, String, String, Option<String>);
    let due: Vec<Row> = {
        let mut st = conn.prepare(
            "SELECT id, category, title, body, snoozed_until FROM notification_receipts
             WHERE status IN ('Pending','PermissionDenied') AND due_at <= ?1 ORDER BY due_at",
        )?;
        let v = st
            .query_map([now], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<Result<_, _>>()?;
        v
    };
    for (id, cat, title, body, snoozed) in due {
        // Opting a category out after enqueueing stops delivery (catch-up digests are 'deadline').
        if !category_enabled(conn, &cat)? {
            continue;
        }
        if snoozed
            .as_deref()
            .and_then(|s| ts(s).ok())
            .is_some_and(|t| t > n)
        {
            rep.deferred_snooze += 1;
            continue;
        }
        if quiet {
            rep.deferred_quiet += 1;
            continue;
        }
        match sink(&cat, &title, &body) {
            Ok(()) => {
                conn.execute(
                    "UPDATE notification_receipts SET status='Delivered', delivered_at=?2, delivered_by_device=?3
                     WHERE id=?1",
                    params![id, now, device],
                )?;
                rep.delivered += 1;
            }
            Err(SinkError::PermissionDenied) => {
                conn.execute(
                    "UPDATE notification_receipts SET status='PermissionDenied' WHERE id=?1",
                    [&id],
                )?;
                audit(conn, now, "deliver", Some(&id), "os_permission_denied")?;
                rep.permission_denied += 1;
            }
            Err(SinkError::Other(_)) => audit(conn, now, "deliver", Some(&id), "sink_error")?,
        }
    }
    conn.execute(
        "INSERT INTO app_meta(key, value) VALUES ('notif_last_tick', ?1)
         ON CONFLICT(key) DO UPDATE SET value = ?1",
        [now],
    )?;
    Ok(rep)
}

#[derive(Debug, Serialize)]
pub struct Disclosure {
    pub last_tick: Option<String>,
    pub lease_holder: Option<String>,
    pub text: String,
}

/// Honest status: nothing runs while every device is asleep.
pub fn disclosure(conn: &Connection, now: &str) -> Result<Disclosure, NotifError> {
    let last: Option<String> = conn
        .query_row(
            "SELECT value FROM app_meta WHERE key='notif_last_tick'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let holder = lease_holder(conn, now)?.map(|l| l.holder);
    let text = match (&last, &holder) {
        (_, Some(_)) => "A device is awake and delivering notifications.".to_string(),
        (Some(l), None) => format!(
            "No device is currently delivering. Last delivery pass: {l}. Local polling and AI stop while all devices are off."
        ),
        (None, None) => "Notifications have never run on this account.".to_string(),
    };
    Ok(Disclosure {
        last_tick: last,
        lease_holder: holder,
        text,
    })
}
