//! P09 projects/music: projects, milestones/backlog and a crash-safe practice timer.
//! The timer persists every transition; elapsed time is derived from stored timestamps, so a
//! restart while running loses nothing and a second concurrent timer is a Conflict.
use crate::commands::IpcError;
use crate::planner::{Ctx, Res};
use crate::records::{self as rec, val};
use jiff::Timestamp;
use rusqlite::types::Value;
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::Value as Json;

pub fn project_create(conn: &mut Connection, ctx: &Ctx, name: &str) -> Res<Json> {
    let cols = vec![("name", rec::text(name, "name", 500)?.into())];
    let tx = conn.transaction()?;
    let id = rec::insert(&tx, ctx, "projects", "project", cols)?;
    tx.commit()?;
    rec::get(conn, "projects", &id)
}

pub fn project_list(conn: &Connection) -> Res<Vec<Json>> {
    rec::list(conn, "projects", "", vec![], "name")
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MilestoneReq {
    pub project_id: String,
    pub title: String,
    pub state: String,
    pub due_date: Option<String>,
    #[serde(default)]
    pub notes: String,
}

fn ms_cols(r: &MilestoneReq) -> Res<Vec<(&'static str, Value)>> {
    Ok(vec![
        ("project_id", r.project_id.clone().into()),
        ("title", rec::text(&r.title, "title", 500)?.into()),
        (
            "state",
            rec::one_of(&r.state, "state", &["backlog", "active", "done"])?.into(),
        ),
        (
            "due_date",
            match &r.due_date {
                Some(d) => Value::from(rec::date(d, "due_date")?),
                None => Value::Null,
            },
        ),
        ("notes", rec::free_text(&r.notes, "notes", 65_536)?.into()),
    ])
}

pub fn milestone_create(conn: &mut Connection, ctx: &Ctx, r: MilestoneReq) -> Res<Json> {
    rec::exists(conn, "projects", &r.project_id)?;
    let cols = ms_cols(&r)?;
    let tx = conn.transaction()?;
    let id = rec::insert(&tx, ctx, "milestones", "milestone", cols)?;
    tx.commit()?;
    rec::get(conn, "milestones", &id)
}

pub fn milestone_update(
    conn: &mut Connection,
    ctx: &Ctx,
    id: &str,
    exp: u64,
    r: MilestoneReq,
) -> Res<Json> {
    rec::exists(conn, "projects", &r.project_id)?;
    let cols = ms_cols(&r)?;
    let tx = conn.transaction()?;
    rec::update(&tx, ctx, "milestones", "milestone", id, exp, cols)?;
    tx.commit()?;
    rec::get(conn, "milestones", id)
}

pub fn milestone_list(conn: &Connection, project_id: &str) -> Res<Vec<Json>> {
    rec::list(
        conn,
        "milestones",
        "AND project_id=?1",
        vec![project_id.to_string().into()],
        "state, due_date IS NULL, due_date, created_at",
    )
}

pub fn milestone_delete(
    conn: &mut Connection,
    ctx: &Ctx,
    id: &str,
    exp: u64,
    confirm: bool,
) -> Res<()> {
    rec::soft_delete(conn, ctx, "milestones", "milestone", id, exp, confirm)
}

// ------------------------------------------------------------------ practice timer

fn secs_between(from: &str, to: &str) -> Res<i64> {
    let a: Timestamp = from
        .parse()
        .map_err(|_| val("stored timestamp is invalid"))?;
    let b: Timestamp = to.parse().map_err(|_| val("clock value is invalid"))?;
    Ok((b.as_second() - a.as_second()).max(0)) // a backwards clock never subtracts time
}

/// Row plus derived `elapsed_s` (accumulated + current running segment).
fn view(conn: &Connection, id: &str, now: &str) -> Res<Json> {
    let mut row = rec::get(conn, "practice_sessions", id)?;
    let mut e = row["accumulated_s"].as_i64().unwrap_or(0);
    if let Some(r) = row["resumed_at"].as_str() {
        e += secs_between(r, now)?;
    }
    row["elapsed_s"] = Json::from(e);
    Ok(row)
}

fn active_id(conn: &Connection) -> Res<Option<String>> {
    Ok(rec::list(
        conn,
        "practice_sessions",
        "AND state IN ('running','paused')",
        vec![],
        "created_at",
    )?
    .first()
    .and_then(|r| r["id"].as_str().map(String::from)))
}

pub fn practice_start(conn: &mut Connection, ctx: &Ctx, kind: &str, title: &str) -> Res<Json> {
    let cols = vec![
        (
            "kind",
            rec::one_of(kind, "kind", &["music", "coding", "htb"])?.into(),
        ),
        ("title", rec::text(title, "title", 200)?.into()),
        ("state", "running".to_string().into()),
        ("resumed_at", ctx.now.to_string().into()),
    ];
    let tx = conn.transaction()?;
    if active_id(&tx)?.is_some() {
        return Err(IpcError::new(
            "Conflict",
            "another practice timer is already active",
        ));
    }
    let id = rec::insert(&tx, ctx, "practice_sessions", "practice_session", cols)?;
    tx.commit()?;
    view(conn, &id, ctx.now)
}

fn transition(
    conn: &mut Connection,
    ctx: &Ctx,
    id: &str,
    exp: u64,
    from: &[&str],
    to: &str,
) -> Res<Json> {
    let tx = conn.transaction()?;
    let cur = rec::get(&tx, "practice_sessions", id)?;
    let state = cur["state"].as_str().unwrap_or("");
    if !from.contains(&state) {
        return Err(val(format!("cannot move a {state} timer to {to}")));
    }
    let mut acc = cur["accumulated_s"].as_i64().unwrap_or(0);
    if let Some(r) = cur["resumed_at"].as_str() {
        acc += secs_between(r, ctx.now)?;
    }
    let mut cols: Vec<(&str, Value)> = vec![
        ("state", to.to_string().into()),
        ("accumulated_s", acc.into()),
        (
            "resumed_at",
            if to == "running" {
                ctx.now.to_string().into()
            } else {
                Value::Null
            },
        ),
    ];
    if to == "stopped" {
        cols.push(("stopped_at", ctx.now.to_string().into()));
    }
    rec::update(
        &tx,
        ctx,
        "practice_sessions",
        "practice_session",
        id,
        exp,
        cols,
    )?;
    tx.commit()?;
    view(conn, id, ctx.now)
}

pub fn practice_pause(conn: &mut Connection, ctx: &Ctx, id: &str, exp: u64) -> Res<Json> {
    transition(conn, ctx, id, exp, &["running"], "paused")
}

pub fn practice_resume(conn: &mut Connection, ctx: &Ctx, id: &str, exp: u64) -> Res<Json> {
    transition(conn, ctx, id, exp, &["paused"], "running")
}

pub fn practice_stop(conn: &mut Connection, ctx: &Ctx, id: &str, exp: u64) -> Res<Json> {
    transition(conn, ctx, id, exp, &["running", "paused"], "stopped")
}

/// The live timer after an app restart, with elapsed time recomputed from storage.
pub fn practice_active(conn: &Connection, now: &str) -> Res<Option<Json>> {
    active_id(conn)?.map(|id| view(conn, &id, now)).transpose()
}

pub fn practice_list(conn: &Connection) -> Res<Vec<Json>> {
    rec::list(conn, "practice_sessions", "", vec![], "created_at DESC")
}
