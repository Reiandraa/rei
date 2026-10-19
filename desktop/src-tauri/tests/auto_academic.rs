//! Automatic academic analysis: real Rust + SQLite tests of the durable queue, worker step
//! and grouped approval. All messages are SYNTHETIC fixtures; inference is a labeled fake
//! (`Fake`) — no Ollama, provider, credential or real app-data profile is touched.
use rei_os_lib::auto_academic::{self as aa, AutoSetReq, BatchReq, Outcome, SummaryReq};
use rei_os_lib::commands::IpcError;
use rei_os_lib::connectors::{self as c, Kind};
use rei_os_lib::db;
use rei_os_lib::inbox::{self as ib, ApproveReq, Infer, RejectReq};
use rei_os_lib::planner::Ctx;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const NOW: &str = "2026-10-19T10:00:00Z";
const DEV: &str = "dev-test";
const SHEET: &str = "bitte geben Sie das Übungsblatt 3 bis zum 14.11.2026 im Moodle ab.";

/// The inference slot is process-global (shared with manual analysis), so tests that
/// run the worker are serialized.
static SERIAL: Mutex<()> = Mutex::new(());
fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

type Db = Arc<Mutex<Connection>>;
type Answer = Arc<dyn Fn(&Value) -> Result<String, IpcError> + Send + Sync>;

/// Labeled mocked inference: runs the test's closure instead of a model.
struct Fake(Answer);
impl Infer for Fake {
    fn infer(&self, m: &Value, _c: &AtomicBool) -> Result<String, IpcError> {
        (self.0)(m)
    }
}

fn now() -> String {
    NOW.to_string()
}

fn ctx(now: &str) -> Ctx<'_> {
    Ctx {
        device_id: DEV,
        now,
    }
}

fn mem() -> Db {
    Arc::new(Mutex::new(db::open_in_memory().unwrap()))
}

fn enable(conn: &Connection) {
    aa::set_settings(
        conn,
        NOW,
        &AutoSetReq {
            date_zone: None,
            enabled: true,
            paused: false,
            model: "qwen3.5:9b".into(),
        },
    )
    .unwrap();
}

fn add(conn: &Connection, subject: &str, body: &str, received: &str) -> String {
    let acc: Option<String> = conn
        .query_row(
            "SELECT id FROM connector_accounts WHERE kind='email'",
            [],
            |r| r.get(0),
        )
        .ok();
    let acc = acc.unwrap_or_else(|| {
        c::create_account(conn, Kind::Email, "hiz-imap", "HIZ (synthetic)", NOW)
            .unwrap()
            .id
    });
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO source_messages(id, account_id, kind, provider_object_id, sender, subject, body_text, received_at, label, ingested_at)
         VALUES (?1, ?2, 'email', ?1, 'prof@example.invalid', ?3, ?4, ?5, 'SYNTHETIC_FIXTURE', ?5)",
        params![id, acc, subject, body, received],
    )
    .unwrap();
    id
}

fn add_sheet(conn: &Connection) -> String {
    add(
        conn,
        "Übungsblatt 3",
        &format!("Liebe Studierende,\n{SHEET}\nProf. Synthetisch"),
        NOW,
    )
}

fn deadline_answer() -> String {
    json!({"routine": false, "items": [{"kind":"deadline","title":"Submit exercise sheet 3","excerpt":SHEET,
        "date_text":"14.11.2026","due_date":"2026-11-14","confidence":0.8}]})
    .to_string()
}

/// Answers by message content, like a model would: newsletters routine, others cited.
fn by_content() -> Answer {
    Arc::new(|m: &Value| {
        let text = m.to_string();
        Ok(if text.contains("Mensa") {
            json!({"routine": true, "items": []}).to_string()
        } else if text.contains("Raumänderung") {
            json!({"routine": false, "items": [{"kind":"update","title":"Lecture moved to room 0.01",
                "excerpt":"Die Vorlesung findet heute in Raum 0.01 statt.","date_text":null,"due_date":null,"confidence":0.7}]})
            .to_string()
        } else if let Some(n) = text
            .split("Blatt-")
            .nth(1)
            .and_then(|s| s.split(' ').next())
        {
            json!({"routine": false, "items": [{"kind":"task","title":format!("Hand in sheet {n}"),
                "excerpt":format!("Abgabe Blatt-{n} bitte im Moodle."),"date_text":null,"due_date":null,"confidence":0.6}]})
            .to_string()
        } else {
            deadline_answer()
        })
    })
}

fn fake(a: Answer) -> impl Fn(&str) -> Result<Box<dyn Infer>, IpcError> {
    move |_model: &str| Ok(Box::new(Fake(a.clone())) as Box<dyn Infer>)
}

fn step(db: &Db, a: Answer) -> Option<Outcome> {
    aa::run_one(db, DEV, &now, &fake(a)).unwrap()
}

