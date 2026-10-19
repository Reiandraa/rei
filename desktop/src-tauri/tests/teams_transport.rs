//! Teams connector: the real sign-in (loopback callback, PKCE S256, ID token RS256
//! verification via JWKS), token refresh/rotation and Graph GET paths through
//! `ureq`, against a local SYNTHETIC HTTP server. The signing key is a labeled
//! test-only fixture. No real tenant, account, token or Teams data is used.
use rei_os_lib::connections;
use rei_os_lib::connectors as c;
use rei_os_lib::db;
use rei_os_lib::providers::{self, ProviderError};
use rei_os_lib::teams::{self, GraphEndpoint, TeamsSettings};
use rei_os_lib::vault::{MemoryStore, SecretKind, SecretStore, VaultKey};
use rusqlite::Connection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

const NOW: &str = "2026-10-19T10:00:00Z";
const CLIENT: &str = "11111111-2222-3333-4444-555555555555";
const TID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const OWNER_OID: &str = "0000aaaa-1111-2222-3333-444455556666";

#[derive(Default)]
struct St {
    challenge: String,
    nonce: String,
    aud: Option<String>,
    token_error: Option<Value>,
    at: String,
    rt: String,
    n: u32,
    fail_401: u32,
    fail_429: u32,
    forbid: bool,
    foreign_next: bool,
    edited: bool,
    oid: Option<String>,
    log: Vec<String>,
}

type Shared = Arc<Mutex<St>>;

fn now_unix() -> i64 {
    NOW.parse::<jiff::Timestamp>().unwrap().as_second()
}

fn id_token(s: &St) -> String {
    let key =
        jsonwebtoken::EncodingKey::from_rsa_pem(include_bytes!("fixtures/synthetic-test-rsa.pem"))
            .unwrap();
    let mut h = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
    h.kid = Some("synthetic-test-key".into());
    let claims = json!({
        "iss": format!("https://login.microsoftonline.com/{TID}/v2.0"),
        "aud": s.aud.clone().unwrap_or(CLIENT.into()), "nonce": s.nonce, "tid": TID,
        "oid": s.oid.clone().unwrap_or(OWNER_OID.into()),
        "exp": now_unix() + 3600, "iat": now_unix(),
    });
    jsonwebtoken::encode(&h, &claims, &key).unwrap()
}

fn msg(id: &str, at: &str, text: &str) -> Value {
    json!({"id": id, "messageType": "message", "createdDateTime": at, "lastModifiedDateTime": at,
           "from": {"user": {"displayName": "Synthetic Tutor"}}, "body": {"contentType": "html", "content": text}})
}

