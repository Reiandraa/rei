//! P01/P04 planner tests against a real SQLite database (fixtures are synthetic).
use rei_os_lib::commands::IpcError;
use rei_os_lib::planner::{self as pl, Ctx, DueIn};
use rei_os_lib::{db, proposals as pr};
use rusqlite::Connection;
use serde_json::json;

const NOW: &str = "2026-10-19T08:00:00Z";

fn ctx(now: &str) -> Ctx<'_> {
    Ctx {
        device_id: "test-device",
        now,
    }
}

fn req<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> T {
    serde_json::from_value(v).unwrap()
}

fn mk(conn: &mut Connection, v: serde_json::Value) -> pl::Task {
    pl::task_create(conn, &ctx(NOW), req(v)).unwrap()
}

fn get(conn: &Connection, id: &str) -> pl::Task {
    pl::task_get(conn, req(json!({ "id": id }))).unwrap()
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

fn range(from: &str, to: &str) -> pl::RangeReq {
    req(json!({"from": from, "to": to}))
}

fn agenda(conn: &Connection, from: &str, to: &str) -> Vec<pl::AgendaItem> {
    pl::agenda_list(conn, range(from, to)).unwrap()
}

#[test]
fn tasks_persist_across_restart_offline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rei.sqlite");
    let id = {
        let mut conn = db::open(&path).unwrap();
        let t = mk(
            &mut conn,
            json!({"title": "  Write report ", "priority": 2, "due": {"kind": "date", "date": "2026-10-20"}}),
        );
        assert_eq!(
            (t.title.as_str(), t.revision, t.status.as_str()),
            ("Write report", 1, "todo")
        );
        t.id
    };
    let conn = db::open(&path).unwrap();
    let t = get(&conn, &id);
    assert_eq!((t.priority, t.due_date.as_deref()), (2, Some("2026-10-20")));
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM outbox WHERE entity_type='task'"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM audit_events WHERE action='task.create'"
        ),
        1
    );
}

#[test]
fn validation_rejects_bad_input_and_writes_nothing() {
    let mut conn = db::open_in_memory().unwrap();
    for bad in [
        json!({"title": "   "}),
        json!({"title": "x", "priority": 4}),
        json!({"title": "x", "duration_min": 0}),
        json!({"title": "x", "energy": "wild"}),
        json!({"title": "x", "due": {"kind": "date", "date": "2026-02-30"}}),
        json!({"title": "x", "due": {"kind": "at", "local": "2026-10-20T09:00:00", "tz": "Mars/Base"}}),
        json!({"title": "x", "recurrence": {"rrule": "FREQ=YEARLY", "tz": "Europe/Berlin", "dtstart_local": "2026-10-19T09:00:00"}}),
        json!({"title": "x", "due": {"kind": "date", "date": "2026-10-20"},
               "recurrence": {"rrule": "FREQ=DAILY", "tz": "Europe/Berlin", "dtstart_local": "2026-10-19T09:00:00"}}),
    ] {
        assert_eq!(
            pl::task_create(&mut conn, &ctx(NOW), req(bad.clone()))
                .unwrap_err()
                .code,
            "Validation",
            "{bad}"
        );
    }
    assert_eq!(count(&conn, "SELECT count(*) FROM tasks"), 0);
    assert_eq!(count(&conn, "SELECT count(*) FROM recurrence_rules"), 0);
    assert!(
        serde_json::from_value::<pl::TaskCreateReq>(json!({"title": "x", "surprise": 1})).is_err()
    );
}

#[test]
fn update_conflicts_on_stale_revision_and_delete_needs_confirmation() {
    let mut conn = db::open_in_memory().unwrap();
    let t = mk(&mut conn, json!({"title": "A"}));
    let t2 = pl::task_update(
        &mut conn,
        &ctx(NOW),
        req(json!({"id": t.id, "expected_revision": 1, "title": "B", "status": "doing"})),
    )
    .unwrap();
    assert_eq!(
        (t2.revision, t2.title.as_str(), t2.status.as_str()),
        (2, "B", "doing")
    );
    let e = pl::task_update(
        &mut conn,
        &ctx(NOW),
        req(json!({"id": t.id, "expected_revision": 1, "title": "C"})),
    )
    .unwrap_err();
    assert_eq!((e.code.as_str(), e.current_revision), ("Conflict", Some(2)));
    assert_eq!(get(&conn, &t.id).title, "B");

    let e = pl::task_delete(
        &mut conn,
        &ctx(NOW),
        req(json!({"id": t.id, "expected_revision": 2, "confirm": false})),
    )
    .unwrap_err();
    assert_eq!(e.code, "ApprovalRequired");
    pl::task_delete(
        &mut conn,
        &ctx(NOW),
        req(json!({"id": t.id, "expected_revision": 2, "confirm": true})),
    )
    .unwrap();
    assert_eq!(
        pl::task_get(&conn, req(json!({"id": t.id})))
            .unwrap_err()
            .code,
        "NotFound"
    );
    // Soft delete: the row and its history survive for sync.
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM tasks WHERE deleted_at IS NOT NULL"
        ),
        1
    );
}