fn drain(db: &Db, a: Answer) -> Vec<Outcome> {
    let mut out = Vec::new();
    while let Some(o) = step(db, a.clone()) {
        out.push(o);
        assert!(out.len() < 1000, "worker did not settle");
    }
    out
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

fn summary(conn: &Connection) -> aa::DailySummary {
    aa::summary(conn, NOW, &SummaryReq::default()).unwrap()
}

fn req(cand: &ib::Candidate) -> ApproveReq {
    ApproveReq {
        id: cand.id.clone(),
        payload_hash: cand.payload_hash.clone(),
        title: None,
        due_date: None,
        date_resolved: None,
        due_time: None,
    }
}

fn batch(conn: &mut Connection, items: Vec<ApproveReq>) -> Result<aa::BatchResult, IpcError> {
    aa::batch_approve(conn, &ctx(NOW), BatchReq { items })
}

#[test]
fn opt_in_persists_and_disabled_worker_does_nothing() {
    let _s = serial();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.sqlite");
    {
        let conn = db::open(&path).unwrap();
        add_sheet(&conn);
        assert!(!aa::get_settings(&conn).unwrap().enabled);
        assert_eq!(aa::get_settings(&conn).unwrap().model, aa::DEFAULT_MODEL);
        let db = Arc::new(Mutex::new(conn));
        assert_eq!(step(&db, by_content()), None, "off by default");
        enable(&db.lock().unwrap());
    }
    let conn = db::open(&path).unwrap();
    let s = aa::get_settings(&conn).unwrap();
    assert!(s.enabled && !s.paused, "opt-in survives restart");
    assert!(aa::set_settings(
        &conn,
        NOW,
        &AutoSetReq {
            date_zone: None,
            enabled: true,
            paused: false,
            model: "bad model;rm".into()
        }
    )
    .is_err());
}

#[test]
fn catch_up_survives_crash_restart_and_never_repeats_unchanged_sources() {
    let _s = serial();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.sqlite");
    let ids: Vec<String> = {
        let conn = db::open(&path).unwrap();
        enable(&conn);
        let ids = (0..3)
            .map(|i| {
                add(
                    &conn,
                    "Blatt",
                    &format!("Abgabe Blatt-{i} bitte im Moodle."),
                    &format!("2026-10-1{i}T08:00:00Z"),
                )
            })
            .collect();
        assert_eq!(aa::enqueue(&conn, NOW).unwrap(), 3);
        assert_eq!(aa::enqueue(&conn, NOW).unwrap(), 0, "no duplicate jobs");
        // Simulated crash: a job was claimed (running) when the process died.
        let job = aa::claim(&conn, NOW).unwrap().unwrap();
        assert_eq!(job.attempts, 1);
        ids
    };
    let conn = db::open(&path).unwrap();
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM analysis_jobs WHERE status='running'"
        ),
        1
    );
    assert_eq!(aa::recover(&conn, NOW).unwrap(), 1);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM analysis_jobs WHERE status='queued'"
        ),
        3
    );
    let db = Arc::new(Mutex::new(conn));
    let outs = drain(&db, by_content());
    assert_eq!(outs, vec![Outcome::Stored("candidates".into()); 3]);
    let conn = db.lock().unwrap();
    assert_eq!(count(&conn, "SELECT count(*) FROM source_analyses"), 3);
    assert_eq!(
        aa::enqueue(&conn, NOW).unwrap(),
        0,
        "unchanged analysed sources are not queued again"
    );
    let s = summary(&conn);
    assert_eq!(
        (s.not_analysed, s.actionable_total, s.queue.queued),
        (0, 3, 0)
    );
    assert!(s.all_analysed);
    // Newest message first in the review list is not required, but every source is covered.
    let covered: std::collections::HashSet<_> =
        s.actionable.iter().map(|c| c.source_id.clone()).collect();
    assert!(ids.iter().all(|i| covered.contains(i)));
    // A changed message is a new version and is queued exactly once.
    conn.execute(
        "UPDATE source_messages SET body_text = 'Abgabe Blatt-9 bitte im Moodle.' WHERE id = ?1",
        [&ids[0]],
    )
    .unwrap();
    assert_eq!(aa::enqueue(&conn, NOW).unwrap(), 1);
}

#[test]
fn a_job_that_keeps_crashing_stops_after_max_attempts() {
    let _s = serial();
    let conn = db::open_in_memory().unwrap();
    enable(&conn);
    add_sheet(&conn);
    aa::enqueue(&conn, NOW).unwrap();
    for _ in 0..aa::MAX_ATTEMPTS {
        aa::claim(&conn, NOW).unwrap().unwrap();
        aa::recover(&conn, NOW).unwrap();
    }
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM analysis_jobs WHERE status='failed'"
        ),
        1
    );
    assert_eq!(aa::claim(&conn, NOW).unwrap(), None);
    let s = summary(&conn);
    assert_eq!(s.attention[0].state, "failed");
    assert!(!s.all_analysed);
}