fn route(
    s: &mut St,
    base: &str,
    method: &str,
    path: &str,
    auth: &str,
    body: &str,
) -> (u16, Value, &'static str) {
    s.log.push(format!("{method} {path}"));
    if path.ends_with("/discovery/v2.0/keys") {
        return (
            200,
            serde_json::from_str(include_str!("fixtures/synthetic-test-jwks.json")).unwrap(),
            "",
        );
    }
    if path.ends_with("/oauth2/v2.0/token") {
        if let Some(e) = s.token_error.clone() {
            return (400, e, "");
        }
        let f: HashMap<String, String> = url::form_urlencoded::parse(body.as_bytes())
            .into_owned()
            .collect();
        assert!(!f.contains_key("client_secret"), "public client: no secret");
        match f["grant_type"].as_str() {
            "authorization_code" => {
                let v = &f["code_verifier"];
                let ch = base64url(&Sha256::digest(v.as_bytes()));
                if f["code"] != "SYNTH-CODE" || ch != s.challenge {
                    return (400, json!({"error": "invalid_grant"}), "");
                }
            }
            "refresh_token" if f["refresh_token"] == s.rt => {}
            _ => return (400, json!({"error": "invalid_grant"}), ""),
        }
        s.n += 1;
        s.at = format!("SYNTH-AT-{}", s.n);
        s.rt = format!("SYNTH-RT-{}", s.n);
        return (
            200,
            json!({"access_token": s.at, "refresh_token": s.rt, "id_token": id_token(s), "expires_in": 3600}),
            "",
        );
    }
    if auth != format!("Bearer {}", s.at) || s.fail_401 > 0 {
        s.fail_401 = s.fail_401.saturating_sub(1);
        return (
            401,
            json!({"error": {"code": "InvalidAuthenticationToken"}}),
            "",
        );
    }
    if s.fail_429 > 0 {
        s.fail_429 -= 1;
        return (429, json!({}), "Retry-After: 0\r\n");
    }
    if s.forbid {
        return (403, json!({"error": {"code": "Forbidden"}}), "");
    }
    let p = path.split('?').next().unwrap();
    let v = match p {
        "/v1.0/me/chats" => {
            json!({"value": [{"id": "19:chatA@thread.v2", "topic": "Analysis study group", "chatType": "group"},
                                            {"id": "19:chatB@thread.v2", "topic": null, "chatType": "oneOnOne"}]})
        }
        "/v1.0/me/joinedTeams" => json!({"value": [{"id": "team-1", "displayName": "Mathe WS26"}]}),
        "/v1.0/teams/team-1/channels" => {
            json!({"value": [{"id": "19:general@thread.tacv2", "displayName": "General"}]})
        }
        "/v1.0/chats/19%3AchatA%40thread.v2/messages"
        | "/v1.0/chats/19:chatA@thread.v2/messages" => {
            if path.contains("page=2") {
                json!({"value": [msg("m1", "2026-10-18T08:00:00Z", "<p>Übungsblatt bis 14.11.2026</p>")]})
            } else {
                let next = if s.foreign_next {
                    "https://evil.example/v1.0/x".to_string()
                } else {
                    format!("{base}/v1.0/chats/19:chatA@thread.v2/messages?page=2")
                };
                let m2 = if s.edited {
                    msg("m2", "2026-10-19T09:00:00Z", "Raum geändert: E2.5")
                } else {
                    msg("m2", "2026-10-18T09:00:00Z", "Raum E2.4")
                };
                let mut del = msg("m3", "2026-10-18T09:30:00Z", "temp");
                if s.edited {
                    del["deletedDateTime"] = json!("2026-10-19T09:10:00Z");
                    del["lastModifiedDateTime"] = json!("2026-10-19T09:10:00Z");
                }
                json!({"value": [m2, del], "@odata.nextLink": next})
            }
        }
        "/v1.0/teams/team-1/channels/19%3Ageneral%40thread.tacv2/messages" => {
            json!({"value": [msg("root1", "2026-10-18T07:00:00Z", "Klausurtermin folgt")]})
        }
        "/v1.0/teams/team-1/channels/19%3Ageneral%40thread.tacv2/messages/root1/replies" => {
            json!({"value": [msg("r1", "2026-10-18T07:30:00Z", "Klausur am 03.02.2027")]})
        }
        _ => return (404, json!({}), ""),
    };
    (200, v, "")
}

fn base64url(b: &[u8]) -> String {
    rei_os_lib::oauth::b64url(b)
}

fn serve() -> (String, Shared) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let st: Shared = Arc::new(Mutex::new(St::default()));
    let (b2, s2) = (base.clone(), st.clone());
    std::thread::spawn(move || {
        for sock in l.incoming() {
            let Ok(mut sock) = sock else { return };
            let mut r = BufReader::new(sock.try_clone().unwrap());
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            let (mut auth, mut len) = (String::new(), 0usize);
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
                let (k, v) = h.split_once(':').unwrap();
                if k.eq_ignore_ascii_case("authorization") {
                    auth = v.trim().into();
                }
                if k.eq_ignore_ascii_case("content-length") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; len];
            r.read_exact(&mut body).unwrap();
            let mut it = line.split_whitespace();
            let (m, p) = (
                it.next().unwrap().to_string(),
                it.next().unwrap().to_string(),
            );
            let (code, v, extra) = route(
                &mut s2.lock().unwrap(),
                &b2,
                &m,
                &p,
                &auth,
                &String::from_utf8_lossy(&body),
            );
            let b = v.to_string();
            let _ = write!(sock, "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n{b}", b.len());
        }
    });
    (base, st)
}

