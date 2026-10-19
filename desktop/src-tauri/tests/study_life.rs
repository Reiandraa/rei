//! study-life tests (P07/P08/P09) against real SQLite and real temp files. All records are
//! SYNTHETIC. No TeX compiler, AnkiConnect, payment API or offensive tool is exercised.
use rei_os_lib::commands::IpcError;
use rei_os_lib::{db, files, life, projects as pj, study};
use rusqlite::Connection;
use serde_json::{json, Value};

const NOW: &str = "2026-10-19T08:00:00Z";

fn ctx(now: &str) -> rei_os_lib::planner::Ctx<'_> {
    rei_os_lib::planner::Ctx {
        device_id: "test-device",
        now,
    }
}

fn r<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).unwrap()
}

fn code(e: IpcError) -> String {
    e.code
}

fn count(c: &Connection, sql: &str) -> i64 {
    c.query_row(sql, [], |r| r.get(0)).unwrap()
}

fn grant(c: &mut Connection, path: &std::path::Path, purpose: &str) -> String {
    files::grant_register(c, &ctx(NOW), path.to_str().unwrap(), purpose).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn study_crud_revisions_and_no_guessed_dates() {
    let mut c = db::open_in_memory().unwrap();
    let hm1 = study::course_create(
        &mut c,
        &ctx(NOW),
        r(json!({"name":"HM1","code":"MA101","term":"WS26"})),
    )
    .unwrap();
    let cid = hm1["id"].as_str().unwrap();
    let item = study::study_item_create(
        &mut c,
        &ctx(NOW),
        r(json!({"course_id":cid,"kind":"exam","title":"Klausur","due_date":"2027-02-10"})),
    )
    .unwrap();
    let undated = study::study_item_create(
        &mut c,
        &ctx(NOW),
        r(json!({"course_id":cid,"kind":"assignment","title":"Blatt 1"})),
    )
    .unwrap();
    assert!(undated["due_date"].is_null(), "no date is invented");
    let list = study::study_item_list(&c, Some(cid)).unwrap();
    assert_eq!(list[0]["title"], "Klausur", "dated items first");

    let iid = item["id"].as_str().unwrap();
    let upd = |c: &mut Connection, rev: u64| {
        study::study_item_update(
            c,
            &ctx(NOW),
            iid,
            rev,
            Some("done"),
            r(json!({"course_id":cid,"kind":"exam","title":"Klausur","due_date":"2027-02-10"})),
        )
    };
    assert_eq!(upd(&mut c, 1).unwrap()["status"], "done");
    assert_eq!(
        code(upd(&mut c, 1).unwrap_err()),
        "Conflict",
        "stale revision rejected"
    );
    for bad in [
        json!({"course_id":cid,"kind":"exam","title":"x","due_date":"2027-13-40"}),
        json!({"course_id":cid,"kind":"party","title":"x"}),
        json!({"course_id":"nope","kind":"note","title":"x"}),
        json!({"course_id":cid,"kind":"note","title":"   "}),
    ] {
        assert!(study::study_item_create(&mut c, &ctx(NOW), r(bad)).is_err());
    }
    assert_eq!(
        code(study::study_item_delete(&mut c, &ctx(NOW), iid, 2, false).unwrap_err()),
        "ApprovalRequired"
    );
    study::study_item_delete(&mut c, &ctx(NOW), iid, 2, true).unwrap();
    assert_eq!(study::study_item_list(&c, Some(cid)).unwrap().len(), 1);
    // Every write produced an outbox op and an audit row.
    assert_eq!(
        count(
            &c,
            "SELECT COUNT(*) FROM outbox WHERE entity_type='study_item'"
        ),
        4
    );
    assert!(
        count(
            &c,
            "SELECT COUNT(*) FROM audit_events WHERE action='study_item.delete'"
        ) == 1
    );
}

#[test]
fn study_logs_language_and_htb() {
    let mut c = db::open_in_memory().unwrap();
    let course = study::course_create(&mut c, &ctx(NOW), r(json!({"name":"LaTeX"}))).unwrap();
    let cid = course["id"].as_str().unwrap();
    for m in [45, 30] {
        study::study_session_create(
            &mut c,
            &ctx(NOW),
            r(json!({"course_id":cid,"topic":"tables","minutes":m,"day":"2026-10-19"})),
        )
        .unwrap();
    }
    study::study_session_create(
        &mut c,
        &ctx(NOW),
        r(json!({"topic":"misc","minutes":10,"day":"2026-10-19"})),
    )
    .unwrap();
    assert!(study::study_session_create(
        &mut c,
        &ctx(NOW),
        r(json!({"topic":"x","minutes":0,"day":"2026-10-19"}))
    )
    .is_err());
    let totals = study::study_minutes_by_course(&c).unwrap();
    assert!(totals.contains(&(Some(cid.to_string()), 75)) && totals.contains(&(None, 10)));

    study::language_log_create(&mut c, &ctx(NOW), r(json!({"language":"Spanish","activity":"Anki + podcast","minutes":25,"day":"2026-10-19"}))).unwrap();
    assert_eq!(study::language_log_list(&c).unwrap().len(), 1);
    assert!(study::language_log_create(
        &mut c,
        &ctx(NOW),
        r(json!({"language":"","activity":"a","minutes":5,"day":"2026-10-19"}))
    )
    .is_err());

    let h = study::htb_create(
        &mut c,
        &ctx(NOW),
        r(json!({"name":"Starting Point Tier 0","kind":"track","status":"planned"})),
    )
    .unwrap();
    let hid = h["id"].as_str().unwrap();
    let h2 = study::htb_update(&mut c, &ctx(NOW), hid, 1, r(json!({"name":"Starting Point Tier 0","kind":"track","status":"in_progress","notes":"authorized lab"}))).unwrap();
    assert_eq!(h2["status"], "in_progress");
    assert!(study::htb_create(
        &mut c,
        &ctx(NOW),
        r(json!({"name":"x","kind":"exploit","status":"planned"}))
    )
    .is_err());
}

#[test]
fn latex_export_neutralises_malicious_tex_and_never_compiles() {
    let mut c = db::open_in_memory().unwrap();
    let course = study::course_create(
        &mut c,
        &ctx(NOW),
        r(json!({"name":"Evil \\input{/etc/passwd} 50% $x$"})),
    )
    .unwrap();
    let cid = course["id"].as_str().unwrap();
    let body = "\\immediate\\write18{calc}\n\\openout3=x \\catcode`\\%=12 {&#_^~}";
    study::study_item_create(
        &mut c,
        &ctx(NOW),
        r(json!({"course_id":cid,"kind":"note","title":"\\input{secret}","body":body})),
    )
    .unwrap();
    let tex = study::latex_export(&c, None).unwrap();
    // Strip every escaped/literal construct the exporter emits; no live backslash command may remain.
    let mut rest = tex.clone();
    for ok in [
        "\\textbackslash{}",
        "\\textasciicircum{}",
        "\\textasciitilde{}",
        "\\{",
        "\\}",
        "\\$",
        "\\&",
        "\\#",
        "\\_",
        "\\%",
    ] {
        rest = rest.replace(ok, "");
    }
    let allowed = [
        "\\documentclass",
        "\\begin",
        "\\title",
        "\\maketitle",
        "\\section*",
        "\\subsection*",
        "\\end",
    ];
    for cmd in rest.split('\\').skip(1) {
        assert!(
            allowed.iter().any(|a| format!("\\{cmd}").starts_with(a)),
            "unexpected control sequence: \\{}",
            &cmd[..cmd.len().min(20)]
        );
    }
    assert!(tex.contains("\\textbackslash{}input\\{secret\\}") && tex.contains("50\\%"));
    assert_eq!(study::latex_escape("a\u{7}b"), "ab");
    assert_eq!(study::latex_escape("a\\b"), "a\\textbackslash{}b");
    // Export is text only; writing needs a granted export path.
    assert_eq!(
        code(study::latex_export_to(&c, None, "no-such-grant").unwrap_err()),
        "ProviderDenied"
    );
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("notes.tex");
    let g = grant(&mut c, &out, "export");
    study::latex_export_to(&c, None, &g).unwrap();
    assert_eq!(std::fs::read_to_string(&out).unwrap(), tex);
}

#[test]
fn anki_roundtrip_and_hostile_import() {
    let mut a = db::open_in_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("in.txt");
    std::fs::write(
        &src,
        "#separator:tab\n#html:false\n#deck column:1\n#tags column:4\nSpanish\tperro\tdog\tnouns\nSpanish\tperro\tdup\t\nSpanish\tgato\tcat\t\nHM1\tonlyfront\n\nHM1\t\tempty front\t\nHM1\t=cmd|calc\tx\ty\n",
    )
    .unwrap();
    let g = grant(&mut a, &src, "import");
    let rep = study::anki_import(&mut a, &ctx(NOW), &g, "Imported").unwrap();
    assert_eq!(
        (rep.imported, rep.duplicates, rep.skipped),
        (4, 1, 1),
        "{rep:?}"
    );
    // Re-import is idempotent.
    let again = study::anki_import(&mut a, &ctx(NOW), &g, "Imported").unwrap();
    assert_eq!((again.imported, again.duplicates), (0, 5));
    // Export then import into a second independent database: same cards.
    let exported = study::anki_export_text(&a).unwrap();
    assert!(exported.starts_with("#separator:tab\n#html:false\n"));
    let mut b = db::open_in_memory().unwrap();
    let rep = study::anki_import_text(&mut b, &ctx(NOW), &exported, "x").unwrap();
    assert_eq!(rep.imported, 4);
    assert_eq!(count(&b, "SELECT COUNT(*) FROM anki_cards WHERE deck='Spanish' AND front='perro' AND tags='nouns'"), 1);
    // Unsupported dialects and oversize input are refused, not guessed.
    assert!(
        study::anki_import_text(&mut b, &ctx(NOW), "#separator:semicolon\na;b\n", "d").is_err()
    );
    assert!(study::anki_import_text(&mut b, &ctx(NOW), "#html:true\na\tb\n", "d").is_err());
    assert!(
        study::anki_import_text(&mut b, &ctx(NOW), &"x".repeat(5 * 1024 * 1024 + 1), "d").is_err()
    );
    // An import grant cannot be used for export and vice-versa.
    assert_eq!(
        code(study::anki_export(&a, &g).unwrap_err()),
        "ProviderDenied"
    );
    // Exported tabs/newlines inside fields cannot add columns or rows.
    let mut c = db::open_in_memory().unwrap();
    study::anki_import_text(&mut c, &ctx(NOW), "q\ta\n", "D").unwrap();
    assert_eq!(study::anki_export_text(&c).unwrap().lines().count(), 5);
}

#[test]
fn money_budgets_shifts_and_currency_separation() {
    let mut c = db::open_in_memory().unwrap();
    let e = |c: &mut Connection, amt: i64, cur: &str, cat: &str, on: &str| {
        life::expense_create(
            c,
            &ctx(NOW),
            r(json!({"amount_minor":amt,"currency":cur,"category":cat,"spent_on":on})),
        )
    };
    e(&mut c, 1999, "EUR", "food", "2026-10-03").unwrap();
    e(&mut c, 1, "EUR", "food", "2026-10-04").unwrap();
    e(&mut c, 500, "USD", "food", "2026-10-05").unwrap();
    e(&mut c, 7777, "EUR", "food", "2026-09-30").unwrap(); // other month
    for bad in [(0, "EUR"), (-5, "EUR"), (100, "eur"), (100, "EURO")] {
        assert!(
            e(&mut c, bad.0, bad.1, "food", "2026-10-05").is_err(),
            "{bad:?}"
        );
    }
    assert!(e(&mut c, 100, "EUR", "food", "2026-02-30").is_err());
    life::budget_set(
        &mut c,
        &ctx(NOW),
        r(json!({"category":"food","currency":"EUR","month":"2026-10","limit_minor":2000})),
    )
    .unwrap();
    let again = life::budget_set(
        &mut c,
        &ctx(NOW),
        r(json!({"category":"food","currency":"EUR","month":"2026-10","limit_minor":1500})),
    )
    .unwrap();
    assert_eq!(again["revision"], 2, "budget_set updates the existing row");
    life::budget_set(
        &mut c,
        &ctx(NOW),
        r(json!({"category":"food","currency":"USD","month":"2026-10","limit_minor":1000})),
    )
    .unwrap();
    let st = life::budget_status(&c, "2026-10").unwrap();
    assert_eq!(st.len(), 2);
    let eur = st.iter().find(|l| l.currency == "EUR").unwrap();
    assert_eq!(
        (eur.spent_minor, eur.remaining_minor, eur.over),
        (2000, -500, true)
    );
    let usd = st.iter().find(|l| l.currency == "USD").unwrap();
    assert_eq!(
        (usd.spent_minor, usd.remaining_minor, usd.over),
        (500, 500, false)
    );

    // Shifts: 7h30 - 30min break at 12.50/h = 87.50 ; rounding half up on odd minutes.
    assert_eq!(life::shift_gross_minor(450, 30, 1250).unwrap(), 8750);
    assert_eq!(life::shift_gross_minor(1, 0, 1250).unwrap(), 21); // 20.83 -> 21
    assert_eq!(life::shift_gross_minor(7, 0, 100).unwrap(), 12); // 11.67 -> 12
    assert!(life::shift_gross_minor(10, 20, 100).is_err());
    life::shift_create(&mut c, &ctx(NOW), r(json!({"employer":"Uni HiWi","shift_date":"2026-10-10","worked_min":450,"break_min":30,"rate_minor_per_hour":1250,"currency":"EUR"}))).unwrap();
    life::income_create(&mut c, &ctx(NOW), r(json!({"amount_minor":93400,"currency":"EUR","source":"BAfoG test","received_on":"2026-10-01"}))).unwrap();
    let s = life::month_summary(&c, "2026-10").unwrap();
    let eur = &s["EUR"];
    assert_eq!(
        (
            eur.income_minor,
            eur.shift_gross_minor,
            eur.expense_minor,
            eur.net_minor
        ),
        (93400, 8750, 2000, 93400 + 8750 - 2000)
    );
    assert_eq!(s["USD"].expense_minor, 500);
    assert_eq!(s["USD"].income_minor, 0, "currencies never merged");
    assert_eq!(life::format_minor(-5), "-0.05");
    assert_eq!(life::format_minor(123456), "1234.56");
}

#[test]
fn csv_export_blocks_formula_injection() {
    let mut c = db::open_in_memory().unwrap();
    life::expense_create(&mut c, &ctx(NOW), r(json!({"amount_minor":250,"currency":"EUR","category":"=HYPERLINK(\"http://x\")","spent_on":"2026-10-03","note":"a,\"b\"\n+SUM(A1)"}))).unwrap();
    let csv = life::expense_csv(&c).unwrap();
    assert!(csv.contains("'=HYPERLINK"), "{csv}");
    assert!(csv.contains("2.50"));
    assert_eq!(life::csv_cell("@x"), "'@x");
    assert_eq!(life::csv_cell("-1"), "'-1");
    assert_eq!(life::csv_cell("plain"), "plain");
}

#[test]
fn admin_deadlines_need_explicit_sources_and_local_documents() {
    let mut c = db::open_in_memory().unwrap();
    let base = |extra: Value| {
        let mut v = json!({"title":"Residence permit renewal","kind":"immigration"});
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        v
    };
    assert!(
        life::admin_case_create(&mut c, &ctx(NOW), r(base(json!({"due_date":"2026-11-01"}))))
            .is_err(),
        "date without source"
    );
    assert!(
        life::admin_case_create(&mut c, &ctx(NOW), r(base(json!({"date_source":"user"})))).is_err(),
        "source without date"
    );
    assert!(life::admin_case_create(
        &mut c,
        &ctx(NOW),
        r(base(json!({"due_date":"2026-11-01","date_source":"guess"})))
    )
    .is_err());
    let dir = tempfile::tempdir().unwrap();
    let pdf = dir.path().join("letter.pdf");
    std::fs::write(&pdf, b"%PDF synthetic").unwrap();
    let g = grant(&mut c, &pdf, "document");
    let dated = life::admin_case_create(&mut c, &ctx(NOW), r(base(json!({"due_date":"2026-11-01","date_source":"source","source_ref":"letter dated 2026-10-01","document_grant_id":g})))).unwrap();
    life::admin_case_create(
        &mut c,
        &ctx(NOW),
        r(base(json!({"title":"No date","kind":"rent"}))),
    )
    .unwrap();
    life::admin_case_create(
        &mut c,
        &ctx(NOW),
        r(base(
            json!({"title":"Far","due_date":"2027-06-01","date_source":"user"}),
        )),
    )
    .unwrap();
    let soon = life::admin_due_soon(&c, "2026-10-19", 30).unwrap();
    assert_eq!(soon.len(), 1, "undated and far cases never alert");
    assert_eq!(soon[0]["id"], dated["id"]);
    // The document stays where the user keeps it; only the grant id is stored.
    assert!(pdf.exists() && dated["document_grant_id"].as_str() == Some(g.as_str()));
    // A bogus or wrong-purpose grant is refused.
    assert_eq!(
        code(
            life::admin_case_create(
                &mut c,
                &ctx(NOW),
                r(base(json!({"document_grant_id":"nope"})))
            )
            .unwrap_err()
        ),
        "ProviderDenied"
    );
    let lg = grant(&mut c, &pdf, "link");
    assert_eq!(
        code(
            life::admin_case_create(&mut c, &ctx(NOW), r(base(json!({"document_grant_id":lg}))))
                .unwrap_err()
        ),
        "ProviderDenied"
    );
    // Completing it removes it from alerts.
    let id = dated["id"].as_str().unwrap();
    life::admin_case_update(
        &mut c,
        &ctx(NOW),
        id,
        1,
        Some("done"),
        r(base(
            json!({"due_date":"2026-11-01","date_source":"source"}),
        )),
    )
    .unwrap();
    assert!(life::admin_due_soon(&c, "2026-10-19", 30)
        .unwrap()
        .is_empty());
}

#[test]
fn file_grants_scope_revocation_and_swap() {
    let mut c = db::open_in_memory().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("score.pdf");
    std::fs::write(&f, b"x").unwrap();
    let p = pj::project_create(&mut c, &ctx(NOW), "Etude").unwrap();
    let pid = p["id"].as_str().unwrap();
    let link = |c: &mut Connection, g: &str| {
        files::file_link_create(
            c,
            &ctx(NOW),
            r(json!({"owner_type":"project","owner_id":pid,"grant_id":g,"label":"score"})),
        )
    };
    // Never-registered paths and traversal strings have no grant.
    assert_eq!(
        code(link(&mut c, "../../etc/passwd").unwrap_err()),
        "ProviderDenied"
    );
    assert!(files::grant_register(
        &mut c,
        &ctx(NOW),
        dir.path().join("missing.pdf").to_str().unwrap(),
        "link"
    )
    .is_err());
    assert!(
        files::grant_register(&mut c, &ctx(NOW), dir.path().to_str().unwrap(), "link").is_err(),
        "directories are not files"
    );
    assert!(files::grant_register(&mut c, &ctx(NOW), "", "link").is_err());
    assert!(files::grant_register(&mut c, &ctx(NOW), f.to_str().unwrap(), "everything").is_err());
    // A `..` path is canonicalised to the same file; registering twice reuses the grant.
    let dotted = dir.path().join("sub").join("..").join("score.pdf");
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let g = grant(&mut c, &f, "link");
    assert_eq!(grant(&mut c, &dotted, "link"), g);
    let l = link(&mut c, &g).unwrap();
    assert_eq!(files::file_link_list(&c, "project", pid).unwrap().len(), 1);
    assert_eq!(l["grant_id"].as_str(), Some(g.as_str()));
    assert!(files::file_link_create(
        &mut c,
        &ctx(NOW),
        r(json!({"owner_type":"project","owner_id":"missing","grant_id":g}))
    )
    .is_err());
    assert!(files::file_link_create(
        &mut c,
        &ctx(NOW),
        r(json!({"owner_type":"secrets","owner_id":pid,"grant_id":g}))
    )
    .is_err());
    // Revocation and a vanished target both deny on use.
    let g2 = grant(&mut c, &f, "document");
    std::fs::remove_file(&f).unwrap();
    assert_eq!(
        code(files::grant_resolve(&c, &g2, &["document"]).unwrap_err()),
        "ProviderDenied"
    );
    std::fs::write(&f, b"x").unwrap();
    files::grant_resolve(&c, &g2, &["document"]).unwrap();
    files::grant_revoke(&mut c, &ctx(NOW), &g2, 1).unwrap();
    assert_eq!(
        code(files::grant_resolve(&c, &g2, &["document"]).unwrap_err()),
        "ProviderDenied"
    );
    // Re-granting the same path after revocation works with a fresh id.
    assert_ne!(grant(&mut c, &f, "document"), g2);
}

#[test]
fn milestones_crud_and_backlog_order() {
    let mut c = db::open_in_memory().unwrap();
    let p = pj::project_create(&mut c, &ctx(NOW), "REI OS").unwrap();
    let pid = p["id"].as_str().unwrap();
    let m = |c: &mut Connection, v: Value| pj::milestone_create(c, &ctx(NOW), r(v));
    let b = m(
        &mut c,
        json!({"project_id":pid,"title":"Backlog idea","state":"backlog"}),
    )
    .unwrap();
    m(
        &mut c,
        json!({"project_id":pid,"title":"Ship","state":"active","due_date":"2026-12-01"}),
    )
    .unwrap();
    assert!(m(
        &mut c,
        json!({"project_id":"nope","title":"x","state":"active"})
    )
    .is_err());
    assert!(m(
        &mut c,
        json!({"project_id":pid,"title":"x","state":"weird"})
    )
    .is_err());
    let l = pj::milestone_list(&c, pid).unwrap();
    assert_eq!(l[0]["state"], "active");
    let bid = b["id"].as_str().unwrap();
    let upd = |c: &mut Connection, rev| {
        pj::milestone_update(
            c,
            &ctx(NOW),
            bid,
            rev,
            r(json!({"project_id":pid,"title":"Backlog idea","state":"done"})),
        )
    };
    assert_eq!(upd(&mut c, 1).unwrap()["state"], "done");
    assert_eq!(code(upd(&mut c, 1).unwrap_err()), "Conflict");
    pj::milestone_delete(&mut c, &ctx(NOW), bid, 2, true).unwrap();
    assert_eq!(pj::milestone_list(&c, pid).unwrap().len(), 1);
}

#[test]
fn practice_timer_survives_restart_and_rejects_concurrent_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rei.sqlite");
    let (t0, t1, t2, t3, t4) = (
        "2026-10-19T10:00:00Z",
        "2026-10-19T10:10:00Z",
        "2026-10-19T10:20:00Z",
        "2026-10-19T10:25:00Z",
        "2026-10-19T11:00:00Z",
    );
    let id = {
        let mut c = db::open(&path).unwrap();
        let s = pj::practice_start(&mut c, &ctx(t0), "music", "Scales").unwrap();
        assert_eq!(s["state"], "running");
        let id = s["id"].as_str().unwrap().to_string();
        // A second concurrent timer is a Conflict and creates no row.
        let e = pj::practice_start(&mut c, &ctx(t0), "coding", "Rust").unwrap_err();
        assert_eq!(code(e), "Conflict");
        assert_eq!(count(&c, "SELECT COUNT(*) FROM practice_sessions"), 1);
        let p = pj::practice_pause(&mut c, &ctx(t1), &id, 1).unwrap();
        assert_eq!(
            (p["state"].as_str(), p["elapsed_s"].as_i64()),
            (Some("paused"), Some(600))
        );
        // Pausing again, or resuming with a stale revision, is rejected.
        assert!(pj::practice_pause(&mut c, &ctx(t1), &id, 2).is_err());
        assert_eq!(
            code(pj::practice_resume(&mut c, &ctx(t2), &id, 1).unwrap_err()),
            "Conflict"
        );
        let rsm = pj::practice_resume(&mut c, &ctx(t2), &id, 2).unwrap();
        assert_eq!(rsm["state"], "running");
        id
    }; // connection dropped: simulated app exit while the timer runs.
    let mut c = db::open(&path).unwrap();
    let live = pj::practice_active(&c, t3)
        .unwrap()
        .expect("timer recovered");
    assert_eq!(live["id"].as_str(), Some(id.as_str()));
    assert_eq!(
        live["elapsed_s"],
        600 + 300,
        "paused 10 min + 5 min running since restart point"
    );
    // While it is still live a new timer conflicts after restart too.
    assert_eq!(
        code(pj::practice_start(&mut c, &ctx(t3), "music", "x").unwrap_err()),
        "Conflict"
    );
    let done = pj::practice_stop(&mut c, &ctx(t4), &id, 3).unwrap();
    assert_eq!(
        (done["state"].as_str(), done["elapsed_s"].as_i64()),
        (Some("stopped"), Some(600 + 2400))
    );
    assert!(pj::practice_active(&c, t4).unwrap().is_none());
    assert!(
        pj::practice_pause(&mut c, &ctx(t4), &id, 4).is_err(),
        "stopped timers are final"
    );
    // After stop a new timer may start; a backwards clock never reduces elapsed time.
    let n = pj::practice_start(&mut c, &ctx(t4), "htb", "authorized lab").unwrap();
    let nid = n["id"].as_str().unwrap();
    let back = pj::practice_stop(&mut c, &ctx(t0), nid, 1).unwrap();
    assert_eq!(back["elapsed_s"], 0);
    assert!(pj::practice_start(&mut c, &ctx(t4), "karaoke", "x").is_err());
}