#[test]
fn complete_and_list_filter() {
    let mut conn = db::open_in_memory().unwrap();
    let a = mk(&mut conn, json!({"title": "A"}));
    mk(&mut conn, json!({"title": "B"}));
    let done = pl::task_complete(
        &mut conn,
        &ctx(NOW),
        req(json!({"id": a.id, "expected_revision": 1})),
    )
    .unwrap();
    assert_eq!((done.status.as_str(), done.revision), ("done", 2));
    let open = pl::task_list(&conn, req(json!({"status": "todo"}))).unwrap();
    assert_eq!(
        open.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(),
        ["B"]
    );
    assert_eq!(pl::task_list(&conn, req(json!({}))).unwrap().len(), 2);
    assert!(pl::task_list(&conn, req(json!({"status": "bogus"}))).is_err());
}

fn weekly(conn: &mut Connection) -> pl::Task {
    mk(
        conn,
        json!({"title": "Lecture", "duration_min": 90, "recurrence": {
        "rrule": "FREQ=WEEKLY;BYDAY=MO", "tz": "Europe/Berlin", "dtstart_local": "2026-10-19T09:00:00"}}),
    )
}

fn edit(conn: &mut Connection, t: &pl::Task, v: serde_json::Value) -> Result<pl::Task, IpcError> {
    let mut body = json!({"task_id": t.id, "expected_revision": t.revision});
    body.as_object_mut()
        .unwrap()
        .extend(v.as_object().unwrap().clone());
    pl::occurrence_edit(conn, &ctx(NOW), req(body))
}

#[test]
fn recurrence_exceptions_this_following_series() {
    let mut conn = db::open_in_memory().unwrap();
    let t = weekly(&mut conn);
    let starts = |conn: &Connection| -> Vec<(String, String)> {
        agenda(conn, "2026-10-19", "2026-11-16")
            .iter()
            .map(|i| (i.local_start.clone(), i.state.clone()))
            .collect()
    };
    assert_eq!(starts(&conn).len(), 5);

    // this: skip 10-26, done 10-19, move 11-02 to Tuesday 14:00.
    let t = edit(
        &mut conn,
        &t,
        json!({"scope": "this", "action": "skip", "original_local": "2026-10-26T09:00:00"}),
    )
    .unwrap();
    let t = edit(
        &mut conn,
        &t,
        json!({"scope": "this", "action": "done", "original_local": "2026-10-19T09:00:00"}),
    )
    .unwrap();
    let t = edit(&mut conn, &t, json!({"scope": "this", "action": "move", "original_local": "2026-11-02T09:00:00", "new_local": "2026-11-03T14:00:00"})).unwrap();
    assert_eq!(t.revision, 4);
    let got = starts(&conn);
    assert_eq!(got[0], ("2026-10-19T09:00:00".into(), "Done".into()));
    assert_eq!(got[1], ("2026-10-26T09:00:00".into(), "Skipped".into()));
    assert!(got.contains(&("2026-11-03T14:00:00".into(), "Moved".into())));
    assert!(
        !got.iter().any(|g| g.0 == "2026-11-02T09:00:00"),
        "moved occurrence must leave its original slot"
    );
    // reset restores the original.
    let t = edit(
        &mut conn,
        &t,
        json!({"scope": "this", "action": "reset", "original_local": "2026-10-26T09:00:00"}),
    )
    .unwrap();
    assert!(starts(&conn).contains(&("2026-10-26T09:00:00".into(), "Normal".into())));
    assert_eq!(count(&conn, "SELECT count(*) FROM occurrences"), 3);

    // non-occurrence and stale revision are refused.
    assert_eq!(
        edit(
            &mut conn,
            &t,
            json!({"scope": "this", "action": "skip", "original_local": "2026-10-20T09:00:00"})
        )
        .unwrap_err()
        .code,
        "Validation"
    );
    let mut stale = t.clone();
    stale.revision -= 1;
    assert_eq!(
        edit(
            &mut conn,
            &stale,
            json!({"scope": "this", "action": "skip", "original_local": "2026-10-26T09:00:00"})
        )
        .unwrap_err()
        .code,
        "Conflict"
    );

    // following: end the series before 11-09.
    let t = edit(
        &mut conn,
        &t,
        json!({"scope": "following", "action": "skip", "original_local": "2026-11-09T09:00:00"}),
    )
    .unwrap();
    assert!(t
        .recurrence
        .as_ref()
        .unwrap()
        .rrule
        .ends_with("UNTIL=20261108"));
    assert!(!starts(&conn)
        .iter()
        .any(|g| g.0.starts_with("2026-11-09") || g.0.starts_with("2026-11-16")));

    // series: retime to 10:30 keeps overrides attached.
    let t = edit(&mut conn, &t, json!({"scope": "series", "action": "move", "original_local": "2026-10-19T09:00:00", "new_local": "2026-10-19T10:30:00"})).unwrap();
    assert_eq!(
        t.recurrence.as_ref().unwrap().dtstart_local,
        "2026-10-19T10:30:00"
    );
    assert_eq!(
        starts(&conn)[0],
        ("2026-10-19T10:30:00".into(), "Done".into())
    );
    assert_eq!(
        edit(
            &mut conn,
            &t,
            json!({"scope": "series", "action": "done", "original_local": "2026-10-19T10:30:00"})
        )
        .unwrap_err()
        .code,
        "Validation"
    );
}