#[test]
fn source_changed_or_deleted_during_inference_saves_nothing_and_lock_is_free() {
    let _s = serial();
    for delete in [false, true] {
        let db = mem();
        let id = {
            let c = db.lock().unwrap();
            enable(&c);
            let id = add_sheet(&c);
            aa::enqueue(&c, NOW).unwrap();
            id
        };
        let (db2, id2) = (db.clone(), id.clone());
        let editing: Answer = Arc::new(move |_m: &Value| {
            // The DB mutex must not be held while the model runs.
            let c = db2.try_lock().expect("DB lock held during inference");
            let sql = if delete {
                "UPDATE source_messages SET deleted_at = '2026-10-19T10:00:01Z' WHERE id = ?1"
            } else {
                "UPDATE source_messages SET body_text = 'Neue Fassung: Abgabe entfällt.' WHERE id = ?1"
            };
            c.execute(sql, [&id2]).unwrap();
            Ok(deadline_answer())
        });
        assert_eq!(step(&db, editing), Some(Outcome::Superseded));
        let c = db.lock().unwrap();
        assert_eq!(count(&c, "SELECT count(*) FROM source_candidates"), 0);
        assert_eq!(count(&c, "SELECT count(*) FROM source_analyses"), 0);
        assert_eq!(
            count(
                &c,
                "SELECT count(*) FROM analysis_jobs WHERE status='superseded'"
            ),
            1
        );
        assert_eq!(
            aa::enqueue(&c, NOW).unwrap(),
            if delete { 0 } else { 1 },
            "{id}"
        );
    }
}

#[test]
fn manual_analysis_excludes_worker_and_pause_cancels_running_inference() {
    let _s = serial();
    let db = mem();
    {
        let c = db.lock().unwrap();
        enable(&c);
        add_sheet(&c);
        aa::enqueue(&c, NOW).unwrap();
    }
    {
        let _manual = ib::try_begin_inference().expect("slot free");
        assert_eq!(step(&db, by_content()), None, "one inference at a time");
    }
    let db2 = db.clone();
    let pausing: Answer = Arc::new(move |_m: &Value| {
        let c = db2.lock().unwrap();
        aa::set_settings(
            &c,
            NOW,
            &AutoSetReq {
                date_zone: None,
                enabled: true,
                paused: true,
                model: "qwen3.5:9b".into(),
            },
        )
        .unwrap();
        assert!(
            ib::cancel_flag().load(Ordering::SeqCst),
            "pause signals the running inference"
        );
        Err(IpcError::new("Cancelled", "Analysis cancelled."))
    });
    assert_eq!(step(&db, pausing), Some(Outcome::Interrupted));
    {
        let c = db.lock().unwrap();
        assert_eq!(count(&c, "SELECT count(*) FROM source_analyses"), 0);
        assert_eq!(
            count(
                &c,
                "SELECT attempts FROM analysis_jobs WHERE status='queued'"
            ),
            0,
            "pause uses no attempt"
        );
        assert_eq!(
            aa::claim(&c, NOW).unwrap(),
            None,
            "paused worker claims nothing"
        );
    }
    assert!(!ib::inference_busy(), "slot released");
    // Model changed during inference: nothing saved under the old configuration.
    enable(&db.lock().unwrap());
    let db3 = db.clone();
    let switching: Answer = Arc::new(move |_m: &Value| {
        let c = db3.lock().unwrap();
        aa::set_settings(
            &c,
            NOW,
            &AutoSetReq {
                date_zone: None,
                enabled: true,
                paused: false,
                model: "llama3.2:3b".into(),
            },
        )
        .unwrap();
        Ok(deadline_answer())
    });
    assert_eq!(step(&db, switching), Some(Outcome::Interrupted));
    assert_eq!(
        count(
            &db.lock().unwrap(),
            "SELECT count(*) FROM source_candidates"
        ),
        0
    );
    assert_eq!(
        drain(&db, by_content()),
        vec![Outcome::Stored("candidates".into())]
    );
}

#[test]
fn offline_backs_off_without_busy_loop_or_counted_attempts() {
    let _s = serial();
    let db = mem();
    {
        let c = db.lock().unwrap();
        enable(&c);
        add_sheet(&c);
        aa::enqueue(&c, NOW).unwrap();
    }
    let offline: Answer = Arc::new(|_m: &Value| {
        Err(IpcError::new(
            "Offline",
            "Ollama is not running on this computer.",
        ))
    });
    assert_eq!(step(&db, offline.clone()), Some(Outcome::Unavailable));
    assert_eq!(step(&db, offline), None, "backoff: no immediate retry");
    let c = db.lock().unwrap();
    let s = aa::get_settings(&c).unwrap();
    assert_eq!(s.worker_state, "offline");
    assert_eq!(s.backoff_until.as_deref(), Some("2026-10-19T10:05:00Z"));
    assert_eq!(count(&c, "SELECT attempts FROM analysis_jobs"), 0);
    let sum = summary(&c);
    assert_eq!((sum.not_analysed, sum.queue.queued), (1, 1));
    assert!(!sum.all_analysed, "offline is never all clear");
    assert!(
        aa::claim(&c, "2026-10-19T10:06:00Z").unwrap().is_some(),
        "retries after backoff"
    );
}

