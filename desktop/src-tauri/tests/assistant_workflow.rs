//! Real SQLite + synthetic sources/inference only. No IMAP/Graph/OS vault or model service.
use rei_os_lib::commands::IpcError;
use rei_os_lib::{
    assistant_workflow::{self as flow, WorkflowReq},
    auto_academic as aa, db, hiz, inbox as ib, planner, providers,
};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::sync::{atomic::AtomicBool, Mutex};
static SERIAL: Mutex<()> = Mutex::new(());
const NOW: &str = "2026-10-19T10:00:00Z";
fn setup(c: &Connection, saved: bool, inbox: bool) -> String {
    let id = hiz::configure(
        c,
        None,
        &hiz::HizSettings {
            username: "synthetic-student".into(),
            initial_count: 50,
        },
        NOW,
    )
    .unwrap();
    providers::set_secret_saved(c, &id, saved, NOW).unwrap();
    let mut found = vec![("folder".into(), "Junk".into(), "Junk".into())];
    if inbox {
        found.push(("folder".into(), "INBOX".into(), "Inbox".into()));
    }
    providers::upsert_discovered(c, &id, &found, NOW).unwrap();
    id
}
fn enable(c: &mut Connection, id: &str) -> flow::WorkflowResult {
    flow::set_workflow(
        c,
        NOW,
        &WorkflowReq {
            enabled: true,
            account_id: Some(id.into()),
        },
    )
    .unwrap()
}
fn source(c: &Connection, id: &str, title: &str, body: &str) -> String {
    let sid = uuid::Uuid::new_v4().to_string();
    c.execute("INSERT INTO source_messages(id,account_id,kind,provider_object_id,subject,body_text,received_at,label,ingested_at) VALUES (?1,?2,'email',?1,?3,?4,?5,'SYNTHETIC_FIXTURE',?5)",params![sid,id,title,body,NOW]).unwrap();
    sid
}
struct Fake;
impl ib::Infer for Fake {
    fn infer(&self, m: &Value, _cancel: &AtomicBool) -> Result<String, IpcError> {
        let body = m.as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap();
        Ok(if body.contains("SYNTHETIC-newsletter") {
            json!({"routine":true,"items":[]}).to_string()
        } else if body.contains("Raum 0.01") {
            json!({"routine":false,"items":[{"kind":"update","title":"Lecture moved","excerpt":"Die Vorlesung findet in Raum 0.01 statt.","date_text":null,"due_date":null,"confidence":0.8}]}).to_string()
        } else if body.contains("Samstag um 18 Uhr") {
            json!({"routine":false,"items":[{"kind":"deadline","title":"Submit sheet","excerpt":"Abgabe bis Samstag um 18 Uhr.","date_text":"bis Samstag um 18 Uhr","due_date":null,"confidence":0.8}]}).to_string()
        } else {
            json!({"routine":false,"items":[{"kind":"deadline","title":"Submit sheet","excerpt":"Abgabe bis Freitag um 12 Uhr.","date_text":"bis Freitag um 12 Uhr","due_date":null,"confidence":0.8}]}).to_string()
        })
    }
}
fn step(d: &Mutex<Connection>) -> Option<aa::Outcome> {
    step_at(d, NOW)
}
fn step_at(d: &Mutex<Connection>, now: &str) -> Option<aa::Outcome> {
    aa::run_one(d, "fixture-device", &|| now.into(), &|_| {
        Ok(Box::new(Fake) as Box<dyn ib::Infer>)
    })
    .unwrap()
}

