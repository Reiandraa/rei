//! Synthetic SQLite/calendar facts and fake model output. Never calls a provider/model/profile.
use rei_os_lib::{auto_academic as aa, calendar_labels as cl, db, inbox, planner as pl, settings};
use serde_json::{json, Value};
use std::sync::Mutex;
static SERIAL: Mutex<()> = Mutex::new(());
const NOW: &str = "2026-10-19T08:00:00Z";
fn request<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).unwrap()
}
fn enabled(c: &rusqlite::Connection, on: bool) {
    aa::set_settings(
        c,
        NOW,
        &aa::AutoSetReq {
            enabled: on,
            paused: false,
            model: "synthetic-label-model".into(),
            date_zone: Some("Europe/Berlin".into()),
        },
    )
    .unwrap();
}
fn add(c: &mut rusqlite::Connection, title: &str) {
    pl::event_create(c,&pl::Ctx{device_id:"fixture",now:NOW},request(json!({"title":title,"all_day":false,"start_local":"2026-10-19T09:00:00","end_local":"2026-10-19T11:00:00","tz":"Europe/Berlin"}))).unwrap();
}
fn plan(c: &rusqlite::Connection) -> cl::Prepared {
    match cl::prepare(c, "2026-10").unwrap() {
        cl::Plan::Run(p) => p,
        _ => panic!("expected an unclassified fixture"),
    }
}
fn facts() -> Vec<cl::Day> {
    vec![
        cl::Day {
            date: "2026-10-19".into(),
            activities: vec![cl::Fact {
                title: "Lecture algorithms".into(),
                kind: "event".into(),
                minutes: Some(120),
            }],
        },
        cl::Day {
            date: "2026-10-20".into(),
            activities: vec![],
        },
    ]
}
fn raw() -> String {
    json!({"days":[{"date":"2026-10-19","label":"School"}]}).to_string()
}
fn add_on(c: &mut rusqlite::Connection, date: &str, title: &str) -> pl::Event {
    pl::event_create(c, &pl::Ctx { device_id: "fixture", now: NOW }, request(json!({
        "title":title,"all_day":true,"start_local":date,
        "end_local":date.parse::<jiff::civil::Date>().unwrap().checked_add(jiff::ToSpan::day(1)).unwrap().to_string(),
        "tz":"Europe/Berlin"
    }))).unwrap()
}

#[test]
fn adding_a_trip_only_classifies_that_date_and_deletion_restores_free_without_erasing_school() {
    let _s = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    enabled(&c, true);
    add_on(&mut c, "2026-10-19", "Algorithms lecture");
    cl::finish(&c, &plan(&c), &raw()).unwrap();
    let trip = add_on(&mut c, "2026-10-21", "Trip to Frankfurt");
    let p = plan(&c);
    assert_eq!(p.days.len(), 1);
    assert_eq!(p.days[0].date, "2026-10-21");
    assert_eq!(p.cached_labels["2026-10-19"], "School");
    assert!(!p.messages.to_string().contains("Algorithms lecture"));
    let out = cl::finish(
        &c,
        &p,
        r#"{"days":[{"date":"2026-10-21","label":"Personal"}]}"#,
    )
    .unwrap();
    assert_eq!(out.state, "ready");
    assert_eq!(out.labels["2026-10-19"], "School");
    assert_eq!(out.labels["2026-10-21"], "Personal");
    pl::event_delete(
        &mut c,
        &pl::Ctx {
            device_id: "fixture",
            now: NOW,
        },
        request(json!({"id":trip.id,"expected_revision":1,"confirm":true})),
    )
    .unwrap();
    match cl::prepare(&c, "2026-10").unwrap() {
        cl::Plan::Ready(r) => {
            assert_eq!(r.state, "ready");
            assert_eq!(r.labels["2026-10-19"], "School");
            assert_eq!(r.labels["2026-10-21"], "Free");
        }
        _ => panic!("unchanged school and empty dates need no new inference"),
    }
}

