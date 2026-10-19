//! Owner-approved, source-bound changes to recurring meetings. No model/provider calls.
use crate::{
    commands::IpcError,
    dates, inbox,
    planner::{self, Ctx, Res},
    recurrence,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A conservative routing guard, not a claim of semantic understanding. Every change still needs review.
pub fn looks_like_change(text: &str) -> bool {
    let s = text.to_lowercase();
    let words: Vec<_> = s.split(|c: char| !c.is_alphanumeric()).collect();
    if [
        "abgabe",
        "einreichen",
        "submit",
        "assignment",
        "deadline",
        "frist",
    ]
    .iter()
    .any(|w| words.contains(w))
    {
        return false;
    }
    let meeting = [
        "vorlesung",
        "lecture",
        "class",
        "seminar",
        "meeting",
        "termin",
        "kelas",
        "jadwal",
        "raum",
        "room",
        "ruangan",
    ]
    .iter()
    .any(|w| words.contains(w));
    let change = [
        "verschob",
        "verlegt",
        "änder",
        "aender",
        "changed",
        "change",
        "instead",
        "statt",
        "pindah",
        "später",
        "spaeter",
        "later",
        "entfällt",
        "cancelled",
        "canceled",
        "khusus",
        "nur diesmal",
    ]
    .iter()
    .any(|w| s.contains(w));
    meeting && change
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyReq {
    pub candidate_id: String,
    pub payload_hash: String,
    pub task_id: String,
    pub expected_revision: u64,
    pub scope: String,
    pub date: String,
    pub new_time: Option<String>,
    pub location: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct Applied {
    pub id: String,
    pub task_id: String,
    pub revision: u64,
    pub replayed: bool,
    pub active: bool,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UndoReq {
    pub id: String,
    pub expected_revision: u64,
}

fn val(s: &str) -> IpcError {
    IpcError::new("Validation", s)
}

pub fn apply(conn: &mut Connection, ctx: &Ctx, req: ApplyReq) -> Res<Applied> {
    let request_hash = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&req).map_err(|_| IpcError::internal())?)
    );
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let previous: Option<(String, String, bool)> = tx
        .query_row(
            "SELECT id,request_hash,active FROM schedule_adjustments WHERE candidate_id=?1",
            [&req.candidate_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((id, hash, active)) = previous {
        if hash != request_hash {
            return Err(IpcError::new(
                "Conflict",
                "This notice already has a different schedule decision.",
            ));
        }
        let task = planner::load_task(&tx, &req.task_id)?;
        return Ok(Applied {
            id,
            task_id: task.id,
            revision: task.revision,
            replayed: true,
            active,
        });
    }
    let c = inbox::verify_schedule_review(&tx, ctx.now, &req.candidate_id, &req.payload_hash)?;
    if c.kind != "update" && !looks_like_change(&c.excerpt) {
        return Err(val("Choose a schedule update, not a normal assignment."));
    }
    let task = planner::load_task(&tx, &req.task_id)?;
    if task.revision != req.expected_revision {
        return Err(IpcError::new(
            "Conflict",
            "The routine changed. Reload its latest schedule.",
        ));
    }
    if task.status == "cancelled" {
        return Err(val("This routine is cancelled."));
    }
    let date: jiff::civil::Date = req
        .date
        .parse()
        .map_err(|_| val("Choose a valid meeting date."))?;
    let rv = crate::schedule_edit::at_date(&tx, &task, date)?;
    if req.date.len() != 10 || !["this", "following"].contains(&req.scope.as_str()) {
        return Err(val(
            "Choose this meeting or this weekday from this date onwards.",
        ));
    }
    let start =
        recurrence::parse_start(&rv.dtstart_local).map_err(|s| IpcError::new("Validation", s))?;
    let rule = recurrence::parse_rule(&rv.rrule).map_err(|s| IpcError::new("Validation", s))?;
    if start.time.is_none()
        || recurrence::expand(&rule, start.date, date, date)
            .map_err(|s| IpcError::new("Validation", s))?
            .is_empty()
    {
        return Err(val(
            "The chosen date is not a timed meeting of this routine.",
        ));
    }
    let time = req.new_time.as_deref().filter(|s| !s.is_empty());
    let location = req
        .location
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if time.is_none() && location.is_none() {
        return Err(val("Choose a new time or room."));
    }
    if let Some(t) = time {
        dates::to_utc(&req.date, t, &rv.tz).map_err(|s| IpcError::new("Validation", s))?;
    }
    if location.is_some_and(|s| s.chars().count() > 120 || s.chars().any(char::is_control)) {
        return Err(val(
            "Room must be 1–120 characters without control characters.",
        ));
    }
    if req.scope == "this" {
        let decided:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM occurrences WHERE series_id=?1 AND substr(original_local,1,10)=?2 AND state<>'Normal' AND deleted_at IS NULL)",params![rv.series_id,req.date],|r|r.get(0))?;
        if decided {
            return Err(IpcError::new("Conflict","This meeting already has a completion, skip or manual move. Undo that decision before applying this change."));
        }
    }
    let received: String = match c.date_anchor.as_ref() {
        Some(bound_anchor) => bound_anchor.clone(),
        None => tx.query_row(
            "SELECT received_at FROM source_messages WHERE id=?1",
            [&c.source_id],
            |r| r.get(0),
        )?,
    };
    let source_order = received
        .parse::<jiff::Timestamp>()
        .map_err(|_| val("The source timestamp is invalid."))?
        .as_second();
    let id = planner::new_id();
    tx.execute("INSERT INTO schedule_adjustments(id,candidate_id,task_id,series_id,request_hash,source_order,from_date,weekday,scope,new_time,location,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![id,c.id,task.id,rv.series_id,request_hash,source_order,req.date,date.weekday().to_monday_zero_offset(),req.scope,time,location,ctx.now])?;
    tx.execute(
        "UPDATE tasks SET revision=revision+1,updated_at=?2 WHERE id=?1",
        params![task.id, ctx.now],
    )?;
    tx.execute(
        "UPDATE source_candidates SET status='rejected',decided_at=?2 WHERE id=?1",
        params![c.id, ctx.now],
    )?;
    // Exact source decision stays in this local table; only identifiers enter the task audit/outbox.
    planner::log_change(
        &tx,
        ctx,
        "User",
        "task",
        &task.id,
        task.revision as i64,
        "schedule.change.apply",
    )?;
    tx.commit()?;
    Ok(Applied {
        id,
        task_id: task.id,
        revision: task.revision + 1,
        replayed: false,
        active: true,
    })
}

pub fn undo(conn: &mut Connection, ctx: &Ctx, req: UndoReq) -> Res<Applied> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (task_id, active): (String, bool) = tx
        .query_row(
            "SELECT task_id,active FROM schedule_adjustments WHERE id=?1",
            [&req.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| IpcError::new("NotFound", "Schedule change not found."))?;
    let task = planner::load_task(&tx, &task_id)?;
    if !active {
        return Ok(Applied {
            id: req.id,
            task_id,
            revision: task.revision,
            replayed: true,
            active: false,
        });
    }
    if task.revision != req.expected_revision {
        return Err(IpcError::new(
            "Conflict",
            "The routine changed after this approval. Reload before changing its schedule again.",
        ));
    }
    tx.execute(
        "UPDATE schedule_adjustments SET active=0 WHERE id=?1",
        [&req.id],
    )?;
    tx.execute(
        "UPDATE tasks SET revision=revision+1,updated_at=?2 WHERE id=?1",
        params![task_id, ctx.now],
    )?;
    planner::log_change(
        &tx,
        ctx,
        "User",
        "task",
        &task_id,
        task.revision as i64,
        "schedule.change.undo",
    )?;
    tx.commit()?;
    Ok(Applied {
        id: req.id,
        task_id,
        revision: task.revision + 1,
        replayed: false,
        active: false,
    })
}

pub fn effective(
    conn: &Connection,
    task_id: &str,
    date: jiff::civil::Date,
    base: &str,
) -> Res<(String, Option<String>)> {
    let mut stmt=conn.prepare("SELECT new_time,location FROM schedule_adjustments WHERE task_id=?1 AND active=1 AND weekday=?2 AND ((scope='this' AND from_date=?3) OR (scope='following' AND from_date<=?3)) ORDER BY CASE scope WHEN 'following' THEN 0 ELSE 1 END,source_order,created_at,rowid")?;
    let changes = stmt.query_map(
        params![
            task_id,
            date.weekday().to_monday_zero_offset(),
            date.to_string()
        ],
        |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
            ))
        },
    )?;
    let mut local = base.to_string();
    let mut location = None;
    for row in changes {
        let (time, room) = row?;
        if let Some(t) = time {
            local = format!("{date}T{t}:00");
        }
        if room.is_some() {
            location = room;
        }
    }
    Ok((local, location))
}
