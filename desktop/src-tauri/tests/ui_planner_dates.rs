//! UI planner + contextual deadlines (docs/UI-PLANNER-DATES-NEXT.md): real Rust + SQLite.
//! All sources are SYNTHETIC fixtures; inference is a labeled fixed-answer fake. Temporary
//! databases only — never the owner's profile.
use rei_os_lib::auto_academic::{self as aa, AutoSetReq};
use rei_os_lib::commands::IpcError;
use rei_os_lib::dates::{self, resolve};
use rei_os_lib::db;
use rei_os_lib::inbox::{self as ib, ApproveReq, Infer, RejectReq};
use rei_os_lib::ollama;
use rei_os_lib::planner::{self as pl, Ctx, PlanReq, RangeReq, RevisionReq};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::atomic::AtomicBool;

const DEV: &str = "dev-test";
const BERLIN: &str = "Europe/Berlin";
/// Wed 21 Oct 2026, 10:00 Berlin (CEST).
const WED: &str = "2026-10-21T08:00:00Z";
/// A week later: the delayed fetch / processing time, which must not be the anchor.
const FETCHED: &str = "2026-10-28T09:00:00Z";

fn ctx(now: &str) -> Ctx<'_> {
    Ctx {
        device_id: DEV,
        now,
    }
}

/// Labeled mocked inference: returns a fixed model answer.
struct Fixed(String);
impl Infer for Fixed {
    fn infer(&self, _m: &Value, _c: &AtomicBool) -> Result<String, IpcError> {
        Ok(self.0.clone())
    }
}

fn answer(items: Value) -> Fixed {
    Fixed(json!({"routine": false, "items": items}).to_string())
}

fn add_source(conn: &Connection, subject: &str, body: &str, received_at: &str) -> String {
    let acc: String = conn
        .query_row(
            "SELECT id FROM connector_accounts WHERE kind='email'",
            [],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| {
            rei_os_lib::connectors::create_account(
                conn,
                rei_os_lib::connectors::Kind::Email,
                "hiz-imap",
                "HIZ (synthetic)",
                received_at,
            )
            .unwrap()
            .id
        });
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO source_messages(id, account_id, kind, provider_object_id, sender, subject, body_text, received_at, label, ingested_at)
         VALUES (?1, ?2, 'email', ?1, 'prof@example.invalid', ?3, ?4, ?5, 'SYNTHETIC_FIXTURE', ?6)",
        params![id, acc, subject, body, received_at, FETCHED],
    )
    .unwrap();
    id
}

fn approve_req(c: &ib::Candidate) -> ApproveReq {
    ApproveReq {
        id: c.id.clone(),
        payload_hash: c.payload_hash.clone(),
        title: None,
        due_date: None,
        date_resolved: None,
        due_time: None,
    }
}

fn r(
    text: &str,
    anchor: &str,
) -> (
    Option<String>,
    Option<String>,
    &'static str,
    Option<&'static str>,
) {
    let x = resolve(text, Some(anchor), BERLIN, false, None);
    (x.due_date, x.due_time, x.status, x.basis)
}

fn ok(
    d: &str,
    t: Option<&str>,
) -> (
    Option<String>,
    Option<String>,
    &'static str,
    Option<&'static str>,
) {
    (
        Some(d.into()),
        t.map(String::from),
        "explicit",
        Some("relative"),
    )
}

fn unclear(text: &str, anchor: &str) -> bool {
    let x = resolve(text, Some(anchor), BERLIN, false, None);
    x.status == "ambiguous" && x.due_date.is_none() && !x.note.is_empty()
}

// ---------------------------------------------------------------- resolver rules

#[test]
fn relative_german_and_english_deadlines_resolve_from_the_source_day() {
    assert_eq!(
        r("bis Freitag um 12 Uhr", WED),
        ok("2026-10-23", Some("12:00"))
    );
    assert_eq!(
        r("morgen", WED),
        ok("2026-10-22", None),
        "no time stays date-only"
    );
    assert_eq!(r("übermorgen, 14:30", WED), ok("2026-10-23", Some("14:30")));
    assert_eq!(r("heute bis 18 Uhr", WED), ok("2026-10-21", Some("18:00")));
    assert_eq!(r("tomorrow at 5 pm", WED), ok("2026-10-22", Some("17:00")));
    assert_eq!(r("by Monday 9 a.m.", WED), ok("2026-10-26", Some("09:00")));
    assert_eq!(r("the day after tomorrow", WED), ok("2026-10-23", None));
    assert_eq!(r("kommenden Dienstag", WED), ok("2026-10-27", None));
    assert_eq!(r("Freitag nächste Woche", WED), ok("2026-10-30", None));
    assert_eq!(r("next week Monday", WED), ok("2026-10-26", None));
    assert_eq!(
        r("diesen Donnerstag 23:59 Uhr", WED),
        ok("2026-10-22", Some("23:59"))
    );
    // "12 am Freitag" is German "at 12 on Friday" phrasing without Uhr: no clock time guessed.
    assert_eq!(r("Freitag", WED), ok("2026-10-23", None));
    // Written dates keep their own basis and may carry a time.
    let w = resolve(
        "bis zum 14.11.2026, 12 Uhr",
        Some(WED),
        BERLIN,
        false,
        Some("2026-11-14"),
    );
    assert_eq!(
        (
            w.due_date.as_deref(),
            w.due_time.as_deref(),
            w.status,
            w.basis
        ),
        (
            Some("2026-11-14"),
            Some("12:00"),
            "explicit",
            Some("written")
        )
    );
    // Weekday + matching written date is consistent.
    let c = resolve("Freitag, 23.10.2026", Some(WED), BERLIN, false, None);
    assert_eq!(
        (c.due_date.as_deref(), c.status),
        (Some("2026-10-23"), "explicit")
    );
}

