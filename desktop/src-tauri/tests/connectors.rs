//! connectors phase tests (P02/P03/P04 + manual import). Every page below is a
//! SYNTHETIC fixture in Microsoft Graph wire format; no login or network I/O occurs.
//! Passing here is fixture evidence only, never live connection evidence.
use rei_os_lib::connectors::{self as c, ConnState, ConnectorError, Evidence, Kind};
use rei_os_lib::db;
use rusqlite::Connection;
use serde_json::json;

const NOW: &str = "2026-10-19T08:00:00Z";
const LATER: &str = "2026-10-19T09:00:00Z";

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// Creates an account moved to `testing` with fixture evidence.
fn fixture_account(conn: &Connection, kind: Kind) -> String {
    let a = c::create_account(conn, kind, "microsoft_graph", "SYNTHETIC account", NOW).unwrap();
    c::transition(
        conn,
        &a.id,
        ConnState::ConsentNeeded,
        Evidence::None,
        None,
        NOW,
    )
    .unwrap();
    c::transition(
        conn,
        &a.id,
        ConnState::Testing,
        Evidence::Fixture,
        None,
        NOW,
    )
    .unwrap();
    a.id
}

#[test]
fn fixture_evidence_can_never_mark_connected() {
    let conn = db::open_in_memory().unwrap();
    let id = fixture_account(&conn, Kind::Email);
    let err = c::transition(
        &conn,
        &id,
        ConnState::Connected,
        Evidence::Fixture,
        None,
        NOW,
    )
    .unwrap_err();
    assert!(matches!(err, ConnectorError::State(_)));
    // The DB CHECK independently rejects a forged row.
    assert!(conn
        .execute(
            "UPDATE connector_accounts SET state='connected', evidence='fixture'",
            []
        )
        .is_err());
    // Skipping consent is not a legal edge.
    let fresh = c::create_account(&conn, Kind::Calendar, "microsoft_graph", "x", NOW).unwrap();
    assert!(c::transition(
        &conn,
        &fresh.id,
        ConnState::Testing,
        Evidence::Fixture,
        None,
        NOW
    )
    .is_err());
    // Live evidence after testing is the only way to connected.
    let a = c::transition(&conn, &id, ConnState::Connected, Evidence::Live, None, NOW).unwrap();
    assert_eq!(a.state, ConnState::Connected);
    assert_eq!(c::state_for_status(401), Some(ConnState::Expired));
    assert_eq!(c::state_for_status(403), Some(ConnState::PermissionDenied));
    assert_eq!(c::state_for_status(200), None);
}

#[test]
fn state_errors_are_redacted_and_audited() {
    let conn = db::open_in_memory().unwrap();
    let id = fixture_account(&conn, Kind::TeamsChannel);
    let a = c::transition(
        &conn,
        &id,
        ConnState::PermissionDenied,
        Evidence::Fixture,
        Some("403 Authorization: Bearer abcdefghijklmnopqrstuvwxyz0123456789ABCDEF"),
        NOW,
    )
    .unwrap();
    assert!(!a.last_error.unwrap().contains("abcdefghijklmnop"));
    assert!(
        count(
            &conn,
            "SELECT COUNT(*) FROM audit_events WHERE action='connector_state'"
        ) >= 3
    );
}

#[test]
fn scopes_are_least_privilege_and_read_only() {
    for k in [
        Kind::Email,
        Kind::Calendar,
        Kind::TeamsChat,
        Kind::TeamsChannel,
    ] {
        assert!(!k.scopes().is_empty());
        for s in k.scopes() {
            assert!(!s.contains("Send") && !s.contains("ReadWrite"), "{s}");
        }
    }
    assert_eq!(Kind::TeamsChat.scopes(), &["Chat.Read"]);
    assert!(Kind::TeamsChannel
        .scopes()
        .contains(&"ChannelMessage.Read.All"));
}

