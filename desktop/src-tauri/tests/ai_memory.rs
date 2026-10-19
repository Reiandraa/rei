//! ai-memory phase tests (P05/P06). All inputs are SYNTHETIC fixtures. The Ollama tests
//! use an in-process loopback stub that speaks the documented /api/tags and /api/chat
//! wire format; they exercise the real HTTP adapter but are NOT a live inference run.
use rei_os_lib::db;
use rei_os_lib::memory::{self, MemoryError, NewMemory, Provenance};
use rei_os_lib::ollama::{self, OllamaClient, OllamaError, OllamaStatus};
use rei_os_lib::tools::{self, ToolKind};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;

const NOW: i64 = 1_792_368_000; // 2026-10-19T00:00:00Z

fn new_mem<'a>(text: &'a str, ttl: Option<i64>) -> NewMemory<'a> {
    NewMemory {
        text,
        provenance: Provenance::User,
        source_ids: &[],
        user_consented: true,
        ttl_days: ttl,
    }
}

fn count(conn: &rusqlite::Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn memory_requires_consent_and_rejects_secrets() {
    let conn = db::open_in_memory().unwrap();
    let mut m = new_mem("Prefers morning study blocks", None);
    m.user_consented = false;
    assert!(matches!(
        memory::create(&conn, &m, NOW),
        Err(MemoryError::ConsentRequired)
    ));
    for t in [
        "password=hunter2",
        "api_key: sk-abcdefghijklmnopqrstuvwxyz0123456789",
        "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.sig",
    ] {
        assert!(
            matches!(
                memory::create(&conn, &new_mem(t, None), NOW),
                Err(MemoryError::SecretRejected)
            ),
            "{t}"
        );
    }
    assert!(matches!(
        memory::create(&conn, &new_mem("   ", None), NOW),
        Err(MemoryError::Bounds("text"))
    ));
    assert!(matches!(
        memory::create(&conn, &new_mem("ok", Some(0)), NOW),
        Err(MemoryError::Bounds("ttl_days"))
    ));
    assert_eq!(count(&conn, "SELECT count(*) FROM memories"), 0);
}

#[test]
fn memory_provenance_edit_search_and_export() {
    let conn = db::open_in_memory().unwrap();
    let src = vec!["msg-123".to_string()];
    let m = memory::create(
        &conn,
        &NewMemory {
            text: "Thesis advisor is Dr. Rivera",
            provenance: Provenance::AiProposal,
            source_ids: &src,
            user_consented: true,
            ttl_days: None,
        },
        NOW,
    )
    .unwrap();
    assert_eq!(m.provenance, "AiProposal");
    assert_eq!(m.source_ids, src);
    assert_eq!(memory::search(&conn, "rivera", 5, NOW).unwrap().len(), 1);

    memory::put_embedding(&conn, &m.id, "nomic-embed-text", 1, &[0.1, 0.2]).unwrap();
    let e = memory::update(&conn, &m.id, "Thesis advisor is Dr. Okafor", NOW + 10).unwrap();
    assert_eq!(e.updated_at, rei_os_lib::time::format_unix(NOW + 10));
    assert!(
        memory::search(&conn, "rivera", 5, NOW).unwrap().is_empty(),
        "old text must leave FTS"
    );
    assert_eq!(memory::search(&conn, "okafor", 5, NOW).unwrap()[0].id, m.id);
    assert_eq!(
        memory::needs_embedding(&conn, "nomic-embed-text", 1).unwrap(),
        vec![m.id.clone()],
        "edit invalidates embedding"
    );

    let export: serde_json::Value =
        serde_json::from_str(&memory::export_json(&conn, NOW).unwrap()).unwrap();
    assert_eq!(export["format"], "rei-os-memory-export");
    assert_eq!(
        export["memories"][0]["text"],
        "Thesis advisor is Dr. Okafor"
    );
    assert!(
        count(
            &conn,
            "SELECT count(*) FROM audit_events WHERE target_type = 'memory'"
        ) >= 2
    );
}

#[test]
fn fts_query_neutralizes_operators() {
    assert_eq!(
        memory::fts_query("a* NEAR(b) text:c").unwrap(),
        "\"a\" OR \"NEAR\" OR \"b\" OR \"text\" OR \"c\""
    );
    assert!(memory::fts_query("\"*^:()").is_none());
    let conn = db::open_in_memory().unwrap();
    memory::create(&conn, &new_mem("Library closes at nine", None), NOW).unwrap();
    assert_eq!(
        memory::search(&conn, "library\" OR text:*", 5, NOW)
            .unwrap()
            .len(),
        1
    );
    assert!(matches!(
        memory::search(&conn, "x", 0, NOW),
        Err(MemoryError::Bounds(_))
    ));
    assert!(matches!(
        memory::search(&conn, "x", 21, NOW),
        Err(MemoryError::Bounds(_))
    ));
}

#[test]
fn forget_removes_fts_and_embeddings() {
    let mut conn = db::open_in_memory().unwrap();
    let m = memory::create(&conn, &new_mem("Allergic to peanuts", None), NOW).unwrap();
    memory::put_embedding(&conn, &m.id, "nomic-embed-text", 1, &[0.5; 8]).unwrap();
    memory::delete(&mut conn, &m.id, NOW).unwrap();
    assert_eq!(count(&conn, "SELECT count(*) FROM memories"), 0);
    assert_eq!(count(&conn, "SELECT count(*) FROM memory_fts"), 0);
    assert_eq!(count(&conn, "SELECT count(*) FROM memory_embeddings"), 0);
    assert!(memory::search(&conn, "peanuts", 5, NOW).unwrap().is_empty());
    assert!(matches!(
        memory::delete(&mut conn, &m.id, NOW),
        Err(MemoryError::NotFound)
    ));
}

#[test]
fn ttl_retention_hides_then_purges() {
    let mut conn = db::open_in_memory().unwrap();
    let short = memory::create(&conn, &new_mem("Temporary parking spot B4", Some(1)), NOW).unwrap();
    memory::put_embedding(&conn, &short.id, "m", 1, &[1.0]).unwrap();
    memory::create(&conn, &new_mem("Permanent fact", None), NOW).unwrap();
    assert_eq!(memory::list(&conn, NOW).unwrap().len(), 2);
    let later = NOW + 86_400;
    assert_eq!(
        memory::list(&conn, later).unwrap().len(),
        1,
        "expired memory hidden before purge"
    );
    assert!(memory::search(&conn, "parking", 5, later)
        .unwrap()
        .is_empty());
    assert_eq!(memory::purge_expired(&mut conn, later).unwrap(), 1);
    assert_eq!(count(&conn, "SELECT count(*) FROM memory_fts"), 1);
    assert_eq!(count(&conn, "SELECT count(*) FROM memory_embeddings"), 0);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM audit_events WHERE action = 'memory.expire'"
        ),
        1
    );
}

