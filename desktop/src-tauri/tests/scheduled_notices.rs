//! Real SQLite + synthetic notices. No model service, account, vault or live profile.
use rei_os_lib::commands::IpcError;
use rei_os_lib::{db, inbox as ib, planner as pl, scheduled as sc};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;
const NOW: &str = "2026-10-19T08:00:00Z";
fn ctx(now: &str) -> pl::Ctx<'_> {
    pl::Ctx {
        device_id: "synthetic",
        now,
    }
}
fn req<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).unwrap()
}
fn routine(c: &mut Connection) -> pl::Task {
    pl::task_create(c,&ctx(NOW),req(json!({"title":"Algorithmen","recurrence":{"rrule":"FREQ=WEEKLY;BYDAY=MO,TH","tz":"Europe/Berlin","dtstart_local":"2026-10-19T09:00:00","weekday_times":{"MO":"09:00","TH":"10:00"}}}))).unwrap()
}
struct Fake(String);
impl ib::Infer for Fake {
    fn infer(&self, _: &Value, _: &AtomicBool) -> Result<String, IpcError> {
        Ok(self.0.clone())
    }
}
fn notice(c: &mut Connection, text: &str, received: &str) -> ib::Candidate {
    let s = ib::import_manual(
        c,
        NOW,
        req(json!({"title":"Algorithmen","text":text,"link":null})),
    )
    .unwrap();
    c.execute(
        "UPDATE source_messages SET received_at=?2 WHERE id=?1",
        rusqlite::params![s, received],
    )
    .unwrap();
    let fake=Fake(json!({"routine":false,"items":[{"kind":"update","title":"Algorithmen schedule change","excerpt":text,"date_text":"Donnerstag","due_date":null,"confidence":0.8}]}).to_string());
    ib::analyze_with(c, &ctx(NOW), &s, "synthetic", &fake)
        .unwrap()
        .candidates
        .remove(0)
}
fn change(
    n: &ib::Candidate,
    t: &pl::Task,
    scope: &str,
    date: &str,
    time: Option<&str>,
    room: Option<&str>,
) -> sc::ApplyReq {
    req(
        json!({"candidate_id":n.id,"payload_hash":n.payload_hash,"task_id":t.id,"expected_revision":t.revision,"scope":scope,"date":date,"new_time":time,"location":room}),
    )
}
fn agenda(c: &Connection) -> Vec<pl::AgendaItem> {
    pl::agenda_list(c, req(json!({"from":"2026-10-19","to":"2026-10-31"}))).unwrap()
}
fn at(c: &Connection, original: &str) -> pl::AgendaItem {
    agenda(c)
        .into_iter()
        .find(|i| i.original_local.as_deref() == Some(original))
        .unwrap()
}
fn get(c: &Connection, id: &str) -> pl::Task {
    pl::task_get(c, req(json!({"id":id}))).unwrap()
}
const TEXT: &str = "Algorithmen findet nur am Donnerstag in Raum B statt.";