#[test]
fn invalid_output_retries_are_bounded_then_visible_and_retryable() {
    let _s = serial();
    let db = mem();
    {
        let c = db.lock().unwrap();
        enable(&c);
        add_sheet(&c);
        aa::enqueue(&c, NOW).unwrap();
    }
    let junk: Answer = Arc::new(|_m: &Value| Ok("I cannot answer in JSON {".into()));
    let mut outs = Vec::new();
    for _ in 0..aa::MAX_ATTEMPTS {
        // Backoff delays retries; make them due for the test.
        db.lock()
            .unwrap()
            .execute("UPDATE analysis_jobs SET next_attempt_at = ?1", [NOW])
            .unwrap();
        outs.push(step(&db, junk.clone()).unwrap());
    }
    assert_eq!(
        outs,
        vec![
            Outcome::Invalid("retry_later"),
            Outcome::Invalid("retry_later"),
            Outcome::Invalid("failed")
        ]
    );
    db.lock()
        .unwrap()
        .execute("UPDATE analysis_jobs SET next_attempt_at = ?1", [NOW])
        .unwrap();
    assert_eq!(step(&db, junk.clone()), None, "no endless retries");
    {
        let c = db.lock().unwrap();
        assert_eq!(aa::enqueue(&c, NOW).unwrap(), 0);
        let s = summary(&c);
        assert_eq!(s.attention_total, 1);
        assert_eq!(s.attention[0].state, "failed");
        assert!(!s.all_analysed);
        assert_eq!(aa::retry(&c, NOW, None).unwrap(), 1);
    }
    assert_eq!(
        step(&db, by_content()),
        Some(Outcome::Stored("candidates".into()))
    );
    // Other errors (e.g. timeout) count as attempts too.
    let db = mem();
    {
        let c = db.lock().unwrap();
        enable(&c);
        add_sheet(&c);
        aa::enqueue(&c, NOW).unwrap();
    }
    let slow: Answer = Arc::new(|_m: &Value| {
        Err(IpcError::new(
            "Unavailable",
            "The local model took too long and was stopped.",
        ))
    });
    assert_eq!(step(&db, slow), Some(Outcome::Error("retry_later")));
    assert_eq!(
        count(
            &db.lock().unwrap(),
            "SELECT attempts FROM analysis_jobs WHERE status='queued'"
        ),
        1
    );
}

#[test]
fn cancel_queue_stops_work_until_explicit_retry() {
    let _s = serial();
    let db = mem();
    {
        let c = db.lock().unwrap();
        enable(&c);
        add_sheet(&c);
        add(&c, "Mensa", "Mensa Speiseplan der Woche", NOW);
        aa::enqueue(&c, NOW).unwrap();
        assert_eq!(aa::cancel_queue(&c, NOW).unwrap(), 2);
        assert_eq!(
            aa::enqueue(&c, NOW).unwrap(),
            0,
            "cancelled versions are not re-queued"
        );
        let s = summary(&c);
        assert_eq!((s.queue.cancelled, s.attention_total), (2, 2));
    }
    assert_eq!(step(&db, by_content()), None);
    assert_eq!(aa::retry(&db.lock().unwrap(), NOW, None).unwrap(), 2);
    assert_eq!(drain(&db, by_content()).len(), 2);
}

#[test]
fn summary_sorts_actionable_updates_and_routine_and_keeps_originals() {
    let _s = serial();
    let db = mem();
    let routine_id = {
        let c = db.lock().unwrap();
        enable(&c);
        add_sheet(&c);
        add(
            &c,
            "Raumänderung",
            "Raumänderung: Die Vorlesung findet heute in Raum 0.01 statt.",
            NOW,
        );
        let r = add(&c, "Mensa", "Mensa Speiseplan der Woche", NOW);
        aa::enqueue(&c, NOW).unwrap();
        r
    };
    drain(&db, by_content());
    let c = db.lock().unwrap();
    let s = summary(&c);
    assert_eq!(
        (s.actionable_total, s.updates_total, s.routine_total),
        (1, 1, 1)
    );
    assert_eq!(s.actionable[0].kind, "deadline");
    assert_eq!(s.actionable[0].due_date.as_deref(), Some("2026-11-14"));
    assert_eq!(s.updates[0].kind, "update");
    assert_eq!(s.routine[0].source_id, routine_id);
    assert!(s.all_analysed);
    // Routine is only a label: the original is still listed and readable.
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM source_messages WHERE deleted_at IS NULL"
        ),
        3
    );
    assert!(ib::get_source(&c, NOW, &routine_id)
        .unwrap()
        .body_text
        .contains("Mensa"));
    assert_eq!(
        count(&c, "SELECT count(*) FROM tasks"),
        0,
        "nothing becomes a task automatically"
    );
}