struct Fx {
    db: Mutex<Connection>,
    path: std::path::PathBuf,
    _dir: tempfile::TempDir,
    store: MemoryStore,
    ep: GraphEndpoint,
    st: Shared,
    chat: String,
}

fn setup(channels: bool) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rei.sqlite");
    let conn = db::open(&path).unwrap();
    let s = TeamsSettings {
        client_id: CLIENT.into(),
        tenant: TID.into(),
        channels,
    };
    let chat = teams::configure(&conn, None, &s, NOW).unwrap();
    let (base, st) = serve();
    Fx {
        db: Mutex::new(conn),
        path,
        _dir: dir,
        store: MemoryStore::default(),
        ep: GraphEndpoint::synthetic_for_tests(&base),
        st,
        chat,
    }
}

/// Acts as the system browser: reads PKCE/state/nonce from the real authorize URL
/// and delivers a synthetic code (or `error`) to the loopback callback.
fn browser(st: Shared, error: Option<&'static str>) -> impl Fn(&str) -> Result<(), ProviderError> {
    move |u: &str| {
        let url = url::Url::parse(u).unwrap();
        assert!(u.starts_with("https://login.microsoftonline.com/"));
        let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(q["code_challenge_method"], "S256");
        assert!(!q["scope"].contains("ReadWrite") && !q["scope"].contains("Send"));
        {
            let mut s = st.lock().unwrap();
            s.challenge = q["code_challenge"].clone();
            s.nonce = q["nonce"].clone();
        }
        let cb = match error {
            None => format!("{}?code=SYNTH-CODE&state={}", q["redirect_uri"], q["state"]),
            Some(e) => format!("{}?error={e}&state={}", q["redirect_uri"], q["state"]),
        };
        std::thread::spawn(move || {
            let _ = ureq::get(&cb).call();
        });
        Ok(())
    }
}

fn sign_in(fx: &Fx) -> Result<String, ProviderError> {
    teams::sign_in(
        &fx.db,
        &fx.store,
        &fx.ep,
        &fx.chat,
        &browser(fx.st.clone(), None),
        &AtomicBool::new(false),
        now_unix(),
    )
}

fn rt(fx: &Fx) -> Option<Vec<u8>> {
    fx.store
        .get(&VaultKey::new(&fx.chat, SecretKind::TeamsRefresh).unwrap())
        .ok()
        .map(|s| s.expose().to_vec())
}

fn select_chat_a(fx: &Fx) {
    teams::discover(
        &fx.db,
        &fx.store,
        &fx.ep,
        &fx.chat,
        &AtomicBool::new(false),
        NOW,
    )
    .unwrap();
    providers::set_selected(
        &mut fx.db.lock().unwrap(),
        &fx.chat,
        &[("chat".into(), "19:chatA@thread.v2".into())],
    )
    .unwrap();
}

fn fetch(fx: &Fx, acct: &str) -> Result<teams::FetchReport, ProviderError> {
    teams::fetch(
        &fx.db,
        &fx.store,
        &fx.ep,
        acct,
        &AtomicBool::new(false),
        NOW,
    )
}

#[test]
fn teams_transport_pkce_sign_in_verifies_id_token_and_vaults_refresh() {
    let fx = setup(false);
    assert_eq!(sign_in(&fx).unwrap(), TID);
    assert_eq!(rt(&fx).unwrap(), b"SYNTH-RT-1");
    let st = providers::status(&fx.db.lock().unwrap()).unwrap();
    assert!(st
        .iter()
        .all(|a| a.sign_in_saved && a.authenticated_at.is_some()));
    assert!(
        st.iter().all(|a| a.state != "connected"),
        "sign-in alone is not a successful read"
    );
    let bytes = std::fs::read(&fx.path).unwrap();
    for secret in ["SYNTH-RT-1", "SYNTH-AT-1", "SYNTH-CODE"] {
        assert!(
            !bytes.windows(secret.len()).any(|w| w == secret.as_bytes()),
            "{secret} leaked to SQLite"
        );
    }
}