#[test]
fn genuine_ambiguity_stays_visible_with_a_reason() {
    for t in [
        "nächsten Freitag",    // this or next week
        "next Friday",         // same in English
        "diesen Montag",       // already passed this week
        "Freitag oder Montag", // two dates
        "10-12 Uhr morgen",    // a range, not one time
        "morgen um 10 oder 12 Uhr",
        "freitags",               // recurring
        "bis 23.10.",             // no year
        "Ende der Woche",         // not one clear date
        "am Morgen",              // in the morning, not tomorrow
        "Donnerstag, 30.10.2026", // weekday and written date disagree (30 Oct 2026 is a Friday)
        "bis 25 Uhr morgen",      // invalid clock time
    ] {
        assert!(unclear(t, WED), "{t} must stay ambiguous");
    }
    // Forwarded text: the forward's timestamp is not the original anchor.
    assert!(dates::looks_forwarded(
        "Subject: WG: Abgabe\n---------- Weitergeleitete Nachricht ----------"
    ));
    let f = resolve("bis Freitag", Some(WED), BERLIN, true, None);
    assert_eq!((f.status, f.due_date), ("ambiguous", None));
    // Invalid anchor or zone refuses relative interpretation.
    assert_eq!(
        resolve("morgen", Some("not-a-time"), BERLIN, false, None).status,
        "ambiguous"
    );
    assert_eq!(
        resolve("morgen", None, BERLIN, false, None).status,
        "ambiguous"
    );
    assert_eq!(
        resolve("morgen", Some(WED), "Mars/Base", false, None).status,
        "ambiguous"
    );
    // Model arithmetic is never authority for a written date.
    assert_eq!(
        resolve("14.11.2026", Some(WED), BERLIN, false, Some("2026-11-15")).status,
        "ambiguous"
    );
}

#[test]
fn same_weekday_rule_is_cautious_and_documented() {
    // Fri 23 Oct 2026 09:00 Berlin, deadline 12:00 still ahead -> that same Friday.
    assert_eq!(
        r("bis Freitag um 12 Uhr", "2026-10-23T07:00:00Z"),
        ok("2026-10-23", Some("12:00"))
    );
    // Fri 13:00 Berlin, after the stated time -> not silently rolled to next week.
    assert!(unclear("bis Freitag um 12 Uhr", "2026-10-23T11:00:00Z"));
    // Same weekday without a time is unclear too.
    assert!(unclear("bis Freitag", "2026-10-23T07:00:00Z"));
}

#[test]
fn zone_year_month_and_dst_boundaries() {
    // 22:30Z on Wed is already Thu 00:30 in Berlin: "morgen" = Fri, not Thu.
    assert_eq!(r("morgen", "2026-10-21T22:30:00Z"), ok("2026-10-23", None));
    // Year / month boundaries.
    assert_eq!(r("morgen", "2026-12-31T10:00:00Z"), ok("2027-01-01", None));
    assert_eq!(r("Montag", "2026-12-30T10:00:00Z"), ok("2027-01-04", None));
    assert_eq!(
        r("übermorgen", "2026-02-27T10:00:00Z"),
        ok("2026-03-01", None)
    );
    // DST gap (29 Mar 2026 02:30 does not exist) and fold (25 Oct 2026 02:30 occurs twice).
    assert!(unclear("morgen um 2:30 Uhr", "2026-03-28T10:00:00Z"));
    assert!(unclear("morgen um 02:30 Uhr", "2026-10-24T10:00:00Z"));
    // Across a DST change the wall time is kept and converted with the right offset.
    assert_eq!(
        dates::to_utc("2026-10-26", "12:00", BERLIN).unwrap(),
        "2026-10-26T11:00:00Z"
    );
    assert_eq!(
        dates::to_utc("2026-10-23", "12:00", BERLIN).unwrap(),
        "2026-10-23T10:00:00Z"
    );
    assert!(dates::to_utc("2026-03-29", "02:30", BERLIN).is_err());
}

