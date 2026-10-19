//! Owner edits to a routine baseline, effective from a chosen date. No provider access.
use crate::{
    commands::IpcError,
    planner::{self, Ctx, RecurrenceIn, RecurrenceView, Res, Task},
    recurrence,
};
use jiff::{civil::Date, ToSpan};
use rusqlite::{params, Connection};
use serde::Deserialize;
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleEdit {
    pub effective_from: String,
    pub recurrence: RecurrenceIn,
}
fn val(s: &str) -> IpcError {
    IpcError::new("Validation", s)
}
pub type Segment = (RecurrenceView, Option<Date>, Option<Date>);
pub fn segments(conn: &Connection, task: &Task) -> Res<Vec<Segment>> {
    let current = task
        .recurrence
        .as_ref()
        .ok_or_else(|| val("Choose a scheduled routine."))?;
    let mut stmt=conn.prepare("SELECT through_date,recurrence_json FROM routine_schedule_history WHERE task_id=?1 ORDER BY through_date")?;
    let rows = stmt
        .query_map([&task.id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut lower = None;
    let mut out = Vec::new();
    for (end, raw) in rows {
        let end: Date = end.parse().map_err(|_| IpcError::internal())?;
        let rv: RecurrenceView = serde_json::from_str(&raw).map_err(|_| IpcError::internal())?;
        if rv.series_id != current.series_id || lower.is_some_and(|l| end < l) {
            return Err(IpcError::internal());
        }
        out.push((rv, lower, Some(end)));
        lower = Some(end.checked_add(1.day()).map_err(|_| IpcError::internal())?);
    }
    out.push((current.clone(), lower, None));
    Ok(out)
}
pub fn at_date(conn: &Connection, task: &Task, date: Date) -> Res<RecurrenceView> {
    segments(conn, task)?
        .into_iter()
        .find(|(_, lo, hi)| lo.is_none_or(|l| date >= l) && hi.is_none_or(|h| date <= h))
        .map(|s| s.0)
        .ok_or_else(IpcError::internal)
}
pub fn update(conn: &Connection, ctx: &Ctx, task: &Task, edit: &ScheduleEdit) -> Res<()> {
    let old = task
        .recurrence
        .as_ref()
        .ok_or_else(|| val("Only scheduled routines have a repeating schedule."))?;
    let rec = &edit.recurrence;
    let rule = recurrence::parse_rule(&rec.rrule).map_err(|s| val(&s))?;
    let start = recurrence::parse_start(&rec.dtstart_local).map_err(|s| val(&s))?;
    let old_start =
        recurrence::parse_start(&old.dtstart_local).map_err(|_| IpcError::internal())?;
    planner::validate_weekday_times(&rule, start, &rec.weekday_times)?;
    if rec.tz != old.tz
        || start.date != old_start.date
        || start.time.is_some() != old_start.time.is_some()
    {
        return Err(val(
            "Keep this routine’s original start date, time zone and timed/all-day type.",
        ));
    }
    let effective: Date = edit
        .effective_from
        .parse()
        .map_err(|_| val("Choose a valid date to apply the new schedule."))?;
    if edit.effective_from.len() != 10 {
        return Err(val("Choose a valid date to apply the new schedule."));
    }
    let today = ctx
        .now
        .parse::<jiff::Timestamp>()
        .map_err(|_| IpcError::internal())?
        .to_zoned(recurrence::tz(&old.tz).map_err(|s| val(&s))?)
        .date();
    let history = segments(conn, task)?;
    let lower = history.last().and_then(|s| s.1).unwrap_or(old_start.date);
    if effective < today || effective < lower {
        return Err(val(
            "Apply the schedule from today or later, and not before the latest scheduled change.",
        ));
    }
    if history.len() > 128 {
        return Err(val("This routine has too many schedule versions."));
    }
    if effective > lower {
        let through = effective
            .checked_sub(1.day())
            .map_err(|_| IpcError::internal())?;
        conn.execute("INSERT INTO routine_schedule_history(id,task_id,through_date,recurrence_json) VALUES (?1,?2,?3,?4)",params![uuid::Uuid::new_v4().to_string(),task.id,through.to_string(),serde_json::to_string(old).map_err(|_|IpcError::internal())?])?;
    }
    conn.execute("UPDATE recurrence_rules SET rrule=?1,tz=?2,dtstart_local=?3,weekday_times_json=?4,revision=revision+1,updated_at=?5 WHERE series_id=?6",params![rec.rrule,rec.tz,rec.dtstart_local,serde_json::to_string(&rec.weekday_times).map_err(|_|IpcError::internal())?,ctx.now,old.series_id])?;
    Ok(())
}
