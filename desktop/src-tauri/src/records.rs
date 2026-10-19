//! Shared helpers for the study-life entities: validated inserts, revision-guarded
//! updates and soft deletes. Column and table names come from code, never from callers;
//! values are always bound parameters. Each write logs an outbox op + audit event.
use crate::commands::IpcError;
use crate::planner::{check_revision, log_change, new_id, Ctx, Res};
use jiff::civil::Date;
use rusqlite::types::Value;
use rusqlite::{params_from_iter, Connection};
use serde_json::{Map, Value as Json};

pub fn val(msg: impl Into<String>) -> IpcError {
    IpcError::new("Validation", msg)
}

pub fn text(s: &str, field: &str, max: usize) -> Res<String> {
    let t = s.trim();
    if t.is_empty() || t.chars().count() > max || t.chars().any(|c| c.is_control()) {
        return Err(val(format!("{field} must be 1-{max} printable characters")));
    }
    Ok(t.to_string())
}

pub fn free_text(s: &str, field: &str, max: usize) -> Res<String> {
    if s.len() > max
        || s.chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t' | '\r'))
    {
        return Err(val(format!("{field} too long or has control characters")));
    }
    Ok(s.to_string())
}

pub fn date(s: &str, field: &str) -> Res<String> {
    s.parse::<Date>()
        .map(|d| d.to_string())
        .map_err(|_| val(format!("{field} must be YYYY-MM-DD")))
}

pub fn month(s: &str) -> Res<String> {
    date(&format!("{s}-01"), "month").map(|_| s.to_string())
}

pub fn currency(s: &str) -> Res<String> {
    if s.len() == 3 && s.bytes().all(|b| b.is_ascii_uppercase()) {
        Ok(s.to_string())
    } else {
        Err(val("currency must be a 3-letter uppercase ISO 4217 code"))
    }
}

pub fn one_of(s: &str, field: &str, allowed: &[&str]) -> Res<String> {
    if allowed.contains(&s) {
        Ok(s.to_string())
    } else {
        Err(val(format!("{field} must be one of {allowed:?}")))
    }
}

pub fn exists(conn: &Connection, table: &str, id: &str) -> Res<()> {
    let n: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM {table} WHERE id=?1 AND deleted_at IS NULL"),
        [id],
        |r| r.get(0),
    )?;
    if n == 1 {
        Ok(())
    } else {
        Err(IpcError::new("NotFound", format!("{table} not found")))
    }
}

pub fn insert(
    tx: &Connection,
    ctx: &Ctx,
    table: &str,
    entity: &str,
    cols: Vec<(&str, Value)>,
) -> Res<String> {
    let id = new_id();
    let mut names = vec!["id", "created_at", "updated_at", "origin_device_id"];
    let mut vals: Vec<Value> = vec![
        id.clone().into(),
        ctx.now.to_string().into(),
        ctx.now.to_string().into(),
        ctx.device_id.to_string().into(),
    ];
    for (n, v) in cols {
        names.push(n);
        vals.push(v);
    }
    let ph: Vec<String> = (1..=names.len()).map(|i| format!("?{i}")).collect();
    tx.execute(
        &format!(
            "INSERT INTO {table}({}) VALUES ({})",
            names.join(","),
            ph.join(",")
        ),
        params_from_iter(vals),
    )?;
    log_change(tx, ctx, "User", entity, &id, 0, &format!("{entity}.create"))?;
    Ok(id)
}

/// Revision-guarded update of the given columns.
pub fn update(
    tx: &Connection,
    ctx: &Ctx,
    table: &str,
    entity: &str,
    id: &str,
    expected: u64,
    cols: Vec<(&str, Value)>,
) -> Res<()> {
    check_revision(tx, table, id, expected)?;
    let mut sets: Vec<String> = vec!["revision=revision+1".into(), "updated_at=?1".into()];
    let mut vals: Vec<Value> = vec![ctx.now.to_string().into()];
    for (n, v) in cols {
        vals.push(v);
        sets.push(format!("{n}=?{}", vals.len()));
    }
    vals.push(id.to_string().into());
    tx.execute(
        &format!(
            "UPDATE {table} SET {} WHERE id=?{}",
            sets.join(","),
            vals.len()
        ),
        params_from_iter(vals),
    )?;
    log_change(
        tx,
        ctx,
        "User",
        entity,
        id,
        expected as i64,
        &format!("{entity}.update"),
    )
}

pub fn soft_delete(
    conn: &mut Connection,
    ctx: &Ctx,
    table: &str,
    entity: &str,
    id: &str,
    expected: u64,
    confirm: bool,
) -> Res<()> {
    if !confirm {
        return Err(IpcError::new(
            "ApprovalRequired",
            "delete requires confirm=true",
        ));
    }
    let tx = conn.transaction()?;
    check_revision(&tx, table, id, expected)?;
    tx.execute(
        &format!(
            "UPDATE {table} SET deleted_at=?1, updated_at=?1, revision=revision+1 WHERE id=?2"
        ),
        rusqlite::params![ctx.now, id],
    )?;
    log_change(
        &tx,
        ctx,
        "User",
        entity,
        id,
        expected as i64,
        &format!("{entity}.delete"),
    )?;
    tx.commit()?;
    Ok(())
}

/// Rows of `table` (excluding soft-deleted) as JSON objects. `filter` is a fixed SQL
/// fragment from code; user values go through `args`.
pub fn list(
    conn: &Connection,
    table: &str,
    filter: &str,
    args: Vec<Value>,
    order: &str,
) -> Res<Vec<Json>> {
    let sql = format!("SELECT * FROM {table} WHERE deleted_at IS NULL {filter} ORDER BY {order}");
    let mut st = conn.prepare(&sql)?;
    let names: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
    let rows = st.query_map(params_from_iter(args), |r| {
        let mut m = Map::new();
        for (i, n) in names.iter().enumerate() {
            let j = match r.get::<_, Value>(i)? {
                Value::Null | Value::Blob(_) => Json::Null,
                Value::Integer(v) => Json::from(v),
                Value::Real(v) => Json::from(v),
                Value::Text(v) => Json::from(v),
            };
            m.insert(n.clone(), j);
        }
        Ok(Json::Object(m))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn get(conn: &Connection, table: &str, id: &str) -> Res<Json> {
    list(conn, table, "AND id=?1", vec![id.to_string().into()], "id")?
        .into_iter()
        .next()
        .ok_or_else(|| IpcError::new("NotFound", format!("{table} not found")))
}