// ---------------------------------------------------------------- end to end

#[test]
fn delayed_fetch_relative_deadline_flows_to_an_exact_timed_task() {
    let mut conn = db::open_in_memory().unwrap();
    let sid = add_source(
        &conn,
        "Projektbericht",
        "Liebe Studierende,\nbitte laden Sie den Projektbericht bis Freitag um 12 Uhr hoch.\nGrüße",
        WED,
    );
    // Analysed a week after it was sent: the anchor is the send time, not FETCHED.
    let res = ib::analyze_with(&mut conn, &ctx(FETCHED), &sid, "m", &answer(json!([
        {"kind":"deadline","title":"Upload project report","excerpt":"bitte laden Sie den Projektbericht bis Freitag um 12 Uhr hoch.",
         "date_text":"bis Freitag um 12 Uhr","due_date":"2026-10-30","confidence":0.9}
    ]))).unwrap();
    let c = &res.candidates[0];
    assert_eq!(
        (
            c.due_date.as_deref(),
            c.due_time.as_deref(),
            c.date_status.as_str(),
            c.date_basis.as_deref()
        ),
        (
            Some("2026-10-23"),
            Some("12:00"),
            "explicit",
            Some("relative")
        ),
        "model's wrong date ignored; deterministic from the source timestamp"
    );
    assert_eq!(
        (c.date_anchor.as_deref(), c.date_zone.as_deref()),
        (Some(WED), Some(BERLIN))
    );
    assert!(c.date_note.contains("send date"));

    // The hash binds the time: a changed stored time refuses approval.
    conn.execute(
        "UPDATE source_candidates SET due_time='13:00' WHERE id=?1",
        [&c.id],
    )
    .unwrap();
    let e = ib::approve(&mut conn, &ctx(FETCHED), approve_req(c)).unwrap_err();
    assert_eq!(e.code, "ApprovalInvalid");
    conn.execute(
        "UPDATE source_candidates SET due_time='12:00' WHERE id=?1",
        [&c.id],
    )
    .unwrap();
    // The zone is bound too.
    conn.execute(
        "UPDATE source_candidates SET date_zone='UTC' WHERE id=?1",
        [&c.id],
    )
    .unwrap();
    assert_eq!(
        ib::approve(&mut conn, &ctx(FETCHED), approve_req(c))
            .unwrap_err()
            .code,
        "ApprovalInvalid"
    );
    conn.execute(
        "UPDATE source_candidates SET date_zone=?2 WHERE id=?1",
        params![c.id, BERLIN],
    )
    .unwrap();

    let a = ib::approve(&mut conn, &ctx(FETCHED), approve_req(c)).unwrap();
    let t = pl::task_get(
        &conn,
        pl::IdReq {
            id: a.task_id.clone(),
        },
    )
    .unwrap();
    assert_eq!(
        (
            t.due_date.as_deref(),
            t.due_at_utc.as_deref(),
            t.due_tz.as_deref()
        ),
        (None, Some("2026-10-23T10:00:00Z"), Some(BERLIN))
    );
    assert_eq!(
        t.due_at_local.as_deref().map(|s| &s[..16]),
        Some("2026-10-23T12:00")
    );
    assert_eq!(t.plan_date, None, "a deadline is not a work time");
    // Replay of the exact approval creates nothing new.
    assert!(
        ib::approve(&mut conn, &ctx(FETCHED), approve_req(c))
            .unwrap()
            .replayed
    );
}