#[test]
fn teams_transport_rejects_wrong_audience_and_tampered_tokens() {
    let fx = setup(false);
    fx.st.lock().unwrap().aud = Some("99999999-2222-3333-4444-555555555555".into());
    assert!(matches!(sign_in(&fx), Err(ProviderError::Protocol(_))));
    assert!(
        rt(&fx).is_none(),
        "nothing stored for an unverified identity"
    );
    let cfg = rei_os_lib::oauth::ClientConfig {
        client_id: CLIENT.into(),
        tenant: TID.into(),
        pinned_tenant_id: Some(TID.into()),
    };
    let mut s = St {
        nonce: "n1".into(),
        ..Default::default()
    };
    let good = id_token(&s);
    assert_eq!(
        teams::verify_id_token(&fx.ep, &cfg, &good, "n1", now_unix()).unwrap(),
        (TID.to_string(), OWNER_OID.to_string())
    );
    assert!(teams::verify_id_token(&fx.ep, &cfg, &good, "other-nonce", now_unix()).is_err());
    let mut parts: Vec<String> = good.split('.').map(str::to_string).collect();
    s.nonce = "n2".into();
    parts[1] = id_token(&s).split('.').nth(1).unwrap().to_string();
    assert!(
        teams::verify_id_token(&fx.ep, &cfg, &parts.join("."), "n2", now_unix()).is_err(),
        "signature must cover claims"
    );
    assert!(
        teams::verify_id_token(&fx.ep, &cfg, &good, "n1", now_unix() + 7200).is_err(),
        "expired"
    );
}

#[test]
fn teams_transport_denial_vs_missing_consent_are_distinct() {
    let fx = setup(false);
    let e = teams::sign_in(
        &fx.db,
        &fx.store,
        &fx.ep,
        &fx.chat,
        &browser(fx.st.clone(), Some("access_denied")),
        &AtomicBool::new(false),
        now_unix(),
    )
    .unwrap_err();
    assert_eq!(e, ProviderError::Denied);
    assert_eq!(
        c::get_account(&fx.db.lock().unwrap(), &fx.chat)
            .unwrap()
            .state
            .as_str(),
        "permission_denied"
    );
    let fx = setup(true);
    fx.st.lock().unwrap().token_error = Some(
        json!({"error": "invalid_client", "error_description": "AADSTS65001: consent required"}),
    );
    assert_eq!(sign_in(&fx).unwrap_err(), ProviderError::Forbidden);
    assert_ne!(
        c::get_account(&fx.db.lock().unwrap(), &fx.chat)
            .unwrap()
            .state
            .as_str(),
        "permission_denied"
    );
    assert!(
        e.to_string().contains("declined")
            && ProviderError::Forbidden
                .to_string()
                .contains("admin consent")
    );
}

#[test]
fn teams_transport_fetches_only_selected_chat_with_paging_and_restart_dedupe() {
    let fx = setup(false);
    sign_in(&fx).unwrap();
    assert!(
        matches!(fetch(&fx, &fx.chat), Err(ProviderError::NotConfigured(_))),
        "nothing selected by default"
    );
    select_chat_a(&fx);
    let srcs = providers::list_sources(&fx.db.lock().unwrap(), &fx.chat).unwrap();
    assert_eq!(srcs.len(), 2);
    let rep = fetch(&fx, &fx.chat).unwrap();
    assert_eq!((rep.inserted, rep.sources), (3, 1), "{rep:?}");
    let log = fx.st.lock().unwrap().log.join("\n");
    assert!(!log.contains("chatB"), "unselected chat never requested");
    assert!(
        log.lines()
            .all(|l| l.starts_with("GET ") || l.contains("/token")),
        "read-only"
    );
    let a = c::get_account(&fx.db.lock().unwrap(), &fx.chat).unwrap();
    assert_eq!(
        (a.state.as_str(), a.evidence.as_str()),
        ("testing", "fixture")
    );
    // Restart, then edit + delete on the provider side.
    *fx.db.lock().unwrap() = db::open(&fx.path).unwrap();
    fx.st.lock().unwrap().edited = true;
    let rep = fetch(&fx, &fx.chat).unwrap();
    assert_eq!(
        (rep.inserted, rep.updated, rep.removed),
        (0, 1, 1),
        "{rep:?}"
    );
    let n: u32 = fx
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM source_messages WHERE deleted_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 2);
    let body: String = fx
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT body_text FROM source_messages WHERE provider_object_id='m2'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(body.contains("E2.5") && !body.contains("<"));
}