/// Two sheets analysed into pending deadlines, ready for grouped review.
fn reviewed(n: usize) -> (Db, Vec<ib::Candidate>) {
    let db = mem();
    {
        let c = db.lock().unwrap();
        enable(&c);
        for i in 0..n {
            add(
                &c,
                "Blatt",
                &format!("Abgabe Blatt-{i} bitte im Moodle."),
                &format!("2026-10-{:02}T08:00:00Z", 1 + i % 28),
            );
        }
        aa::enqueue(&c, NOW).unwrap();
    }
    drain(&db, by_content());
    let s = aa::summary(
        &db.lock().unwrap(),
        NOW,
        &SummaryReq {
            limit: Some(100),
            ..Default::default()
        },
    )
    .unwrap();
    (db, s.actionable)
}

#[test]
fn grouped_approval_is_exact_atomic_and_replay_safe() {
    let _s = serial();
    let (db, cands) = reviewed(3);
    let mut c = db.lock().unwrap();
    // Wrong hash on one item: nothing is created, the others are held, not dropped.
    let mut items: Vec<ApproveReq> = cands.iter().map(req).collect();
    items[1].payload_hash = "0".repeat(64);
    let r = batch(&mut c, items).unwrap();
    assert!(!r.committed);
    assert_eq!(r.results[1].code.as_deref(), Some("ApprovalInvalid"));
    assert_eq!(r.results[0].code.as_deref(), Some("Held"));
    assert_eq!(count(&c, "SELECT count(*) FROM tasks"), 0);
    // Bounds and duplicate IDs.
    assert!(batch(&mut c, vec![]).is_err());
    assert!(batch(&mut c, vec![req(&cands[0]), req(&cands[0])]).is_err());
    // Owner edits the first title and excludes the third.
    let mut items = vec![req(&cands[0]), req(&cands[1])];
    items[0].title = Some("Sheet 0 — my wording".into());
    let r = batch(&mut c, items).unwrap();
    assert!(r.committed);
    assert_eq!((r.created, r.replayed), (2, 0));
    assert_eq!(count(&c, "SELECT count(*) FROM tasks"), 2);
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM tasks WHERE title = 'Sheet 0 — my wording'"
        ),
        1
    );
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM source_candidates WHERE status='pending'"
        ),
        1,
        "excluded item untouched"
    );
    // The owner later edits the task; replaying the same approval creates nothing and keeps the edit.
    let t0 = r.results[0].task_id.clone().unwrap();
    c.execute(
        "UPDATE tasks SET title = 'Owner edit after approval' WHERE id = ?1",
        [&t0],
    )
    .unwrap();
    let r = batch(&mut c, vec![req(&cands[0]), req(&cands[1])]).unwrap();
    assert!(r.committed);
    assert_eq!((r.created, r.replayed), (0, 2));
    assert_eq!(count(&c, "SELECT count(*) FROM tasks"), 2);
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM tasks WHERE title = 'Owner edit after approval'"
        ),
        1
    );
    // Provenance links back to the exact source version.
    let prov: String = c
        .query_row(
            "SELECT provenance_json FROM tasks WHERE id = ?1",
            [&t0],
            |r| r.get(0),
        )
        .unwrap();
    assert!(prov.contains(&cands[0].source_hash) && prov.contains(&cands[0].payload_hash));
}

#[test]
fn grouped_approval_rejects_stale_expired_and_preserves_rejections() {
    let _s = serial();
    let (db, cands) = reviewed(3);
    {
        let mut c = db.lock().unwrap();
        // Rejected stays rejected: it cannot be batch-approved and re-analysis keeps it.
        ib::reject(
            &mut c,
            &ctx(NOW),
            RejectReq {
                id: cands[2].id.clone(),
                payload_hash: cands[2].payload_hash.clone(),
            },
        )
        .unwrap();
        let r = batch(&mut c, vec![req(&cands[2])]).unwrap();
        assert!(!r.committed);
        // Explicit re-analysis of the unchanged message keeps the owner's rejection.
        let again = ib::analyze_with(
            &mut c,
            &ctx(NOW),
            &cands[2].source_id,
            "qwen3.5:9b",
            &Fake(by_content()),
        )
        .unwrap();
        assert_eq!((again.created, again.already_known), (0, 1));
        // Source edited after review: stale, nothing created for the other item either.
        c.execute("UPDATE source_messages SET body_text = 'Abgabe Blatt-1 bitte im Moodle. Korrektur.' WHERE id = ?1", [&cands[1].source_id]).unwrap();
        let r = batch(&mut c, vec![req(&cands[0]), req(&cands[1])]).unwrap();
        assert!(!r.committed);
        assert_eq!(r.results[0].code.as_deref(), Some("Held"));
        assert_eq!(count(&c, "SELECT count(*) FROM tasks"), 0);
        assert_eq!(
            count(
                &c,
                &format!(
                    "SELECT count(*) FROM source_candidates WHERE id='{}' AND status='stale'",
                    cands[1].id
                )
            ),
            1
        );
        // Expired review.
        let late = Ctx {
            device_id: DEV,
            now: "2026-12-30T10:00:00Z",
        };
        let r = aa::batch_approve(
            &mut c,
            &late,
            BatchReq {
                items: vec![req(&cands[0])],
            },
        )
        .unwrap();
        assert!(!r.committed);
        assert!(r.results[0].message.as_deref().unwrap().contains("expired"));
    }
    let c = db.lock().unwrap();
    assert_eq!(
        aa::enqueue(&c, NOW).unwrap(),
        1,
        "only the edited message is queued again"
    );
    assert_eq!(
        count(
            &c,
            &format!(
                "SELECT count(*) FROM source_candidates WHERE id='{}' AND status='rejected'",
                cands[2].id
            )
        ),
        1
    );
}