#[test]
fn mail_delta_pages_commit_rows_with_cursor_and_dedupe() {
    let mut conn = db::open_in_memory().unwrap();
    let id = fixture_account(&conn, Kind::Email);
    assert_eq!(
        c::mail_next_request(&conn, &id).unwrap(),
        c::MAIL_DELTA_START
    );
    let page1 = json!({
        "value": [
            {"id": "m1", "subject": "SYNTHETIC: lab report due", "receivedDateTime": "2026-10-18T10:00:00Z",
             "lastModifiedDateTime": "2026-10-18T10:00:00Z", "from": {"emailAddress": {"address": "prof@example.edu"}},
             "body": {"contentType": "html", "content": "<p>Please submit by <b>Friday</b> &amp; cite sources.</p>"}},
            {"subject": "no id -> skipped"}
        ],
        "@odata.nextLink": "https://graph.microsoft.com/v1.0/me/mailFolders/inbox/messages/delta?$skiptoken=A"
    });
    let s = c::apply_mail_delta_page(&mut conn, &id, &page1, NOW).unwrap();
    assert_eq!((s.inserted, s.skipped, s.complete), (1, 1, false));
    assert!(c::mail_next_request(&conn, &id)
        .unwrap()
        .ends_with("skiptoken=A"));
    let page2 = json!({
        "value": [
            {"id": "m1", "subject": "SYNTHETIC: lab report due", "receivedDateTime": "2026-10-18T10:00:00Z",
             "lastModifiedDateTime": "2026-10-18T10:00:00Z", "bodyPreview": "dup"},
            {"id": "m2", "subject": "SYNTHETIC: newsletter", "receivedDateTime": "2026-10-18T11:00:00Z",
             "lastModifiedDateTime": "2026-10-18T11:00:00Z", "bodyPreview": "hello"}
        ],
        "@odata.deltaLink": "https://graph.microsoft.com/v1.0/me/mailFolders/inbox/messages/delta?$deltatoken=B"
    });
    let s = c::apply_mail_delta_page(&mut conn, &id, &page2, NOW).unwrap();
    assert_eq!((s.inserted, s.unchanged, s.complete), (1, 1, true));
    assert!(c::mail_next_request(&conn, &id)
        .unwrap()
        .ends_with("deltatoken=B"));
    let msgs = c::list_messages(&conn, Some(&id), 50).unwrap();
    assert_eq!(msgs.len(), 2);
    let m1 = msgs
        .iter()
        .find(|m| m.subject.as_deref() == Some("SYNTHETIC: lab report due"))
        .unwrap();
    assert_eq!(m1.body_text, "Please submit by Friday & cite sources.");
    assert_eq!(m1.label, "SYNTHETIC_FIXTURE");
    // Removal tombstones and clears content.
    let page3 = json!({"value": [{"id": "m2", "@removed": {"reason": "deleted"}}],
        "@odata.deltaLink": "https://graph.microsoft.com/v1.0/me/mailFolders/inbox/messages/delta?$deltatoken=C"});
    assert_eq!(
        c::apply_mail_delta_page(&mut conn, &id, &page3, LATER)
            .unwrap()
            .removed,
        1
    );
    assert_eq!(c::list_messages(&conn, Some(&id), 50).unwrap().len(), 1);
}

#[test]
fn malicious_paging_link_rolls_back_whole_page() {
    let mut conn = db::open_in_memory().unwrap();
    let id = fixture_account(&conn, Kind::Email);
    let page = json!({
        "value": [{"id": "x1", "receivedDateTime": "2026-10-18T10:00:00Z", "bodyPreview": "SYNTHETIC"}],
        "@odata.deltaLink": "https://attacker.example/steal?token="
    });
    assert!(matches!(
        c::apply_mail_delta_page(&mut conn, &id, &page, NOW),
        Err(ConnectorError::Invalid(_))
    ));
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM source_messages"), 0);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM source_cursors"), 0);
}

#[test]
fn wrong_kind_or_unconfigured_account_is_rejected() {
    let mut conn = db::open_in_memory().unwrap();
    let chat = fixture_account(&conn, Kind::TeamsChat);
    let page = json!({"value": []});
    assert!(matches!(
        c::apply_mail_delta_page(&mut conn, &chat, &page, NOW),
        Err(ConnectorError::State(_))
    ));
    let raw = c::create_account(&conn, Kind::Email, "microsoft_graph", "x", NOW).unwrap();
    assert!(matches!(
        c::apply_mail_delta_page(&mut conn, &raw.id, &page, NOW),
        Err(ConnectorError::State(_))
    ));
    assert!(
        c::apply_chat_messages_page(&mut conn, &chat, "../../me/sendMail", &page, NOW).is_err()
    );
}