#[test]
fn embedding_version_mismatch_requires_rebuild() {
    let conn = db::open_in_memory().unwrap();
    let a = memory::create(&conn, &new_mem("Fact A", None), NOW).unwrap();
    let b = memory::create(&conn, &new_mem("Fact B", None), NOW).unwrap();
    memory::put_embedding(&conn, &a.id, "embed", 1, &[0.0]).unwrap();
    memory::put_embedding(&conn, &b.id, "embed", 1, &[0.0]).unwrap();
    assert!(memory::needs_embedding(&conn, "embed", 1)
        .unwrap()
        .is_empty());
    assert_eq!(memory::needs_embedding(&conn, "embed", 2).unwrap().len(), 2);
    memory::put_embedding(&conn, &a.id, "embed", 2, &[0.0, 1.0]).unwrap();
    assert_eq!(
        memory::needs_embedding(&conn, "embed", 2).unwrap(),
        vec![b.id]
    );
}

fn load_fixture(name: &str) -> serde_json::Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/synthetic")
        .join(name);
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    assert!(v["label"].as_str().unwrap().starts_with("SYNTHETIC"));
    v
}

#[test]
fn injection_fixtures_cannot_escape_allow_list_or_write() {
    for name in [
        "injection-email-exfiltrate.json",
        "injection-teams-delete.json",
    ] {
        let fx = load_fixture(name);
        for call in fx["model_tool_calls"].as_array().unwrap() {
            let res = tools::parse_call(&call.to_string());
            assert!(res.is_err(), "{name}: {call} must be rejected");
        }
        let conn = db::open_in_memory().unwrap();
        for t in fx["memory_texts"].as_array().unwrap() {
            let r = memory::create(&conn, &new_mem(t.as_str().unwrap(), None), NOW);
            assert!(matches!(r, Err(MemoryError::SecretRejected)), "{name}: {t}");
        }
        assert_eq!(count(&conn, "SELECT count(*) FROM memories"), 0);
    }
    // The only memory-writing tool the model has is a proposal, never a direct write.
    let ok = tools::parse_call(
        r#"{"name":"propose_memory_add","arguments":{"fields":{"text":"x"},"rationale":"r"}}"#,
    )
    .unwrap();
    assert_eq!(ok.kind(), ToolKind::Proposal);
}

#[test]
fn ollama_rejects_non_loopback_urls() {
    for bad in [
        "http://10.0.0.5:11434",
        "http://example.com:11434",
        "https://127.0.0.1:11434",
        "http://user@127.0.0.1:11434",
        "http://127.0.0.1:11434/api",
        "http://0.0.0.0:11434",
    ] {
        assert!(OllamaClient::new(bad, 30).is_err(), "{bad}");
    }
    assert_eq!(
        ollama::parse_loopback_url("http://192.168.1.2:11434"),
        Err(OllamaError::NonLoopback)
    );
    assert!(OllamaClient::new("http://127.0.0.1:11434", 30).is_ok());
    assert!(OllamaClient::new("http://[::1]:11434/", 30).is_ok());
}