#[test]
fn citation_rules_excerpt_fallback_and_owner_time() {
    let mut conn = db::open_in_memory().unwrap();
    let sid = add_source(
        &conn,
        "Übung",
        "Abgabe bis 14.11.2026. Die Vorlesung am 12.11.2026 entfällt. Klausur morgen.",
        WED,
    );
    let res = ib::analyze_with(&mut conn, &ctx(WED), &sid, "m", &answer(json!([
        {"kind":"deadline","title":"Submit sheet","excerpt":"Abgabe bis 14.11.2026.","date_text":null,"due_date":null,"confidence":0.8},
        {"kind":"update","title":"Lecture cancelled","excerpt":"Die Vorlesung am 12.11.2026 entfällt.","date_text":null,"due_date":null,"confidence":0.8},
        {"kind":"task","title":"Exam","excerpt":"Klausur morgen.","date_text":"übermorgen","due_date":null,"confidence":0.8}
    ]))).unwrap();
    let by = |title: &str| {
        res.candidates
            .iter()
            .find(|c| c.title == title)
            .unwrap()
            .clone()
    };
    let sheet = by("Submit sheet");
    assert_eq!(
        (sheet.due_date.as_deref(), sheet.date_basis.as_deref()),
        (Some("2026-11-14"), Some("written"))
    );
    assert_eq!(
        (
            by("Lecture cancelled").date_status.as_str(),
            by("Lecture cancelled").due_date
        ),
        ("none", None),
        "an unrelated lecture date is not a deadline"
    );
    // Date text that is not literally in the message is dropped, never resolved.
    let exam = by("Exam");
    assert_eq!(
        (exam.date_status.as_str(), exam.due_date.as_deref()),
        ("none", None)
    );

    // Owner resolves an ambiguous one with a date and a clock time.
    let sid2 = add_source(&conn, "X", "Bitte nächsten Freitag um 10 Uhr abgeben.", WED);
    let r2 = ib::analyze_with(&mut conn, &ctx(WED), &sid2, "m", &answer(json!([
        {"kind":"deadline","title":"Hand in","excerpt":"Bitte nächsten Freitag um 10 Uhr abgeben.","date_text":"nächsten Freitag um 10 Uhr","due_date":null,"confidence":0.7}
    ]))).unwrap();
    let c = &r2.candidates[0];
    assert_eq!(c.date_status, "ambiguous");
    let mut q = approve_req(c);
    q.due_time = Some("10:00".into());
    assert_eq!(
        ib::approve(&mut conn, &ctx(WED), q).unwrap_err().code,
        "Validation"
    );
    let mut q = approve_req(c);
    q.date_resolved = Some(true);
    q.due_date = Some("2026-03-29".into());
    q.due_time = Some("02:30".into());
    assert_eq!(
        ib::approve(&mut conn, &ctx(WED), q).unwrap_err().code,
        "Validation",
        "DST gap refused"
    );
    let mut q = approve_req(c);
    q.date_resolved = Some(true);
    q.due_date = Some("2026-10-30".into());
    q.due_time = Some("10:00".into());
    let a = ib::approve(&mut conn, &ctx(WED), q).unwrap();
    let t = pl::task_get(&conn, pl::IdReq { id: a.task_id }).unwrap();
    assert_eq!(
        t.due_at_utc.as_deref(),
        Some("2026-10-30T09:00:00Z"),
        "CET after the DST change"
    );
}

