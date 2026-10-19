//! P01 tasks/recurrence and P04 local calendar events. Every write runs in one SQLite
//! transaction together with an outbox op and an audit event (docs/INTERFACES.md §4.4),
//! and mutations are guarded by `expected_revision` (optimistic concurrency).
use crate::commands::IpcError;
use crate::recurrence::{self, Start};
use jiff::civil::Date;
use jiff::ToSpan;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

pub type Res<T> = Result<T, IpcError>;

pub struct Ctx<'a> {
    pub device_id: &'a str,
    pub now: &'a str,
}

fn val(msg: impl Into<String>) -> IpcError {
    IpcError::new("Validation", msg)
}

fn not_found(what: &str) -> IpcError {
    IpcError::new("NotFound", format!("{what} not found"))
}

fn conflict(current: i64) -> IpcError {
    IpcError {
        code: "Conflict".into(),
        message: "revision mismatch".into(),
        current_revision: Some(current as u64),
    }
}

pub(crate) fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Outbox op (ciphertext is produced by the sync phase) and audit event for one change.
pub(crate) fn log_change(
    tx: &Connection,
    ctx: &Ctx,
    actor: &str,
    entity_type: &str,
    entity_id: &str,
    base_revision: i64,
    action: &str,
) -> Res<()> {
    tx.execute(
        "INSERT INTO outbox(op_id, entity_type, entity_id, base_revision, logical_seq)
         VALUES (?1, ?2, ?3, ?4, (SELECT COALESCE(MAX(logical_seq), 0) + 1 FROM outbox))",
        params![new_id(), entity_type, entity_id, base_revision],
    )?;
    tx.execute(
        "INSERT INTO audit_events(id, at, actor, action, target_type, target_id, outcome)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'ok')",
        params![new_id(), ctx.now, actor, action, entity_type, entity_id],
    )?;
    Ok(())
}

// ---------------------------------------------------------------- validation helpers

fn check_title(t: &str) -> Res<String> {
    let t = t.trim();
    if t.is_empty() || t.chars().count() > 500 {
        return Err(val("title must be 1-500 characters"));
    }
    Ok(t.to_string())
}

fn check_notes(n: &str) -> Res<()> {
    if n.len() > 65_536 {
        return Err(val("notes exceed 64 KiB"));
    }
    Ok(())
}

fn parse_date(s: &str) -> Res<Date> {
    s.parse().map_err(|_| val(format!("invalid date: {s}")))
}

fn check_range(from: &str, to: &str) -> Res<(Date, Date)> {
    let (f, t) = (parse_date(from)?, parse_date(to)?);
    if t < f || t.since(f).map_err(|e| val(e.to_string()))?.get_days() > 366 {
        return Err(val("range must be 0-366 days with to >= from"));
    }
    Ok((f, t))
}