#[test]
fn database_failure_rolls_back_the_notice_routine_and_amendment_together() {
    let mut c = db::open_in_memory().unwrap();
    let t = routine(&mut c);
    let n = notice(&mut c, TEXT, NOW);
    c.execute_batch("CREATE TRIGGER synthetic_refusal BEFORE INSERT ON audit_events WHEN NEW.action='schedule.change.apply' BEGIN SELECT RAISE(ABORT,'synthetic refusal'); END;").unwrap();
    assert!(sc::apply(
        &mut c,
        &ctx(NOW),
        change(&n, &t, "this", "2026-10-22", Some("12:00"), Some("B"))
    )
    .is_err());
    assert_eq!(get(&c, &t.id).revision, t.revision);
    assert_eq!(ib::candidate(&c, NOW, &n.id).unwrap().status, "pending");
    assert_eq!(
        c.query_row("SELECT count(*) FROM schedule_adjustments", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM outbox", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    c.execute_batch("DROP TRIGGER synthetic_refusal;").unwrap();
    assert!(sc::apply(
        &mut c,
        &ctx(NOW),
        change(&n, &t, "this", "2026-10-22", Some("12:00"), Some("B"))
    )
    .is_ok());
}

#[test]
fn temporary_time_and_room_change_keeps_series_identity_and_next_week_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notice.sqlite");
    let mut c = db::open(&path).unwrap();
    let t = routine(&mut c);
    let n = notice(&mut c, TEXT, NOW);
    let first = sc::apply(
        &mut c,
        &ctx(NOW),
        change(&n, &t, "this", "2026-10-22", Some("12:00"), Some("B")),
    )
    .unwrap();
    assert!(!first.replayed);
    assert_eq!(
        ib::candidate(&c, NOW, &n.id).unwrap().schedule_active,
        Some(true)
    );
    assert_eq!(first.revision, t.revision + 1);
    let amended = at(&c, "2026-10-22T10:00:00");
    assert_eq!(amended.local_start, "2026-10-22T12:00:00");
    assert_eq!(amended.location.as_deref(), Some("B"));
    assert_eq!(
        at(&c, "2026-10-29T10:00:00").local_start,
        "2026-10-29T10:00:00"
    );
    assert!(at(&c, "2026-10-29T10:00:00").location.is_none());
    assert_eq!(
        at(&c, "2026-10-26T09:00:00").local_start,
        "2026-10-26T09:00:00"
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM tasks", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    let replay = sc::apply(
        &mut c,
        &ctx(NOW),
        change(&n, &t, "this", "2026-10-22", Some("12:00"), Some("B")),
    )
    .unwrap();
    assert!(replay.replayed);
    drop(c);
    let mut c = db::open(&path).unwrap();
    assert_eq!(at(&c, "2026-10-22T10:00:00").location.as_deref(), Some("B"));
    let undone = sc::undo(
        &mut c,
        &ctx(NOW),
        sc::UndoReq {
            id: first.id,
            expected_revision: first.revision,
        },
    )
    .unwrap();
    assert!(!undone.active);
    assert_eq!(
        ib::candidate(&c, NOW, &n.id).unwrap().schedule_active,
        Some(false)
    );
    assert_eq!(
        at(&c, "2026-10-22T10:00:00").local_start,
        "2026-10-22T10:00:00"
    );
    let replay = sc::apply(
        &mut c,
        &ctx(NOW),
        change(&n, &t, "this", "2026-10-22", Some("12:00"), Some("B")),
    )
    .unwrap();
    assert!(!replay.active);
}
#[test]
fn lasting_change_only_affects_its_weekday_and_a_single_date_exception_takes_priority() {
    let mut c = db::open_in_memory().unwrap();
    let t = routine(&mut c);
    let n = notice(
        &mut c,
        "Algorithmen findet ab sofort am Donnerstag in Raum B statt.",
        NOW,
    );
    sc::apply(
        &mut c,
        &ctx(NOW),
        change(&n, &t, "following", "2026-10-22", Some("12:00"), Some("B")),
    )
    .unwrap();
    let t = get(&c, &t.id);
    let n = notice(
        &mut c,
        "Algorithmen findet nur am Donnerstag in Raum C statt.",
        NOW,
    );
    sc::apply(
        &mut c,
        &ctx(NOW),
        change(&n, &t, "this", "2026-10-29", Some("16:00"), Some("C")),
    )
    .unwrap();
    assert_eq!(
        at(&c, "2026-10-22T10:00:00").local_start,
        "2026-10-22T12:00:00"
    );
    let x = at(&c, "2026-10-29T10:00:00");
    assert_eq!(x.local_start, "2026-10-29T16:00:00");
    assert_eq!(x.location.as_deref(), Some("C"));
    assert_eq!(
        at(&c, "2026-10-26T09:00:00").local_start,
        "2026-10-26T09:00:00"
    );
}
#[test]
fn older_notice_approved_later_cannot_replace_a_newer_announcement() {
    let mut c = db::open_in_memory().unwrap();
    let t = routine(&mut c);
    let newer = notice(&mut c, TEXT, "2026-10-19T07:00:00Z");
    sc::apply(
        &mut c,
        &ctx(NOW),
        change(&newer, &t, "following", "2026-10-22", Some("13:00"), None),
    )
    .unwrap();
    let t = get(&c, &t.id);
    let older = notice(
        &mut c,
        "Algorithmen wird am Donnerstag verlegt.",
        "2026-10-18T07:00:00Z",
    );
    sc::apply(
        &mut c,
        &ctx(NOW),
        change(&older, &t, "following", "2026-10-22", Some("12:00"), None),
    )
    .unwrap();
    assert_eq!(
        at(&c, "2026-10-22T10:00:00").local_start,
        "2026-10-22T13:00:00"
    );
}
#[test]
fn completed_meeting_freezes_approved_time_and_later_changes_preserve_owner_decisions() {
    let mut c = db::open_in_memory().unwrap();
    let t = routine(&mut c);
    let n = notice(&mut c, TEXT, "2026-10-19T07:00:00Z");
    sc::apply(
        &mut c,
        &ctx(NOW),
        change(&n, &t, "following", "2026-10-22", Some("12:00"), Some("B")),
    )
    .unwrap();
    let t = get(&c, &t.id);
    let t=pl::occurrence_edit(&mut c,&ctx(NOW),req(json!({"task_id":t.id,"expected_revision":t.revision,"scope":"this","action":"done","original_local":"2026-10-22T10:00:00"}))).unwrap();
    let n = notice(
        &mut c,
        "Algorithmen wird am Donnerstag in Raum C verlegt.",
        "2026-10-19T07:59:00Z",
    );
    assert!(sc::apply(
        &mut c,
        &ctx(NOW),
        change(&n, &t, "this", "2026-10-22", Some("13:00"), Some("C"))
    )
    .is_err());
    sc::apply(
        &mut c,
        &ctx(NOW),
        change(&n, &t, "following", "2026-10-22", Some("13:00"), Some("C")),
    )
    .unwrap();
    let done = at(&c, "2026-10-22T10:00:00");
    assert_eq!(done.state, "Done");
    assert_eq!(done.local_start, "2026-10-22T12:00:00");
    assert_eq!(done.location.as_deref(), Some("B"));
    // Later notices affect future meetings, while the completed meeting retains its approved snapshot.
    assert_eq!(
        at(&c, "2026-10-29T10:00:00").local_start,
        "2026-10-29T13:00:00"
    );
}
#[test]
fn stale_source_hash_target_revision_expiry_and_invalid_dates_never_write_an_adjustment() {
    for failure in [
        "hash", "source", "target", "expiry", "date", "scope", "tamper", "empty",
    ] {
        let mut c = db::open_in_memory().unwrap();
        let t = routine(&mut c);
        let n = notice(&mut c, TEXT, NOW);
        let mut r = change(&n, &t, "this", "2026-10-22", Some("12:00"), Some("B"));
        let mut now = NOW;
        match failure {
            "hash" => r.payload_hash = "wrong".into(),
            "source" => {
                c.execute(
                    "UPDATE source_messages SET body_text='changed' WHERE id=?1",
                    [&n.source_id],
                )
                .unwrap();
            }
            "target" => r.expected_revision += 1,
            "expiry" => now = "2026-11-20T08:00:00Z",
            "date" => r.date = "2026-10-20".into(),
            "scope" => r.scope = "all".into(),
            "tamper" => {
                c.execute(
                    "UPDATE source_candidates SET excerpt='tampered' WHERE id=?1",
                    [&n.id],
                )
                .unwrap();
            }
            "empty" => {
                r.new_time = None;
                r.location = None;
            }
            _ => unreachable!(),
        }
        assert!(sc::apply(&mut c, &ctx(now), r).is_err(), "{failure}");
        assert_eq!(
            c.query_row("SELECT count(*) FROM schedule_adjustments", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(get(&c, &t.id).revision, t.revision);
    }
}
#[test]
fn an_undo_with_a_stale_revision_cannot_remove_an_approved_change() {
    let mut c = db::open_in_memory().unwrap();
    let t = routine(&mut c);
    let n = notice(&mut c, TEXT, NOW);
    let applied = sc::apply(
        &mut c,
        &ctx(NOW),
        change(&n, &t, "this", "2026-10-22", None, Some("B")),
    )
    .unwrap();
    assert!(sc::undo(
        &mut c,
        &ctx(NOW),
        sc::UndoReq {
            id: applied.id,
            expected_revision: t.revision
        }
    )
    .is_err());
    assert_eq!(at(&c, "2026-10-22T10:00:00").location.as_deref(), Some("B"));
}
#[test]
fn a_cited_schedule_change_is_an_update_and_cannot_be_converted_into_a_plain_task() {
    let raw=json!({"routine":false,"items":[{"kind":"task","title":"Move lecture","excerpt":TEXT,"date_text":"Donnerstag","due_date":null,"confidence":0.8}]}).to_string();
    assert_eq!(
        ib::validate_output_at(&raw, TEXT, Some(NOW), "Europe/Berlin")
            .unwrap()
            .items[0]
            .kind,
        "update"
    );
    let mut c = db::open_in_memory().unwrap();
    let n = notice(&mut c, TEXT, NOW);
    let error = ib::approve(
        &mut c,
        &ctx(NOW),
        req(json!({"id":n.id,"payload_hash":n.payload_hash})),
    )
    .unwrap_err();
    assert!(error.message.contains("schedule"));
    assert!(!sc::looks_like_change(
        "Bitte die Abgabe für die Vorlesung bis Donnerstag einreichen."
    ));
    let mut value = serde_json::to_value(change(
        &n,
        &routine(&mut c),
        "this",
        "2026-10-22",
        None,
        Some("B"),
    ))
    .unwrap();
    value["endpoint"] = json!("https://example.invalid");
    assert!(serde_json::from_value::<sc::ApplyReq>(value).is_err());
}

#[test]
fn source_notice_uses_the_schedule_version_for_its_date_and_keeps_approved_exceptions() {
    let mut c = db::open_in_memory().unwrap();
    let t = routine(&mut c);
    let n = notice(&mut c, TEXT, NOW);
    sc::apply(
        &mut c,
        &ctx(NOW),
        change(&n, &t, "this", "2026-10-29", Some("12:00"), Some("B")),
    )
    .unwrap();
    let t = get(&c, &t.id);
    pl::task_update(&mut c,&ctx(NOW),req(json!({"id":t.id,"expected_revision":t.revision,"schedule":{"effective_from":"2026-10-26","recurrence":{"rrule":"FREQ=WEEKLY;BYDAY=MO,TH","dtstart_local":"2026-10-19T09:00:00","tz":"Europe/Berlin","weekday_times":{"MO":"09:00","TH":"11:00"}}}}))).unwrap();
    let a = at(&c, "2026-10-29T11:00:00");
    assert_eq!(a.local_start, "2026-10-29T12:00:00");
    assert_eq!(a.location.as_deref(), Some("B"));
    let other = notice(
        &mut c,
        "Algorithmen findet nur am Donnerstag in Raum C statt.",
        "2026-10-19T09:00:00Z",
    );
    let t = get(&c, &t.id);
    sc::apply(
        &mut c,
        &ctx(NOW),
        change(&other, &t, "this", "2026-10-22", None, Some("C")),
    )
    .unwrap();
    assert_eq!(at(&c, "2026-10-22T10:00:00").location.as_deref(), Some("C"));
}

#[test]
fn retimed_done_identity_is_preserved_and_blocks_source_update_until_explicit_reset() {
    let mut c = db::open_in_memory().unwrap();
    let t = routine(&mut c);
    let t=pl::occurrence_edit(&mut c,&ctx(NOW),req(json!({"task_id":t.id,"expected_revision":t.revision,"scope":"this","action":"done","original_local":"2026-10-29T10:00:00"}))).unwrap();
    let t=pl::task_update(&mut c,&ctx(NOW),req(json!({"id":t.id,"expected_revision":t.revision,"schedule":{"effective_from":"2026-10-26","recurrence":{"rrule":"FREQ=WEEKLY;BYDAY=MO,TH","dtstart_local":"2026-10-19T09:00:00","tz":"Europe/Berlin","weekday_times":{"MO":"09:00","TH":"11:00"}}}}))).unwrap();
    assert_eq!(at(&c, "2026-10-29T10:00:00").state, "Done");
    assert_eq!(
        agenda(&c)
            .iter()
            .filter(|i| i.local_start.starts_with("2026-10-29"))
            .count(),
        1
    );
    let n = notice(&mut c, TEXT, NOW);
    assert!(sc::apply(
        &mut c,
        &ctx(NOW),
        change(&n, &t, "this", "2026-10-29", Some("12:00"), None)
    )
    .is_err());
    let t=pl::occurrence_edit(&mut c,&ctx(NOW),req(json!({"task_id":t.id,"expected_revision":t.revision,"scope":"this","action":"reset","original_local":"2026-10-29T10:00:00"}))).unwrap();
    assert_eq!(at(&c, "2026-10-29T11:00:00").state, "Normal");
    assert!(sc::apply(
        &mut c,
        &ctx(NOW),
        change(&n, &t, "this", "2026-10-29", Some("12:00"), None)
    )
    .is_ok());
}