#[test]
fn calendar_delta_creates_read_only_planner_events() {
    let mut conn = db::open_in_memory().unwrap();
    let id = fixture_account(&conn, Kind::Calendar);
    let page = json!({
        "value": [
            {"id": "e1", "subject": "SYNTHETIC: Seminar", "changeKey": "k1", "isAllDay": false,
             "start": {"dateTime": "2026-10-20T09:00:00.0000000", "timeZone": "UTC"},
             "end": {"dateTime": "2026-10-20T10:30:00.0000000", "timeZone": "UTC"}},
            {"id": "e2", "subject": "SYNTHETIC: Berlin call", "changeKey": "k1",
             "start": {"dateTime": "2026-10-20T09:00:00", "timeZone": "Europe/Berlin"},
             "end": {"dateTime": "2026-10-20T09:30:00", "timeZone": "Europe/Berlin"}},
            {"id": "e3", "subject": "Windows zone is rejected, not guessed",
             "start": {"dateTime": "2026-10-20T09:00:00", "timeZone": "W. Europe Standard Time"},
             "end": {"dateTime": "2026-10-20T09:30:00", "timeZone": "W. Europe Standard Time"}}
        ],
        "@odata.deltaLink": "https://graph.microsoft.com/v1.0/me/calendarView/delta?$deltatoken=D"
    });
    let s = c::apply_calendar_delta_page(&mut conn, &id, "dev1", &page, NOW).unwrap();
    assert_eq!((s.inserted, s.skipped), (2, 1));
    let (start, ro): (String, i64) = conn
        .query_row(
            "SELECT start_value, read_only FROM events WHERE provider_object_id='e2'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((start.as_str(), ro), ("2026-10-20T07:00:00Z", 1));
    // Same changeKey is unchanged; new changeKey bumps the revision; removal tombstones.
    let page2 = json!({"value": [
        {"id": "e1", "subject": "SYNTHETIC: Seminar", "changeKey": "k1",
         "start": {"dateTime": "2026-10-20T09:00:00", "timeZone": "UTC"}, "end": {"dateTime": "2026-10-20T10:30:00", "timeZone": "UTC"}},
        {"id": "e2", "subject": "SYNTHETIC: Berlin call (moved)", "changeKey": "k2",
         "start": {"dateTime": "2026-10-20T10:00:00", "timeZone": "Europe/Berlin"}, "end": {"dateTime": "2026-10-20T10:30:00", "timeZone": "Europe/Berlin"}},
        {"id": "e1", "@removed": {"reason": "deleted"}}
    ]});
    let s = c::apply_calendar_delta_page(&mut conn, &id, "dev1", &page2, LATER).unwrap();
    assert_eq!((s.unchanged, s.updated, s.removed), (1, 1, 1));
    assert_eq!(
        count(
            &conn,
            "SELECT revision FROM events WHERE provider_object_id='e2'"
        ),
        2
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM events WHERE deleted_at IS NULL"
        ),
        1
    );
}

