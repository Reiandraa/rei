//! Real SQLite + capability/IPC-schema contract tests (docs/INTERFACES.md §9, foundation).
use rei_os_lib::{commands, db};
use rusqlite::{Connection, OptionalExtension};
use std::collections::BTreeSet;

fn repo(path: &str) -> String {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(path);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn state() -> commands::AppState {
    commands::AppState {
        db: std::sync::Mutex::new(db::open_in_memory().unwrap()),
        device_id: "test-device".into(),
    }
}

#[test]
fn migrations_apply_from_empty_and_are_idempotent() {
    let mut conn = db::open_in_memory().unwrap();
    assert_eq!(db::schema_version(&conn).unwrap(), db::supported_version());
    assert_eq!(db::migrate(&mut conn).unwrap(), db::supported_version());
    let n: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN
             ('tasks','proposals','approvals','outbox','audit_events','settings')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 6);
}

#[test]
fn file_database_persists_and_rejects_downgrade() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.sqlite");
    drop(db::open(&path).unwrap());
    {
        let conn = Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", db::supported_version() + 1)
            .unwrap();
    }
    match db::open(&path) {
        Err(db::DbError::Downgrade { found, supported }) => assert_eq!(found, supported + 1),
        other => panic!("expected downgrade rejection, got {other:?}"),
    }
}

#[test]
fn failed_migration_rolls_back() {
    let mut conn = Connection::open_in_memory().unwrap();
    let tx = conn.transaction().unwrap();
    assert!(tx
        .execute_batch("CREATE TABLE a(x); THIS IS NOT SQL;")
        .is_err());
    drop(tx);
    let n: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 0);
}

#[test]
fn schema_constraints_hold() {
    let conn = db::open_in_memory().unwrap();
    let ins = |sql: &str| conn.execute(sql, []);
    let base = "INSERT INTO tasks(id,created_at,updated_at,origin_device_id,title";
    assert!(ins(&format!("{base},priority) VALUES ('1','t','t','d','x',9)")).is_err());
    assert!(ins(&format!(
        "{base},status) VALUES ('1','t','t','d','x','bogus')"
    ))
    .is_err());
    assert!(ins(&format!(
        "{base},due_date,due_at_utc,due_tz) VALUES ('1','t','t','d','x','2026-10-25','2026-10-25T00:00:00Z','UTC')"
    ))
    .is_err());
    assert!(ins(&format!("{base}) VALUES ('1','t','t','d','ok')")).is_ok());

    conn.execute(
        "INSERT INTO recurrence_rules(id,created_at,updated_at,origin_device_id,series_id,rrule,tz,dtstart_local)
         VALUES ('r','t','t','d','s1','FREQ=DAILY','Europe/Berlin','2026-10-24T09:00:00')",
        [],
    )
    .unwrap();
    let occ = "INSERT INTO occurrences(id,created_at,updated_at,origin_device_id,series_id,original_local) VALUES";
    assert!(ins(&format!(
        "{occ} ('o1','t','t','d','s1','2026-10-25T02:30:00')"
    ))
    .is_ok());
    assert!(ins(&format!(
        "{occ} ('o2','t','t','d','s1','2026-10-25T02:30:00')"
    ))
    .is_err());

    conn.execute(
        "INSERT INTO proposals(id,created_at,updated_at,origin_device_id,action,payload_json,payload_hash,expires_at)
         VALUES ('p','t','t','d','a','{}','h','e')",
        [],
    )
    .unwrap();
    let ap = "INSERT INTO approvals(id,proposal_id,payload_hash,approved_at,actor_device_id,execution_id) VALUES";
    assert!(ins(&format!("{ap} ('a1','p','h','t','d','e1')")).is_ok());
    assert!(
        ins(&format!("{ap} ('a2','p','h','t','d','e1')")).is_err(),
        "execution_id must be unique"
    );
}

#[test]
fn commands_status_health_settings() {
    let s = state();
    let st = commands::status_impl(&s).unwrap();
    assert_eq!(st.ipc_version, 1);
    assert_eq!(st.schema_version, db::supported_version());
    assert!(commands::health_impl(&s).unwrap().database_ok);

    let get = |k: &str| commands::settings_get_impl(&s, commands::SettingsGetReq { key: k.into() });
    let set = |k: &str, v: serde_json::Value, rev| {
        commands::settings_set_impl(
            &s,
            commands::SettingsSetReq {
                key: k.into(),
                value: v,
                expected_revision: rev,
            },
            "2026-10-19T00:00:00Z",
        )
    };
    assert_eq!(get("theme").unwrap(), None);
    assert_eq!(set("theme", "dark".into(), 0).unwrap().revision, 1);
    assert_eq!(
        get("theme").unwrap().unwrap().value,
        serde_json::json!("dark")
    );
    let conflict = set("theme", "light".into(), 0).unwrap_err();
    assert_eq!(
        (conflict.code.as_str(), conflict.current_revision),
        ("Conflict", Some(1))
    );
    assert_eq!(set("theme", "light".into(), 1).unwrap().revision, 2);
    assert_eq!(
        set("theme", "neon".into(), 2).unwrap_err().code,
        "Validation"
    );
    assert_eq!(
        set("api_token", "x".into(), 0).unwrap_err().code,
        "Validation"
    );
    assert!(get("api_token").is_err());
}

