//! Proposal → exact, expiring approval → single transactional execution (docs/INTERFACES.md §4).
//! Producers (the deterministic planner now, AI tools later) can only create proposals; the
//! approval path re-validates hash, expiry and target revision before anything is written.
use crate::commands::IpcError;
use crate::planner::{self, Ctx, DueIn, Res};
use jiff::{SignedDuration, Timestamp};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const ACTION_TASK_RESCHEDULE: &str = "task.reschedule";
const TTL_HOURS: i64 = 24;

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Proposal {
    pub id: String,
    pub action: String,
    pub target_id: Option<String>,
    pub target_title: Option<String>,
    pub expected_target_revision: Option<u64>,
    pub payload: Value,
    pub payload_hash: String,
    pub expires_at: String,
    pub status: String,
    pub confidence: f64,
    pub rationale: String,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalListReq {
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalApproveReq {
    pub id: String,
    /// The hash the user saw; approval is bound to exactly this payload.
    pub payload_hash: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalIdReq {
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuggestReq {
    /// Local date considered "today" by the caller.
    pub today: String,
}

/// Hash of the canonical form. serde_json maps are key-sorted and compact, which is
/// deterministic for the string/integer payloads used here; strict RFC 8785 number
/// canonicalisation is owned by the security phase before AI producers exist.
pub fn hash_payload(
    action: &str,
    target_id: &Option<String>,
    rev: Option<u64>,
    payload: &Value,
    expires_at: &str,
) -> String {
    let canonical = json!({
        "action": action, "target_id": target_id, "expected_target_revision": rev,
        "payload": payload, "source_revisions": [], "expires_at": expires_at,
    });
    let digest = Sha256::digest(
        serde_json::to_string(&canonical)
            .unwrap_or_default()
            .as_bytes(),
    );
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

const SELECT: &str = "SELECT p.id, p.action, p.target_id, (SELECT title FROM tasks WHERE id = p.target_id), p.expected_target_revision,
    p.payload_json, p.payload_hash, p.expires_at, p.status, p.confidence, p.rationale, p.created_at FROM proposals p";

fn from_row(r: &Row) -> rusqlite::Result<Proposal> {
    let payload: String = r.get(5)?;
    Ok(Proposal {
        id: r.get(0)?,
        action: r.get(1)?,
        target_id: r.get(2)?,
        target_title: r.get(3)?,
        expected_target_revision: r.get::<_, Option<i64>>(4)?.map(|v| v as u64),
        payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
        payload_hash: r.get(6)?,
        expires_at: r.get(7)?,
        status: r.get(8)?,
        confidence: r.get(9)?,
        rationale: r.get(10)?,
        created_at: r.get(11)?,
    })
}

fn load(conn: &Connection, id: &str) -> Res<Proposal> {
    conn.query_row(
        &format!("{SELECT} WHERE p.id = ?1 AND p.deleted_at IS NULL"),
        [id],
        from_row,
    )
    .optional()?
    .ok_or_else(|| IpcError::new("NotFound", "proposal not found"))
}

fn instant(s: &str) -> Res<Timestamp> {
    s.parse().map_err(|_| IpcError::internal())
}

fn invalid(msg: &str) -> IpcError {
    IpcError::new("ApprovalInvalid", msg)
}

/// Creates a pending reschedule proposal bound to the target's current revision.
pub fn create_reschedule(
    conn: &mut Connection,
    ctx: &Ctx,
    task_id: &str,
    due: DueIn,
    rationale: &str,
    confidence: f64,
) -> Res<Proposal> {
    planner::resolve_due(&due)?; // payload must be valid before it can ever be approved
    let tx = conn.transaction()?;
    let task = planner::load_task(&tx, task_id)?;
    if task.recurrence.is_some() {
        return Err(IpcError::new(
            "Validation",
            "recurring tasks are rescheduled per occurrence",
        ));
    }
    let payload = json!({ "due": match &due {
        DueIn::Date { date } => json!({"kind": "date", "date": date}),
        DueIn::At { local, tz } => json!({"kind": "at", "local": local, "tz": tz}),
    }});
    let expires_at = instant(ctx.now)?
        .checked_add(SignedDuration::from_hours(TTL_HOURS))
        .map_err(|_| IpcError::internal())?
        .strftime("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let target = Some(task_id.to_string());
    let hash = hash_payload(
        ACTION_TASK_RESCHEDULE,
        &target,
        Some(task.revision),
        &payload,
        &expires_at,
    );
    let id = planner::new_id();
    tx.execute(
        "INSERT INTO proposals(id, created_at, updated_at, origin_device_id, action, target_id, expected_target_revision,
           payload_json, payload_hash, expires_at, confidence, rationale)
         VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![id, ctx.now, ctx.device_id, ACTION_TASK_RESCHEDULE, task_id, task.revision, payload.to_string(), hash, expires_at, confidence, rationale],
    )?;
    planner::log_change(&tx, ctx, "Worker", "proposal", &id, 0, "proposal.create")?;
    tx.commit()?;
    load(conn, &id)
}

/// Deterministic producer: overdue date-only tasks get a "move to today" proposal (no duplicates).
pub fn suggest_reschedules(
    conn: &mut Connection,
    ctx: &Ctx,
    req: SuggestReq,
) -> Res<Vec<Proposal>> {
    let today: jiff::civil::Date = req
        .today
        .parse()
        .map_err(|_| IpcError::new("Validation", "invalid date"))?;
    expire_due(conn, ctx)?;
    let tasks = planner::task_list(conn, planner::TaskListReq { status: None })?;
    let mut out = vec![];
    for t in tasks {
        let overdue = t.recurrence.is_none()
            && matches!(t.status.as_str(), "todo" | "doing")
            && t.due_date
                .as_deref()
                .and_then(|d| d.parse::<jiff::civil::Date>().ok())
                .is_some_and(|d| d < today);
        let pending: i64 = conn.query_row(
            "SELECT count(*) FROM proposals WHERE target_id=?1 AND status='Pending' AND deleted_at IS NULL",
            [&t.id],
            |r| r.get(0),
        )?;
        if overdue && pending == 0 {
            let why = format!(
                "Overdue since {}; move to today.",
                t.due_date.clone().unwrap_or_default()
            );
            out.push(create_reschedule(
                conn,
                ctx,
                &t.id,
                DueIn::Date {
                    date: today.to_string(),
                },
                &why,
                0.9,
            )?);
        }
    }
    Ok(out)
}

pub fn expire_due(conn: &Connection, ctx: &Ctx) -> Res<()> {
    conn.execute("UPDATE proposals SET status='Expired', updated_at=?1, revision=revision+1 WHERE status='Pending' AND expires_at <= ?1", [ctx.now])?;
    Ok(())
}

pub fn list(conn: &Connection, ctx: &Ctx, req: ProposalListReq) -> Res<Vec<Proposal>> {
    expire_due(conn, ctx)?;
    let mut stmt = conn.prepare(&format!("{SELECT} WHERE p.deleted_at IS NULL AND (?1 IS NULL OR p.status = ?1) ORDER BY p.created_at DESC, p.id"))?;
    let rows = stmt.query_map([req.status], from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn mark(tx: &Connection, ctx: &Ctx, id: &str, status: &str, action: &str) -> Res<()> {
    tx.execute(
        "UPDATE proposals SET status=?1, updated_at=?2, revision=revision+1 WHERE id=?3",
        params![status, ctx.now, id],
    )?;
    planner::log_change(tx, ctx, "User", "proposal", id, 0, action)
}

pub fn reject(conn: &mut Connection, ctx: &Ctx, req: ProposalIdReq) -> Res<Proposal> {
    let tx = conn.transaction()?;
    if load(&tx, &req.id)?.status != "Pending" {
        return Err(invalid("proposal is no longer pending"));
    }
    mark(&tx, ctx, &req.id, "Rejected", "proposal.reject")?; // target is never touched
    tx.commit()?;
    load(conn, &req.id)
}

/// Marks the proposal stale (committed, so the refusal is durable) and returns `ApprovalInvalid`.
fn refuse_stale(tx: rusqlite::Transaction, ctx: &Ctx, id: &str, reason: &str) -> Res<Proposal> {
    mark(&tx, ctx, id, "Expired", "proposal.stale")?;
    tx.commit()?;
    Err(invalid(reason))
}

pub fn approve(conn: &mut Connection, ctx: &Ctx, req: ProposalApproveReq) -> Res<Proposal> {
    let tx = conn.transaction()?;
    let p = load(&tx, &req.id)?;
    if p.status == "Executed" {
        // Idempotent replay: same exact payload returns the original result, anything else is refused.
        let approved: Option<String> = tx
            .query_row(
                "SELECT payload_hash FROM approvals WHERE proposal_id=?1",
                [&p.id],
                |r| r.get(0),
            )
            .optional()?;
        return match approved {
            Some(h) if h == req.payload_hash => Ok(p),
            _ => Err(invalid(
                "proposal already executed with a different payload",
            )),
        };
    }
    if p.status != "Pending" {
        return Err(invalid("proposal is not pending"));
    }
    let recomputed = hash_payload(
        &p.action,
        &p.target_id,
        p.expected_target_revision,
        &p.payload,
        &p.expires_at,
    );
    if recomputed != p.payload_hash || req.payload_hash != p.payload_hash {
        return Err(invalid("payload hash mismatch")); // nothing changes; user may re-review
    }
    if instant(ctx.now)? >= instant(&p.expires_at)? {
        return refuse_stale(tx, ctx, &p.id, "proposal expired");
    }
    if p.action != ACTION_TASK_RESCHEDULE {
        return Err(invalid("action not allowed"));
    }
    let target = p
        .target_id
        .clone()
        .ok_or_else(|| invalid("missing target"))?;
    let Ok(task) = planner::load_task(&tx, &target) else {
        return refuse_stale(tx, ctx, &p.id, "target no longer exists");
    };
    if Some(task.revision) != p.expected_target_revision || task.recurrence.is_some() {
        return refuse_stale(tx, ctx, &p.id, "target changed since the proposal was made");
    }
    let due: DueIn = serde_json::from_value(p.payload["due"].clone())
        .map_err(|_| invalid("malformed payload"))?;
    let due = planner::resolve_due(&due)?;
    tx.execute(
        "UPDATE tasks SET due_date=?1, due_at_utc=?2, due_tz=?3, revision=revision+1, updated_at=?4 WHERE id=?5 AND revision=?6",
        params![due.date, due.utc, due.tz, ctx.now, target, task.revision],
    )?;
    tx.execute(
        "INSERT INTO approvals(id, proposal_id, payload_hash, approved_at, actor_device_id, execution_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![planner::new_id(), p.id, p.payload_hash, ctx.now, ctx.device_id, planner::new_id()],
    )?;
    planner::log_change(
        &tx,
        ctx,
        "User",
        "task",
        &target,
        task.revision as i64,
        "task.reschedule.approved",
    )?;
    mark(&tx, ctx, &p.id, "Executed", "proposal.execute")?;
    tx.commit()?;
    load(conn, &req.id)
}