#[test]
fn teams_transport_401_refreshes_once_and_rotates_429_retries() {
    let fx = setup(false);
    sign_in(&fx).unwrap();
    select_chat_a(&fx);
    teams::forget_access(&fx.chat);
    {
        let mut s = fx.st.lock().unwrap();
        s.fail_429 = 1;
        s.at = "server-revoked".into();
    }
    fetch(&fx, &fx.chat).unwrap();
    assert!(
        rt(&fx).unwrap().starts_with(b"SYNTH-RT-"),
        "rotated refresh token stored"
    );
    assert_ne!(rt(&fx).unwrap(), b"SYNTH-RT-1");
    fx.st.lock().unwrap().fail_401 = 10;
    assert_eq!(
        fetch(&fx, &fx.chat).unwrap_err(),
        ProviderError::Auth,
        "only one refresh attempt"
    );
    assert_eq!(
        c::get_account(&fx.db.lock().unwrap(), &fx.chat)
            .unwrap()
            .state
            .as_str(),
        "expired"
    );
}

#[test]
fn teams_transport_403_and_foreign_next_link_are_refused() {
    let fx = setup(false);
    sign_in(&fx).unwrap();
    select_chat_a(&fx);
    fx.st.lock().unwrap().foreign_next = true;
    assert_eq!(
        fetch(&fx, &fx.chat).unwrap_err(),
        ProviderError::Destination
    );
    assert!(!fx.st.lock().unwrap().log.iter().any(|l| l.contains("evil")));
    fx.st.lock().unwrap().foreign_next = false;
    fx.st.lock().unwrap().forbid = true;
    assert_eq!(fetch(&fx, &fx.chat).unwrap_err(), ProviderError::Forbidden);
    let a = c::get_account(&fx.db.lock().unwrap(), &fx.chat).unwrap();
    assert_eq!(
        a.state.as_str(),
        "degraded",
        "403 is not recorded as a proven denial"
    );
    assert!(a.last_error.unwrap().contains("admin consent"));
}

