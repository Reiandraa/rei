//! Pure SQLite/memory-vault regressions. No provider, browser or OS-vault access.
use providers::ProviderError;
use rei_os_lib::vault::{MemoryStore, Secret, SecretKind, SecretStore, VaultError, VaultKey};
use rei_os_lib::{connections, db, hiz, providers, teams};

const NOW: &str = "2026-10-19T15:00:00Z";

fn hiz_settings(name: &str) -> hiz::HizSettings {
    hiz::HizSettings {
        username: name.into(),
        initial_count: 50,
    }
}

#[test]
fn disconnect_cannot_race_sign_in_and_resurrect_a_removed_secret() {
    let mut c = db::open_in_memory().unwrap();
    let id = hiz::configure(&c, None, &hiz_settings("synthetic-owner"), NOW).unwrap();
    let store = MemoryStore::default();
    let key = VaultKey::new(&id, SecretKind::HizPassword).unwrap();
    store
        .put(&key, &Secret::new(b"synthetic-password".to_vec()))
        .unwrap();
    providers::set_secret_saved(&c, &id, true, NOW).unwrap();
    let active = providers::operation_guard(&c, &id).unwrap();
    assert_eq!(
        connections::disconnect(&mut c, &store, &id, true),
        Err(ProviderError::Busy)
    );
    assert_eq!(store.get(&key).unwrap().expose(), b"synthetic-password");
    drop(active);
    connections::disconnect(&mut c, &store, &id, false).unwrap();
    assert!(matches!(store.get(&key), Err(VaultError::NotFound)));
}

#[test]
fn settings_and_source_changes_cannot_race_account_network_operations() {
    let mut c = db::open_in_memory().unwrap();
    let id = hiz::configure(&c, None, &hiz_settings("synthetic-owner"), NOW).unwrap();
    let active = providers::operation_guard(&c, &id).unwrap();
    assert_eq!(
        hiz::configure(&c, Some(&id), &hiz_settings("different-owner"), NOW),
        Err(ProviderError::Busy)
    );
    assert_eq!(
        providers::set_selected(&mut c, &id, &[]),
        Err(ProviderError::Busy)
    );
    assert_eq!(
        providers::load_config(&c, &id, hiz::PROVIDER).unwrap()["username"],
        "synthetic-owner"
    );
    drop(active);
    assert_eq!(providers::set_selected(&mut c, &id, &[]).unwrap(), 0);
}

#[test]
fn teams_chat_and_channel_share_operation_lock_cancellation_and_disconnect() {
    let mut c = db::open_in_memory().unwrap();
    let settings = teams::TeamsSettings {
        client_id: "11111111-1111-1111-1111-111111111111".into(),
        tenant: "organizations".into(),
        channels: true,
    };
    let id = teams::configure(&c, None, &settings, NOW).unwrap();
    let channel = teams::linked(&c, &id)
        .unwrap()
        .into_iter()
        .find(|v| v != &id)
        .unwrap();
    assert_eq!(providers::operation_identity(&c, &channel).unwrap(), id);
    assert!(matches!(
        teams::configure(&c, Some(&channel), &settings, NOW),
        Err(ProviderError::Invalid(_))
    ));
    let store = MemoryStore::default();
    let key = VaultKey::new(&id, SecretKind::TeamsRefresh).unwrap();
    store
        .put(&key, &Secret::new(b"synthetic-refresh".to_vec()))
        .unwrap();
    let active = providers::operation_guard(&c, &id).unwrap();
    assert!(matches!(
        providers::operation_guard(&c, &channel),
        Err(ProviderError::Busy)
    ));
    assert_eq!(
        connections::disconnect(&mut c, &store, &channel, false),
        Err(ProviderError::Busy)
    );
    assert_eq!(
        teams::configure(&c, Some(&id), &settings, NOW),
        Err(ProviderError::Busy)
    );
    assert!(providers::status(&c)
        .unwrap()
        .iter()
        .all(|a| a.fetch_running));
    assert!(providers::cancel(
        &providers::operation_identity(&c, &channel).unwrap()
    ));
    assert!(active.cancelled());
    drop(active);
    connections::disconnect(&mut c, &store, &channel, false).unwrap();
    assert!(matches!(store.get(&key), Err(VaultError::NotFound)));
}