#[test]
fn ambiguous_dates_conflicts_and_updates_need_explicit_owner_decisions() {
    let _s = serial();
    let db = mem();
    {
        let c = db.lock().unwrap();
        enable(&c);
        add(
            &c,
            "Projekt",
            "Projektbericht bitte bis nächsten Freitag hochladen.",
            NOW,
        );
        add(
            &c,
            "Klausur A",
            "Die Klausur Analysis findet am 03.02.2027 statt.",
            NOW,
        );
        add(
            &c,
            "Klausur B",
            "Korrektur: Die Klausur Analysis findet am 10.02.2027 statt.",
            "2026-10-18T10:00:00Z",
        );
        add(
            &c,
            "Raumänderung",
            "Raumänderung: Die Vorlesung findet heute in Raum 0.01 statt.",
            NOW,
        );
        aa::enqueue(&c, NOW).unwrap();
    }
    let model: Answer = Arc::new(|m: &Value| {
        let t = m.to_string();
        let item = // "nächsten Freitag" can mean this or next week: stays ambiguous (a bare
        // "bis Freitag" now resolves deterministically; see tests/ui_planner_dates.rs).
        if t.contains("Freitag") {
            json!({"kind":"task","title":"Upload project report","excerpt":"Projektbericht bitte bis nächsten Freitag hochladen.","date_text":"nächsten Freitag","due_date":null,"confidence":0.9})
        } else if t.contains("10.02.2027") {
            json!({"kind":"deadline","title":"Analysis exam","excerpt":"Die Klausur Analysis findet am 10.02.2027 statt.","date_text":"10.02.2027","due_date":"2027-02-10","confidence":0.9})
        } else if t.contains("03.02.2027") {
            json!({"kind":"deadline","title":"Analysis exam","excerpt":"Die Klausur Analysis findet am 03.02.2027 statt.","date_text":"03.02.2027","due_date":"2027-02-03","confidence":0.99})
        } else {
            json!({"kind":"update","title":"Lecture moved to room 0.01","excerpt":"Die Vorlesung findet heute in Raum 0.01 statt.","date_text":null,"due_date":null,"confidence":0.7})
        };
        Ok(json!({"routine": false, "items": [item]}).to_string())
    });
    drain(&db, model);
    let mut c = db.lock().unwrap();
    let s = aa::summary(&c, NOW, &SummaryReq::default()).unwrap();
    assert_eq!(
        (s.actionable_total, s.needs_decision, s.updates_total),
        (3, 3, 1)
    );
    let amb = s
        .actionable
        .iter()
        .find(|x| x.date_status == "ambiguous")
        .unwrap()
        .clone();
    assert_eq!(amb.due_date, None, "no guessed date");
    let exams: Vec<_> = s
        .actionable
        .iter()
        .filter(|x| x.conflict)
        .cloned()
        .collect();
    assert_eq!(exams.len(), 2);
    // High confidence does not grant permission.
    for x in [&amb, &exams[0]] {
        let r = batch(&mut c, vec![req(x)]).unwrap();
        assert!(!r.committed);
        assert_eq!(r.results[0].code.as_deref(), Some("ApprovalRequired"));
    }
    // Updates are information, not tasks, in grouped review.
    let r = batch(&mut c, vec![req(&s.updates[0])]).unwrap();
    assert!(!r.committed);
    // Bad owner date is rejected; explicit resolution is accepted.
    let mut bad = req(&amb);
    bad.date_resolved = Some(true);
    bad.due_date = Some("31.10.".into());
    assert!(!batch(&mut c, vec![bad]).unwrap().committed);
    let mut a = req(&amb);
    a.date_resolved = Some(true);
    a.due_date = Some("2026-10-23".into());
    let mut e = req(exams
        .iter()
        .find(|x| x.due_date.as_deref() == Some("2027-02-10"))
        .unwrap());
    e.date_resolved = Some(true);
    e.due_date = Some("2027-02-10".into());
    let r = batch(&mut c, vec![a, e]).unwrap();
    assert!(r.committed, "{r:?}");
    assert_eq!(
        count(
            &c,
            "SELECT count(*) FROM tasks WHERE due_date IN ('2026-10-23','2027-02-10')"
        ),
        2
    );
    // Approved tasks are date-only planner tasks (no invented time) visible in the agenda source list.
    assert_eq!(ib::overview(&c, NOW).unwrap().source_tasks.len(), 2);
}