// ---------------------------------------------------------------- tasks

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum DueIn {
    Date { date: String },
    At { local: String, tz: String },
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RecurrenceIn {
    pub rrule: String,
    pub tz: String,
    pub dtstart_local: String,
    #[serde(default)]
    pub weekday_times: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskCreateReq {
    pub title: String,
    pub notes: Option<String>,
    pub priority: Option<u8>,
    pub energy: Option<String>,
    pub duration_min: Option<u32>,
    pub due: Option<DueIn>,
    pub recurrence: Option<RecurrenceIn>,
    pub reminder_offsets_min: Option<Vec<i32>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskUpdateReq {
    pub id: String,
    pub expected_revision: u64,
    pub title: Option<String>,
    pub notes: Option<String>,
    pub status: Option<String>,
    pub priority: Option<u8>,
    pub energy: Option<String>,
    pub duration_min: Option<u32>,
    pub due: Option<DueIn>,
    #[serde(default)]
    pub clear_due: bool,
    pub reminder_offsets_min: Option<Vec<i32>>,
    pub schedule: Option<crate::schedule_edit::ScheduleEdit>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskListReq {
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdReq {
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionReq {
    pub id: String,
    pub expected_revision: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteReq {
    pub id: String,
    pub expected_revision: u64,
    /// Soft delete needs an explicit confirmation (docs/INTERFACES.md §2.1).
    pub confirm: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct RecurrenceView {
    pub series_id: String,
    pub rrule: String,
    pub tz: String,
    pub dtstart_local: String,
    pub weekday_times: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Task {
    pub id: String,
    pub revision: u64,
    pub title: String,
    pub notes: String,
    pub status: String,
    pub priority: u8,
    pub energy: Option<String>,
    pub duration_min: Option<u32>,
    pub due_date: Option<String>,
    pub due_at_utc: Option<String>,
    pub due_tz: Option<String>,
    /// Wall-clock view of `due_at_utc` in `due_tz`, derived (never stored).
    pub due_at_local: Option<String>,
    pub recurrence: Option<RecurrenceView>,
    pub reminder_offsets_min: Vec<i32>,
    pub created_at: String,
    pub updated_at: String,
    /// Work plan: when the owner intends to work on it. Never the deadline.
    pub plan_date: Option<String>,
    pub plan_time: Option<String>,
    pub plan_duration_min: Option<u32>,
    pub plan_tz: Option<String>,
    /// Bounded Undo available for the latest change at this revision (complete/plan/unplan).
    pub undo_action: Option<String>,
    pub has_schedule_changes: bool,
    pub schedule_from: Option<String>,
}

const TASK_SELECT: &str =
    "SELECT t.id, t.revision, t.title, t.notes, t.status, t.priority, t.energy, t.duration_min,
    t.due_date, t.due_at_utc, t.due_tz, t.reminder_offsets_json, t.created_at, t.updated_at,
    r.series_id, r.rrule, r.tz, r.dtstart_local,
    t.plan_date, t.plan_time, t.plan_duration_min, t.plan_tz,
    (SELECT u.action FROM task_undo u WHERE u.task_id = t.id AND u.after_revision = t.revision
       AND u.used_at IS NULL ORDER BY u.created_at DESC LIMIT 1), r.weekday_times_json,
    EXISTS(SELECT 1 FROM schedule_adjustments a WHERE a.task_id=t.id AND a.active=1),
    (SELECT date(MAX(h.through_date), '+1 day') FROM routine_schedule_history h WHERE h.task_id=t.id)
    FROM tasks t LEFT JOIN recurrence_rules r ON r.id = t.recurrence_rule_id";

fn task_from_row(r: &Row) -> rusqlite::Result<Task> {
    let due_at_utc: Option<String> = r.get(9)?;
    let due_tz: Option<String> = r.get(10)?;
    let offsets: String = r.get(11)?;
    let series: Option<String> = r.get(14)?;
    Ok(Task {
        id: r.get(0)?,
        revision: r.get(1)?,
        title: r.get(2)?,
        notes: r.get(3)?,
        status: r.get(4)?,
        priority: r.get(5)?,
        energy: r.get(6)?,
        duration_min: r.get(7)?,
        due_date: r.get(8)?,
        due_at_local: match (&due_at_utc, &due_tz) {
            (Some(u), Some(z)) => recurrence::utc_to_local(u, z).ok(),
            _ => None,
        },
        due_at_utc,
        due_tz,
        recurrence: match series {
            Some(series_id) => Some(RecurrenceView {
                series_id,
                rrule: r.get(15)?,
                tz: r.get(16)?,
                dtstart_local: r.get(17)?,
                weekday_times: serde_json::from_str(&r.get::<_, String>(23)?).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        23,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?,
            }),
            None => None,
        },
        reminder_offsets_min: serde_json::from_str(&offsets).unwrap_or_default(),
        created_at: r.get(12)?,
        updated_at: r.get(13)?,
        plan_date: r.get(18)?,
        plan_time: r.get(19)?,
        plan_duration_min: r.get(20)?,
        plan_tz: r.get(21)?,
        undo_action: r.get(22)?,
        has_schedule_changes: r.get(24)?,
        schedule_from: r.get(25)?,
    })
}

pub(crate) fn load_task(conn: &Connection, id: &str) -> Res<Task> {
    conn.query_row(
        &format!("{TASK_SELECT} WHERE t.id = ?1 AND t.deleted_at IS NULL"),
        [id],
        task_from_row,
    )
    .optional()?
    .ok_or_else(|| not_found("task"))
}

pub fn task_get(conn: &Connection, req: IdReq) -> Res<Task> {
    load_task(conn, &req.id)
}

pub fn task_list(conn: &Connection, req: TaskListReq) -> Res<Vec<Task>> {
    if let Some(s) = &req.status {
        if !["todo", "doing", "done", "cancelled"].contains(&s.as_str()) {
            return Err(val("unknown status filter"));
        }
    }
    let mut stmt = conn.prepare(&format!(
        "{TASK_SELECT} WHERE t.deleted_at IS NULL AND (?1 IS NULL OR t.status = ?1)
         ORDER BY COALESCE(t.due_date, t.due_at_utc, '9999'), t.created_at, t.id"
    ))?;
    let rows = stmt.query_map([req.status], task_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub(crate) struct Due {
    pub(crate) date: Option<String>,
    pub(crate) utc: Option<String>,
    pub(crate) tz: Option<String>,
}

pub(crate) fn resolve_due(d: &DueIn) -> Res<Due> {
    match d {
        DueIn::Date { date } => Ok(Due {
            date: Some(parse_date(date)?.to_string()),
            utc: None,
            tz: None,
        }),
        DueIn::At { local, tz } => {
            let utc = recurrence::local_to_utc(local, tz).map_err(val)?;
            Ok(Due {
                date: None,
                utc: Some(utc),
                tz: Some(tz.clone()),
            })
        }
    }
}

fn check_task_fields(
    priority: Option<u8>,
    energy: &Option<String>,
    duration: Option<u32>,
    offsets: &Option<Vec<i32>>,
) -> Res<()> {
    if priority.is_some_and(|p| p > 3) {
        return Err(val("priority must be 0-3"));
    }
    if energy
        .as_deref()
        .is_some_and(|e| !["low", "medium", "high"].contains(&e))
    {
        return Err(val("energy must be low, medium or high"));
    }
    if duration.is_some_and(|d| d == 0 || d > 1440) {
        return Err(val("duration_min must be 1-1440"));
    }
    if let Some(o) = offsets {
        if o.len() > 10 || o.iter().any(|m| m.abs() > 60 * 24 * 30) {
            return Err(val("at most 10 reminder offsets within 30 days"));
        }
    }
    Ok(())
}

pub fn task_create(conn: &mut Connection, ctx: &Ctx, req: TaskCreateReq) -> Res<Task> {
    let title = check_title(&req.title)?;
    let notes = req.notes.unwrap_or_default();
    check_notes(&notes)?;
    check_task_fields(
        req.priority,
        &req.energy,
        req.duration_min,
        &req.reminder_offsets_min,
    )?;
    if req.recurrence.is_some() && req.due.is_some() {
        return Err(val(
            "a recurring task takes its schedule from the recurrence, not a due value",
        ));
    }
    let due = req.due.as_ref().map(resolve_due).transpose()?;
    let tx = conn.transaction()?;
    let rule_id = match &req.recurrence {
        Some(rec) => {
            let rule = recurrence::parse_rule(&rec.rrule).map_err(val)?;
            let start = recurrence::parse_start(&rec.dtstart_local).map_err(val)?;
            validate_weekday_times(&rule, start, &rec.weekday_times)?;
            recurrence::tz(&rec.tz).map_err(val)?;
            let rid = new_id();
            tx.execute(
                "INSERT INTO recurrence_rules(id, created_at, updated_at, origin_device_id, series_id, rrule, tz, dtstart_local, weekday_times_json)
                 VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![rid, ctx.now, ctx.device_id, new_id(), rec.rrule, rec.tz, rec.dtstart_local,
                    serde_json::to_string(&rec.weekday_times).map_err(|_| IpcError::internal())?],
            )?;
            Some(rid)
        }
        None => None,
    };
    let id = new_id();
    tx.execute(
        "INSERT INTO tasks(id, created_at, updated_at, origin_device_id, title, notes, priority, energy, duration_min,
           due_date, due_at_utc, due_tz, recurrence_rule_id, reminder_offsets_json, provenance_json)
         VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            id,
            ctx.now,
            ctx.device_id,
            title,
            notes,
            req.priority.unwrap_or(0),
            req.energy,
            req.duration_min,
            due.as_ref().and_then(|d| d.date.clone()),
            due.as_ref().and_then(|d| d.utc.clone()),
            due.as_ref().and_then(|d| d.tz.clone()),
            rule_id,
            serde_json::to_string(&req.reminder_offsets_min.unwrap_or_default()).map_err(|_| IpcError::internal())?,
            serde_json::json!({"kind": "User", "device_id": ctx.device_id}).to_string(),
        ],
    )?;
    log_change(&tx, ctx, "User", "task", &id, 0, "task.create")?;
    tx.commit()?;
    load_task(conn, &id)
}

/// Inserts one plain task approved from an inbox candidate, inside the caller's
/// transaction. Provenance holds IDs and the source hash only, never source text.
pub(crate) fn insert_source_task(
    tx: &Connection,
    ctx: &Ctx,
    title: &str,
    due_date: Option<&str>,
    provenance: serde_json::Value,
) -> Res<String> {
    let title = check_title(title)?;
    if let Some(d) = due_date {
        if d.len() != 10 || d.parse::<jiff::civil::Date>().is_err() {
            return Err(val("due date must be YYYY-MM-DD"));
        }
    }
    let id = new_id();
    tx.execute(
        "INSERT INTO tasks(id, created_at, updated_at, origin_device_id, title, notes, due_date, reminder_offsets_json, provenance_json)
         VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6, '[]', ?7)",
        params![
            id,
            ctx.now,
            ctx.device_id,
            title,
            "Created from an inbox item you approved. Open Inbox to see the original message.",
            due_date,
            provenance.to_string(),
        ],
    )?;
    log_change(tx, ctx, "User", "task", &id, 0, "task.create")?;
    Ok(id)
}

pub(crate) fn check_revision(conn: &Connection, table: &str, id: &str, expected: u64) -> Res<()> {
    let cur: Option<i64> = conn
        .query_row(
            &format!("SELECT revision FROM {table} WHERE id = ?1 AND deleted_at IS NULL"),
            [id],
            |r| r.get(0),
        )
        .optional()?;
    match cur {
        None => Err(not_found(table)),
        Some(c) if c as u64 != expected => Err(conflict(c)),
        Some(_) => Ok(()),
    }
}

pub fn task_update(conn: &mut Connection, ctx: &Ctx, req: TaskUpdateReq) -> Res<Task> {
    check_task_fields(
        req.priority,
        &req.energy,
        req.duration_min,
        &req.reminder_offsets_min,
    )?;
    if let Some(s) = &req.status {
        if !["todo", "doing", "done", "cancelled"].contains(&s.as_str()) {
            return Err(val("unknown status"));
        }
    }
    if let Some(n) = &req.notes {
        check_notes(n)?;
    }
    let tx = conn.transaction()?;
    check_revision(&tx, "tasks", &req.id, req.expected_revision)?;
    let cur = load_task(&tx, &req.id)?;
    let recurring = cur.recurrence.is_some();
    if recurring
        && (req.due.is_some() || req.clear_due || matches!(req.status.as_deref(), Some("done")))
    {
        return Err(val(
            "recurring tasks change schedule/completion through occurrence_edit",
        ));
    }
    if let Some(edit) = &req.schedule {
        crate::schedule_edit::update(&tx, ctx, &cur, edit)?;
    }
    let due = if req.clear_due {
        Due {
            date: None,
            utc: None,
            tz: None,
        }
    } else if let Some(d) = &req.due {
        resolve_due(d)?
    } else {
        Due {
            date: cur.due_date.clone(),
            utc: cur.due_at_utc.clone(),
            tz: cur.due_tz.clone(),
        }
    };
    let title = match &req.title {
        Some(t) => check_title(t)?,
        None => cur.title.clone(),
    };
    tx.execute(
        "UPDATE tasks SET title=?1, notes=?2, status=?3, priority=?4, energy=?5, duration_min=?6,
           due_date=?7, due_at_utc=?8, due_tz=?9, reminder_offsets_json=?10, revision=revision+1, updated_at=?11
         WHERE id=?12 AND revision=?13",
        params![
            title,
            req.notes.unwrap_or(cur.notes),
            req.status.unwrap_or(cur.status),
            req.priority.unwrap_or(cur.priority),
            req.energy.or(cur.energy),
            req.duration_min.or(cur.duration_min),
            due.date,
            due.utc,
            due.tz,
            serde_json::to_string(&req.reminder_offsets_min.unwrap_or(cur.reminder_offsets_min)).map_err(|_| IpcError::internal())?,
            ctx.now,
            req.id,
            req.expected_revision,
        ],
    )?;
    log_change(
        &tx,
        ctx,
        "User",
        "task",
        &req.id,
        req.expected_revision as i64,
        "task.update",
    )?;
    tx.commit()?;
    load_task(conn, &req.id)
}

pub fn task_complete(conn: &mut Connection, ctx: &Ctx, req: RevisionReq) -> Res<Task> {
    let tx = conn.transaction()?;
    check_revision(&tx, "tasks", &req.id, req.expected_revision)?;
    if load_task(&tx, &req.id)?.recurrence.is_some() {
        return Err(val(
            "complete a recurring task per occurrence with occurrence_edit",
        ));
    }
    let prior = snapshot(&tx, &req.id)?;
    tx.execute(
        "UPDATE tasks SET status='done', revision=revision+1, updated_at=?1 WHERE id=?2",
        params![ctx.now, req.id],
    )?;
    record_undo(
        &tx,
        ctx,
        &req.id,
        "complete",
        &prior,
        req.expected_revision + 1,
    )?;
    log_change(
        &tx,
        ctx,
        "User",
        "task",
        &req.id,
        req.expected_revision as i64,
        "task.complete",
    )?;
    tx.commit()?;
    load_task(conn, &req.id)
}

pub fn task_delete(conn: &mut Connection, ctx: &Ctx, req: DeleteReq) -> Res<()> {
    if !req.confirm {
        return Err(IpcError::new(
            "ApprovalRequired",
            "deleting a task requires confirm=true",
        ));
    }
    let tx = conn.transaction()?;
    check_revision(&tx, "tasks", &req.id, req.expected_revision)?;
    let prior = snapshot(&tx, &req.id)?;
    let recurring = load_task(&tx, &req.id)?.recurrence.is_some();
    tx.execute(
        "UPDATE tasks SET deleted_at=?1, updated_at=?1, revision=revision+1 WHERE id=?2",
        params![ctx.now, req.id],
    )?;
    // A deleted series also deletes its rule; Undo is offered only for single tasks.
    if !recurring {
        record_undo(
            &tx,
            ctx,
            &req.id,
            "delete",
            &prior,
            req.expected_revision + 1,
        )?;
    }
    tx.execute(
        "UPDATE recurrence_rules SET deleted_at=?1, updated_at=?1, revision=revision+1
         WHERE id=(SELECT recurrence_rule_id FROM tasks WHERE id=?2)",
        params![ctx.now, req.id],
    )?;
    log_change(
        &tx,
        ctx,
        "User",
        "task",
        &req.id,
        req.expected_revision as i64,
        "task.delete",
    )?;
    tx.commit()?;
    Ok(())
}

/// Source task with a clock-time deadline (UTC instant + zone) from an exact approval.
pub(crate) fn insert_source_task_at(
    tx: &Connection,
    ctx: &Ctx,
    title: &str,
    due_at_utc: &str,
    tz: &str,
    provenance: serde_json::Value,
) -> Res<String> {
    let title = check_title(title)?;
    let id = new_id();
    tx.execute(
        "INSERT INTO tasks(id, created_at, updated_at, origin_device_id, title, notes, due_at_utc, due_tz, reminder_offsets_json, provenance_json)
         VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6, ?7, '[]', ?8)",
        params![
            id,
            ctx.now,
            ctx.device_id,
            title,
            "Created from an inbox item you approved. Open Messages to see the original message.",
            due_at_utc,
            tz,
            provenance.to_string(),
        ],
    )?;
    log_change(tx, ctx, "User", "task", &id, 0, "task.create")?;
    Ok(id)
}

// ---------------------------------------------------------------- work plans + undo

/// Bounded Undo window (docs/UI-PLANNER-DATES-NEXT.md §B).
pub const UNDO_WINDOW_MIN: i64 = 10;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanReq {
    pub id: String,
    pub expected_revision: u64,
    /// YYYY-MM-DD local day the owner wants to work on it.
    pub plan_date: String,
    /// Optional HH:MM start in `tz`.
    pub plan_time: Option<String>,
    pub duration_min: Option<u32>,
    /// IANA zone of the wall time; required with `plan_time`.
    pub tz: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
struct Snapshot {
    status: String,
    plan_date: Option<String>,
    plan_time: Option<String>,
    plan_duration_min: Option<u32>,
    plan_tz: Option<String>,
}

fn snapshot(tx: &Connection, id: &str) -> Res<Snapshot> {
    Ok(tx.query_row(
        "SELECT status, plan_date, plan_time, plan_duration_min, plan_tz FROM tasks WHERE id = ?1",
        [id],
        |r| {
            Ok(Snapshot {
                status: r.get(0)?,
                plan_date: r.get(1)?,
                plan_time: r.get(2)?,
                plan_duration_min: r.get(3)?,
                plan_tz: r.get(4)?,
            })
        },
    )?)
}

fn record_undo(
    tx: &Connection,
    ctx: &Ctx,
    id: &str,
    action: &str,
    prior: &Snapshot,
    after: u64,
) -> Res<()> {
    let expires = ctx
        .now
        .parse::<jiff::Timestamp>()
        .map_err(|_| IpcError::internal())?
        .checked_add(jiff::SignedDuration::from_mins(UNDO_WINDOW_MIN))
        .map_err(|_| IpcError::internal())?
        .to_string();
    tx.execute(
        "INSERT INTO task_undo(id, task_id, action, prior_json, after_revision, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![new_id(), id, action, serde_json::to_string(prior).map_err(|_| IpcError::internal())?, after, ctx.now, expires],
    )?;
    Ok(())
}

fn not_recurring(tx: &Connection, id: &str, what: &str) -> Res<Task> {
    let t = load_task(tx, id)?;
    if t.recurrence.is_some() {
        return Err(val(format!(
            "{what} for a recurring series is not supported yet; change single occurrences in the calendar"
        )));
    }
    Ok(t)
}

/// Sets the work plan. The hard deadline (`due_*`) is untouched.
pub fn task_plan(conn: &mut Connection, ctx: &Ctx, req: PlanReq) -> Res<Task> {
    let date = parse_date(&req.plan_date)?.to_string();
    let (time, tz) = match (&req.plan_time, &req.tz) {
        (None, _) => (None, None),
        (Some(t), Some(z)) => {
            if t.len() != 5 || t.parse::<jiff::civil::Time>().is_err() {
                return Err(val("plan time must be HH:MM"));
            }
            // Refuses wall times skipped/repeated by DST instead of shifting them.
            crate::dates::to_utc(&date, t, z).map_err(val)?;
            (Some(t.clone()), Some(z.clone()))
        }
        (Some(_), None) => return Err(val("a planned time needs a time zone")),
    };
    if req.duration_min.is_some_and(|d| !(5..=1440).contains(&d)) {
        return Err(val("planned duration must be 5-1440 minutes"));
    }
    let tx = conn.transaction()?;
    check_revision(&tx, "tasks", &req.id, req.expected_revision)?;
    not_recurring(&tx, &req.id, "Planning")?;
    let prior = snapshot(&tx, &req.id)?;
    tx.execute(
        "UPDATE tasks SET plan_date=?1, plan_time=?2, plan_duration_min=?3, plan_tz=?4,
           revision=revision+1, updated_at=?5 WHERE id=?6",
        params![date, time, req.duration_min, tz, ctx.now, req.id],
    )?;
    record_undo(&tx, ctx, &req.id, "plan", &prior, req.expected_revision + 1)?;
    log_change(
        &tx,
        ctx,
        "User",
        "task",
        &req.id,
        req.expected_revision as i64,
        "task.plan",
    )?;
    tx.commit()?;
    load_task(conn, &req.id)
}

/// Removes only the work block; task, deadline and source link stay.
pub fn task_unplan(conn: &mut Connection, ctx: &Ctx, req: RevisionReq) -> Res<Task> {
    let tx = conn.transaction()?;
    check_revision(&tx, "tasks", &req.id, req.expected_revision)?;
    let t = not_recurring(&tx, &req.id, "Unplanning")?;
    if t.plan_date.is_none() {
        return Err(val("this task has no work plan"));
    }
    let prior = snapshot(&tx, &req.id)?;
    tx.execute(
        "UPDATE tasks SET plan_date=NULL, plan_time=NULL, plan_duration_min=NULL, plan_tz=NULL,
           revision=revision+1, updated_at=?1 WHERE id=?2",
        params![ctx.now, req.id],
    )?;
    record_undo(
        &tx,
        ctx,
        &req.id,
        "unplan",
        &prior,
        req.expected_revision + 1,
    )?;
    log_change(
        &tx,
        ctx,
        "User",
        "task",
        &req.id,
        req.expected_revision as i64,
        "task.unplan",
    )?;
    tx.commit()?;
    load_task(conn, &req.id)
}

/// Reopens a completed single task.
pub fn task_reopen(conn: &mut Connection, ctx: &Ctx, req: RevisionReq) -> Res<Task> {
    let tx = conn.transaction()?;
    check_revision(&tx, "tasks", &req.id, req.expected_revision)?;
    let t = not_recurring(&tx, &req.id, "Reopening")?;
    if t.status != "done" {
        return Err(val("only a completed task can be reopened"));
    }
    tx.execute(
        "UPDATE tasks SET status='todo', revision=revision+1, updated_at=?1 WHERE id=?2",
        params![ctx.now, req.id],
    )?;
    log_change(
        &tx,
        ctx,
        "User",
        "task",
        &req.id,
        req.expected_revision as i64,
        "task.reopen",
    )?;
    tx.commit()?;
    load_task(conn, &req.id)
}

/// Undoes the latest complete/delete/plan/unplan of this task if nothing changed since
/// (revision check) and the window has not passed. Restores the stored database state.
pub fn task_undo(conn: &mut Connection, ctx: &Ctx, req: RevisionReq) -> Res<Task> {
    let tx = conn.transaction()?;
    let cur: Option<i64> = tx
        .query_row("SELECT revision FROM tasks WHERE id = ?1", [&req.id], |r| {
            r.get(0)
        })
        .optional()?;
    let cur = cur.ok_or_else(|| not_found("tasks"))?;
    if cur as u64 != req.expected_revision {
        return Err(conflict(cur));
    }
    let row: Option<(String, String, String, String)> = tx
        .query_row(
            "SELECT id, action, prior_json, expires_at FROM task_undo
             WHERE task_id = ?1 AND after_revision = ?2 AND used_at IS NULL ORDER BY created_at DESC LIMIT 1",
            params![req.id, cur],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((undo_id, action, prior, expires)) = row else {
        return Err(val("nothing to undo for this version"));
    };
    let now = ctx
        .now
        .parse::<jiff::Timestamp>()
        .map_err(|_| IpcError::internal())?;
    if expires
        .parse::<jiff::Timestamp>()
        .map_err(|_| IpcError::internal())?
        <= now
    {
        return Err(val("the undo window has passed"));
    }
    let p: Snapshot = serde_json::from_str(&prior).map_err(|_| IpcError::internal())?;
    tx.execute(
        "UPDATE tasks SET status=?1, plan_date=?2, plan_time=?3, plan_duration_min=?4, plan_tz=?5,
           deleted_at=NULL, revision=revision+1, updated_at=?6 WHERE id=?7",
        params![
            p.status,
            p.plan_date,
            p.plan_time,
            p.plan_duration_min,
            p.plan_tz,
            ctx.now,
            req.id
        ],
    )?;
    tx.execute(
        "UPDATE task_undo SET used_at = ?1 WHERE id = ?2",
        params![ctx.now, undo_id],
    )?;
    log_change(
        &tx,
        ctx,
        "User",
        "task",
        &req.id,
        cur,
        &format!("task.undo_{action}"),
    )?;
    tx.commit()?;
    load_task(conn, &req.id)
}

// ---------------------------------------------------------------- occurrences

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OccurrenceEditReq {
    pub task_id: String,
    /// Revision of the task (series); every occurrence edit bumps it.
    pub expected_revision: u64,
    /// `this` | `following` | `series`
    pub scope: String,
    /// `done` | `skip` | `move` | `reset` (this); `skip` (following); `move` retimes (series).
    pub action: String,
    /// Local start of the occurrence being edited (date for all-day series).
    pub original_local: String,
    pub new_local: Option<String>,
}

const WEEKDAYS: [&str; 7] = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];

pub(crate) fn validate_weekday_times(
    rule: &recurrence::Rule,
    start: Start,
    times: &std::collections::BTreeMap<String, String>,
) -> Res<()> {
    if times.is_empty() {
        return Ok(());
    }
    if rule.freq != recurrence::Freq::Weekly
        || rule.by_day.is_empty()
        || start.time.is_none()
        || times.len() != rule.by_day.len()
    {
        return Err(val(
            "per-day times require a timed weekly rule and exactly one time per selected weekday",
        ));
    }
    for day in &rule.by_day {
        let key = WEEKDAYS[day.to_monday_zero_offset() as usize];
        let time = times
            .get(key)
            .ok_or_else(|| val("missing selected weekday time"))?;
        if time.len() != 5
            || !time.is_ascii()
            || time.as_bytes()[2] != b':'
            || time.parse::<jiff::civil::Time>().is_err()
        {
            return Err(val("weekday times must be valid HH:MM wall times"));
        }
    }
    Ok(())
}

pub(crate) fn scheduled_original(date: Date, start: Start, rv: &RecurrenceView) -> Res<String> {
    let key = WEEKDAYS[date.weekday().to_monday_zero_offset() as usize];
    let time = rv
        .weekday_times
        .get(key)
        .map(|v| {
            v.parse::<jiff::civil::Time>()
                .map_err(|_| val("invalid weekday time"))
        })
        .transpose()?
        .or(start.time);
    Ok(recurrence::format_local(date, time))
}

fn rule_of(task: &Task) -> Res<(recurrence::Rule, Start, &RecurrenceView)> {
    let rv = task
        .recurrence
        .as_ref()
        .ok_or_else(|| val("task is not recurring"))?;
    let rule = recurrence::parse_rule(&rv.rrule).map_err(val)?;
    let start = recurrence::parse_start(&rv.dtstart_local).map_err(val)?;
    validate_weekday_times(&rule, start, &rv.weekday_times)?;
    Ok((rule, start, rv))
}

fn rewrite_until(rrule: &str, last: Date) -> String {
    let mut parts: Vec<String> = rrule
        .trim_start_matches("RRULE:")
        .split(';')
        .filter(|p| !p.is_empty() && !p.starts_with("COUNT=") && !p.starts_with("UNTIL="))
        .map(String::from)
        .collect();
    parts.push(format!(
        "UNTIL={:04}{:02}{:02}",
        last.year(),
        last.month(),
        last.day()
    ));
    parts.join(";")
}

pub fn occurrence_edit(conn: &mut Connection, ctx: &Ctx, req: OccurrenceEditReq) -> Res<Task> {
    let tx = conn.transaction()?;
    check_revision(&tx, "tasks", &req.task_id, req.expected_revision)?;
    let mut task = load_task(&tx, &req.task_id)?;
    let orig = recurrence::parse_start(&req.original_local).map_err(val)?;
    if req.scope != "this" && task.schedule_from.is_some() {
        return Err(val("Use Edit schedule for routine changes; individual meetings remain editable in Calendar."));
    }
    task.recurrence = Some(crate::schedule_edit::at_date(&tx, &task, orig.date)?);
    let (rule, start, rv) = rule_of(&task)?;
    let saved:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM occurrences WHERE series_id=?1 AND original_local=?2 AND deleted_at IS NULL)",params![rv.series_id,req.original_local],|r|r.get(0))?;
    let scheduled =
        recurrence::parse_start(&scheduled_original(orig.date, start, rv)?).map_err(val)?;
    if orig.time != scheduled.time && !(req.scope == "this" && saved) {
        return Err(val("original_local does not match the series time of day"));
    }
    let hit = recurrence::expand(&rule, start.date, orig.date, orig.date).map_err(val)?;
    if hit.is_empty() && !(req.scope == "this" && saved) {
        return Err(val("original_local is not an occurrence of this series"));
    }
    match (req.scope.as_str(), req.action.as_str()) {
        ("this", a @ ("done" | "skip" | "move" | "reset")) => {
            let (local, room) =
                crate::scheduled::effective(&tx, &task.id, orig.date, &req.original_local)?;
            let snapshot = (local != req.original_local || room.is_some())
                .then(|| serde_json::json!({"scheduled_local":local,"location":room}).to_string());
            let (state, over) = match a {
                "done" => ("Done", snapshot.clone()),
                "skip" => ("Skipped", snapshot),
                "reset" => ("Normal", None),
                _ => {
                    let nl = req
                        .new_local
                        .as_deref()
                        .ok_or_else(|| val("move requires new_local"))?;
                    let n = recurrence::parse_start(nl).map_err(val)?;
                    if n.time.is_some() != start.time.is_some() {
                        return Err(val("new_local must match the series (date vs date-time)"));
                    }
                    (
                        "Moved",
                        Some(serde_json::json!({"new_local": nl, "location":room}).to_string()),
                    )
                }
            };
            tx.execute(
                "INSERT INTO occurrences(id, created_at, updated_at, origin_device_id, series_id, original_local, state, override_json)
                 VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(series_id, original_local) DO UPDATE SET state=excluded.state,
                   override_json=excluded.override_json, updated_at=excluded.updated_at, revision=revision+1, deleted_at=NULL",
                params![new_id(), ctx.now, ctx.device_id, rv.series_id, req.original_local, state, over],
            )?;
        }
        ("following", "skip") => {
            if orig.date <= start.date {
                tx.execute(
                    "UPDATE tasks SET status='cancelled' WHERE id=?1",
                    [&req.task_id],
                )?;
            } else {
                let last = orig
                    .date
                    .checked_sub(1.day())
                    .map_err(|e| val(e.to_string()))?;
                tx.execute(
                    "UPDATE recurrence_rules SET rrule=?1, revision=revision+1, updated_at=?2 WHERE series_id=?3",
                    params![rewrite_until(&rv.rrule, last), ctx.now, rv.series_id],
                )?;
            }
        }
        ("series", "move") => {
            let nl = req
                .new_local
                .as_deref()
                .ok_or_else(|| val("series move requires new_local"))?;
            let n = recurrence::parse_start(nl).map_err(val)?;
            let (Some(new_time), Some(_)) = (n.time, start.time) else {
                return Err(val("only timed series can be retimed"));
            };
            if !rv.weekday_times.is_empty() && !new_time.to_string().ends_with(":00") {
                return Err(val("per-day schedules use whole minutes"));
            }
            let new_start = recurrence::format_local(start.date, Some(new_time));
            let retimed: std::collections::BTreeMap<_, _> = rv
                .weekday_times
                .keys()
                .map(|k| (k.clone(), new_time.to_string()[..5].to_string()))
                .collect();
            tx.execute(
                "UPDATE recurrence_rules SET dtstart_local=?1, revision=revision+1, updated_at=?2, weekday_times_json=?4 WHERE series_id=?3",
                params![new_start, ctx.now, rv.series_id, serde_json::to_string(&retimed).map_err(|_| IpcError::internal())?],
            )?;
            // Overrides are keyed by original wall time; re-key them to the new time of day.
            tx.execute(
                "UPDATE occurrences SET original_local = substr(original_local, 1, 10) || 'T' || ?1 WHERE series_id=?2",
                params![new_time.to_string(), rv.series_id],
            )?;
        }
        _ => return Err(val("unsupported scope/action combination")),
    }
    tx.execute(
        "UPDATE tasks SET revision=revision+1, updated_at=?1 WHERE id=?2",
        params![ctx.now, req.task_id],
    )?;
    log_change(
        &tx,
        ctx,
        "User",
        "task",
        &req.task_id,
        req.expected_revision as i64,
        &format!("occurrence.{}.{}", req.scope, req.action),
    )?;
    tx.commit()?;
    load_task(conn, &req.task_id)
}

// ---------------------------------------------------------------- events (P04 local calendar)

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventCreateReq {
    pub title: String,
    pub all_day: bool,
    /// Date (all-day, end inclusive) or wall-clock date-time in `tz`.
    pub start_local: String,
    pub end_local: String,
    pub tz: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventUpdateReq {
    pub id: String,
    pub expected_revision: u64,
    pub title: Option<String>,
    pub start_local: Option<String>,
    pub end_local: Option<String>,
    pub tz: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RangeReq {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Event {
    pub id: String,
    pub revision: u64,
    pub title: String,
    pub all_day: bool,
    pub start_local: String,
    pub end_local: String,
    pub start_utc: Option<String>,
    pub end_utc: Option<String>,
    pub tz: String,
    pub provider: Option<String>,
    pub read_only: bool,
}

fn event_from_row(r: &Row) -> rusqlite::Result<Event> {
    let all_day: bool = r.get(3)?;
    let tz: String = r.get(4)?;
    let (s, e): (String, String) = (r.get(5)?, r.get(6)?);
    let local = |v: &str| {
        if all_day {
            v.to_string()
        } else {
            recurrence::utc_to_local(v, &tz).unwrap_or_else(|_| v.to_string())
        }
    };
    Ok(Event {
        id: r.get(0)?,
        revision: r.get(1)?,
        title: r.get(2)?,
        all_day,
        start_local: local(&s),
        end_local: local(&e),
        start_utc: (!all_day).then(|| s.clone()),
        end_utc: (!all_day).then(|| e.clone()),
        tz,
        provider: r.get(7)?,
        read_only: r.get::<_, i64>(8)? != 0,
    })
}

const EVENT_SELECT: &str =
    "SELECT id, revision, title, all_day, tz, start_value, end_value, provider, read_only
    FROM events WHERE deleted_at IS NULL";

fn load_event(conn: &Connection, id: &str) -> Res<Event> {
    conn.query_row(&format!("{EVENT_SELECT} AND id = ?1"), [id], event_from_row)
        .optional()?
        .ok_or_else(|| not_found("event"))
}

/// Normalises request values to stored form: all-day → dates, timed → UTC instants.
fn event_values(all_day: bool, start: &str, end: &str, tz: &str) -> Res<(String, String)> {
    recurrence::tz(tz).map_err(val)?;
    if all_day {
        let (s, e) = (parse_date(start)?, parse_date(end)?);
        if e < s {
            return Err(val("end must not precede start"));
        }
        return Ok((s.to_string(), e.to_string()));
    }
    let (s, e) = (
        recurrence::local_to_utc(start, tz).map_err(val)?,
        recurrence::local_to_utc(end, tz).map_err(val)?,
    );
    let secs = |v: &str| {
        v.parse::<jiff::Timestamp>()
            .map(|t| t.as_second())
            .unwrap_or(0)
    };
    if secs(&e) <= secs(&s) {
        return Err(val("end must be after start"));
    }
    if secs(&e) - secs(&s) > 14 * 86_400 {
        return Err(val("events are limited to 14 days"));
    }
    Ok((s, e))
}

pub fn event_create(conn: &mut Connection, ctx: &Ctx, req: EventCreateReq) -> Res<Event> {
    let title = check_title(&req.title)?;
    let (s, e) = event_values(req.all_day, &req.start_local, &req.end_local, &req.tz)?;
    let tx = conn.transaction()?;
    let id = new_id();
    tx.execute(
        "INSERT INTO events(id, created_at, updated_at, origin_device_id, title, start_value, end_value, all_day, tz)
         VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![id, ctx.now, ctx.device_id, title, s, e, req.all_day, req.tz],
    )?;
    log_change(&tx, ctx, "User", "event", &id, 0, "event.create")?;
    tx.commit()?;
    load_event(conn, &id)
}

pub fn event_update(conn: &mut Connection, ctx: &Ctx, req: EventUpdateReq) -> Res<Event> {
    let tx = conn.transaction()?;
    check_revision(&tx, "events", &req.id, req.expected_revision)?;
    let cur = load_event(&tx, &req.id)?;
    if cur.read_only || cur.provider.is_some() {
        return Err(IpcError::new(
            "Forbidden",
            "source-owned events are read-only without write consent",
        ));
    }
    let title = match &req.title {
        Some(t) => check_title(t)?,
        None => cur.title.clone(),
    };
    let tz = req.tz.unwrap_or(cur.tz);
    let (s, e) = event_values(
        cur.all_day,
        req.start_local.as_deref().unwrap_or(&cur.start_local),
        req.end_local.as_deref().unwrap_or(&cur.end_local),
        &tz,
    )?;
    tx.execute(
        "UPDATE events SET title=?1, start_value=?2, end_value=?3, tz=?4, revision=revision+1, updated_at=?5 WHERE id=?6",
        params![title, s, e, tz, ctx.now, req.id],
    )?;
    log_change(
        &tx,
        ctx,
        "User",
        "event",
        &req.id,
        req.expected_revision as i64,
        "event.update",
    )?;
    tx.commit()?;
    load_event(conn, &req.id)
}

pub fn event_delete(conn: &mut Connection, ctx: &Ctx, req: DeleteReq) -> Res<()> {
    if !req.confirm {
        return Err(IpcError::new(
            "ApprovalRequired",
            "deleting an event requires confirm=true",
        ));
    }
    let tx = conn.transaction()?;
    check_revision(&tx, "events", &req.id, req.expected_revision)?;
    let cur = load_event(&tx, &req.id)?;
    if cur.read_only || cur.provider.is_some() {
        return Err(IpcError::new(
            "Forbidden",
            "source-owned events cannot be deleted locally",
        ));
    }
    tx.execute(
        "UPDATE events SET deleted_at=?1, updated_at=?1, revision=revision+1 WHERE id=?2",
        params![ctx.now, req.id],
    )?;
    log_change(
        &tx,
        ctx,
        "User",
        "event",
        &req.id,
        req.expected_revision as i64,
        "event.delete",
    )?;
    tx.commit()?;
    Ok(())
}

pub fn event_list(conn: &Connection, req: RangeReq) -> Res<Vec<Event>> {
    let (from, to) = check_range(&req.from, &req.to)?;
    let mut stmt = conn.prepare(&format!("{EVENT_SELECT} ORDER BY start_value, id"))?;
    let all = stmt
        .query_map([], event_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(all
        .into_iter()
        .filter(|e| {
            // Overlap of the event's local date span with [from, to].
            let (s, en) = (
                e.start_local.get(..10).and_then(|x| x.parse::<Date>().ok()),
                e.end_local.get(..10).and_then(|x| x.parse::<Date>().ok()),
            );
            matches!((s, en), (Some(s), Some(en)) if s <= to && en >= from)
        })
        .collect())
}

// ---------------------------------------------------------------- agenda

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct AgendaItem {
    /// `task` (deadline) | `plan` (work block) | `occurrence` | `event`
    pub kind: String,
    pub id: String,
    pub title: String,
    pub local_start: String,
    pub all_day: bool,
    /// `Normal` | `Done` | `Skipped` | `Moved` for occurrences; task status for tasks.
    pub state: String,
    pub original_local: Option<String>,
    pub task_revision: Option<u64>,
    pub recurring: bool,
    pub read_only: bool,
    /// True when this timed item overlaps another active timed item.
    pub conflict: bool,
    pub location: Option<String>,
    #[serde(skip)]
    span: Option<(i64, i64)>,
}

fn ts(utc: &str) -> Option<i64> {
    utc.parse::<jiff::Timestamp>().ok().map(|t| t.as_second())
}

pub fn agenda_list(conn: &Connection, req: RangeReq) -> Res<Vec<AgendaItem>> {
    let (from, to) = check_range(&req.from, &req.to)?;
    let in_range = |d: Date| d >= from && d <= to;
    let mut items: Vec<AgendaItem> = vec![];

    for t in task_list(conn, TaskListReq { status: None })? {
        let dur = t.duration_min.map(|m| m as i64 * 60);
        match &t.recurrence {
            None => {
                if let Some(pd) = &t.plan_date {
                    let (local, span) = match (&t.plan_time, &t.plan_tz) {
                        (Some(pt), Some(z)) => {
                            let s = crate::dates::to_utc(pd, pt, z).ok().and_then(|u| ts(&u));
                            let d = t.plan_duration_min.map(|m| m as i64 * 60).or(dur);
                            (format!("{pd}T{pt}"), s.zip(d).map(|(s, d)| (s, s + d)))
                        }
                        _ => (pd.clone(), None),
                    };
                    if pd.parse().ok().is_some_and(in_range) {
                        items.push(AgendaItem {
                            kind: "plan".into(),
                            id: t.id.clone(),
                            title: t.title.clone(),
                            all_day: t.plan_time.is_none(),
                            local_start: local,
                            state: t.status.clone(),
                            original_local: None,
                            task_revision: Some(t.revision),
                            recurring: false,
                            read_only: false,
                            conflict: false,
                            location: None,
                            span,
                        });
                    }
                }
                let (local, all_day, span) = match (&t.due_date, &t.due_at_local, &t.due_at_utc) {
                    (Some(d), _, _) => (d.clone(), true, None),
                    (_, Some(l), Some(u)) => {
                        (l.clone(), false, ts(u).zip(dur).map(|(s, d)| (s, s + d)))
                    }
                    _ => continue,
                };
                if !local
                    .get(..10)
                    .and_then(|x| x.parse().ok())
                    .is_some_and(in_range)
                {
                    continue;
                }
                items.push(AgendaItem {
                    kind: "task".into(),
                    id: t.id.clone(),
                    title: t.title.clone(),
                    local_start: local,
                    all_day,
                    state: t.status.clone(),
                    original_local: None,
                    task_revision: Some(t.revision),
                    recurring: false,
                    read_only: false,
                    conflict: false,
                    location: None,
                    span,
                });
            }
            Some(rv) => {
                let (_, start, _) = rule_of(&t)?;
                let tz_name = &rv.tz;
                let overrides: Vec<(String, String, Option<String>)> = {
                    let mut st = conn.prepare("SELECT original_local, state, override_json FROM occurrences WHERE series_id=?1 AND deleted_at IS NULL")?;
                    let rows = st
                        .query_map([&rv.series_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                        .collect::<Result<_, _>>()?;
                    rows
                };
                let mut push =
                    |orig: String, local: String, state: String, location: Option<String>| {
                        let span = (start.time.is_some())
                            .then(|| {
                                recurrence::local_to_utc(&local, tz_name)
                                    .ok()
                                    .and_then(|u| ts(&u))
                            })
                            .flatten()
                            .zip(dur)
                            .map(|(s, d)| (s, s + d));
                        items.push(AgendaItem {
                            kind: "occurrence".into(),
                            id: t.id.clone(),
                            title: t.title.clone(),
                            all_day: start.time.is_none(),
                            local_start: local,
                            state,
                            original_local: Some(orig),
                            task_revision: Some(t.revision),
                            recurring: true,
                            read_only: false,
                            conflict: false,
                            location,
                            span,
                        });
                    };
                if t.status != "cancelled" {
                    // Pad the expansion window so occurrences moved into/out of range are found.
                    let pad_from = from.checked_sub(366.days()).unwrap_or(from);
                    let mut emitted = std::collections::BTreeSet::new();
                    for (version, lower, upper) in crate::schedule_edit::segments(conn, &t)? {
                        let version_rule = recurrence::parse_rule(&version.rrule).map_err(val)?;
                        let version_start =
                            recurrence::parse_start(&version.dtstart_local).map_err(val)?;
                        for d in recurrence::expand(&version_rule, version_start.date, pad_from, to)
                            .map_err(val)?
                        {
                            if lower.is_some_and(|lo| d < lo) || upper.is_some_and(|hi| d > hi) {
                                continue;
                            }
                            let base = scheduled_original(d, version_start, &version)?;
                            let decision = overrides.iter().find(|o| o.0 == base).or_else(|| {
                                overrides.iter().find(|o| {
                                    o.1 != "Normal" && o.0.get(..10) == Some(&d.to_string())
                                })
                            });
                            let orig = decision
                                .filter(|o| o.1 != "Normal")
                                .map(|o| o.0.clone())
                                .unwrap_or(base);
                            if let Some(o) = decision {
                                emitted.insert(o.0.clone());
                            }
                            let (amended, room) =
                                crate::scheduled::effective(conn, &t.id, d, &orig)?;
                            match decision {
                                Some((_, state, over)) if state == "Moved" => {
                                    let new_local = over
                                        .as_deref()
                                        .and_then(|j| {
                                            serde_json::from_str::<serde_json::Value>(j).ok()
                                        })
                                        .and_then(|v| v["new_local"].as_str().map(String::from));
                                    if let Some(nl) = new_local.filter(|nl| {
                                        nl.get(..10)
                                            .and_then(|x| x.parse().ok())
                                            .is_some_and(in_range)
                                    }) {
                                        let room = over
                                            .as_deref()
                                            .and_then(|s| {
                                                serde_json::from_str::<serde_json::Value>(s).ok()
                                            })
                                            .and_then(|v| v["location"].as_str().map(String::from));
                                        push(orig, nl, "Moved".into(), room);
                                    }
                                }
                                Some((_, state, over)) if in_range(d) => {
                                    if state == "Normal" {
                                        push(orig, amended, state.clone(), room);
                                    } else {
                                        let snapshot = over.as_deref().and_then(|s| {
                                            serde_json::from_str::<serde_json::Value>(s).ok()
                                        });
                                        let local = snapshot
                                            .as_ref()
                                            .and_then(|v| v["scheduled_local"].as_str())
                                            .map(String::from)
                                            .unwrap_or_else(|| orig.clone());
                                        let room = snapshot
                                            .as_ref()
                                            .and_then(|v| v["location"].as_str())
                                            .map(String::from);
                                        push(orig, local, state.clone(), room);
                                    }
                                }
                                None if in_range(d) => push(orig, amended, "Normal".into(), room),
                                _ => {}
                            }
                        }
                    }
                    for (orig, state, over) in &overrides {
                        if emitted.contains(orig) || state == "Normal" {
                            continue;
                        }
                        let v = over
                            .as_deref()
                            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
                        let local = v
                            .as_ref()
                            .and_then(|v| {
                                v[if state == "Moved" {
                                    "new_local"
                                } else {
                                    "scheduled_local"
                                }]
                                .as_str()
                            })
                            .unwrap_or(orig)
                            .to_string();
                        if local
                            .get(..10)
                            .and_then(|d| d.parse().ok())
                            .is_some_and(in_range)
                        {
                            let room = v
                                .as_ref()
                                .and_then(|v| v["location"].as_str())
                                .map(String::from);
                            push(orig.clone(), local, state.clone(), room);
                        }
                    }
                }
            }
        }
    }

    for e in event_list(
        conn,
        RangeReq {
            from: req.from.clone(),
            to: req.to.clone(),
        },
    )? {
        items.push(AgendaItem {
            kind: "event".into(),
            id: e.id,
            title: e.title,
            local_start: e.start_local,
            all_day: e.all_day,
            state: "Normal".into(),
            original_local: None,
            task_revision: None,
            recurring: false,
            read_only: e.read_only || e.provider.is_some(),
            conflict: false,
            location: None,
            span: e
                .start_utc
                .as_deref()
                .and_then(ts)
                .zip(e.end_utc.as_deref().and_then(ts)),
        });
    }

    let active =
        |i: &AgendaItem| !matches!(i.state.as_str(), "Done" | "Skipped" | "done" | "cancelled");
    let spans: Vec<Option<(i64, i64)>> = items
        .iter()
        .map(|i| if active(i) { i.span } else { None })
        .collect();
    for (i, item) in items.iter_mut().enumerate() {
        if let Some((s, e)) = spans[i] {
            item.conflict = spans
                .iter()
                .enumerate()
                .any(|(j, o)| j != i && o.is_some_and(|(s2, e2)| s < e2 && s2 < e));
        }
    }
    items.sort_by(|a, b| (&a.local_start, &a.title, &a.id).cmp(&(&b.local_start, &b.title, &b.id)));
    Ok(items)
}
