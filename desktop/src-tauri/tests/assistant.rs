//! Synthetic SQLite, owner notes and fabricated model output; no model/account access.
use rei_os_lib::{assistant as a, auto_academic as aa, db, inbox as ib};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::sync::Mutex;
static SERIAL: Mutex<()> = Mutex::new(());
const NOW: &str = "2026-10-19T20:00:00Z";
fn req<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).unwrap()
}
fn enable(c: &Connection) {
    aa::set_settings(
        c,
        NOW,
        &aa::AutoSetReq {
            enabled: true,
            paused: false,
            model: "synthetic-model".into(),
            date_zone: Some("Europe/Berlin".into()),
        },
    )
    .unwrap();
}
fn add(c: &mut Connection, title: &str, body: &str) -> String {
    ib::import_manual(c, NOW, req(json!({"title":title,"text":body,"link":null}))).unwrap()
}
fn prepare(c: &Connection) -> a::Prepared {
    a::prepare(c, NOW)
        .unwrap()
        .expect("new message needs a summary")
}
fn output(importance: &str, text: &str, excerpt: &str) -> String {
    json!({"importance":importance,"points":[{"text":text,"excerpt":excerpt}]}).to_string()
}
fn count(c: &Connection, table: &str) -> i64 {
    c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn owner_request_is_required_and_new_messages_are_summarized_incrementally_without_task_writes() {
    let _s = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    enable(&c);
    let id = add(
        &mut c,
        "Lecture changed",
        "The lecture is cancelled tomorrow.",
    );
    assert!(a::prepare(&c, NOW).unwrap().is_none());
    assert_eq!(a::get(&c, NOW).unwrap().pending, 1);
    a::request(&c, NOW).unwrap();
    let p = prepare(&c);
    assert!(p.messages.to_string().contains("untrusted_source"));
    assert!(a::finish(
        &c,
        &p,
        &output(
            "important",
            "The lecture is cancelled tomorrow.",
            "lecture is cancelled tomorrow"
        ),
        NOW
    )
    .unwrap());
    let brief = a::get(&c, NOW).unwrap();
    assert_eq!(brief.entries.len(), 1);
    assert_eq!(brief.entries[0].source_id, id);
    assert_eq!(brief.pending, 0);
    assert!(a::prepare(&c, NOW).unwrap().is_none());
    a::request(&c, NOW).unwrap();
    assert!(a::prepare(&c, NOW).unwrap().is_none());
    add(&mut c, "Newsletter", "Buy our discounted laptop today.");
    let p = prepare(&c);
    assert!(!p.messages.to_string().contains("lecture is cancelled"));
    a::finish(&c, &p, r#"{"importance":"routine","points":[]}"#, NOW).unwrap();
    let brief = a::get(&c, NOW).unwrap();
    assert_eq!(brief.routine, 1);
    assert_eq!(brief.entries.len(), 1);
    for table in ["tasks", "source_candidates", "proposals", "memories"] {
        assert_eq!(count(&c, table), 0);
    }
}

#[test]
fn unsupported_quotes_extra_actions_and_empty_important_output_are_rejected_and_failure_is_bounded()
{
    let _s = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    enable(&c);
    add(&mut c, "Notice", "The seminar is in room 2.");
    a::request(&c, NOW).unwrap();
    let p = prepare(&c);
    for raw in [
        output("important", "Invented deadline", "Submit by Friday"),
        r#"{"importance":"important","points":[]}"#.into(),
        r#"{"importance":"routine","points":[],"tool":"delete_everything"}"#.into(),
        output("unknown", "Notice", "room 2"),
    ] {
        assert!(a::finish(&c, &p, &raw, NOW).is_err());
    }
    assert_eq!(count(&c, "assistant_message_summaries"), 0);
    let error = a::finish(&c, &p, &output("important", "Bad", "not in message"), NOW).unwrap_err();
    a::failure(&c, &p, &error, NOW).unwrap();
    assert!(a::prepare(&c, NOW).unwrap().is_none());
    assert_eq!(a::get(&c, NOW).unwrap().entries[0].importance, "failed");
    a::request(&c, NOW).unwrap();
    assert!(a::prepare(&c, NOW).unwrap().is_some());
}

#[test]
fn edited_deleted_paused_or_changed_context_cannot_commit_stale_results() {
    let _s = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    enable(&c);
    let id = add(&mut c, "Seminar", "Friday in room 2.");
    a::request(&c, NOW).unwrap();
    let p = prepare(&c);
    c.execute(
        "UPDATE source_messages SET body_text='Monday in room 3.' WHERE id=?1",
        [&id],
    )
    .unwrap();
    assert!(!a::finish(
        &c,
        &p,
        &output("important", "Friday in room 2.", "Friday in room 2."),
        NOW
    )
    .unwrap());
    let p = prepare(&c);
    aa::set_settings(
        &c,
        NOW,
        &aa::AutoSetReq {
            enabled: true,
            paused: true,
            model: "synthetic-model".into(),
            date_zone: Some("Europe/Berlin".into()),
        },
    )
    .unwrap();
    assert!(!a::finish(
        &c,
        &p,
        &output("important", "Monday in room 3.", "Monday in room 3."),
        NOW
    )
    .unwrap());
    enable(&c);
    let p = prepare(&c);
    a::notes(
        &mut c,
        req(json!({"action":"save","text":"I study computer science","confirm":true})),
        NOW,
    )
    .unwrap();
    assert!(!a::finish(
        &c,
        &p,
        &output("important", "Monday in room 3.", "Monday in room 3."),
        NOW
    )
    .unwrap());
    let p = prepare(&c);
    c.execute(
        "UPDATE source_messages SET deleted_at=?2 WHERE id=?1",
        rusqlite::params![id, NOW],
    )
    .unwrap();
    assert!(!a::finish(
        &c,
        &p,
        &output("important", "Monday in room 3.", "Monday in room 3."),
        NOW
    )
    .unwrap());
    assert_eq!(count(&c, "assistant_message_summaries"), 0);
}

#[test]
fn long_or_unclear_messages_are_not_silently_dropped_as_routine() {
    let _s = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    enable(&c);
    add(
        &mut c,
        "Long notice",
        &format!("{} Deadline at the end.", "intro ".repeat(1500)),
    );
    a::request(&c, NOW).unwrap();
    let p = prepare(&c);
    assert!(p.partial);
    a::finish(&c, &p, r#"{"importance":"routine","points":[]}"#, NOW).unwrap();
    let brief = a::get(&c, NOW).unwrap();
    assert_eq!(brief.routine, 0);
    assert_eq!(brief.entries[0].importance, "uncertain");
    assert!(brief.entries[0].partial);
    assert!(!brief.entries[0].detail.is_empty());
}

#[test]
fn academic_jobs_take_priority_and_offline_backoff_keeps_completed_summaries() {
    let _s = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    enable(&c);
    add(&mut c, "Lecture", "Cancelled tomorrow.");
    a::request(&c, NOW).unwrap();
    let p = prepare(&c);
    a::finish(
        &c,
        &p,
        &output("important", "Cancelled tomorrow.", "Cancelled tomorrow."),
        NOW,
    )
    .unwrap();
    add(&mut c, "Another lecture", "Room 4 tomorrow.");
    ib::import_manual(
        &mut c,
        NOW,
        req(json!({"title":"Queued","text":"Assignment due Friday","link":null})),
    )
    .unwrap();
    aa::enqueue(&c, NOW).unwrap();
    assert!(a::prepare(&c, NOW).unwrap().is_none());
    c.execute("UPDATE analysis_jobs SET status='done'", [])
        .unwrap();
    let p = prepare(&c);
    a::failure(
        &c,
        &p,
        &rei_os_lib::commands::IpcError::new("Offline", "synthetic offline"),
        NOW,
    )
    .unwrap();
    let brief = a::get(&c, NOW).unwrap();
    assert_eq!(brief.state, "offline");
    assert_eq!(brief.entries.len(), 1);
    assert!(a::prepare(&c, NOW).unwrap().is_none());
    a::request(&c, NOW).unwrap();
    assert!(a::prepare(&c, NOW).unwrap().is_some());
}

#[test]
fn preferences_require_explicit_consent_and_exact_current_hash_and_can_be_forgotten() {
    let _s = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    assert!(a::notes(
        &mut c,
        req(json!({"action":"save","text":"I study math"})),
        NOW
    )
    .is_err());
    assert_eq!(count(&c, "memories"), 0);
    let notes = a::notes(
        &mut c,
        req(json!({"action":"save","text":"I study math","confirm":true})),
        NOW,
    )
    .unwrap();
    let n = &notes[0];
    assert!(a::notes(&mut c,req(json!({"action":"save","id":n.id,"text":"I study physics","expected_hash":"stale","confirm":true})),NOW).is_err());
    let edited=a::notes(&mut c,req(json!({"action":"save","id":n.id,"text":"I study physics","expected_hash":n.hash,"confirm":true})),NOW).unwrap();
    assert_eq!(edited[0].text, "I study physics");
    assert!(a::notes(
        &mut c,
        req(json!({"action":"delete","id":n.id,"expected_hash":n.hash,"confirm":true})),
        NOW
    )
    .is_err());
    let result=a::notes(&mut c,req(json!({"action":"delete","id":edited[0].id,"expected_hash":edited[0].hash,"confirm":true})),NOW).unwrap();
    assert!(result.is_empty());
    assert!(a::notes(
        &mut c,
        req(json!({"action":"save","text":"password=secret","confirm":true})),
        NOW
    )
    .is_err());
}

#[test]
fn summaries_survive_reopen_and_edited_sources_are_removed_from_the_visible_brief() {
    let _s = SERIAL.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brief.sqlite");
    let id;
    {
        let mut c = db::open(&path).unwrap();
        enable(&c);
        id = add(&mut c, "Lecture", "Room 2 on Friday.");
        a::request(&c, NOW).unwrap();
        let p = prepare(&c);
        a::finish(
            &c,
            &p,
            &output("important", "Room 2 on Friday.", "Room 2 on Friday."),
            NOW,
        )
        .unwrap();
    }
    let c = db::open(&path).unwrap();
    assert_eq!(a::get(&c, NOW).unwrap().entries.len(), 1);
    assert!(a::prepare(&c, NOW).unwrap().is_none());
    c.execute(
        "UPDATE source_messages SET body_text='Revised notice' WHERE id=?1",
        [id],
    )
    .unwrap();
    let brief = a::get(&c, NOW).unwrap();
    assert!(brief.entries.is_empty());
    assert_eq!(brief.pending, 1);
}

#[test]
fn repeated_timeouts_are_bounded_and_an_explicit_refresh_can_retry_the_failed_message() {
    let _s = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    enable(&c);
    add(&mut c, "Notice", "A seminar announcement.");
    a::request(&c, NOW).unwrap();
    for now in [NOW, "2026-10-19T20:06:00Z", "2026-10-19T20:12:00Z"] {
        let p = a::prepare(&c, now).unwrap().unwrap();
        a::failure(
            &c,
            &p,
            &rei_os_lib::commands::IpcError::new("Unavailable", "synthetic timeout"),
            now,
        )
        .unwrap();
    }
    assert!(a::prepare(&c, "2026-10-19T20:18:00Z").unwrap().is_none());
    assert_eq!(a::get(&c, NOW).unwrap().entries[0].importance, "failed");
    a::request(&c, NOW).unwrap();
    assert!(a::prepare(&c, NOW).unwrap().is_some());
}

#[test]
fn a_physical_source_purge_removes_its_summary_and_the_scope_is_explicitly_bounded() {
    let _s = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    enable(&c);
    let id = add(&mut c, "Notice", "Room 2.");
    a::request(&c, NOW).unwrap();
    let p = prepare(&c);
    a::finish(&c, &p, &output("important", "Room 2.", "Room 2."), NOW).unwrap();
    c.execute("DELETE FROM source_messages WHERE id=?1", [id])
        .unwrap();
    assert_eq!(count(&c, "assistant_message_summaries"), 0);
    for n in 0..101 {
        add(&mut c, &format!("Notice {n}"), "A seminar notice.");
    }
    let brief = a::get(&c, NOW).unwrap();
    assert_eq!(brief.total, 101);
    assert_eq!(brief.scope_limit, 100);
    assert_eq!(brief.pending, 100);
    assert!(serde_json::from_value::<a::MemoryReq>(
        json!({"action":"list","host":"https://example.invalid"})
    )
    .is_err());
}

#[test]
fn supplied_german_group_notice_accepts_reflowed_quotes_without_weakening_fact_checks() {
    let _s = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    enable(&c);
    // Owner-supplied source text; model output below is fabricated, not a live-model quality test.
    add(
        &mut c,
        "Einteilung in die Übungsgruppen",
        concat!(
            "Alle, die sich ordnungsgemäß für den Übungsbetrieb der Vorlesung\r\n",
            "angemeldet haben, wurden nun vom System einer Übungsgruppe zugewiesen. Sie\r\n",
            "finden Ihre Gruppe in Ihrem Profil unter Moodle.\r\n",
            "Beachten Sie noch einmal, dass Gruppenabgaben nur mit bis zu 2 Personen AUS\r\n",
            "DERSELBEN ÜBUNGSGRUPPE möglich sind.\r\n\r\n",
            "Die Übungen beginnen nächste Woche mit einer Kennenlern-Übung, bevor ab\r\n",
            "Woche 4 die Votieraufgaben (Übungsblatt 1, Übungsblatt 2, ...) besprochen\r\nwerden."
        ),
    );
    a::request(&c, NOW).unwrap();
    let p = prepare(&c);
    let good = json!({"importance":"important","points":[
        {"text":"Your exercise group has been assigned; check your Moodle profile.",
         "excerpt":"finden Ihre Gruppe in Ihrem Profil unter Moodle."},
        {"text":"Group submissions allow up to two people from the same exercise group.",
         "excerpt":"Gruppenabgaben nur mit bis zu 2 Personen AUS DERSELBEN ÜBUNGSGRUPPE möglich sind."},
        {"text":"Exercises begin nächste Woche with an introductory session; voting exercises start ab Woche 4.",
         "excerpt":"Die Übungen beginnen nächste Woche mit einer Kennenlern-Übung, bevor ab Woche 4 die Votieraufgaben"}
    ]}).to_string();
    for bad in [
        good.replace("2 Personen", "3 Personen"),
        good.replace("Woche 4 die", "Woche 3 die"),
        good.replace("AUS DERSELBEN", "AUS EINER ANDEREN"),
    ] {
        assert!(a::finish(&c, &p, &bad, NOW).is_err());
    }
    a::finish(&c, &p, &good, NOW).unwrap();
    let brief = a::get(&c, NOW).unwrap();
    assert_eq!(brief.entries[0].importance, "important");
    assert_eq!(brief.entries[0].points.len(), 3);
    assert_eq!(brief.routine, 0);
    for table in ["tasks", "source_candidates", "proposals", "memories"] {
        assert_eq!(count(&c, table), 0);
    }
}

#[test]
fn citation_whitespace_is_tolerant_but_translation_punctuation_and_unsent_text_are_not_evidence() {
    let _s = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    enable(&c);
    add(
        &mut c,
        "Class",
        "Friday\r\n\tin room\u{a0}2.\nDoor co\u{7}de: 9.",
    );
    a::request(&c, NOW).unwrap();
    let p = prepare(&c);
    assert!(p.messages.to_string().contains("Door code: 9."));
    for quote in [
        "Monday in room 2.",
        "Friday in room 3.",
        "Freitag in Raum 2.",
        "Friday, in room 2.",
        "Friday in room2.",
        "Door co\u{7}de: 9.",
        "Europe/Berlin",
    ] {
        assert!(
            a::finish(&c, &p, &output("important", "Class update", quote), NOW).is_err(),
            "{quote}"
        );
    }
    a::finish(
        &c,
        &p,
        &output(
            "important",
            "Class is Friday in room 2.",
            "Friday in room 2.",
        ),
        NOW,
    )
    .unwrap();
}

#[test]
fn only_one_complete_json_envelope_is_accepted_and_safe_failure_reasons_are_kept() {
    let _s = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    enable(&c);
    add(&mut c, "Notice", "Friday in room 2.");
    a::request(&c, NOW).unwrap();
    let p = prepare(&c);
    let good = output(
        "important",
        "Class is Friday in room 2.",
        "Friday in room 2.",
    );
    for raw in [
        format!("Explanation: {good}"),
        format!("{good}\n{good}"),
        format!("```json\n{good}\n```\nDo something"),
        format!("```json\n{good}"),
        r#"{"importance":"routine","points":[],"action":"delete"}"#.into(),
    ] {
        let error = a::finish(&c, &p, &raw, NOW).unwrap_err();
        a::failure(&c, &p, &error, NOW).unwrap();
        assert!(a::get(&c, NOW).unwrap().entries[0]
            .detail
            .contains("format"));
    }
    for (raw, reason) in [
        (" ".into(), "no summary text"),
        ("x".repeat(6001), "oversized"),
        (
            r#"{"importance":"important","points":[]}"#.into(),
            "short points",
        ),
        (
            output("important", "Class", "room 3"),
            "quote did not match",
        ),
    ] {
        let error = a::finish(&c, &p, &raw, NOW).unwrap_err();
        a::failure(&c, &p, &error, NOW).unwrap();
        assert!(a::get(&c, NOW).unwrap().entries[0].detail.contains(reason));
    }
    // Arbitrary/provider errors must never become stored/displayed model text.
    a::failure(
        &c,
        &p,
        &rei_os_lib::commands::IpcError::new("Validation", "PRIVATE MODEL OUTPUT"),
        NOW,
    )
    .unwrap();
    assert!(!a::get(&c, NOW).unwrap().entries[0]
        .detail
        .contains("PRIVATE"));
    for raw in [
        good.clone(),
        format!("```json\n{good}\n```"),
        format!("```json\r\n{good}\r\n```"),
    ] {
        a::finish(&c, &p, &raw, NOW).unwrap();
        assert_eq!(a::get(&c, NOW).unwrap().entries[0].importance, "important");
    }
}

#[test]
fn refreshing_failed_messages_preserves_other_successful_summaries() {
    let _s = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    enable(&c);
    let kept = add(
        &mut c,
        "Shared project",
        "You have been invited to a project.",
    );
    a::request(&c, NOW).unwrap();
    let p = prepare(&c);
    a::finish(
        &c,
        &p,
        &output(
            "important",
            "Project invitation received.",
            "invited to a project",
        ),
        NOW,
    )
    .unwrap();
    add(&mut c, "Class", "Room 2 tomorrow.");
    let p = prepare(&c);
    let error = a::finish(&c, &p, "not JSON", NOW).unwrap_err();
    a::failure(&c, &p, &error, NOW).unwrap();
    let brief = a::request(&c, NOW).unwrap();
    assert_eq!(brief.pending, 1);
    assert_eq!(brief.entries.len(), 1);
    assert_eq!(brief.entries[0].source_id, kept);
    let p = prepare(&c);
    assert!(!p.messages.to_string().contains("invited to a project"));
    a::finish(
        &c,
        &p,
        &output("important", "Room 2 tomorrow.", "Room 2 tomorrow."),
        NOW,
    )
    .unwrap();
    assert_eq!(a::get(&c, NOW).unwrap().entries.len(), 2);
}

fn analysed_summary(
    c: &mut Connection,
    title: &str,
    body: &str,
    kind: Option<&str>,
    date: Option<&str>,
) -> (String, Option<ib::Candidate>) {
    let id = add(c, title, body);
    let candidate = kind.map(|kind| {
        let prep = ib::prepare(c, &id).unwrap();
        let raw = json!({"routine":false,"items":[{"kind":kind,"title":title,"excerpt":body,
            "due_date":date,"date_text":date,"confidence":0.9}]})
        .to_string();
        let result = ib::store(
            c,
            &rei_os_lib::planner::Ctx {
                device_id: "test",
                now: NOW,
            },
            &prep,
            "synthetic-model",
            &raw,
        )
        .unwrap();
        assert_eq!(result.candidates.len(), 1);
        result.candidates[0].clone()
    });
    let prep = prepare(c);
    a::finish(c, &prep, &output("important", title, body), NOW).unwrap();
    (id, candidate)
}

#[test]
fn briefing_prioritizes_due_work_then_schedule_review_and_keeps_all_ten_updates() {
    let _s = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    enable(&c);
    a::request(&c, NOW).unwrap();
    let overdue = analysed_summary(
        &mut c,
        "Submit overdue sheet",
        "Submit by 2026-10-18.",
        Some("deadline"),
        Some("2026-10-18"),
    )
    .0;
    let soon = analysed_summary(
        &mut c,
        "Submit new sheet",
        "Submit by 2026-10-20.",
        Some("deadline"),
        Some("2026-10-20"),
    )
    .0;
    let change = analysed_summary(
        &mut c,
        "Lecture room changed",
        "The lecture room changed.",
        Some("update"),
        None,
    )
    .0;
    analysed_summary(
        &mut c,
        "Register for class",
        "Please register for class.",
        Some("task"),
        None,
    );
    for n in 0..6 {
        analysed_summary(
            &mut c,
            &format!("Confirmation {n}"),
            &format!("Project {n} invitation received."),
            None,
            None,
        );
    }
    let brief = a::get(&c, NOW).unwrap();
    assert_eq!(brief.entries.len(), 10);
    assert_eq!(
        brief.entries[..3]
            .iter()
            .map(|e| e.source_id.as_str())
            .collect::<Vec<_>>(),
        vec![overdue.as_str(), soon.as_str(), change.as_str()]
    );
    assert_eq!(brief.entries[0].priority.reason, "Overdue");
    assert_eq!(brief.entries[1].priority.reason, "Due soon");
    assert_eq!(
        brief.entries[2].priority.reason,
        "Schedule change to review"
    );
    assert_eq!(brief.entries[3].priority.reason, "Action to review");
    assert_eq!(count(&c, "tasks"), 0);
    assert!(a::prepare(&c, NOW).unwrap().is_none());
}

#[test]
fn priority_tracks_approved_task_edits_completion_and_local_midnight_without_resummarizing() {
    let _s = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    enable(&c);
    a::request(&c, NOW).unwrap();
    let (_, candidate) = analysed_summary(
        &mut c,
        "Submit sheet",
        "Submit by 2026-10-20.",
        Some("deadline"),
        Some("2026-10-20"),
    );
    let candidate = candidate.unwrap();
    let approved = ib::approve(
        &mut c,
        &rei_os_lib::planner::Ctx {
            device_id: "test",
            now: NOW,
        },
        req(json!({"id":candidate.id,"payload_hash":candidate.payload_hash})),
    )
    .unwrap();
    c.execute(
        "UPDATE tasks SET due_date='2026-11-20' WHERE id=?1",
        [&approved.task_id],
    )
    .unwrap();
    let brief = a::get(&c, NOW).unwrap();
    assert_eq!(brief.entries[0].priority.rank, 2);
    assert_eq!(
        brief.entries[0].priority.due_date.as_deref(),
        Some("2026-11-20")
    );
    c.execute("UPDATE tasks SET due_date=NULL,due_at_utc='2026-10-20T21:00:00Z',due_tz='Europe/Berlin' WHERE id=?1",[&approved.task_id]).unwrap();
    assert_eq!(
        a::get(&c, "2026-10-19T22:30:00Z").unwrap().entries[0]
            .priority
            .reason,
        "Due today"
    );
    c.execute(
        "UPDATE tasks SET status='done' WHERE id=?1",
        [&approved.task_id],
    )
    .unwrap();
    let brief = a::get(&c, NOW).unwrap();
    assert_eq!(brief.entries[0].priority.rank, 3);
    assert_eq!(brief.pending, 0);
    assert_eq!(count(&c, "assistant_message_summaries"), 1);
    assert!(a::prepare(&c, NOW).unwrap().is_none());
}

#[test]
fn expired_rejected_changed_source_and_ambiguous_dates_cannot_create_false_due_priority() {
    let _s = SERIAL.lock().unwrap();
    let mut c = db::open_in_memory().unwrap();
    enable(&c);
    a::request(&c, NOW).unwrap();
    let (id, candidate) = analysed_summary(
        &mut c,
        "Submit sheet",
        "Submit by 2026-10-20.",
        Some("deadline"),
        Some("2026-10-20"),
    );
    let candidate = candidate.unwrap();
    c.execute(
        "UPDATE source_candidates SET expires_at=?1 WHERE id=?2",
        rusqlite::params![NOW, candidate.id],
    )
    .unwrap();
    assert_eq!(a::get(&c, NOW).unwrap().entries[0].priority.rank, 3);
    c.execute("UPDATE source_candidates SET expires_at='2026-11-20T00:00:00Z',status='rejected' WHERE id=?1",[&candidate.id]).unwrap();
    assert_eq!(a::get(&c, NOW).unwrap().entries[0].priority.rank, 3);
    c.execute(
        "UPDATE source_candidates SET status='pending',date_status='ambiguous' WHERE id=?1",
        [&candidate.id],
    )
    .unwrap();
    let brief = a::get(&c, NOW).unwrap();
    assert_eq!(brief.entries[0].priority.rank, 2);
    assert_eq!(brief.entries[0].priority.due_date, None);
    c.execute(
        "UPDATE source_messages SET body_text='Revised notice.' WHERE id=?1",
        [&id],
    )
    .unwrap();
    let prep = prepare(&c);
    a::finish(
        &c,
        &prep,
        &output("important", "Revised notice.", "Revised notice."),
        NOW,
    )
    .unwrap();
    assert_eq!(a::get(&c, NOW).unwrap().entries[0].priority.rank, 3);
}