fn legacy_hash(c: &ib::Candidate, due: Option<&str>, status: &str, expires: &str) -> String {
    let v = json!({
        "id": c.id, "source_id": c.source_id, "source_hash": c.source_hash, "kind": c.kind, "title": c.title,
        "due_date": due, "date_status": status, "excerpt": c.excerpt, "expires_at": expires,
    });
    let mut h = Sha256::new();
    h.update(v.to_string().as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Turns a stored candidate into exactly what the previous resolver stored.
fn make_legacy(conn: &Connection, c: &ib::Candidate, due: Option<&str>, status: &str) {
    let h = legacy_hash(c, due, status, &c.expires_at);
    conn.execute(
        "UPDATE source_candidates SET due_date=?2, date_status=?3, due_time=NULL, date_basis=NULL, date_anchor=NULL,
           date_zone=NULL, date_note='', resolver_version=1, payload_hash=?4 WHERE id=?1",
        params![c.id, due, status, h],
    )
    .unwrap();
}

#[test]
fn legacy_full_date_reviews_stay_valid() {
    let mut conn = db::open_in_memory().unwrap();
    let sid = add_source(&conn, "Blatt", "Abgabe bis zum 14.11.2026 im Moodle.", WED);
    let res = ib::analyze_with(&mut conn, &ctx(WED), &sid, "m", &answer(json!([
        {"kind":"deadline","title":"Sheet","excerpt":"Abgabe bis zum 14.11.2026 im Moodle.","date_text":"14.11.2026","due_date":"2026-11-14","confidence":0.8}
    ]))).unwrap();
    let c = res.candidates[0].clone();
    make_legacy(&conn, &c, Some("2026-11-14"), "explicit");
    let mut req = approve_req(&c);
    req.payload_hash = legacy_hash(&c, Some("2026-11-14"), "explicit", &c.expires_at);
    let a = ib::approve(&mut conn, &ctx(WED), req).unwrap();
    let t = pl::task_get(&conn, pl::IdReq { id: a.task_id }).unwrap();
    assert_eq!(t.due_date.as_deref(), Some("2026-11-14"));
}

#[test]
fn bounded_upgrade_resolves_eligible_pending_results_once_and_keeps_decisions() {
    let path = std::env::temp_dir().join(format!("rei-upd-{}.sqlite", uuid::Uuid::new_v4()));
    let ids;
    {
        let mut conn = db::open(&path).unwrap();
        let mut made = vec![];
        for (subject, title) in [
            ("A", "Pending"),
            ("B", "Approved"),
            ("C", "Rejected"),
            ("D", "Unclear"),
        ] {
            let body = if title == "Unclear" {
                format!("{subject}: Abgabe nächsten Freitag.")
            } else {
                format!("{subject}: Abgabe bis Freitag um 12 Uhr.")
            };
            let dt = if title == "Unclear" {
                "nächsten Freitag"
            } else {
                "bis Freitag um 12 Uhr"
            };
            let sid = add_source(&conn, subject, &body, WED);
            let res = ib::analyze_with(&mut conn, &ctx(WED), &sid, "m", &answer(json!([
                {"kind":"deadline","title":title,"excerpt":body,"date_text":dt,"due_date":null,"confidence":0.8}
            ]))).unwrap();
            let c = res.candidates[0].clone();
            make_legacy(&conn, &c, None, "ambiguous");
            made.push(c);
        }
        // Owner decisions made on the legacy (unresolved) versions.
        let mut q = approve_req(&made[1]);
        q.payload_hash = legacy_hash(&made[1], None, "ambiguous", &made[1].expires_at);
        q.date_resolved = Some(true);
        q.due_date = Some("2026-11-02".into());
        ib::approve(&mut conn, &ctx(WED), q).unwrap();
        ib::reject(
            &mut conn,
            &ctx(WED),
            RejectReq {
                id: made[2].id.clone(),
                payload_hash: legacy_hash(&made[2], None, "ambiguous", &made[2].expires_at),
            },
        )
        .unwrap();
        ids = made.iter().map(|c| c.id.clone()).collect::<Vec<_>>();
    }
    let count = |c: &Connection| -> i64 {
        c.query_row("SELECT count(*) FROM source_candidates", [], |r| r.get(0))
            .unwrap()
    };
    // Restart: the upgrade runs against the reopened database.
    let mut conn = db::open(&path).unwrap();
    let before = count(&conn);
    let u = ib::upgrade_pending_dates(&mut conn, 50).unwrap();
    assert_eq!(
        (u.checked, u.resolved),
        (2, 1),
        "only pending legacy rows are checked"
    );
    let get = |c: &Connection, id: &str| -> (Option<String>, Option<String>, String, String) {
        c.query_row(
            "SELECT due_date, due_time, date_status, status FROM source_candidates WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap()
    };
    assert_eq!(
        get(&conn, &ids[0]),
        (
            Some("2026-10-23".into()),
            Some("12:00".into()),
            "explicit".into(),
            "pending".into()
        )
    );
    assert_eq!(get(&conn, &ids[1]).3, "approved");
    assert_eq!(get(&conn, &ids[1]).0, None, "approved decision untouched");
    assert_eq!(get(&conn, &ids[2]).3, "rejected", "rejection not revived");
    assert_eq!(
        get(&conn, &ids[3]).2,
        "ambiguous",
        "genuine ambiguity stays"
    );
    let approved_task_due: String = conn
        .query_row("SELECT t.due_date FROM tasks t JOIN source_candidates c ON c.task_id=t.id WHERE c.id=?1", [&ids[1]], |r| r.get(0))
        .unwrap();
    assert_eq!(approved_task_due, "2026-11-02", "owner-edited date kept");
    // Idempotent: second run checks nothing, no duplicates.
    assert_eq!(
        ib::upgrade_pending_dates(&mut conn, 50).unwrap(),
        ib::UpgradeResult {
            checked: 0,
            resolved: 0
        }
    );
    assert_eq!(count(&conn), before);
    // Upgraded result is approvable with its new exact hash.
    let c = ib::candidate(&conn, WED, &ids[0]).unwrap();
    let a = ib::approve(&mut conn, &ctx(WED), approve_req(&c)).unwrap();
    assert!(!a.replayed);
    drop(conn);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn upgrade_skips_changed_sources_and_is_bounded() {
    let mut conn = db::open_in_memory().unwrap();
    let mut last = None;
    for i in 0..5 {
        let body = format!("Abgabe {i} morgen.");
        let sid = add_source(&conn, "S", &body, WED);
        let res = ib::analyze_with(&mut conn, &ctx(WED), &sid, "m", &answer(json!([
            {"kind":"deadline","title":format!("T{i}"),"excerpt":body,"date_text":"morgen","due_date":null,"confidence":0.8}
        ]))).unwrap();
        make_legacy(&conn, &res.candidates[0], None, "ambiguous");
        last = Some(sid);
    }
    conn.execute(
        "UPDATE source_messages SET body_text='changed' WHERE id=?1",
        [last.unwrap()],
    )
    .unwrap();
    let u = ib::upgrade_pending_dates(&mut conn, 2).unwrap();
    assert_eq!(u.checked, 2, "bounded per round");
    let u2 = ib::upgrade_pending_dates(&mut conn, 50).unwrap();
    assert_eq!(
        (u2.checked, u.resolved + u2.resolved),
        (3, 4),
        "the changed source is not upgraded"
    );
}

// ---------------------------------------------------------------- work plans

fn new_task(conn: &mut Connection, title: &str) -> pl::Task {
    pl::task_create(
        conn,
        &ctx(WED),
        serde_json::from_value(json!({"title": title})).unwrap(),
    )
    .unwrap()
}

fn plan(id: &str, rev: u64, date: &str, time: Option<&str>) -> PlanReq {
    PlanReq {
        id: id.into(),
        expected_revision: rev,
        plan_date: date.into(),
        plan_time: time.map(String::from),
        duration_min: Some(60),
        tz: time.map(|_| BERLIN.to_string()),
    }
}

fn rev(id: &str, r: u64) -> RevisionReq {
    RevisionReq {
        id: id.into(),
        expected_revision: r,
    }
}

#[test]
fn work_plan_is_separate_from_deadline_and_unplan_undo_restore_real_state() {
    let path = std::env::temp_dir().join(format!("rei-plan-{}.sqlite", uuid::Uuid::new_v4()));
    let mut conn = db::open(&path).unwrap();
    let sid = add_source(&conn, "P", "Bericht bis Freitag um 12 Uhr hochladen.", WED);
    let res = ib::analyze_with(&mut conn, &ctx(WED), &sid, "m", &answer(json!([
        {"kind":"deadline","title":"Report","excerpt":"Bericht bis Freitag um 12 Uhr hochladen.","date_text":"bis Freitag um 12 Uhr","due_date":null,"confidence":0.9}
    ]))).unwrap();
    let a = ib::approve(&mut conn, &ctx(WED), approve_req(&res.candidates[0])).unwrap();
    let t = pl::task_get(
        &conn,
        pl::IdReq {
            id: a.task_id.clone(),
        },
    )
    .unwrap();
    // Due Friday, planned Wednesday 14:00 — both coexist.
    let t = pl::task_plan(
        &mut conn,
        &ctx(WED),
        plan(&t.id, t.revision, "2026-10-21", Some("14:00")),
    )
    .unwrap();
    assert_eq!(
        (
            t.plan_date.as_deref(),
            t.plan_time.as_deref(),
            t.plan_duration_min
        ),
        (Some("2026-10-21"), Some("14:00"), Some(60))
    );
    assert_eq!(
        t.due_at_utc.as_deref(),
        Some("2026-10-23T10:00:00Z"),
        "deadline unchanged"
    );
    assert_eq!(t.undo_action.as_deref(), Some("plan"));
    let week = RangeReq {
        from: "2026-10-19".into(),
        to: "2026-10-25".into(),
    };
    let ag = pl::agenda_list(&conn, RangeReq { ..week }).unwrap();
    let kinds: Vec<(&str, &str)> = ag
        .iter()
        .filter(|i| i.id == t.id)
        .map(|i| (i.kind.as_str(), &i.local_start[..10]))
        .collect();
    assert!(
        kinds.contains(&("plan", "2026-10-21")) && kinds.contains(&("task", "2026-10-23")),
        "{kinds:?}"
    );
    // Stale revision refused.
    assert_eq!(
        pl::task_unplan(&mut conn, &ctx(WED), rev(&t.id, t.revision - 1))
            .unwrap_err()
            .code,
        "Conflict"
    );
    // Unplan keeps task/deadline/source; it returns to unscheduled.
    let u = pl::task_unplan(&mut conn, &ctx(WED), rev(&t.id, t.revision)).unwrap();
    assert_eq!(
        (
            u.plan_date.as_deref(),
            u.due_at_utc.as_deref(),
            u.status.as_str()
        ),
        (None, Some("2026-10-23T10:00:00Z"), "todo")
    );
    let ag = pl::agenda_list(
        &conn,
        RangeReq {
            from: "2026-10-19".into(),
            to: "2026-10-25".into(),
        },
    )
    .unwrap();
    assert!(!ag.iter().any(|i| i.kind == "plan"));
    let prov: String = conn
        .query_row(
            "SELECT provenance_json FROM tasks WHERE id=?1",
            [&t.id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(prov.contains(&sid), "source link kept");
    // Undo restores the plan in the database.
    let back = pl::task_undo(&mut conn, &ctx(WED), rev(&u.id, u.revision)).unwrap();
    assert_eq!(back.plan_time.as_deref(), Some("14:00"));
    // Undo is single use.
    assert!(pl::task_undo(&mut conn, &ctx(WED), rev(&back.id, back.revision)).is_err());
    // Complete -> undo -> todo; complete -> reopen persists across reopen.
    let done = pl::task_complete(&mut conn, &ctx(WED), rev(&back.id, back.revision)).unwrap();
    assert_eq!(done.undo_action.as_deref(), Some("complete"));
    let undone = pl::task_undo(&mut conn, &ctx(WED), rev(&done.id, done.revision)).unwrap();
    assert_eq!(undone.status, "todo");
    let done = pl::task_complete(&mut conn, &ctx(WED), rev(&undone.id, undone.revision)).unwrap();
    // The undo window is bounded.
    let late = "2026-10-21T08:30:00Z";
    assert!(
        pl::task_undo(&mut conn, &ctx(late), rev(&done.id, done.revision))
            .unwrap_err()
            .message
            .contains("window")
    );
    let re = pl::task_reopen(&mut conn, &ctx(late), rev(&done.id, done.revision)).unwrap();
    assert_eq!(re.status, "todo");
    drop(conn);
    let mut conn = db::open(&path).unwrap();
    let t = pl::task_get(&conn, pl::IdReq { id: re.id.clone() }).unwrap();
    assert_eq!(
        (t.status.as_str(), t.plan_date.as_deref(), t.revision),
        ("todo", Some("2026-10-21"), re.revision)
    );
    // Delete -> undo brings it back; a concurrent edit blocks undo.
    pl::task_delete(
        &mut conn,
        &ctx(WED),
        pl::DeleteReq {
            id: t.id.clone(),
            expected_revision: t.revision,
            confirm: true,
        },
    )
    .unwrap();
    assert!(pl::task_get(&conn, pl::IdReq { id: t.id.clone() }).is_err());
    assert_eq!(
        pl::task_undo(&mut conn, &ctx(WED), rev(&t.id, t.revision))
            .unwrap_err()
            .code,
        "Conflict"
    );
    let r = pl::task_undo(&mut conn, &ctx(WED), rev(&t.id, t.revision + 1)).unwrap();
    assert_eq!(r.title, "Report");
    drop(conn);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn plan_validation_and_recurring_scope_are_explicit() {
    let mut conn = db::open_in_memory().unwrap();
    let t = new_task(&mut conn, "Read chapter");
    let bad =
        |conn: &mut Connection, p: PlanReq| pl::task_plan(conn, &ctx(WED), p).unwrap_err().code;
    assert_eq!(
        bad(&mut conn, plan(&t.id, t.revision, "21.10.2026", None)),
        "Validation"
    );
    assert_eq!(
        bad(
            &mut conn,
            plan(&t.id, t.revision, "2026-03-29", Some("02:30"))
        ),
        "Validation",
        "DST gap"
    );
    let mut p = plan(&t.id, t.revision, "2026-10-21", Some("09:00"));
    p.tz = None;
    assert_eq!(bad(&mut conn, p), "Validation");
    let mut p = plan(&t.id, t.revision, "2026-10-21", None);
    p.duration_min = Some(2);
    assert_eq!(bad(&mut conn, p), "Validation");
    // Date-only plan; unplan of an unplanned task is refused.
    let t2 = pl::task_plan(
        &mut conn,
        &ctx(WED),
        plan(&t.id, t.revision, "2026-10-22", None),
    )
    .unwrap();
    assert_eq!(
        (
            t2.plan_date.as_deref(),
            t2.plan_time.as_deref(),
            t2.due_date.as_deref()
        ),
        (Some("2026-10-22"), None, None)
    );
    let t3 = pl::task_unplan(&mut conn, &ctx(WED), rev(&t2.id, t2.revision)).unwrap();
    assert!(pl::task_unplan(&mut conn, &ctx(WED), rev(&t3.id, t3.revision)).is_err());
    // Recurring series: planning the whole series is refused, not silently applied.
    let rt = pl::task_create(&mut conn, &ctx(WED), serde_json::from_value(json!({
        "title": "Weekly review",
        "recurrence": {"rrule": "FREQ=WEEKLY", "tz": BERLIN, "dtstart_local": "2026-10-19T09:00"}
    })).unwrap());
    if let Ok(rt) = rt {
        let e = pl::task_plan(
            &mut conn,
            &ctx(WED),
            plan(&rt.id, rt.revision, "2026-10-21", None),
        )
        .unwrap_err();
        assert!(e.message.contains("recurring"), "{}", e.message);
    } else {
        panic!("recurring fixture could not be created: {:?}", rt.err());
    }
}

// ---------------------------------------------------------------- settings / model

#[test]
fn configured_zone_drives_resolution_and_is_validated() {
    let mut conn = db::open_in_memory().unwrap();
    let set = |c: &Connection, z: Option<&str>| {
        aa::set_settings(
            c,
            WED,
            &AutoSetReq {
                enabled: true,
                paused: false,
                model: "qwen3:8b".into(),
                date_zone: z.map(String::from),
            },
        )
    };
    assert_eq!(
        set(&conn, Some("Mars/Base")).unwrap_err().code,
        "Validation"
    );
    assert_eq!(
        aa::get_settings(&conn).unwrap().date_zone,
        BERLIN,
        "default owner zone"
    );
    set(&conn, Some("America/New_York")).unwrap();
    // 2026-10-22T02:00Z is still Wed 21 Oct in New York -> "morgen" = Thu 22.
    let sid = add_source(&conn, "Z", "Bitte morgen abgeben.", "2026-10-22T02:00:00Z");
    let res = ib::analyze_with(&mut conn, &ctx(FETCHED), &sid, "m", &answer(json!([
        {"kind":"deadline","title":"Z","excerpt":"Bitte morgen abgeben.","date_text":"morgen","due_date":null,"confidence":0.8}
    ]))).unwrap();
    assert_eq!(
        (
            res.candidates[0].due_date.as_deref(),
            res.candidates[0].date_zone.as_deref()
        ),
        (Some("2026-10-22"), Some("America/New_York"))
    );
    // Omitting the zone keeps it; the model setting persists for the worker.
    set(&conn, None).unwrap();
    let s = aa::get_settings(&conn).unwrap();
    assert_eq!(
        (s.date_zone.as_str(), s.model.as_str()),
        ("America/New_York", "qwen3:8b")
    );
}

#[test]
fn installed_model_listing_is_bounded_and_filtered() {
    let body = br#"{"models":[{"name":"qwen3:8b"},{"name":"llama3.2:latest"},{"name":"bad name; rm"},{"name":"qwen3:8b"},{"name":""}]}"#;
    assert_eq!(
        ollama::model_names(body),
        vec!["llama3.2:latest".to_string(), "qwen3:8b".to_string()]
    );
    assert!(ollama::model_names(b"not json").is_empty());
    let many =
        json!({"models": (0..500).map(|i| json!({"name": format!("m{i}")})).collect::<Vec<_>>()});
    assert_eq!(ollama::model_names(many.to_string().as_bytes()).len(), 100);
}

// ---------------------------------------------------------------- fresh-review regressions

#[test]
fn weekday_beside_a_written_date_is_checked_against_that_date_not_re_anchored() {
    // Sent Fri 16 Oct 2026 for a deadline two weeks later: not "sent on that weekday" and
    // not the upcoming Friday; the written date decides and the weekday must agree with it.
    let fri = "2026-10-16T08:00:00Z";
    let x = resolve(
        "bis Freitag, 30.10.2026 um 12 Uhr",
        Some(fri),
        BERLIN,
        false,
        None,
    );
    assert_eq!(
        (
            x.due_date.as_deref(),
            x.due_time.as_deref(),
            x.status,
            x.basis
        ),
        (
            Some("2026-10-30"),
            Some("12:00"),
            "explicit",
            Some("written")
        )
    );
    // A forwarded message with a full written date needs no anchor.
    let f = resolve("Friday, 30.10.2026", None, BERLIN, true, None);
    assert_eq!(f.due_date.as_deref(), Some("2026-10-30"));
    // A contradicting weekday stays unclear instead of picking either.
    assert!(unclear("bis Donnerstag, 30.10.2026", WED));
    // Relative day words beside a written date are still checked against the send day.
    assert!(unclear("morgen, 30.10.2026", WED));
}

#[test]
fn english_by_alone_does_not_turn_an_unrelated_mention_into_a_deadline() {
    let text = "Guest lecture by Prof. Example on Monday. Please submit by Friday 12:00.";
    let raw = json!({"routine": false, "items": [
        {"kind":"update","title":"Guest lecture","excerpt":"Guest lecture by Prof. Example on Monday.","date_text":null,"due_date":null,"confidence":0.7},
        {"kind":"deadline","title":"Submit","excerpt":"Please submit by Friday 12:00.","date_text":null,"due_date":null,"confidence":0.7}
    ]}).to_string();
    let v = ib::validate_output_at(&raw, text, Some(WED), BERLIN).unwrap();
    assert_eq!(
        (v.items[0].date_status, v.items[0].due_date.as_deref()),
        ("none", None)
    );
    assert_eq!(
        (
            v.items[1].due_date.as_deref(),
            v.items[1].due_time.as_deref()
        ),
        (Some("2026-10-23"), Some("12:00"))
    );
}

#[test]
fn owner_changing_only_the_date_can_keep_the_stated_time_on_the_task() {
    let mut conn = db::open_in_memory().unwrap();
    let sid = add_source(&conn, "Blatt 3", "Abgabe bis Freitag um 12 Uhr.", WED);
    let res = ib::analyze_with(&mut conn, &ctx(FETCHED), &sid, "m", &answer(json!([
        {"kind":"deadline","title":"Submit sheet 3","excerpt":"Abgabe bis Freitag um 12 Uhr.","date_text":"bis Freitag um 12 Uhr","due_date":null,"confidence":0.9}
    ]))).unwrap();
    let c = res.candidates[0].clone();
    assert_eq!(c.due_time.as_deref(), Some("12:00"));
    let mut req = approve_req(&c);
    req.date_resolved = Some(true);
    req.due_date = Some("2026-10-30".into());
    req.due_time = c.due_time.clone();
    let out = ib::approve(&mut conn, &ctx(FETCHED), req).unwrap();
    let t = pl::task_get(&conn, pl::IdReq { id: out.task_id }).unwrap();
    // 12:00 CET on 30 Oct 2026 (after the DST change) = 11:00 UTC; never midnight.
    assert_eq!(t.due_at_utc.as_deref(), Some("2026-10-30T11:00:00Z"));
    assert_eq!(t.due_tz.as_deref(), Some(BERLIN));
    assert_eq!(t.due_date, None);
}