#[test]
fn ollama_tags_parsing() {
    let body = br#"{"models":[{"name":"llama3.1:8b","digest":"sha256:abc"},{"name":"qwen2.5:latest","digest":"d2"}]}"#;
    assert_eq!(
        ollama::status_from_tags(body, "llama3.1:8b"),
        OllamaStatus::Ready {
            model: "llama3.1:8b".into(),
            digest: "sha256:abc".into()
        }
    );
    assert_eq!(
        ollama::status_from_tags(body, "qwen2.5"),
        OllamaStatus::Ready {
            model: "qwen2.5:latest".into(),
            digest: "d2".into()
        }
    );
    assert_eq!(
        ollama::status_from_tags(body, "mistral"),
        OllamaStatus::ModelMissing
    );
    assert!(matches!(
        ollama::status_from_tags(b"not json", "x"),
        OllamaStatus::Error(_)
    ));
}

/// Serves one canned HTTP response on a loopback port; returns the URL.
fn stub(response: &'static str) -> (String, std::thread::JoinHandle<String>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let h = std::thread::spawn(move || {
        let (s, _) = l.accept().unwrap();
        let mut r = BufReader::new(s.try_clone().unwrap());
        let mut head = String::new();
        let mut len = 0usize;
        loop {
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                len = v.trim().parse().unwrap();
            }
            head.push_str(&line);
            if line == "\r\n" {
                break;
            }
        }
        let mut body = vec![0; len];
        r.read_exact(&mut body).unwrap();
        let mut s = s;
        s.write_all(response.as_bytes()).unwrap();
        head + &String::from_utf8(body).unwrap()
    });
    (url, h)
}

#[test]
fn ollama_status_not_running_and_ready_via_stub() {
    let free = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let c = OllamaClient::new(&format!("http://{free}"), 2).unwrap();
    assert_eq!(c.status("llama3.1:8b"), OllamaStatus::NotRunning);

    let (url, h) = stub("HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{\"models\":[{\"name\":\"llama3.1:8b\",\"digest\":\"sha256:feed\"}]}");
    let c = OllamaClient::new(&url, 5).unwrap();
    assert_eq!(
        c.status("llama3.1:8b"),
        OllamaStatus::Ready {
            model: "llama3.1:8b".into(),
            digest: "sha256:feed".into()
        }
    );
    assert!(h.join().unwrap().starts_with("GET /api/tags HTTP/1.0"));
}

#[test]
fn ollama_chat_streams_content_and_returns_unexecuted_tool_calls() {
    let (url, h) = stub(concat!(
        "HTTP/1.0 200 OK\r\nContent-Type: application/x-ndjson\r\n\r\n",
        "{\"message\":{\"role\":\"assistant\",\"content\":\"Hel\"},\"done\":false}\n",
        "{\"message\":{\"role\":\"assistant\",\"content\":\"lo\",\"tool_calls\":[{\"function\":{\"name\":\"run_shell\",\"arguments\":{\"cmd\":\"rm -rf /\"}}}]},\"done\":false}\n",
        "{\"done\":true}\n"
    ));
    let c = OllamaClient::new(&url, 5).unwrap();
    let cancel = AtomicBool::new(false);
    let out = c
        .chat(
            "llama3.1:8b",
            &serde_json::json!([{"role":"user","content":"hi"}]),
            &serde_json::json!([]),
            8192,
            &cancel,
        )
        .unwrap();
    assert_eq!(out.content, "Hello");
    assert_eq!(out.tool_calls.len(), 1);
    assert!(
        tools::parse_call(&out.tool_calls[0]).is_err(),
        "model-emitted shell call must fail validation"
    );
    let req = h.join().unwrap();
    assert!(req.contains("\"stream\":true") && req.contains("\"num_ctx\":8192"));
}

#[test]
fn ollama_chat_cancel_error_and_oversized_prompt() {
    let (url, h) =
        stub("HTTP/1.0 200 OK\r\n\r\n{\"message\":{\"content\":\"x\"},\"done\":false}\n");
    let c = OllamaClient::new(&url, 5).unwrap();
    let cancel = AtomicBool::new(true);
    assert_eq!(
        c.chat(
            "m",
            &serde_json::json!([]),
            &serde_json::json!([]),
            4096,
            &cancel
        ),
        Err(OllamaError::Cancelled)
    );
    h.join().unwrap();

    let (url, h) = stub("HTTP/1.0 404 Not Found\r\n\r\n{\"error\":\"model not found\"}");
    let c = OllamaClient::new(&url, 5).unwrap();
    let r = c.chat(
        "m",
        &serde_json::json!([]),
        &serde_json::json!([]),
        4096,
        &AtomicBool::new(false),
    );
    assert!(matches!(r, Err(OllamaError::Protocol(_))));
    h.join().unwrap();

    let big = "a".repeat(ollama::MAX_PROMPT_BYTES + 1);
    let r = c.chat(
        "m",
        &serde_json::json!([{"role":"user","content":big}]),
        &serde_json::json!([]),
        4096,
        &AtomicBool::new(false),
    );
    assert_eq!(r, Err(OllamaError::TooLarge));
}