#[test]
fn chat_overlap_window_dedupes_and_tracks_edits() {
    let mut conn = db::open_in_memory().unwrap();
    let id = fixture_account(&conn, Kind::TeamsChat);
    let chat = "19:SYNTHETICchat@unq.gbl.spaces";
    assert_eq!(c::chat_poll_since(&conn, &id, chat).unwrap(), None);
    let page = json!({"value": [
        {"id": "1", "messageType": "message", "createdDateTime": "2026-10-19T07:00:00Z",
         "lastModifiedDateTime": "2026-10-19T07:00:00Z", "from": {"user": {"displayName": "SYNTHETIC Alex"}},
         "body": {"contentType": "text", "content": "Meet at 3?"}},
        {"id": "2", "messageType": "systemEventMessage", "createdDateTime": "2026-10-19T07:01:00Z", "body": {"content": ""}}
    ]});
    let s = c::apply_chat_messages_page(&mut conn, &id, chat, &page, NOW).unwrap();
    assert_eq!((s.inserted, s.skipped), (1, 1));
    assert_eq!(
        c::chat_poll_since(&conn, &id, chat).unwrap().as_deref(),
        Some("2026-10-19T06:50:00Z")
    );
    let url = c::chat_messages_url(chat, Some("2026-10-19T06:50:00Z")).unwrap();
    assert!(url.starts_with("https://graph.microsoft.com/v1.0/chats/"));
    assert!(
        url.contains("$orderby=lastModifiedDateTime%20desc")
            && url.contains("gt%202026-10-19T06:50:00Z")
    );
    // Overlap re-delivery is unchanged; an edit (newer lastModified) updates.
    let again = json!({"value": [
        {"id": "1", "messageType": "message", "createdDateTime": "2026-10-19T07:00:00Z",
         "lastModifiedDateTime": "2026-10-19T07:00:00Z", "body": {"content": "Meet at 3?"}},
        {"id": "1", "messageType": "message", "createdDateTime": "2026-10-19T07:00:00Z",
         "lastModifiedDateTime": "2026-10-19T07:05:00Z", "body": {"content": "Meet at 4?"}}
    ]});
    let s = c::apply_chat_messages_page(&mut conn, &id, chat, &again, LATER).unwrap();
    assert_eq!((s.unchanged, s.updated), (1, 1));
    assert_eq!(
        c::list_messages(&conn, Some(&id), 10).unwrap()[0].body_text,
        "Meet at 4?"
    );
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM source_messages"), 1);
}

#[test]
fn channel_messages_and_replies_reconcile_per_thread() {
    let mut conn = db::open_in_memory().unwrap();
    let id = fixture_account(&conn, Kind::TeamsChannel);
    let (team, chan) = ("SYNTHETIC-team-1", "19:SYNTHETICchan@thread.tacv2");
    let roots = json!({"value": [
        {"id": "r1", "messageType": "message", "createdDateTime": "2026-10-19T06:00:00Z", "subject": "Group project",
         "from": {"user": {"displayName": "SYNTHETIC Sam"}}, "body": {"contentType": "html", "content": "<div>Who takes part 2?</div>"}},
        {"id": "r2", "messageType": "message", "createdDateTime": "2026-10-19T06:10:00Z", "body": {"content": "FYI"}}
    ], "@odata.nextLink": "https://graph.microsoft.com/v1.0/teams/t/channels/c/messages?$skiptoken=X"});
    let s = c::apply_channel_messages_page(&mut conn, &id, team, chan, None, &roots, NOW).unwrap();
    assert_eq!((s.inserted, s.complete), (2, false));
    assert_eq!(
        c::channel_threads_to_reconcile(&conn, &id, team, chan).unwrap(),
        vec!["r1", "r2"]
    );
    let replies = json!({"value": [
        {"id": "p1", "replyToId": "r1", "messageType": "message", "createdDateTime": "2026-10-19T06:20:00Z", "body": {"content": "I will"}}
    ]});
    c::apply_channel_messages_page(&mut conn, &id, team, chan, Some("r1"), &replies, NOW).unwrap();
    c::apply_channel_messages_page(
        &mut conn,
        &id,
        team,
        chan,
        Some("r2"),
        &json!({"value": []}),
        NOW,
    )
    .unwrap();
    assert!(c::channel_threads_to_reconcile(&conn, &id, team, chan)
        .unwrap()
        .is_empty());
    let parent: String = conn
        .query_row(
            "SELECT parent_id FROM source_messages WHERE provider_object_id='p1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(parent, "r1");
    // Chat and channel stores are distinct even for identical provider IDs.
    let chat = fixture_account(&conn, Kind::TeamsChat);
    c::apply_chat_messages_page(
        &mut conn,
        &chat,
        "19:x@unq.gbl.spaces",
        &json!({"value": [
        {"id": "r1", "createdDateTime": "2026-10-19T06:00:00Z", "body": {"content": "chat"}}]}),
        NOW,
    )
    .unwrap();
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM source_messages WHERE provider_object_id='r1'"
        ),
        2
    );
}