#[test]
fn unknown_request_fields_are_rejected() {
    let bad = serde_json::json!({"key":"theme","value":"dark","expected_revision":0,"extra":1});
    assert!(serde_json::from_value::<commands::SettingsSetReq>(bad).is_err());
}

fn string_set(v: &serde_json::Value) -> BTreeSet<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn ipc_schema_matches_rust_commands() {
    let schema: serde_json::Value = serde_json::from_str(&repo("schemas/ipc-v1.json")).unwrap();
    assert_eq!(schema["ipc_version"], commands::IPC_VERSION);
    let declared: BTreeSet<String> = schema["commands"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let rust: BTreeSet<String> = commands::COMMANDS.iter().map(|s| s.to_string()).collect();
    assert_eq!(declared, rust);
    let build_rs = repo("src-tauri/build.rs");
    let lib_rs = repo("src-tauri/src/lib.rs");
    // Commands may live in feature modules. Compare the actual handler entries,
    // rather than assuming every registered function lives in `commands`.
    let handler_body = lib_rs
        .split_once(".invoke_handler(tauri::generate_handler![")
        .expect("missing invoke_handler")
        .1
        .split_once("])")
        .expect("unterminated invoke_handler")
        .0;
    let handler_entries: Vec<&str> = handler_body
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let registered: BTreeSet<String> = handler_entries
        .iter()
        .map(|entry| {
            let (module, name) = entry.rsplit_once("::").expect("qualified command path");
            assert!(
                matches!(
                    module,
                    "email_discovery"
                        | "local_setup"
                        | "commands"
                        | "ai"
                        | "inbox"
                        | "connections"
                        | "auto_academic"
                        | "assistant_workflow"
                        | "assistant"
                ),
                "unreviewed command module {module}"
            );
            name.to_string()
        })
        .collect();
    assert_eq!(
        registered.len(),
        handler_entries.len(),
        "duplicate command registration"
    );
    assert_eq!(
        registered, declared,
        "handler must register exactly the declared commands"
    );
    for c in commands::COMMANDS {
        assert!(
            build_rs.contains(&format!("\"{c}\"")),
            "{c} missing from build.rs manifest"
        );
    }
}

#[test]
fn capability_grants_exactly_listed_commands() {
    let cap: serde_json::Value =
        serde_json::from_str(&repo("src-tauri/capabilities/default.json")).unwrap();
    let granted = string_set(&cap["permissions"]);
    let expected: BTreeSet<String> = commands::COMMANDS
        .iter()
        .map(|c| format!("allow-{}", c.replace('_', "-")))
        .collect();
    assert_eq!(granted, expected);
    for p in &granted {
        for banned in ["shell", "fs", "http", "process", "core:"] {
            assert!(!p.contains(banned), "banned permission {p}");
        }
    }
    assert_eq!(
        string_set(&cap["windows"]),
        BTreeSet::from(["main".to_string()])
    );
}

#[test]
fn csp_has_no_remote_scripts_or_eval() {
    let conf: serde_json::Value = serde_json::from_str(&repo("src-tauri/tauri.conf.json")).unwrap();
    let csp = conf["app"]["security"]["csp"].as_str().unwrap();
    assert!(csp.contains("script-src 'self'"));
    assert!(!csp.contains("unsafe-eval"));
    assert!(!csp.contains("https:") && !csp.contains("*"));
}

#[test]
fn email_migration_keeps_legacy_sign_in_and_selected_inbox() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    for (_, sql) in rei_os_lib::db::MIGRATIONS.iter().take(16) {
        conn.execute_batch(sql).unwrap();
    }
    conn.pragma_update(None, "user_version", 16).unwrap();
    let now = "2026-10-20T00:00:00Z";
    let id = rei_os_lib::hiz::configure(
        &conn,
        None,
        &rei_os_lib::hiz::HizSettings {
            username: "synthetic-owner".into(),
            initial_count: 50,
        },
        now,
    )
    .unwrap();
    rei_os_lib::providers::set_secret_saved(&conn, &id, true, now).unwrap();
    rei_os_lib::providers::upsert_discovered(
        &conn,
        &id,
        &[("folder".into(), "INBOX".into(), "Inbox".into())],
        now,
    )
    .unwrap();
    conn.execute("UPDATE provider_sources SET selected=1", [])
        .unwrap();
    let before: String = conn
        .query_row(
            "SELECT config FROM provider_config WHERE account_id=?1",
            [&id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rei_os_lib::db::migrate(&mut conn).unwrap(), 17);
    let after: (String, bool) = conn
        .query_row(
            "SELECT config,secret_saved FROM provider_config WHERE account_id=?1",
            [&id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(after, (before, true));
    assert_eq!(
        rei_os_lib::providers::selected(&conn, &id, "folder").unwrap(),
        vec!["INBOX"]
    );
    assert_eq!(
        rei_os_lib::email_discovery::require_email(&conn, &id).unwrap(),
        "hiz_imap"
    );
    assert!(conn
        .query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
        .optional()
        .unwrap()
        .is_none());
}