#[test]
fn arrivals_edits_interrupted_analysis_and_next_day_reopen_keep_exact_owner_decisions() {
    let _serial = SERIAL.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic-day.sqlite");
    let mut c = db::open(&path).unwrap();
    let id = setup(&c, true, true);
    enable(&mut c, &id);
    assert_eq!(aa::enqueue(&c, NOW).unwrap(), 0);
    let d = Mutex::new(c);
    assert_eq!(step(&d), None);

    // Mail arrives after activation, rather than only existing at startup.
    let c = d.into_inner().unwrap();
    let sid = source(&c, &id, "Sheet", "Abgabe bis Freitag um 12 Uhr.");
    assert_eq!(aa::enqueue(&c, NOW).unwrap(), 1);
    let d = Mutex::new(c);
    assert!(matches!(step(&d), Some(aa::Outcome::Stored(_))));
    let mut c = d.into_inner().unwrap();
    let s = aa::summary(&c, NOW, &aa::SummaryReq::default()).unwrap();
    assert_eq!(s.actionable_total, 1);
    let old = &s.actionable[0];
    assert_eq!(old.due_date.as_deref(), Some("2026-10-23"));
    assert_eq!(old.due_time.as_deref(), Some("12:00"));
    let request = |id: &str, hash: &str| aa::BatchReq {
        items: vec![ib::ApproveReq {
            id: id.into(),
            payload_hash: hash.into(),
            title: None,
            due_date: None,
            date_resolved: None,
            due_time: None,
        }],
    };
    let ctx = planner::Ctx {
        device_id: "fixture-device",
        now: NOW,
    };
    // The provider has edited the message while the old review is still open.
    c.execute(
        "UPDATE source_messages SET body_text='Abgabe bis Samstag um 18 Uhr.' WHERE id=?1",
        [&sid],
    )
    .unwrap();
    let refused = aa::batch_approve(&mut c, &ctx, request(&old.id, &old.payload_hash)).unwrap();
    assert!(!refused.committed);
    assert_eq!(refused.created, 0);
    assert!(refused.results[0]
        .message
        .as_ref()
        .unwrap()
        .contains("changed"));
    assert_eq!(
        c.query_row("SELECT count(*) FROM tasks", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(aa::enqueue(&c, NOW).unwrap(), 1);
    assert!(aa::claim(&c, NOW).unwrap().is_some());
    // A process interruption leaves the durable analysis job running on disk.
    drop(c);

    const NEXT_DAY: &str = "2026-10-20T07:00:00Z";
    let c = db::open(&path).unwrap();
    assert!(providers::status(&c).unwrap()[0].sign_in_saved);
    assert!(providers::get_polling(&c).unwrap().enabled);
    assert!(aa::get_settings(&c).unwrap().enabled);
    assert_eq!(aa::recover(&c, NEXT_DAY).unwrap(), 1);
    source(&c, &id, "Room", "Die Vorlesung findet in Raum 0.01 statt.");
    source(&c, &id, "Mensa", "SYNTHETIC-newsletter");
    assert_eq!(aa::enqueue(&c, NEXT_DAY).unwrap(), 2);
    let d = Mutex::new(c);
    for _ in 0..3 {
        assert!(matches!(
            step_at(&d, NEXT_DAY),
            Some(aa::Outcome::Stored(_))
        ));
    }
    assert_eq!(step_at(&d, NEXT_DAY), None);
    let mut c = d.into_inner().unwrap();
    let s = aa::summary(&c, NEXT_DAY, &aa::SummaryReq::default()).unwrap();
    assert_eq!(
        (s.actionable_total, s.updates_total, s.routine_total),
        (1, 1, 1)
    );
    let current = &s.actionable[0];
    assert_ne!(current.id, old.id);
    assert_eq!(current.due_date.as_deref(), Some("2026-10-24"));
    assert_eq!(current.due_time.as_deref(), Some("18:00"));
    let ctx = planner::Ctx {
        now: NEXT_DAY,
        ..ctx
    };
    let approved =
        aa::batch_approve(&mut c, &ctx, request(&current.id, &current.payload_hash)).unwrap();
    assert!(approved.committed);
    assert_eq!(approved.created, 1);
    let replay =
        aa::batch_approve(&mut c, &ctx, request(&current.id, &current.payload_hash)).unwrap();
    assert!(replay.committed);
    assert_eq!((replay.created, replay.replayed), (0, 1));
    ib::reject(
        &mut c,
        &ctx,
        ib::RejectReq {
            id: s.updates[0].id.clone(),
            payload_hash: s.updates[0].payload_hash.clone(),
        },
    )
    .unwrap();
    drop(c);

    let c = db::open(&path).unwrap();
    assert_eq!(aa::recover(&c, NEXT_DAY).unwrap(), 0);
    assert_eq!(aa::enqueue(&c, NEXT_DAY).unwrap(), 0);
    let s = aa::summary(&c, NEXT_DAY, &aa::SummaryReq::default()).unwrap();
    assert_eq!(
        (s.actionable_total, s.updates_total, s.routine_total),
        (0, 0, 1)
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM tasks", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        c.query_row("SELECT due_at_utc, due_tz FROM tasks", [], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .unwrap(),
        ("2026-10-24T16:00:00Z".into(), "Europe/Berlin".into())
    );
    assert_eq!(
        c.query_row(
            "SELECT status FROM source_candidates WHERE id=?1",
            [&old.id],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "stale"
    );
}

#[test]
fn one_activation_persists_intake_and_analysis_and_only_defaults_to_inbox() {
    let _serial = SERIAL.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workflow.sqlite");
    let mut c = db::open(&path).unwrap();
    let id = setup(&c, true, true);
    aa::set_settings(
        &c,
        NOW,
        &aa::AutoSetReq {
            enabled: false,
            paused: false,
            model: "existing-model:9b".into(),
            date_zone: Some("Europe/Berlin".into()),
        },
    )
    .unwrap();
    let r = enable(&mut c, &id);
    assert!(r.selected_inbox && r.settings.enabled && !r.settings.paused && r.polling.enabled);
    assert_eq!(r.settings.model, "existing-model:9b");
    assert_eq!(r.settings.date_zone, "Europe/Berlin");
    assert_eq!(
        providers::selected(&c, &id, "folder").unwrap(),
        vec!["INBOX"]
    );
    drop(c);
    let mut c = db::open(&path).unwrap();
    assert!(providers::get_polling(&c).unwrap().enabled);
    assert!(aa::get_settings(&c).unwrap().enabled);
    assert!(!enable(&mut c, &id).selected_inbox);
    assert_eq!(providers::pollable_accounts(&c).unwrap().len(), 1);
    let cfg = providers::load_config(&c, &id, hiz::PROVIDER)
        .unwrap()
        .to_string();
    assert!(!cfg.contains("password"));
}

#[test]
fn existing_folder_choices_are_kept_and_busy_fetch_does_not_force_login() {
    let _serial = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    let id = setup(&c, true, true);
    providers::set_selected(&mut c, &id, &[("folder".into(), "Junk".into())]).unwrap();
    let running = providers::operation_guard(&c, &id).unwrap();
    let r = enable(&mut c, &id);
    assert!(!r.selected_inbox);
    assert_eq!(
        providers::selected(&c, &id, "folder").unwrap(),
        vec!["Junk"]
    );
    drop(running);
}

#[test]
fn missing_auth_or_missing_inbox_cannot_half_enable_the_pipeline() {
    let _serial = SERIAL.lock().unwrap();
    for (saved, inbox) in [(false, true), (true, false)] {
        let mut c = db::open_in_memory().unwrap();
        let id = setup(&c, saved, inbox);
        let err = flow::set_workflow(
            &mut c,
            NOW,
            &WorkflowReq {
                enabled: true,
                account_id: Some(id.clone()),
            },
        )
        .unwrap_err();
        assert_eq!(
            err.code,
            if saved {
                "Validation"
            } else {
                "ProviderAuthRequired"
            }
        );
        assert!(!providers::get_polling(&c).unwrap().enabled);
        assert!(!aa::get_settings(&c).unwrap().enabled);
        assert!(providers::selected(&c, &id, "folder").unwrap().is_empty());
    }
}

#[test]
fn a_database_refusal_rolls_back_default_scope_and_both_flags() {
    let _serial = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    let id = setup(&c, true, true);
    c.execute_batch("CREATE TRIGGER fixture_refusal BEFORE INSERT ON auto_analysis_settings BEGIN SELECT RAISE(ABORT,'synthetic refusal'); END;").unwrap();
    assert!(flow::set_workflow(
        &mut c,
        NOW,
        &WorkflowReq {
            enabled: true,
            account_id: Some(id.clone())
        }
    )
    .is_err());
    assert!(providers::selected(&c, &id, "folder").unwrap().is_empty());
    assert!(!providers::get_polling(&c).unwrap().enabled);
}

#[test]
fn pause_cancels_current_fetch_and_keeps_signin_sources_and_waiting_jobs() {
    let _serial = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    let id = setup(&c, true, true);
    enable(&mut c, &id);
    source(&c, &id, "Sheet", "Abgabe bis Freitag um 12 Uhr.");
    aa::enqueue(&c, NOW).unwrap();
    let active = providers::operation_guard(&c, &id).unwrap();
    let r = flow::set_workflow(
        &mut c,
        NOW,
        &WorkflowReq {
            enabled: false,
            account_id: None,
        },
    )
    .unwrap();
    assert!(r.settings.paused);
    assert!(!r.polling.enabled);
    assert!(active.cancelled());
    assert!(providers::status(&c).unwrap()[0].sign_in_saved);
    assert_eq!(
        providers::selected(&c, &id, "folder").unwrap(),
        vec!["INBOX"]
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM analysis_jobs WHERE status='queued'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    drop(active);
    assert!(enable(&mut c, &id).polling.enabled);
}

#[test]
fn intake_queue_sorts_tasks_updates_and_routine_without_manual_analysis_or_silent_tasks() {
    let _serial = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    let id = setup(&c, true, true);
    enable(&mut c, &id);
    source(&c, &id, "Sheet", "Abgabe bis Freitag um 12 Uhr.");
    source(&c, &id, "Room", "Die Vorlesung findet in Raum 0.01 statt.");
    source(&c, &id, "Mensa", "SYNTHETIC-newsletter");
    assert_eq!(aa::enqueue(&c, NOW).unwrap(), 3);
    let d = Mutex::new(c);
    for _ in 0..3 {
        assert!(matches!(step(&d), Some(aa::Outcome::Stored(_))));
    }
    let mut c = d.lock().unwrap();
    let s = aa::summary(&c, NOW, &aa::SummaryReq::default()).unwrap();
    assert_eq!(
        (s.actionable_total, s.updates_total, s.routine_total),
        (1, 1, 1)
    );
    assert_eq!(s.actionable[0].due_date.as_deref(), Some("2026-10-23"));
    assert_eq!(s.actionable[0].due_time.as_deref(), Some("12:00"));
    assert_eq!(
        c.query_row("SELECT count(*) FROM tasks", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let p = &s.actionable[0];
    let r = aa::batch_approve(
        &mut c,
        &planner::Ctx {
            device_id: "fixture-device",
            now: NOW,
        },
        aa::BatchReq {
            items: vec![ib::ApproveReq {
                id: p.id.clone(),
                payload_hash: p.payload_hash.clone(),
                title: None,
                due_date: None,
                date_resolved: None,
                due_time: None,
            }],
        },
    )
    .unwrap();
    assert!(r.committed);
    assert_eq!(r.created, 1);
    assert_eq!(aa::enqueue(&c, NOW).unwrap(), 0);
    assert_eq!(
        c.query_row("SELECT count(*) FROM tasks", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn older_routine_results_are_rechecked_once_and_existing_decisions_are_not_requeued() {
    let _serial = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    let id = setup(&c, true, true);
    enable(&mut c, &id);
    let sid = source(&c, &id, "Room", "Die Vorlesung findet in Raum 0.01 statt.");
    ib::analyze_with(
        &mut c,
        &planner::Ctx {
            device_id: "fixture",
            now: NOW,
        },
        &sid,
        "fake",
        &Routine,
    )
    .unwrap();
    c.execute("UPDATE source_analyses SET policy_version=1", [])
        .unwrap();
    assert_eq!(aa::enqueue(&c, NOW).unwrap(), 1);
    let d = Mutex::new(c);
    assert!(matches!(step(&d), Some(aa::Outcome::Stored(_))));
    let mut c = d.lock().unwrap();
    assert_eq!(aa::enqueue(&c, NOW).unwrap(), 0);
    let s = aa::summary(&c, NOW, &aa::SummaryReq::default()).unwrap();
    assert_eq!(s.updates_total, 1);
    assert_eq!(s.routine_total, 0);
    let p = &s.updates[0];
    ib::reject(
        &mut c,
        &planner::Ctx {
            device_id: "fixture",
            now: NOW,
        },
        ib::RejectReq {
            id: p.id.clone(),
            payload_hash: p.payload_hash.clone(),
        },
    )
    .unwrap();
    // Even a legacy routine result after a rejection must never revive that source.
    c.execute(
        "UPDATE source_analyses SET status='routine',policy_version=1",
        [],
    )
    .unwrap();
    assert_eq!(aa::enqueue(&c, NOW).unwrap(), 0);
    assert_eq!(
        c.query_row(
            "SELECT status FROM source_candidates WHERE id=?1",
            [&p.id],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "rejected"
    );
}
struct Routine;
impl ib::Infer for Routine {
    fn infer(&self, _: &Value, _: &AtomicBool) -> Result<String, IpcError> {
        Ok(json!({"routine":true,"items":[]}).to_string())
    }
}

#[test]
fn workflow_requests_cannot_accept_passwords_or_remote_endpoints() {
    assert!(serde_json::from_value::<WorkflowReq>(
        json!({"enabled":true,"account_id":"fixture","password":"synthetic"})
    )
    .is_err());
    assert!(serde_json::from_value::<WorkflowReq>(
        json!({"enabled":true,"account_id":"fixture","host":"example.invalid"})
    )
    .is_err());
}