#[test]
fn injected_source_text_is_stored_inert_and_wrapped_for_model() {
    let mut conn = db::open_in_memory().unwrap();
    let id = fixture_account(&conn, Kind::Email);
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../fixtures/synthetic/injection-teams-delete.json"
    ))
    .unwrap();
    let text = fixture["source_text"].as_str().unwrap();
    let page = json!({"value": [{"id": "inj", "subject": "SYNTHETIC injection", "receivedDateTime": "2026-10-18T10:00:00Z",
        "bodyPreview": text}]});
    c::apply_mail_delta_page(&mut conn, &id, &page, NOW).unwrap();
    let before =
        count(&conn, "SELECT COUNT(*) FROM proposals") + count(&conn, "SELECT COUNT(*) FROM tasks");
    let msg = &c::list_messages(&conn, None, 10).unwrap()[0];
    let wrapped = c::model_context(&conn, &msg.id, "BOUNDARY123").unwrap();
    assert!(wrapped.contains("BOUNDARY123") && wrapped.contains("delete every task"));
    assert_eq!(before, 0, "ingestion must not create proposals or tasks");
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM source_messages WHERE untrusted = 1"
        ),
        1
    );
}

#[test]
fn manual_import_is_labeled_and_validated() {
    let mut conn = db::open_in_memory().unwrap();
    let m = c::create_account(&conn, Kind::Manual, "manual", "Pasted notes", NOW).unwrap();
    let mid = c::import_manual(
        &mut conn,
        &m.id,
        "SYNTHETIC syllabus",
        "Exam on Nov 3",
        Some("https://example.edu/s"),
        NOW,
    )
    .unwrap();
    let msg = c::list_messages(&conn, Some(&m.id), 5).unwrap();
    assert_eq!(msg[0].id, mid);
    assert_eq!(msg[0].label, "MANUAL_IMPORT");
    assert!(msg[0].body_text.contains("Link: https://example.edu/s"));
    assert!(
        c::import_manual(&mut conn, &m.id, "x", "y", Some("javascript:alert(1)"), NOW).is_err()
    );
    assert!(c::import_manual(&mut conn, &m.id, " ", "y", None, NOW).is_err());
    let mail = fixture_account(&conn, Kind::Email);
    assert!(c::import_manual(&mut conn, &mail, "x", "y", None, NOW).is_err());
}

#[test]
fn disconnect_clears_cursors_and_optionally_cache() {
    let mut conn = db::open_in_memory().unwrap();
    let id = fixture_account(&conn, Kind::Email);
    let page = json!({"value": [{"id": "m1", "receivedDateTime": "2026-10-18T10:00:00Z", "bodyPreview": "SYNTHETIC"}],
        "@odata.deltaLink": "https://graph.microsoft.com/v1.0/me/mailFolders/inbox/messages/delta?$deltatoken=Z"});
    c::apply_mail_delta_page(&mut conn, &id, &page, NOW).unwrap();
    let a = c::disconnect(&mut conn, &id, false, LATER).unwrap();
    assert_eq!(
        (a.state, a.evidence),
        (ConnState::NotConfigured, Evidence::None)
    );
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM source_cursors"), 0);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM source_messages"), 1);
    c::disconnect(&mut conn, &id, true, LATER).unwrap();
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM source_messages"), 0);
}

#[test]
fn retry_policy_honours_retry_after_and_caps_attempts() {
    use std::time::Duration;
    assert_eq!(
        c::retry_delay(0, Some(7), 0.5),
        Some(Duration::from_secs(7))
    );
    assert_eq!(
        c::retry_delay(0, Some(10_000), 0.0),
        Some(Duration::from_secs(300))
    );
    assert_eq!(
        c::retry_delay(2, None, 0.0),
        Some(Duration::from_millis(4000))
    );
    assert!(c::retry_delay(3, None, 0.5).unwrap() > c::retry_delay(2, None, 0.5).unwrap());
    assert_eq!(c::retry_delay(c::MAX_ATTEMPTS, None, 0.0), None);
}