#[test]
fn recurring_task_cannot_be_completed_wholesale_and_whole_series_can_be_cancelled() {
    let mut conn = db::open_in_memory().unwrap();
    let t = weekly(&mut conn);
    let e = pl::task_complete(
        &mut conn,
        &ctx(NOW),
        req(json!({"id": t.id, "expected_revision": 1})),
    )
    .unwrap_err();
    assert_eq!(e.code, "Validation");
    let t = edit(
        &mut conn,
        &t,
        json!({"scope": "following", "action": "skip", "original_local": "2026-10-19T09:00:00"}),
    )
    .unwrap();
    assert_eq!(t.status, "cancelled");
    assert!(agenda(&conn, "2026-10-19", "2026-12-31").is_empty());
}

#[test]
fn dst_days_keep_wall_clock_and_all_day_stays_a_date() {
    let mut conn = db::open_in_memory().unwrap();
    mk(
        &mut conn,
        json!({"title": "Daily 02:30", "recurrence": {"rrule": "FREQ=DAILY", "tz": "Europe/Berlin", "dtstart_local": "2026-03-28T02:30:00"}}),
    );
    mk(
        &mut conn,
        json!({"title": "Holiday", "recurrence": {"rrule": "FREQ=DAILY;COUNT=2", "tz": "Europe/Berlin", "dtstart_local": "2026-10-25"}}),
    );
    mk(
        &mut conn,
        json!({"title": "Dentist", "due": {"kind": "at", "local": "2026-10-25T02:30:00", "tz": "Europe/Berlin"}}),
    );
    let spring = agenda(&conn, "2026-03-28", "2026-03-30");
    assert_eq!(
        spring
            .iter()
            .map(|i| i.local_start.as_str())
            .collect::<Vec<_>>(),
        [
            "2026-03-28T02:30:00",
            "2026-03-29T02:30:00",
            "2026-03-30T02:30:00"
        ]
    );
    let fall = agenda(&conn, "2026-10-25", "2026-10-26");
    let holiday: Vec<_> = fall.iter().filter(|i| i.title == "Holiday").collect();
    assert_eq!(holiday.len(), 2);
    assert!(holiday
        .iter()
        .all(|i| i.all_day && i.local_start.len() == 10));
    // Fold: stored instant is the earlier (CEST) one and round-trips to the same wall time.
    let d = pl::task_list(&conn, req(json!({})))
        .unwrap()
        .into_iter()
        .find(|t| t.title == "Dentist")
        .unwrap();
    assert_eq!(
        (d.due_at_utc.as_deref(), d.due_at_local.as_deref()),
        (Some("2026-10-25T00:30:00Z"), Some("2026-10-25T02:30:00"))
    );
}