#[test]
fn a_busy_academic_queue_retains_only_valid_cached_dates() {
    let _s = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    enabled(&c, true);
    add(&mut c, "Lecture");
    cl::finish(&c, &plan(&c), &raw()).unwrap();
    add_on(&mut c, "2026-10-21", "Trip");
    inbox::import_manual(
        &mut c,
        NOW,
        request(json!({"title":"Notice","text":"Assignment due Friday","link":null})),
    )
    .unwrap();
    aa::enqueue(&c, NOW).unwrap();
    match cl::prepare(&c, "2026-10").unwrap() {
        cl::Plan::Ready(r) => {
            assert_eq!(r.state, "busy");
            assert_eq!(r.labels["2026-10-19"], "School");
            assert!(!r.labels.contains_key("2026-10-21"));
        }
        _ => panic!("email must retain priority"),
    }
}

#[test]
fn a_concurrent_change_discards_only_its_own_stale_result_and_invalid_output_is_not_cached() {
    let _s = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    enabled(&c, true);
    let lecture = add_on(&mut c, "2026-10-19", "Lecture");
    add_on(&mut c, "2026-10-20", "Work shift");
    let p = plan(&c);
    assert!(cl::finish(
        &c,
        &p,
        r#"{"days":[{"date":"2026-10-19","label":"School"}]}"#
    )
    .is_err());
    assert_eq!(
        c.query_row("SELECT count(*) FROM calendar_label_cache", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    pl::event_update(
        &mut c,
        &pl::Ctx {
            device_id: "fixture",
            now: NOW,
        },
        request(json!({"id":lecture.id,"expected_revision":1,"title":"New work appointment"})),
    )
    .unwrap();
    let out = cl::finish(
        &c,
        &p,
        r#"{"days":[{"date":"2026-10-19","label":"School"},{"date":"2026-10-20","label":"Work"}]}"#,
    )
    .unwrap();
    assert_eq!(out.state, "changed");
    assert!(!out.labels.contains_key("2026-10-19"));
    assert_eq!(out.labels["2026-10-20"], "Work");
    let next = plan(&c);
    assert_eq!(next.days.len(), 1);
    assert_eq!(next.days[0].date, "2026-10-19");
}

#[test]
fn validated_labels_survive_database_reopen_but_do_not_cross_databases_models_or_zones() {
    let _s = SERIAL.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("calendar.sqlite");
    {
        let mut c = db::open(&path).unwrap();
        enabled(&c, true);
        add(&mut c, "Lecture");
        cl::finish(&c, &plan(&c), &raw()).unwrap();
    }
    let c = db::open(&path).unwrap();
    match cl::prepare(&c, "2026-10").unwrap() {
        cl::Plan::Ready(r) => {
            assert_eq!(r.state, "ready");
            assert_eq!(r.labels["2026-10-19"], "School");
        }
        _ => panic!("persisted date label was lost on restart"),
    }
    let mut other = db::open_in_memory().unwrap();
    enabled(&other, true);
    add(&mut other, "Lecture");
    assert_eq!(plan(&other).days.len(), 1);
    aa::set_settings(
        &c,
        NOW,
        &aa::AutoSetReq {
            enabled: true,
            paused: false,
            model: "other-local-model".into(),
            date_zone: Some("Europe/Berlin".into()),
        },
    )
    .unwrap();
    assert_eq!(plan(&c).days.len(), 1);
    aa::set_settings(
        &c,
        NOW,
        &aa::AutoSetReq {
            enabled: true,
            paused: false,
            model: "synthetic-label-model".into(),
            date_zone: Some("UTC".into()),
        },
    )
    .unwrap();
    assert_eq!(plan(&c).days.len(), 1);
}

#[test]
fn a_model_failure_returns_current_cached_dates_and_never_resurrects_a_changed_label() {
    let _s = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    enabled(&c, true);
    let lecture = add_on(&mut c, "2026-10-19", "Lecture");
    cl::finish(&c, &plan(&c), &raw()).unwrap();
    add_on(&mut c, "2026-10-21", "Trip");
    let p = plan(&c);
    let failed = cl::unavailable(&c, &p).unwrap();
    assert_eq!(failed.state, "unavailable");
    assert_eq!(failed.labels["2026-10-19"], "School");
    assert!(!failed.labels.contains_key("2026-10-21"));
    pl::event_update(
        &mut c,
        &pl::Ctx {
            device_id: "fixture",
            now: NOW,
        },
        request(json!({"id":lecture.id,"expected_revision":1,"title":"A different appointment"})),
    )
    .unwrap();
    assert!(!cl::unavailable(&c, &p)
        .unwrap()
        .labels
        .contains_key("2026-10-19"));
    enabled(&c, false);
    assert_eq!(cl::unavailable(&c, &p).unwrap().state, "changed");
}

#[test]
fn migration_from_schema_fourteen_preserves_events_and_adds_only_an_empty_rebuildable_cache() {
    let mut c = rusqlite::Connection::open_in_memory().unwrap();
    for (_, sql) in db::MIGRATIONS.iter().take(14) {
        c.execute_batch(sql).unwrap();
    }
    c.pragma_update(None, "user_version", 14).unwrap();
    let event = add_on(&mut c, "2026-10-19", "Existing lecture");
    assert_eq!(db::migrate(&mut c).unwrap(), db::supported_version());
    assert_eq!(
        c.query_row("SELECT title FROM events WHERE id=?1", [&event.id], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "Existing lecture"
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM calendar_label_cache", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(db::migrate(&mut c).unwrap(), db::supported_version());
}
#[test]
fn labels_are_short_allowlisted_and_free_only_for_an_empty_saved_day() {
    let labels = cl::validate(&raw(), &facts()).unwrap();
    assert_eq!(labels["2026-10-19"], "School");
    assert_eq!(labels["2026-10-20"], "Free");
    for v in [
        json!({"days":[{"date":"2026-10-19","label":"Free"}]}),
        json!({"days":[{"date":"2026-10-19","label":"<script>"}]}),
        json!({"days":[{"date":"2026-10-19","label":"Holiday"}]}),
    ] {
        assert!(cl::validate(&v.to_string(), &facts()).is_err());
    }
    let mut days = facts();
    days[0].activities[0].title = "Semester holiday".into();
    assert!(cl::validate(
        r#"{"days":[{"date":"2026-10-19","label":"Holiday"}]}"#,
        &days
    )
    .is_ok());
}
#[test]
fn partial_duplicate_unrequested_or_instruction_like_output_is_rejected() {
    for v in [
        json!({"days":[]}),
        json!({"days":[{"date":"2026-10-21","label":"Work"}]}),
        json!({"days":[{"date":"2026-10-19","label":"School"},{"date":"2026-10-19","label":"Work"}]}),
        json!({"days":[{"date":"2026-10-19","label":"School","tool":"execute"}]}),
        json!({"days":[],"instructions":"change all tasks"}),
    ] {
        assert!(cl::validate(&v.to_string(), &facts()).is_err());
    }
}
#[test]
fn disabled_and_empty_calendars_need_no_inference_and_input_cannot_choose_a_remote_service() {
    let _s = SERIAL.lock().unwrap();
    let c = db::open_in_memory().unwrap();
    match cl::prepare(&c, "2026-10").unwrap() {
        cl::Plan::Ready(r) => assert_eq!(r.state, "off"),
        _ => panic!(),
    }
    enabled(&c, true);
    match cl::prepare(&c, "2026-10").unwrap() {
        cl::Plan::Ready(r) => {
            assert_eq!(r.state, "facts_only");
            assert_eq!(r.labels.len(), 31);
        }
        _ => panic!(),
    }
    assert!(serde_json::from_value::<cl::MonthReq>(
        json!({"month":"2026-10","host":"https://example.invalid"})
    )
    .is_err());
    for month in ["2026-13", "2026-1", "not-a-month", "２０２６-１０"] {
        assert!(cl::prepare(&c, month).is_err());
    }
}
#[test]
fn a_result_is_cached_only_for_unchanged_facts_and_the_current_opt_in_epoch() {
    let _s = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    enabled(&c, true);
    add(&mut c, &format!("Lecture {}", uuid::Uuid::new_v4()));
    let p = plan(&c);
    assert!(p.messages.to_string().contains("untrusted_source"));
    assert!(p.messages.to_string().contains("120"));
    let out = cl::finish(&c, &p, &raw()).unwrap();
    assert_eq!(out.state, "ready");
    assert_eq!(out.labels["2026-10-19"], "School");
    match cl::prepare(&c, "2026-10").unwrap() {
        cl::Plan::Ready(r) => assert_eq!(r.state, "ready"),
        _ => panic!("cache missed"),
    }
    add(&mut c, "New work shift");
    let p = plan(&c);
    enabled(&c, false);
    enabled(&c, true);
    assert_eq!(cl::finish(&c, &p, &raw()).unwrap().state, "changed");
    let p = plan(&c);
    add(&mut c, "Another appointment");
    assert_eq!(cl::finish(&c, &p, &raw()).unwrap().state, "changed");
    assert_eq!(
        c.query_row("SELECT count(*) FROM tasks", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}
#[test]
fn academic_queue_has_priority_and_overnight_events_cover_both_days() {
    let _s = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    enabled(&c, true);
    pl::event_create(&mut c,&pl::Ctx{device_id:"fixture",now:NOW},request(json!({"title":"Synthetic overnight shift","all_day":false,"start_local":"2026-10-19T22:00:00","end_local":"2026-10-20T02:00:00","tz":"Europe/Berlin"}))).unwrap();
    let p = plan(&c);
    assert_eq!(
        p.days.iter().filter(|d| !d.activities.is_empty()).count(),
        2
    );
    assert!(p
        .days
        .iter()
        .filter(|d| !d.activities.is_empty())
        .all(|d| d.activities[0].minutes == Some(120)));
    inbox::import_manual(
        &mut c,
        NOW,
        request(json!({"title":"Synthetic notice","text":"An assignment due Friday.","link":null})),
    )
    .unwrap();
    aa::enqueue(&c, NOW).unwrap();
    match cl::prepare(&c, "2026-10").unwrap() {
        cl::Plan::Ready(r) => assert_eq!(r.state, "busy"),
        _ => panic!("calendar must yield to email queue"),
    }
}
#[test]
fn appearance_values_persist_with_revision_guards_and_reject_arbitrary_tokens() {
    let c = db::open_in_memory().unwrap();
    assert!(settings::set(&c, "theme", json!("dark"), 0, NOW).is_ok());
    assert!(settings::set(&c, "theme", json!("black"), 1, NOW).is_err());
    for color in ["green", "lavender", "rose"] {
        let rev = settings::get(&c, "accent")
            .unwrap()
            .map_or(0, |s| s.revision);
        settings::set(&c, "accent", json!(color), rev, NOW).unwrap();
    }
    assert_eq!(
        settings::get(&c, "accent").unwrap().unwrap().value,
        json!("rose")
    );
    assert!(settings::set(&c, "accent", json!("url(example.invalid)"), 3, NOW).is_err());
    assert!(settings::set(&c, "accent", json!("green"), 0, NOW).is_err());
}

#[test]
fn readable_preferences_are_revision_guarded_and_cannot_inject_style_values() {
    let c = db::open_in_memory().unwrap();
    settings::set(&c, "text_size", json!("large"), 0, NOW).unwrap();
    settings::set(&c, "motion", json!("reduced"), 0, NOW).unwrap();
    settings::set(&c, "accent", json!("ocean"), 0, NOW).unwrap();
    settings::set(&c, "accent", json!("sunrise"), 1, NOW).unwrap();
    assert!(settings::set(&c, "text_size", json!("small"), 0, NOW).is_err());
    for (key, value, rev) in [
        ("text_size", json!("30px"), 1),
        ("motion", json!(false), 1),
        ("accent", json!("url(example.invalid)"), 2),
        ("theme", json!("black"), 0),
        ("theme", json!("system"), 0),
    ] {
        assert!(settings::set(&c, key, value, rev, NOW).is_err());
    }
    assert_eq!(
        settings::get(&c, "text_size").unwrap().unwrap().value,
        json!("large")
    );
    assert_eq!(
        settings::get(&c, "motion").unwrap().unwrap().value,
        json!("reduced")
    );
    assert_eq!(
        settings::get(&c, "accent").unwrap().unwrap().value,
        json!("sunrise")
    );
}