#[test]
fn paging_and_batch_bounds_cover_more_than_fifty_items() {
    let _s = serial();
    let (db, first) = reviewed(120);
    let mut c = db.lock().unwrap();
    let s = aa::summary(&c, NOW, &SummaryReq::default()).unwrap();
    assert_eq!((s.sources_total, s.actionable_total), (120, 120));
    assert_eq!(first.len(), 100);
    let rest = aa::summary(
        &c,
        NOW,
        &SummaryReq {
            limit: Some(100),
            actionable_offset: Some(100),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(rest.actionable.len(), 20);
    let all: std::collections::HashSet<_> = first
        .iter()
        .chain(rest.actionable.iter())
        .map(|x| x.id.clone())
        .collect();
    assert_eq!(all.len(), 120, "pages are disjoint and complete");
    let too_many: Vec<ApproveReq> = first.iter().take(aa::MAX_BATCH + 1).map(req).collect();
    assert!(batch(&mut c, too_many).is_err());
    let r = batch(&mut c, first.iter().take(aa::MAX_BATCH).map(req).collect()).unwrap();
    assert_eq!(r.created as usize, aa::MAX_BATCH);
    let after = aa::summary(&c, NOW, &SummaryReq::default()).unwrap();
    assert_eq!(after.actionable_total, 70);
}

#[test]
fn manual_analysis_of_a_queued_message_is_not_repeated_by_the_worker() {
    let _s = serial();
    let db = mem();
    let id = {
        let c = db.lock().unwrap();
        enable(&c);
        let id = add_sheet(&c);
        aa::enqueue(&c, NOW).unwrap();
        id
    };
    ib::analyze_with(
        &mut db.lock().unwrap(),
        &ctx(NOW),
        &id,
        "qwen3.5:9b",
        &Fake(by_content()),
    )
    .unwrap();
    let never: Answer = Arc::new(|_m: &Value| panic!("worker must not run inference again"));
    assert_eq!(step(&db, never), Some(Outcome::AlreadyAnalysed));
    let c = db.lock().unwrap();
    assert_eq!(count(&c, "SELECT count(*) FROM source_analyses"), 1);
    assert_eq!(
        count(&c, "SELECT count(*) FROM analysis_jobs WHERE status='done'"),
        1
    );
}

#[test]
fn a_source_changed_back_to_an_earlier_version_is_analysed_again() {
    let _s = serial();
    let db = mem();
    let id = {
        let c = db.lock().unwrap();
        enable(&c);
        let id = add_sheet(&c);
        assert_eq!(aa::enqueue(&c, NOW).unwrap(), 1);
        let original: String = c
            .query_row(
                "SELECT body_text FROM source_messages WHERE id = ?1",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        c.execute(
            "UPDATE source_messages SET body_text = 'Kurz geändert.' WHERE id = ?1",
            [&id],
        )
        .unwrap();
        assert_eq!(aa::enqueue(&c, NOW).unwrap(), 1, "changed version queued");
        c.execute(
            "UPDATE source_messages SET body_text = ?2 WHERE id = ?1",
            params![id, original],
        )
        .unwrap();
        // The earlier version's job was superseded; it must be queued again, not ignored.
        assert_eq!(
            aa::enqueue(&c, NOW).unwrap(),
            1,
            "reverted version queued again"
        );
        assert_eq!(
            count(
                &c,
                "SELECT count(*) FROM analysis_jobs WHERE status='queued'"
            ),
            1
        );
        id
    };
    assert_eq!(
        drain(&db, by_content()),
        vec![Outcome::Stored("candidates".into())]
    );
    let c = db.lock().unwrap();
    let s = summary(&c);
    assert_eq!(s.not_analysed, 0, "{id}");
    assert!(s.all_analysed);
    assert_eq!(
        aa::enqueue(&c, NOW).unwrap(),
        0,
        "no repeated unchanged analysis"
    );
}

#[test]
fn an_unusable_manual_analysis_is_picked_up_once_and_stays_bounded() {
    let _s = serial();
    for model_keeps_failing in [false, true] {
        let db = mem();
        {
            let mut c = db.lock().unwrap();
            enable(&c);
            let id = add_sheet(&c);
            let prep = ib::prepare(&c, &id).unwrap();
            let res = ib::store(&mut c, &ctx(NOW), &prep, "qwen3.5:9b", "not json {").unwrap();
            assert_eq!(res.status, "invalid_output");
            assert!(!summary(&c).all_analysed);
            // Previously never queued: it stayed invalid with nothing to retry.
            assert_eq!(aa::enqueue(&c, NOW).unwrap(), 1);
        }
        if !model_keeps_failing {
            assert_eq!(
                drain(&db, by_content()),
                vec![Outcome::Stored("candidates".into())]
            );
            let c = db.lock().unwrap();
            assert!(summary(&c).all_analysed);
            assert_eq!(aa::enqueue(&c, NOW).unwrap(), 0);
            continue;
        }
        let junk: Answer = Arc::new(|_m: &Value| Ok("still not json".into()));
        for _ in 0..aa::MAX_ATTEMPTS {
            db.lock()
                .unwrap()
                .execute("UPDATE analysis_jobs SET next_attempt_at = ?1", [NOW])
                .unwrap();
            step(&db, junk.clone()).unwrap();
        }
        let c = db.lock().unwrap();
        assert_eq!(
            count(
                &c,
                "SELECT count(*) FROM analysis_jobs WHERE status='failed'"
            ),
            1
        );
        assert_eq!(
            aa::enqueue(&c, NOW).unwrap(),
            0,
            "failed version is not re-queued"
        );
        let s = summary(&c);
        assert_eq!(s.attention[0].state, "failed");
        assert!(!s.all_analysed);
    }
}

#[test]
fn a_panic_during_inference_releases_the_slot_and_is_bounded_by_recovery() {
    let _s = serial();
    let db = mem();
    {
        let c = db.lock().unwrap();
        enable(&c);
        add_sheet(&c);
        aa::enqueue(&c, NOW).unwrap();
    }
    let boom: Answer = Arc::new(|_m: &Value| panic!("synthetic inference crash"));
    for round in 1..=aa::MAX_ATTEMPTS {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            aa::run_one(&db, DEV, &now, &fake(boom.clone()))
        }));
        assert!(r.is_err(), "fake model panicked");
        assert!(
            ib::try_begin_inference().is_some(),
            "slot released after panic"
        );
        // DB mutex was not held during inference, so it is not poisoned.
        let c = db.lock().expect("DB mutex not poisoned");
        aa::recover(&c, NOW).unwrap();
        let expect = if round < aa::MAX_ATTEMPTS {
            "queued"
        } else {
            "failed"
        };
        let st: String = c
            .query_row("SELECT status FROM analysis_jobs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(st, expect, "round {round}");
    }
    assert_eq!(step(&db, by_content()), None, "no endless crash loop");
}

#[test]
fn transient_setting_changes_discard_even_a_successful_uncancellable_answer() {
    let _s = serial();
    for model_change in [false, true] {
        let db = mem();
        {
            let c = db.lock().unwrap();
            enable(&c);
            add_sheet(&c);
            aa::enqueue(&c, NOW).unwrap();
        }
        let other = db.clone();
        let changing: Answer = Arc::new(move |_| {
            let c = other.lock().unwrap();
            aa::set_settings(
                &c,
                NOW,
                &AutoSetReq {
                    date_zone: None,
                    enabled: true,
                    paused: !model_change,
                    model: if model_change {
                        "synthetic-other"
                    } else {
                        "qwen3.5:9b"
                    }
                    .into(),
                },
            )
            .unwrap();
            enable(&c); // Model A -> B -> A, or pause -> resume, before inference returns.
            Ok(deadline_answer()) // A late adapter answer that ignored cancellation.
        });
        assert_eq!(step(&db, changing), Some(Outcome::Interrupted));
        let c = db.lock().unwrap();
        assert_eq!(count(&c, "SELECT count(*) FROM source_analyses"), 0);
        assert_eq!(
            count(
                &c,
                "SELECT attempts FROM analysis_jobs WHERE status='queued'"
            ),
            0
        );
    }
}

#[test]
fn summary_finds_current_proposals_beyond_five_thousand_expired_rows() {
    let _s = serial();
    let (db, original) = reviewed(1);
    let c = db.lock().unwrap();
    // Synthetic history rows deliberately precede the current valid proposal.
    // They test whole-history coverage, not approval integrity of those expired fixtures.
    c.execute(
        "UPDATE source_candidates SET extracted_at='2026-10-18T10:00:00Z'",
        [],
    )
    .unwrap();
    c.execute(
        "WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM seq WHERE n < 5001)
         INSERT INTO source_candidates(id,source_id,source_hash,analysis_id,dedupe_key,kind,title,due_date,date_text,date_status,
           excerpt,confidence,model,extracted_at,expires_at,payload_hash)
         SELECT 'expired-'||n,c.source_id,c.source_hash,c.analysis_id,'expired-'||n,c.kind,c.title,c.due_date,c.date_text,c.date_status,
           c.excerpt,c.confidence,c.model,'2026-10-19T10:00:00Z','2026-10-18T10:00:00Z',c.payload_hash
         FROM seq JOIN source_candidates c ON c.id=?1",
        [&original[0].id],
    ).unwrap();
    let s = summary(&c);
    assert_eq!(s.actionable_total, 1);
    assert_eq!(s.actionable[0].id, original[0].id);
}