#[test]
fn events_crud_local_time_roundtrip_and_conflict_flags() {
    let mut conn = db::open_in_memory().unwrap();
    let ev = pl::event_create(
        &mut conn,
        &ctx(NOW),
        req(json!({"title": "Seminar", "all_day": false, "start_local": "2026-10-20T10:00:00", "end_local": "2026-10-20T11:30:00", "tz": "Europe/Berlin"})),
    )
    .unwrap();
    assert_eq!(
        (ev.start_utc.as_deref(), ev.start_local.as_str()),
        (Some("2026-10-20T08:00:00Z"), "2026-10-20T10:00:00")
    );
    // Recurring-task occurrence overlapping the seminar, plus a non-overlapping one.
    let study = mk(
        &mut conn,
        json!({"title": "Study", "duration_min": 60, "recurrence": {"rrule": "FREQ=DAILY;COUNT=2", "tz": "Europe/Berlin", "dtstart_local": "2026-10-20T11:00:00"}}),
    );
    let items = agenda(&conn, "2026-10-20", "2026-10-21");
    let flag = |title: &str, day: &str| {
        items
            .iter()
            .find(|i| i.title == title && i.local_start.starts_with(day))
            .unwrap()
            .conflict
    };
    assert!(flag("Seminar", "2026-10-20") && flag("Study", "2026-10-20"));
    assert!(!flag("Study", "2026-10-21"));
    // Skipping the occurrence clears the conflict.
    edit(
        &mut conn,
        &study,
        json!({"scope": "this", "action": "skip", "original_local": "2026-10-20T11:00:00"}),
    )
    .unwrap();
    assert!(!agenda(&conn, "2026-10-20", "2026-10-20")
        .iter()
        .any(|i| i.conflict));

    let up = pl::event_update(&mut conn, &ctx(NOW), req(json!({"id": ev.id, "expected_revision": 1, "title": "Seminar II", "end_local": "2026-10-20T12:00:00"}))).unwrap();
    assert_eq!(
        (up.revision, up.end_utc.as_deref()),
        (2, Some("2026-10-20T10:00:00Z"))
    );
    assert_eq!(
        pl::event_update(
            &mut conn,
            &ctx(NOW),
            req(json!({"id": ev.id, "expected_revision": 1, "title": "x"}))
        )
        .unwrap_err()
        .code,
        "Conflict"
    );
    assert_eq!(
        pl::event_update(
            &mut conn,
            &ctx(NOW),
            req(json!({"id": ev.id, "expected_revision": 2, "end_local": "2026-10-20T09:00:00"}))
        )
        .unwrap_err()
        .code,
        "Validation"
    );

    let all_day = pl::event_create(&mut conn, &ctx(NOW), req(json!({"title": "Trip", "all_day": true, "start_local": "2026-10-22", "end_local": "2026-10-23", "tz": "Europe/Berlin"}))).unwrap();
    assert!(all_day.start_utc.is_none());
    assert_eq!(
        pl::event_list(&conn, range("2026-10-23", "2026-10-23"))
            .unwrap()
            .len(),
        1
    );
    assert!(pl::event_list(&conn, range("2026-10-24", "2026-10-25"))
        .unwrap()
        .is_empty());
    assert!(pl::event_list(&conn, range("2026-10-25", "2026-10-20")).is_err());

    pl::event_delete(
        &mut conn,
        &ctx(NOW),
        req(json!({"id": ev.id, "expected_revision": 2, "confirm": true})),
    )
    .unwrap();
    assert_eq!(
        pl::event_list(&conn, range("2026-10-20", "2026-10-20"))
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn source_owned_events_are_read_only_without_write_consent() {
    let mut conn = db::open_in_memory().unwrap();
    conn.execute(
        "INSERT INTO events(id, created_at, updated_at, origin_device_id, title, provider, account_id, provider_object_id, start_value, end_value, tz, read_only)
         VALUES ('src1', ?1, ?1, 'd', 'Imported (SYNTHETIC)', 'GraphCalendar', 'acct', 'obj', '2026-10-20T08:00:00Z', '2026-10-20T09:00:00Z', 'Europe/Berlin', 1)",
        [NOW],
    )
    .unwrap();
    let e = pl::event_update(
        &mut conn,
        &ctx(NOW),
        req(json!({"id": "src1", "expected_revision": 1, "title": "mine"})),
    )
    .unwrap_err();
    assert_eq!(e.code, "Forbidden");
    let e = pl::event_delete(
        &mut conn,
        &ctx(NOW),
        req(json!({"id": "src1", "expected_revision": 1, "confirm": true})),
    )
    .unwrap_err();
    assert_eq!(e.code, "Forbidden");
    assert!(agenda(&conn, "2026-10-20", "2026-10-20")[0].read_only);
    // Source identity is unique.
    let dup = conn.execute(
        "INSERT INTO events(id, created_at, updated_at, origin_device_id, title, provider, account_id, provider_object_id, start_value, end_value, tz)
         VALUES ('src2', ?1, ?1, 'd', 'dup', 'GraphCalendar', 'acct', 'obj', 'a', 'b', 'UTC')",
        [NOW],
    );
    assert!(dup.is_err());
}

fn overdue(conn: &mut Connection) -> pl::Task {
    mk(
        conn,
        json!({"title": "Hand in form", "due": {"kind": "date", "date": "2026-10-15"}}),
    )
}

fn suggest(conn: &mut Connection, now: &str, today: &str) -> Vec<pr::Proposal> {
    pr::suggest_reschedules(conn, &ctx(now), req(json!({ "today": today }))).unwrap()
}

fn approve(
    conn: &mut Connection,
    now: &str,
    p: &pr::Proposal,
    hash: &str,
) -> Result<pr::Proposal, IpcError> {
    pr::approve(
        conn,
        &ctx(now),
        req(json!({"id": p.id, "payload_hash": hash})),
    )
}

#[test]
fn suggest_approve_executes_once_atomically_and_replays_idempotently() {
    let mut conn = db::open_in_memory().unwrap();
    let t = overdue(&mut conn);
    let made = suggest(&mut conn, NOW, "2026-10-19");
    assert_eq!(made.len(), 1);
    assert!(
        suggest(&mut conn, NOW, "2026-10-19").is_empty(),
        "no duplicates"
    );
    let p = &made[0];
    assert_eq!(
        (p.status.as_str(), p.expected_target_revision),
        ("Pending", Some(1))
    );
    assert_eq!(
        get(&conn, &t.id).due_date.as_deref(),
        Some("2026-10-15"),
        "proposal alone changes nothing"
    );

    let before = (
        count(&conn, "SELECT count(*) FROM outbox"),
        count(&conn, "SELECT count(*) FROM audit_events"),
    );
    let done = approve(&mut conn, NOW, p, &p.payload_hash).unwrap();
    assert_eq!(done.status, "Executed");
    let after = get(&conn, &t.id);
    assert_eq!(
        (after.due_date.as_deref(), after.revision),
        (Some("2026-10-19"), 2)
    );
    assert_eq!(count(&conn, "SELECT count(*) FROM approvals"), 1);
    // One outbox op + audit event for the task change and one for the proposal status.
    assert_eq!(count(&conn, "SELECT count(*) FROM outbox"), before.0 + 2);
    assert_eq!(
        count(&conn, "SELECT count(*) FROM audit_events"),
        before.1 + 2
    );

    // Replay with the same hash returns the original result without a second write; a different hash is refused.
    let again = approve(&mut conn, NOW, p, &p.payload_hash).unwrap();
    assert_eq!(again, done);
    assert_eq!(count(&conn, "SELECT count(*) FROM approvals"), 1);
    assert_eq!(get(&conn, &t.id).revision, 2);
    assert_eq!(
        approve(&mut conn, NOW, p, "00").unwrap_err().code,
        "ApprovalInvalid"
    );
}

#[test]
fn wrong_hash_and_tampered_payload_never_execute() {
    let mut conn = db::open_in_memory().unwrap();
    let t = overdue(&mut conn);
    let p = suggest(&mut conn, NOW, "2026-10-19").remove(0);
    assert_eq!(
        approve(&mut conn, NOW, &p, "deadbeef").unwrap_err().code,
        "ApprovalInvalid"
    );
    // Payload edited in the database after hashing (as injected content might attempt).
    conn.execute(
        "UPDATE proposals SET payload_json = ?1 WHERE id = ?2",
        rusqlite::params![r#"{"due":{"date":"2030-01-01","kind":"date"}}"#, p.id],
    )
    .unwrap();
    assert_eq!(
        approve(&mut conn, NOW, &p, &p.payload_hash)
            .unwrap_err()
            .code,
        "ApprovalInvalid"
    );
    assert_eq!(get(&conn, &t.id).due_date.as_deref(), Some("2026-10-15"));
    assert_eq!(count(&conn, "SELECT count(*) FROM approvals"), 0);
}

#[test]
fn stale_approval_after_user_edit_is_rejected_and_expires_proposal() {
    let mut conn = db::open_in_memory().unwrap();
    let t = overdue(&mut conn);
    let p = suggest(&mut conn, NOW, "2026-10-19").remove(0);
    // The user edits the task first (e.g. drags it elsewhere) so the target revision moves on.
    pl::task_update(&mut conn, &ctx(NOW), req(json!({"id": t.id, "expected_revision": 1, "due": {"kind": "date", "date": "2026-10-30"}}))).unwrap();
    assert_eq!(
        approve(&mut conn, NOW, &p, &p.payload_hash)
            .unwrap_err()
            .code,
        "ApprovalInvalid"
    );
    let after = get(&conn, &t.id);
    assert_eq!(
        (after.due_date.as_deref(), after.revision),
        (Some("2026-10-30"), 2),
        "user's edit preserved"
    );
    assert_eq!(
        pr::list(&conn, &ctx(NOW), req(json!({"status": "Expired"})))
            .unwrap()
            .len(),
        1,
        "stale proposal durably expired"
    );
    assert_eq!(count(&conn, "SELECT count(*) FROM approvals"), 0);
}

#[test]
fn expired_proposals_cannot_be_approved() {
    let mut conn = db::open_in_memory().unwrap();
    let t = overdue(&mut conn);
    let p = suggest(&mut conn, NOW, "2026-10-19").remove(0);
    assert_eq!(p.expires_at, "2026-10-20T08:00:00Z");
    assert_eq!(
        approve(&mut conn, "2026-10-20T08:00:00Z", &p, &p.payload_hash)
            .unwrap_err()
            .code,
        "ApprovalInvalid"
    );
    assert_eq!(get(&conn, &t.id).due_date.as_deref(), Some("2026-10-15"));
    assert_eq!(
        pr::list(
            &conn,
            &ctx("2026-10-20T09:00:00Z"),
            req(json!({"status": "Expired"}))
        )
        .unwrap()
        .len(),
        1
    );
    // A fresh suggestion can be produced afterwards.
    assert_eq!(
        suggest(&mut conn, "2026-10-20T09:00:00Z", "2026-10-20").len(),
        1
    );
}

#[test]
fn reject_preserves_target_and_cannot_be_approved_later() {
    let mut conn = db::open_in_memory().unwrap();
    let t = overdue(&mut conn);
    let p = suggest(&mut conn, NOW, "2026-10-19").remove(0);
    let r = pr::reject(&mut conn, &ctx(NOW), req(json!({"id": p.id}))).unwrap();
    assert_eq!(r.status, "Rejected");
    let after = get(&conn, &t.id);
    assert_eq!(
        (after.due_date.as_deref(), after.revision),
        (Some("2026-10-15"), 1)
    );
    assert_eq!(
        approve(&mut conn, NOW, &p, &p.payload_hash)
            .unwrap_err()
            .code,
        "ApprovalInvalid"
    );
    assert_eq!(
        pr::reject(&mut conn, &ctx(NOW), req(json!({"id": p.id})))
            .unwrap_err()
            .code,
        "ApprovalInvalid"
    );
}

#[test]
fn only_overdue_open_tasks_are_suggested_and_timed_proposals_work() {
    let mut conn = db::open_in_memory().unwrap();
    mk(
        &mut conn,
        json!({"title": "Future", "due": {"kind": "date", "date": "2026-10-25"}}),
    );
    let old = mk(
        &mut conn,
        json!({"title": "Done old", "due": {"kind": "date", "date": "2026-10-01"}}),
    );
    pl::task_complete(
        &mut conn,
        &ctx(NOW),
        req(json!({"id": old.id, "expected_revision": 1})),
    )
    .unwrap();
    assert!(suggest(&mut conn, NOW, "2026-10-19").is_empty());
    // Explicit timed proposal via the producer API.
    let t = mk(&mut conn, json!({"title": "Call"}));
    let due = DueIn::At {
        local: "2026-10-20T15:00:00".into(),
        tz: "Europe/Berlin".into(),
    };
    let p = pr::create_reschedule(&mut conn, &ctx(NOW), &t.id, due, "test", 0.5).unwrap();
    approve(&mut conn, NOW, &p, &p.payload_hash).unwrap();
    let t = get(&conn, &t.id);
    assert_eq!(
        (t.due_at_utc.as_deref(), t.due_date),
        (Some("2026-10-20T13:00:00Z"), None)
    );
    // Invalid payloads cannot even become proposals.
    assert!(pr::create_reschedule(
        &mut conn,
        &ctx(NOW),
        &t.id,
        DueIn::Date {
            date: "nope".into()
        },
        "x",
        0.1
    )
    .is_err());
}

fn weekly_fixture() -> serde_json::Value {
    json!({"title":"Synthetic study routine","duration_min":60,"recurrence":{
        "rrule":"FREQ=WEEKLY;BYDAY=MO,TH","tz":"Europe/Berlin",
        "dtstart_local":"2026-10-19T09:00:00","weekday_times":{"MO":"09:00","TH":"14:00"}}})
}

#[test]
fn weekday_times_persist_and_expand_across_dst_without_duplicate_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("weekly.sqlite");
    let id = {
        let mut conn = db::open(&path).unwrap();
        mk(&mut conn, weekly_fixture()).id
    };
    let conn = db::open(&path).unwrap();
    let task = get(&conn, &id);
    assert_eq!(
        task.recurrence
            .as_ref()
            .unwrap()
            .weekday_times
            .get("TH")
            .unwrap(),
        "14:00"
    );
    let items = agenda(&conn, "2026-10-19", "2026-10-31");
    let local: Vec<_> = items.iter().map(|i| i.local_start.as_str()).collect();
    assert_eq!(
        local,
        [
            "2026-10-19T09:00:00",
            "2026-10-22T14:00:00",
            "2026-10-26T09:00:00",
            "2026-10-29T14:00:00"
        ]
    );
    assert_eq!(
        rei_os_lib::recurrence::local_to_utc(local[0], "Europe/Berlin").unwrap(),
        "2026-10-19T07:00:00Z"
    );
    assert_eq!(
        rei_os_lib::recurrence::local_to_utc(local[2], "Europe/Berlin").unwrap(),
        "2026-10-26T08:00:00Z"
    );
    assert_eq!(count(&conn, "SELECT count(*) FROM tasks"), 1);
}

#[test]
fn weekday_times_reject_incomplete_extra_invalid_or_nonweekly_values_atomically() {
    let mut conn = db::open_in_memory().unwrap();
    for times in [
        json!({"MO":"09:00"}),
        json!({"MO":"09:00","FR":"14:00"}),
        json!({"MO":"09:00","TH":"14:00","FR":"10:00"}),
        json!({"MO":"25:00","TH":"14:00"}),
        json!({"MO":"9:00","TH":"14:00"}),
        json!({"MO":"09:00:30","TH":"14:00"}),
    ] {
        let mut value = weekly_fixture();
        value["recurrence"]["weekday_times"] = times;
        assert!(pl::task_create(&mut conn, &ctx(NOW), req(value)).is_err());
    }
    for (rule, start) in [
        ("FREQ=DAILY", "2026-10-19T09:00:00"),
        ("FREQ=WEEKLY", "2026-10-19T09:00:00"),
        ("FREQ=WEEKLY;BYDAY=MO,TH", "2026-10-19"),
    ] {
        let mut value = weekly_fixture();
        value["recurrence"]["rrule"] = json!(rule);
        value["recurrence"]["dtstart_local"] = json!(start);
        assert!(pl::task_create(&mut conn, &ctx(NOW), req(value)).is_err());
    }
    assert_eq!(count(&conn, "SELECT count(*) FROM tasks"), 0);
    assert_eq!(count(&conn, "SELECT count(*) FROM recurrence_rules"), 0);
    assert_eq!(count(&conn, "SELECT count(*) FROM outbox"), 0);
}

#[test]
fn weekday_occurrence_actions_use_its_own_time_and_retime_preserves_decisions() {
    let mut conn = db::open_in_memory().unwrap();
    let task = mk(&mut conn, weekly_fixture());
    let edit = |rev, scope: &str, action: &str, original: &str, new: Option<&str>| {
        req(json!({
        "task_id":task.id,"expected_revision":rev,"scope":scope,"action":action,
        "original_local":original,"new_local":new}))
    };
    assert!(pl::occurrence_edit(
        &mut conn,
        &ctx(NOW),
        edit(1, "this", "done", "2026-10-22T09:00:00", None)
    )
    .is_err());
    assert_eq!(get(&conn, &task.id).revision, 1);
    let t = pl::occurrence_edit(
        &mut conn,
        &ctx(NOW),
        edit(1, "this", "done", "2026-10-22T14:00:00", None),
    )
    .unwrap();
    let t = pl::occurrence_edit(
        &mut conn,
        &ctx(NOW),
        edit(
            t.revision,
            "this",
            "move",
            "2026-10-26T09:00:00",
            Some("2026-10-27T11:00:00"),
        ),
    )
    .unwrap();
    let items = agenda(&conn, "2026-10-19", "2026-10-31");
    assert!(items
        .iter()
        .any(|i| i.local_start == "2026-10-22T14:00:00" && i.state == "Done"));
    assert!(items
        .iter()
        .any(|i| i.local_start == "2026-10-27T11:00:00" && i.state == "Moved"));
    assert!(!items.iter().any(|i| i.local_start == "2026-10-26T09:00:00"));
    assert!(pl::occurrence_edit(
        &mut conn,
        &ctx(NOW),
        edit(
            t.revision,
            "series",
            "move",
            "2026-10-19T09:00:00",
            Some("2026-10-19T10:30:25")
        )
    )
    .is_err());
    let t = pl::occurrence_edit(
        &mut conn,
        &ctx(NOW),
        edit(
            t.revision,
            "series",
            "move",
            "2026-10-19T09:00:00",
            Some("2026-10-19T10:30:00"),
        ),
    )
    .unwrap();
    assert!(t
        .recurrence
        .unwrap()
        .weekday_times
        .values()
        .all(|time| time == "10:30"));
    let items = agenda(&conn, "2026-10-19", "2026-10-31");
    assert!(items
        .iter()
        .any(|i| i.original_local.as_deref() == Some("2026-10-22T10:30:00") && i.state == "Done"));
    assert!(items.iter().any(
        |i| i.original_local.as_deref() == Some("2026-10-26T10:30:00")
            && i.local_start == "2026-10-27T11:00:00"
    ));
}

#[test]
fn schema_eleven_upgrade_preserves_legacy_single_time_rules_and_overrides() {
    let mut conn = Connection::open_in_memory().unwrap();
    for (_, sql) in db::MIGRATIONS.iter().take(11) {
        conn.execute_batch(sql).unwrap();
    }
    conn.pragma_update(None, "user_version", 11).unwrap();
    conn.execute("INSERT INTO recurrence_rules(id,created_at,updated_at,origin_device_id,series_id,rrule,tz,dtstart_local) VALUES ('old',?1,?1,'fixture','old-series','FREQ=WEEKLY;BYDAY=MO,TH','Europe/Berlin','2026-10-19T08:00:00')",[NOW]).unwrap();
    conn.execute("INSERT INTO tasks(id,created_at,updated_at,origin_device_id,title,recurrence_rule_id) VALUES ('old-task',?1,?1,'fixture','Old series','old')",[NOW]).unwrap();
    conn.execute("INSERT INTO occurrences(id,created_at,updated_at,origin_device_id,series_id,original_local,state) VALUES ('old-override',?1,?1,'fixture','old-series','2026-10-22T08:00:00','Skipped')",[NOW]).unwrap();
    db::migrate(&mut conn).unwrap();
    let t = get(&conn, "old-task");
    assert!(t.recurrence.unwrap().weekday_times.is_empty());
    let items = agenda(&conn, "2026-10-19", "2026-10-23");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].local_start, "2026-10-19T08:00:00");
    assert_eq!(items[1].state, "Skipped");
}