#[test]
fn saved_hiz_credential_cannot_be_reused_for_another_identifier() {
    let c = db::open_in_memory().unwrap();
    let id = hiz::configure(&c, None, &hiz_settings("synthetic-owner"), NOW).unwrap();
    providers::set_secret_saved(&c, &id, true, NOW).unwrap();
    assert!(matches!(
        hiz::configure(&c, Some(&id), &hiz_settings("other-owner"), NOW),
        Err(ProviderError::Invalid(_))
    ));
    assert_eq!(
        providers::load_config(&c, &id, hiz::PROVIDER).unwrap()["username"],
        "synthetic-owner"
    );
}

#[test]
fn source_limit_errors_do_not_silently_save_a_partial_selection() {
    let mut c = db::open_in_memory().unwrap();
    let id = hiz::configure(&c, None, &hiz_settings("synthetic-owner"), NOW).unwrap();
    providers::upsert_discovered(
        &c,
        &id,
        &[("folder".into(), "INBOX".into(), "INBOX".into())],
        NOW,
    )
    .unwrap();
    providers::set_selected(&mut c, &id, &[("folder".into(), "INBOX".into())]).unwrap();
    let too_many: Vec<_> = (0..=hiz::MAX_FOLDERS)
        .map(|i| ("folder".into(), format!("fixture-{i}")))
        .collect();
    assert!(matches!(
        providers::set_selected(&mut c, &id, &too_many),
        Err(ProviderError::Invalid(_))
    ));
    assert_eq!(
        providers::selected(&c, &id, "folder").unwrap(),
        vec!["INBOX"]
    );
}

#[test]
fn native_teams_registration_matches_the_exact_fixed_loopback_port() {
    let c = db::open_in_memory().unwrap();
    let settings = teams::TeamsSettings {
        client_id: "11111111-1111-1111-1111-111111111111".into(),
        tenant: "organizations".into(),
        channels: false,
    };
    let id = teams::configure(&c, None, &settings, NOW).unwrap();
    let uri = url::Url::parse(teams::REDIRECT_URI).unwrap();
    assert_eq!(uri.host_str(), Some("127.0.0.1"));
    assert_eq!(uri.port(), Some(teams::REDIRECT_PORT));
    assert_eq!(uri.path(), "/callback");
    assert_eq!(
        providers::load_config(&c, &id, teams::PROVIDER).unwrap()["redirect_uri"],
        teams::REDIRECT_URI
    );
    assert!(rei_os_lib::oauth::validate_redirect(teams::REDIRECT_URI).is_ok());
}

#[test]
fn old_channel_roots_are_revisited_for_replies_without_a_root_edit() {
    use rei_os_lib::connectors::{self as c, Kind};
    use serde_json::json;
    let mut conn = db::open_in_memory().unwrap();
    let account = c::create_account(
        &conn,
        Kind::TeamsChannel,
        "synthetic",
        "synthetic channel",
        NOW,
    )
    .unwrap();
    c::transition(
        &conn,
        &account.id,
        c::ConnState::ConsentNeeded,
        c::Evidence::Fixture,
        None,
        NOW,
    )
    .unwrap();
    c::transition(
        &conn,
        &account.id,
        c::ConnState::Testing,
        c::Evidence::Fixture,
        None,
        NOW,
    )
    .unwrap();
    let roots = json!({"value":[{"id":"root-old", "messageType":"message", "createdDateTime":"2026-10-18T10:00:00Z", "body":{"content":"Original announcement"}}]});
    c::apply_channel_messages_page(&mut conn, &account.id, "team", "channel", None, &roots, NOW)
        .unwrap();
    c::apply_channel_messages_page(
        &mut conn,
        &account.id,
        "team",
        "channel",
        Some("root-old"),
        &json!({"value":[]}),
        NOW,
    )
    .unwrap();
    assert!(
        c::channel_threads_to_reconcile(&conn, &account.id, "team", "channel")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        c::channel_threads_to_poll(&conn, &account.id, "team", "channel", 50).unwrap(),
        vec!["root-old"]
    );
    let newer = json!({"value":[{"id":"root-new", "messageType":"message", "createdDateTime":"2026-10-19T12:00:00Z", "body":{"content":"New announcement"}}]});
    c::apply_channel_messages_page(&mut conn, &account.id, "team", "channel", None, &newer, NOW)
        .unwrap();
    assert_eq!(
        c::channel_threads_to_poll(&conn, &account.id, "team", "channel", 1).unwrap(),
        vec!["root-new"]
    );
    c::apply_channel_messages_page(
        &mut conn,
        &account.id,
        "team",
        "channel",
        Some("root-new"),
        &json!({"value":[]}),
        "2026-10-19T16:00:00Z",
    )
    .unwrap();
    assert_eq!(
        c::channel_threads_to_poll(&conn, &account.id, "team", "channel", 1).unwrap(),
        vec!["root-old"]
    );
}