#[test]
fn teams_transport_selected_channel_messages_and_replies() {
    let fx = setup(true);
    sign_in(&fx).unwrap();
    teams::discover(
        &fx.db,
        &fx.store,
        &fx.ep,
        &fx.chat,
        &AtomicBool::new(false),
        NOW,
    )
    .unwrap();
    let chan = teams::linked(&fx.db.lock().unwrap(), &fx.chat)
        .unwrap()
        .into_iter()
        .find(|a| a != &fx.chat)
        .unwrap();
    let srcs = providers::list_sources(&fx.db.lock().unwrap(), &chan).unwrap();
    assert_eq!(srcs[0].display_name, "Mathe WS26 / General");
    assert!(!srcs[0].selected);
    providers::set_selected(
        &mut fx.db.lock().unwrap(),
        &chan,
        &[("channel".into(), "team-1/19:general@thread.tacv2".into())],
    )
    .unwrap();
    let rep = fetch(&fx, &chan).unwrap();
    assert_eq!((rep.inserted, rep.threads_reconciled), (2, 1), "{rep:?}");
    let parent: String = fx
        .db
        .lock()
        .unwrap()
        .query_row(
            "SELECT parent_id FROM source_messages WHERE provider_object_id='r1'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(parent, "root1");
    // Disconnect removes the shared refresh token for both linked accounts.
    connections::disconnect(&mut fx.db.lock().unwrap(), &fx.store, &chan, false).unwrap();
    assert!(rt(&fx).is_none());
    assert_eq!(
        c::get_account(&fx.db.lock().unwrap(), &fx.chat)
            .unwrap()
            .state
            .as_str(),
        "not_configured"
    );
}

#[test]
fn teams_settings_ids_and_ipc_requests_are_validated() {
    for (id, t) in [
        ("not-a-guid", "organizations"),
        (CLIENT, "common"),
        (CLIENT, "../x"),
    ] {
        assert!(TeamsSettings {
            client_id: id.into(),
            tenant: t.into(),
            channels: false
        }
        .validate()
        .is_err());
    }
    for bad in ["../me", "a/b", "x?y", "", "id with space"] {
        assert!(!teams::valid_graph_id(bad), "{bad}");
    }
    let r: Result<connections::TeamsConfigureReq, _> = serde_json::from_value(json!({
        "client_id": CLIENT, "tenant": TID, "channels": false, "client_secret": "x"}));
    assert!(r.is_err(), "no client secret accepted");
    assert_eq!(GraphEndpoint::production().evidence(), c::Evidence::Live);
    let s = TeamsSettings {
        client_id: CLIENT.into(),
        tenant: TID.into(),
        channels: false,
    };
    assert!(
        !s.scopes().iter().any(|x| x.contains("Channel")),
        "channel scopes only when requested"
    );
    let _ = std::io::sink().flush();
}

#[test]
fn teams_transport_unmatched_state_denial_is_not_a_denial() {
    // A local process can reach the loopback callback; without this flow's state an
    // `error=access_denied` must not mark the account permission_denied.
    let fx = setup(false);
    let forged = |u: &str| -> Result<(), ProviderError> {
        let url = url::Url::parse(u).unwrap();
        let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
        let cb = format!("{}?error=access_denied&state=FORGED", q["redirect_uri"]);
        std::thread::spawn(move || {
            let _ = ureq::get(&cb).call();
        });
        Ok(())
    };
    let e = teams::sign_in(
        &fx.db,
        &fx.store,
        &fx.ep,
        &fx.chat,
        &forged,
        &AtomicBool::new(false),
        now_unix(),
    )
    .unwrap_err();
    assert_ne!(e, ProviderError::Denied);
    assert_ne!(
        c::get_account(&fx.db.lock().unwrap(), &fx.chat)
            .unwrap()
            .state
            .as_str(),
        "permission_denied"
    );
    assert!(rt(&fx).is_none(), "nothing saved");
}

#[test]
fn teams_transport_account_is_bound_to_first_verified_user_and_tenant() {
    let fx = setup(false);
    sign_in(&fx).unwrap();
    assert_eq!(rt(&fx).unwrap(), b"SYNTH-RT-1");
    // Same user again: allowed (re-authentication).
    sign_in(&fx).unwrap();
    let rt_before = rt(&fx).unwrap();
    // A different user of the same tenant signs in: refused, vault untouched.
    fx.st.lock().unwrap().oid = Some("9999aaaa-1111-2222-3333-444455556666".into());
    let e = sign_in(&fx).unwrap_err();
    assert!(e.to_string().contains("different Microsoft account"), "{e}");
    assert_eq!(rt(&fx).unwrap(), rt_before, "other user's token not stored");
    // An ID token without a user object ID is not accepted as an identity.
    fx.st.lock().unwrap().oid = Some(String::new());
    assert!(matches!(sign_in(&fx), Err(ProviderError::Protocol(_))));
    // The bound account cannot be re-pointed at another tenant or app registration.
    let other = TeamsSettings {
        client_id: CLIENT.into(),
        tenant: "organizations".into(),
        channels: false,
    };
    let c = fx.db.lock().unwrap();
    assert!(matches!(
        teams::configure(&c, Some(&fx.chat), &other, NOW),
        Err(ProviderError::Invalid(_))
    ));
    // Unchanged settings (e.g. toggling nothing) keep the binding.
    let same = TeamsSettings {
        client_id: CLIENT.into(),
        tenant: TID.into(),
        channels: false,
    };
    teams::configure(&c, Some(&fx.chat), &same, NOW).unwrap();
    let cfg = providers::load_config(&c, &fx.chat, teams::PROVIDER).unwrap();
    assert_eq!(cfg["verified_oid"], OWNER_OID);
    assert_eq!(cfg["verified_tid"], TID);
}