fn weekly_edit(t: &pl::Task, from: &str) -> serde_json::Value {
    json!({"id":t.id,"expected_revision":t.revision,"schedule":{"effective_from":from,"recurrence":{"rrule":"FREQ=WEEKLY;BYDAY=TU,FR","tz":"Europe/Berlin","dtstart_local":"2026-10-19T09:00:00","weekday_times":{"TU":"12:00","FR":"15:00"}}}})
}
fn editable_routine(c: &mut Connection) -> pl::Task {
    mk(
        c,
        json!({"title":"Schedule fixture","recurrence":{"rrule":"FREQ=WEEKLY;BYDAY=MO,TH","tz":"Europe/Berlin","dtstart_local":"2026-10-19T09:00:00","weekday_times":{"MO":"09:00","TH":"10:00"}}}),
    )
}
#[test]
fn editing_weekdays_and_times_from_a_date_preserves_earlier_schedule_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("routine.sqlite");
    let mut c = db::open(&path).unwrap();
    let t = editable_routine(&mut c);
    let series = t.recurrence.as_ref().unwrap().series_id.clone();
    let saved = pl::task_update(&mut c, &ctx(NOW), req(weekly_edit(&t, "2026-10-26"))).unwrap();
    assert_eq!(saved.id, t.id);
    assert_eq!(saved.recurrence.unwrap().series_id, series);
    assert_eq!(saved.schedule_from.as_deref(), Some("2026-10-26"));
    drop(c);
    let c = db::open(&path).unwrap();
    let rows = agenda(&c, "2026-10-19", "2026-10-31");
    let times: Vec<_> = rows.iter().map(|i| i.local_start.as_str()).collect();
    assert_eq!(
        times,
        vec![
            "2026-10-19T09:00:00",
            "2026-10-22T10:00:00",
            "2026-10-27T12:00:00",
            "2026-10-30T15:00:00"
        ]
    );
    assert_eq!(count(&c, "SELECT count(*) FROM tasks"), 1);
    assert_eq!(
        count(&c, "SELECT count(*) FROM routine_schedule_history"),
        1
    );
}
#[test]
fn routine_edit_keeps_completed_skipped_and_manually_moved_meeting_decisions() {
    let mut c = db::open_in_memory().unwrap();
    let mut t = editable_routine(&mut c);
    for (original, action, new) in [
        ("2026-10-19T09:00:00", "done", None),
        ("2026-10-26T09:00:00", "skip", None),
        ("2026-10-29T10:00:00", "move", Some("2026-10-28T16:00:00")),
    ] {
        t=pl::occurrence_edit(&mut c,&ctx(NOW),req(json!({"task_id":t.id,"expected_revision":t.revision,"scope":"this","action":action,"original_local":original,"new_local":new}))).unwrap();
    }
    let t = pl::task_update(&mut c, &ctx(NOW), req(weekly_edit(&t, "2026-10-26"))).unwrap();
    let rows = agenda(&c, "2026-10-19", "2026-10-31");
    assert!(rows
        .iter()
        .any(|i| i.state == "Done" && i.original_local.as_deref() == Some("2026-10-19T09:00:00")));
    assert!(rows.iter().any(
        |i| i.state == "Skipped" && i.original_local.as_deref() == Some("2026-10-26T09:00:00")
    ));
    assert!(rows
        .iter()
        .any(|i| i.state == "Moved" && i.local_start == "2026-10-28T16:00:00"));
    let t=pl::occurrence_edit(&mut c,&ctx(NOW),req(json!({"task_id":t.id,"expected_revision":t.revision,"scope":"this","action":"reset","original_local":"2026-10-19T09:00:00"}))).unwrap();
    assert_eq!(agenda(&c, "2026-10-19", "2026-10-19")[0].state, "Normal");
    assert!(pl::occurrence_edit(&mut c,&ctx(NOW),req(json!({"task_id":t.id,"expected_revision":t.revision,"scope":"this","action":"done","original_local":"2026-10-27T12:00:00"}))).is_ok());
}
#[test]
fn schedule_edit_validation_and_stale_revisions_do_not_partially_write() {
    let mut c = db::open_in_memory().unwrap();
    let t = editable_routine(&mut c);
    let mut invalid = Vec::new();
    let mut v = weekly_edit(&t, "2026-10-26");
    v["expected_revision"] = json!(0);
    invalid.push(v);
    let mut v = weekly_edit(&t, "2026-10-18");
    invalid.push(v.clone());
    v["schedule"]["effective_from"] = json!("2026-02-30");
    invalid.push(v);
    let mut v = weekly_edit(&t, "2026-10-26");
    v["schedule"]["recurrence"]["weekday_times"] = json!({"TU":"12:00"});
    invalid.push(v);
    let mut v = weekly_edit(&t, "2026-10-26");
    v["schedule"]["recurrence"]["tz"] = json!("UTC");
    invalid.push(v);
    for v in invalid {
        assert!(pl::task_update(&mut c, &ctx(NOW), req(v)).is_err());
        assert_eq!(get(&c, &t.id), t);
        assert_eq!(
            count(&c, "SELECT count(*) FROM routine_schedule_history"),
            0
        );
    }
    let saved = pl::task_update(&mut c, &ctx(NOW), req(weekly_edit(&t, "2026-10-27"))).unwrap();
    assert!(pl::task_update(&mut c, &ctx(NOW), req(weekly_edit(&saved, "2026-10-26"))).is_err());
    assert_eq!(get(&c, &t.id), saved);
}
#[test]
fn audit_refusal_rolls_back_schedule_history_rule_and_task_together() {
    let mut c = db::open_in_memory().unwrap();
    let t = editable_routine(&mut c);
    c.execute_batch("CREATE TRIGGER refuse_schedule_edit BEFORE INSERT ON audit_events WHEN NEW.action='task.update' BEGIN SELECT RAISE(ABORT,'synthetic refusal'); END;").unwrap();
    assert!(pl::task_update(&mut c, &ctx(NOW), req(weekly_edit(&t, "2026-10-26"))).is_err());
    assert_eq!(get(&c, &t.id), t);
    assert_eq!(
        count(&c, "SELECT count(*) FROM routine_schedule_history"),
        0
    );
    assert_eq!(count(&c, "SELECT count(*) FROM outbox"), 1);
}
