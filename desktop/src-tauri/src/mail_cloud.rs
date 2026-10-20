//! Publisher-configured, read-only Gmail / Microsoft mail. System-browser PKCE,
//! native loopback callback and OS-vault refresh tokens. No end-user client IDs.
use crate::{
    connectors::{self, Evidence, Kind},
    email_discovery::{GOOGLE, MICROSOFT},
    oauth,
    providers::{self, PRes, ProviderError},
    vault::{Secret, SecretKind, SecretStore, VaultKey},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};
const GOOGLE_SCOPE: &str = "https://www.googleapis.com/auth/gmail.readonly";
const MS_SCOPE: &str = "offline_access User.Read Mail.Read";
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_PAGES: usize = 20;

#[derive(Deserialize, Default)]
struct PublisherIds {
    #[serde(default)]
    google_client_id: String,
    #[serde(default)]
    microsoft_client_id: String,
}
static PUBLISHER: std::sync::OnceLock<PublisherIds> = std::sync::OnceLock::new();
fn client_id(provider: &str) -> Option<&'static str> {
    let published = PUBLISHER.get_or_init(|| {
        serde_json::from_str(include_str!("../../email-publisher.json")).unwrap_or_default()
    });
    let value = match provider {
        GOOGLE => {
            Some(option_env!("REI_GOOGLE_CLIENT_ID").unwrap_or(published.google_client_id.as_str()))
        }
        MICROSOFT => Some(
            option_env!("REI_MICROSOFT_CLIENT_ID")
                .unwrap_or(published.microsoft_client_id.as_str()),
        ),
        _ => None,
    }?;
    let valid = if provider == GOOGLE {
        value.ends_with(".apps.googleusercontent.com")
            && value.len() < 256
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b))
    } else {
        uuid::Uuid::parse_str(value).is_ok()
    };
    valid.then_some(value)
}
pub fn publisher_ready(provider: &str) -> bool {
    client_id(provider).is_some()
}
fn client(provider: &str) -> PRes<&'static str> {
    client_id(provider).ok_or(ProviderError::NotConfigured("the REI publisher must enable this sign-in provider once; users do not need developer settings"))
}
fn refresh_key(id: &str) -> PRes<VaultKey> {
    VaultKey::new(id, SecretKind::Refresh).map_err(|_| ProviderError::Invalid("account id".into()))
}
fn api_url(provider: &str, path: &str) -> String {
    format!(
        "{}{path}",
        if provider == GOOGLE {
            "https://gmail.googleapis.com/gmail/v1/users/me"
        } else {
            "https://graph.microsoft.com/v1.0/me"
        }
    )
}
fn token_url(provider: &str) -> &'static str {
    if provider == GOOGLE {
        "https://oauth2.googleapis.com/token"
    } else {
        "https://login.microsoftonline.com/common/oauth2/v2.0/token"
    }
}
fn valid_url(provider: &str, url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else {
        return false;
    };
    u.scheme() == "https"
        && u.port_or_known_default() == Some(443)
        && u.username().is_empty()
        && u.password().is_none()
        && u.fragment().is_none()
        && if provider == GOOGLE {
            u.host_str() == Some("gmail.googleapis.com")
                && u.path().starts_with("/gmail/v1/users/me/")
        } else {
            u.host_str() == Some("graph.microsoft.com")
                && (u.path() == "/v1.0/me" || u.path().starts_with("/v1.0/me/"))
        }
}
fn network_error(e: ureq::Error) -> ProviderError {
    match e {
        ureq::Error::Status(404, _) => {
            ProviderError::Protocol("email object or history cursor no longer available")
        }
        ureq::Error::Status(401, _) => ProviderError::Auth,
        ureq::Error::Status(403, _) => ProviderError::Forbidden,
        ureq::Error::Status(429, _) => ProviderError::TimedOut,
        _ => ProviderError::Network,
    }
}
fn read_json(response: ureq::Response) -> PRes<Value> {
    let mut data = Vec::new();
    response
        .into_reader()
        .take(MAX_BYTES + 1)
        .read_to_end(&mut data)
        .map_err(|_| ProviderError::Network)?;
    if data.len() as u64 > MAX_BYTES {
        return Err(ProviderError::Protocol("email response too large"));
    }
    serde_json::from_slice(&data).map_err(|_| ProviderError::Protocol("invalid email response"))
}
pub trait CloudTransport {
    fn get(&self, provider: &str, url: &str, token: &Secret) -> PRes<Value>;
    fn token(&self, provider: &str, form: &[(&str, &str)]) -> PRes<Value>;
}
pub struct Https;
fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .redirects(0)
        .try_proxy_from_env(false)
        .timeout(Duration::from_secs(25))
        .build()
}
impl CloudTransport for Https {
    fn get(&self, provider: &str, url: &str, token: &Secret) -> PRes<Value> {
        if !valid_url(provider, url) {
            return Err(ProviderError::Destination);
        }
        let value = std::str::from_utf8(token.expose()).map_err(|_| ProviderError::Auth)?;
        read_json(
            agent()
                .get(url)
                .set("Authorization", &format!("Bearer {value}"))
                .set("Prefer", "outlook.body-content-type=\"text\"")
                .call()
                .map_err(network_error)?,
        )
    }
    fn token(&self, provider: &str, form: &[(&str, &str)]) -> PRes<Value> {
        read_json(
            agent()
                .post(token_url(provider))
                .send_form(form)
                .map_err(|e| match e {
                    ureq::Error::Status(400 | 401, _) => ProviderError::Auth,
                    other => network_error(other),
                })?,
        )
    }
}
pub fn configure(conn: &Connection, provider: &str, email: &str, now: &str) -> PRes<String> {
    client(provider)?;
    if !email.is_empty() {
        crate::email_discovery::parts(email)?;
    }
    let name = if provider == GOOGLE {
        "Google email"
    } else if provider == MICROSOFT {
        "Microsoft email"
    } else {
        return Err(ProviderError::Destination);
    };
    let account = connectors::create_account(conn, Kind::Email, name, name, now)?;
    connectors::transition(
        conn,
        &account.id,
        connectors::ConnState::ConsentNeeded,
        Evidence::None,
        None,
        now,
    )?;
    providers::save_config(
        conn,
        &account.id,
        provider,
        &json!({"email":email,"username":email,"initial_count":50}),
        now,
    )?;
    Ok(account.id)
}
fn random() -> PRes<String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| ProviderError::Network)?;
    Ok(oauth::b64url(&bytes))
}
fn authorize_url(
    provider: &str,
    id: &str,
    email: &str,
    redirect: &str,
    state: &str,
    verifier: &str,
) -> PRes<String> {
    let mut u = url::Url::parse(if provider == GOOGLE {
        "https://accounts.google.com/o/oauth2/v2/auth"
    } else {
        "https://login.microsoftonline.com/common/oauth2/v2.0/authorize"
    })
    .map_err(|_| ProviderError::Destination)?;
    u.query_pairs_mut().extend_pairs([
        ("client_id", id),
        ("response_type", "code"),
        ("redirect_uri", redirect),
        (
            "scope",
            if provider == GOOGLE {
                GOOGLE_SCOPE
            } else {
                MS_SCOPE
            },
        ),
        ("state", state),
        ("code_challenge", &oauth::pkce_challenge(verifier)),
        ("code_challenge_method", "S256"),
    ]);
    if !email.is_empty() {
        u.query_pairs_mut().append_pair("login_hint", email);
    }
    if provider == GOOGLE {
        u.query_pairs_mut()
            .append_pair("access_type", "offline")
            .append_pair("prompt", "consent");
    }
    Ok(u.into())
}
fn callback_query(line: &str, state: &str) -> PRes<String> {
    let mut words = line.split_whitespace();
    if words.next() != Some("GET") {
        return Err(ProviderError::Protocol("invalid login callback"));
    }
    let path = words
        .next()
        .and_then(|p| p.strip_prefix("/callback?"))
        .ok_or(ProviderError::Protocol("invalid login callback"))?;
    let pairs: Vec<(String, String)> = url::form_urlencoded::parse(path.as_bytes())
        .into_owned()
        .collect();
    for key in ["state", "code", "error"] {
        if pairs.iter().filter(|(k, _)| k == key).count() > 1 {
            return Err(ProviderError::Protocol("duplicate login callback field"));
        }
    }
    let get = |key: &str| {
        pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    if get("state") != Some(state) {
        return Err(ProviderError::Protocol(
            "login response did not match this request",
        ));
    }
    if get("error") == Some("access_denied") {
        return Err(ProviderError::Denied);
    }
    if get("error").is_some() {
        return Err(ProviderError::Auth);
    }
    get("code")
        .filter(|s| !s.is_empty() && s.len() < 4096)
        .map(str::to_string)
        .ok_or(ProviderError::Auth)
}
fn await_code(listener: TcpListener, state: &str, cancel: &AtomicBool) -> PRes<String> {
    listener
        .set_nonblocking(true)
        .map_err(|_| ProviderError::Network)?;
    let start = Instant::now();
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(ProviderError::Cancelled);
        }
        if start.elapsed() > Duration::from_secs(300) {
            return Err(ProviderError::TimedOut);
        }
        match listener.accept() {
            Ok((mut socket, peer)) => {
                if !peer.ip().is_loopback() {
                    continue;
                }
                socket.set_nonblocking(false).ok();
                socket.set_read_timeout(Some(Duration::from_secs(3))).ok();
                let mut line = String::new();
                let n = BufReader::new((&socket).take(8193))
                    .read_line(&mut line)
                    .map_err(|_| ProviderError::Network)?;
                if n > 8192 || !line.starts_with("GET /callback?") {
                    let _ = socket.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    continue;
                }
                let response = callback_query(&line, state);
                let page="<!doctype html><meta charset=utf-8><title>REI</title><p>Return to REI to check your email connection.</p>";
                let _=write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",page.len());
                // Unmatched local callbacks do not consume or cancel the owner's flow.
                if matches!(&response, Err(ProviderError::Protocol(_))) {
                    continue;
                }
                return response;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(_) => return Err(ProviderError::Network),
        }
    }
}
fn access(value: &Value, provider: &str) -> PRes<Secret> {
    if value.get("error").is_some() {
        return Err(ProviderError::Auth);
    }
    if let Some(scope) = value["scope"].as_str() {
        let wanted = if provider == GOOGLE {
            GOOGLE_SCOPE
        } else {
            "Mail.Read"
        };
        if !scope
            .split_whitespace()
            .any(|s| s == wanted || provider == MICROSOFT && s.rsplit('/').next() == Some(wanted))
        {
            return Err(ProviderError::Forbidden);
        }
    }
    let token = value["access_token"]
        .as_str()
        .filter(|t| !t.is_empty() && t.len() < 16384 && !t.contains(['\r', '\n']))
        .ok_or(ProviderError::Auth)?;
    Ok(Secret::new(token.as_bytes().to_vec()))
}
fn identity(provider: &str, value: &Value, expected: &str) -> PRes<(String, String)> {
    let email = if provider == GOOGLE {
        value["emailAddress"].as_str()
    } else {
        value["mail"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| value["userPrincipalName"].as_str())
    }
    .ok_or(ProviderError::Auth)?;
    crate::email_discovery::parts(email)?;
    let matches = expected.is_empty()
        || email.eq_ignore_ascii_case(expected)
        || provider == MICROSOFT
            && value["userPrincipalName"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case(expected));
    if !matches {
        return Err(ProviderError::Invalid("the signed-in account differs from the entered email; enter its primary address or choose the intended account".into()));
    }
    let subject = if provider == GOOGLE {
        email.to_ascii_lowercase()
    } else {
        value["id"].as_str().ok_or(ProviderError::Auth)?.into()
    };
    Ok((email.into(), subject))
}
fn profile_request(provider: &str) -> String {
    api_url(
        provider,
        if provider == GOOGLE {
            "/profile"
        } else {
            "?$select=id,mail,userPrincipalName"
        },
    )
}
pub fn sign_in(
    db: &Mutex<Connection>,
    store: &dyn SecretStore,
    id: &str,
    cancel: &AtomicBool,
    open: &dyn Fn(&str) -> PRes<()>,
) -> PRes<String> {
    let (provider, cfg) = {
        let c = providers::lock(db)?;
        let p = crate::email_discovery::require_email(&c, id)?;
        if !matches!(p.as_str(), GOOGLE | MICROSOFT) {
            return Err(ProviderError::Destination);
        }
        let cfg = providers::load_config(&c, id, &p)?;
        (p, cfg)
    };
    let client = client(&provider)?;
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|_| ProviderError::Network)?;
    let redirect = format!(
        "http://{}:{}/callback",
        if provider == MICROSOFT {
            "localhost"
        } else {
            "127.0.0.1"
        },
        listener
            .local_addr()
            .map_err(|_| ProviderError::Network)?
            .port()
    );
    let state = random()?;
    let verifier = random()?;
    let email = cfg["email"].as_str().unwrap_or("");
    open(&authorize_url(
        &provider, client, email, &redirect, &state, &verifier,
    )?)?;
    let code = await_code(listener, &state, cancel)?;
    let tokens = Https.token(
        &provider,
        &[
            ("client_id", client),
            ("code", &code),
            ("code_verifier", &verifier),
            ("redirect_uri", &redirect),
            ("grant_type", "authorization_code"),
        ],
    )?;
    if cancel.load(Ordering::SeqCst) {
        return Err(ProviderError::Cancelled);
    }
    let token = access(&tokens, &provider)?;
    let who = Https.get(&provider, &profile_request(&provider), &token)?;
    let (email, subject) = identity(&provider, &who, email)?;
    if cfg["subject"].as_str().is_some_and(|s| s != subject) {
        return Err(ProviderError::Auth);
    }
    let refresh = tokens["refresh_token"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() < 16384)
        .ok_or(ProviderError::NotConfigured(
            "offline access was not granted; sign in and allow REI to keep the connection",
        ))?;
    if cancel.load(Ordering::SeqCst) {
        return Err(ProviderError::Cancelled);
    }
    store
        .put(&refresh_key(id)?, &Secret::new(refresh.as_bytes().to_vec()))
        .map_err(|_| ProviderError::VaultUnavailable)?;
    let c = providers::lock(db)?;
    let now = crate::time::now_utc_rfc3339();
    let mut updated = cfg;
    updated["email"] = email.clone().into();
    updated["username"] = email.clone().into();
    updated["subject"] = subject.into();
    providers::save_config(&c, id, &provider, &updated, &now)?;
    providers::set_secret_saved(&c, id, true, &now)?;
    providers::record_authenticated(&c, id, &now)?;
    providers::upsert_discovered(
        &c,
        id,
        &[("folder".into(), "INBOX".into(), "Inbox".into())],
        &now,
    )?;
    Ok(email)
}
fn token_for(
    db: &Mutex<Connection>,
    store: &dyn SecretStore,
    id: &str,
    provider: &str,
    cfg: &Value,
    http: &dyn CloudTransport,
) -> PRes<Secret> {
    let secret = store
        .get(&refresh_key(id)?)
        .map_err(|_| ProviderError::SecretMissing)?;
    let refresh = std::str::from_utf8(secret.expose()).map_err(|_| ProviderError::Auth)?;
    let tokens = http.token(
        provider,
        &[
            ("client_id", client(provider)?),
            ("refresh_token", refresh),
            ("grant_type", "refresh_token"),
        ],
    )?;
    let token = access(&tokens, provider)?;
    let who = http.get(provider, &profile_request(provider), &token)?;
    let (_, subject) = identity(provider, &who, cfg["email"].as_str().unwrap_or(""))?;
    if cfg["subject"].as_str() != Some(subject.as_str()) {
        return Err(ProviderError::Auth);
    }
    if let Some(refresh) = tokens["refresh_token"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() < 16384)
    {
        store
            .rotate(&refresh_key(id)?, &Secret::new(refresh.as_bytes().to_vec()))
            .map_err(|_| ProviderError::VaultUnavailable)?;
    }
    // Lock is not held across OAuth, profile or message requests.
    drop(providers::lock(db)?);
    Ok(token)
}
fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 512
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_=".contains(&b))
}
fn google_text(
    payload: &Value,
    depth: u8,
    budget: &mut usize,
    plain: &mut String,
    html: &mut String,
) {
    if depth > 16 || *budget == 0 {
        return;
    }
    *budget -= 1;
    if let Some(data) = payload["body"]["data"]
        .as_str()
        .filter(|s| s.len() < 1_000_000)
    {
        if let Ok(bytes) = URL_SAFE_NO_PAD.decode(data.trim_end_matches('=')) {
            let text = String::from_utf8_lossy(&bytes);
            match payload["mimeType"].as_str() {
                Some("text/plain") => {
                    plain.extend(text.chars().take(64000 - plain.chars().count().min(64000)))
                }
                Some("text/html") => {
                    html.extend(text.chars().take(64000 - html.chars().count().min(64000)))
                }
                _ => {}
            }
        }
    }
    if let Some(parts) = payload["parts"].as_array() {
        for part in parts.iter().take(64) {
            google_text(part, depth + 1, budget, plain, html);
        }
    }
}
fn gmail_message(value: &Value) -> PRes<Value> {
    let id = value["id"]
        .as_str()
        .filter(|s| safe_id(s))
        .ok_or(ProviderError::Protocol("invalid email id"))?;
    let received = value["internalDate"]
        .as_str()
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|n| *n >= 0 && *n <= 253402300799000)
        .ok_or(ProviderError::Protocol("invalid email date"))?;
    let headers = value["payload"]["headers"]
        .as_array()
        .ok_or(ProviderError::Protocol("email headers missing"))?;
    let header = |name: &str| {
        headers
            .iter()
            .find(|h| {
                h["name"]
                    .as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case(name))
            })
            .and_then(|h| h["value"].as_str())
            .unwrap_or("")
    };
    let mut plain = String::new();
    let mut html = String::new();
    google_text(&value["payload"], 0, &mut 128, &mut plain, &mut html);
    let body = if plain.trim().is_empty() {
        connectors::html_to_text(&html)
    } else {
        plain
    };
    Ok(
        json!({"id":id,"receivedDateTime":crate::time::format_unix(received/1000),"subject":header("Subject"),"from":{"emailAddress":{"address":header("From")}},"body":{"contentType":"text","content":body},"conversationId":value["threadId"]}),
    )
}
// Read current membership so arrival/deletion/archive races do not stall a cursor.
fn gmail_current(http: &dyn CloudTransport, token: &Secret, message: &str) -> PRes<Value> {
    match http.get(
        GOOGLE,
        &api_url(GOOGLE, &format!("/messages/{message}?format=full")),
        token,
    ) {
        Ok(value) => {
            if !value["labelIds"]
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == "INBOX"))
            {
                return Ok(json!({"id":message,"@removed":{"reason":"removed from Inbox"}}));
            }
            gmail_message(&value)
        }
        Err(ProviderError::Protocol("email object or history cursor no longer available")) => {
            Ok(json!({"id":message,"@removed":{"reason":"deleted"}}))
        }
        Err(e) => Err(e),
    }
}
fn checkpoint(conn: &Connection, id: &str, provider: &str, cfg: &Value, now: &str) -> PRes<()> {
    providers::save_config(conn, id, provider, cfg, now)
}
fn fetch_gmail(
    db: &Mutex<Connection>,
    http: &dyn CloudTransport,
    id: &str,
    token: &Secret,
    mut cfg: Value,
    cancel: &AtomicBool,
    now: &str,
) -> PRes<usize> {
    let initial = cfg["history_id"].as_str().is_none();
    let mut count = 0;
    if initial {
        let profile = http.get(GOOGLE, &api_url(GOOGLE, "/profile"), token)?;
        let history = profile["historyId"]
            .as_str()
            .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
            .ok_or(ProviderError::Protocol("mail history missing"))?;
        let page = http.get(
            GOOGLE,
            &api_url(GOOGLE, "/messages?labelIds=INBOX&maxResults=50"),
            token,
        )?;
        let mut values = Vec::new();
        if let Some(messages) = page["messages"].as_array() {
            for row in messages.iter().take(50) {
                if cancel.load(Ordering::SeqCst) {
                    return Err(ProviderError::Cancelled);
                }
                let message = row["id"]
                    .as_str()
                    .filter(|s| safe_id(s))
                    .ok_or(ProviderError::Protocol("invalid email id"))?;
                values.push(gmail_current(http, token, message)?);
            }
        }
        let mut c = providers::lock(db)?;
        connectors::apply_mail_delta_page(&mut c, id, &json!({"value":values}), now)?;
        count += values.len();
        cfg["history_id"] = history.into();
        cfg["next_page"] = Value::Null;
        checkpoint(&c, id, GOOGLE, &cfg, now)?;
        return Ok(count);
    }
    let history = cfg["history_id"]
        .as_str()
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .ok_or(ProviderError::Protocol("mail history invalid"))?
        .to_string();
    for _ in 0..MAX_PAGES {
        if cancel.load(Ordering::SeqCst) {
            return Err(ProviderError::Cancelled);
        }
        let mut url = url::Url::parse(&api_url(GOOGLE, "/history"))
            .map_err(|_| ProviderError::Destination)?;
        url.query_pairs_mut()
            .append_pair("startHistoryId", &history)
            .append_pair("maxResults", "20");
        if let Some(page) = cfg["next_page"].as_str() {
            if page.len() > 4096 {
                return Err(ProviderError::Protocol("mail page cursor too long"));
            }
            url.query_pairs_mut().append_pair("pageToken", page);
        }
        let page = match http.get(GOOGLE, url.as_str(), token) {
            Ok(p) => p,
            Err(ProviderError::Protocol("email object or history cursor no longer available")) => {
                cfg["history_id"] = Value::Null;
                cfg["next_page"] = Value::Null;
                let c = providers::lock(db)?;
                checkpoint(&c, id, GOOGLE, &cfg, now)?;
                return Err(ProviderError::NotConfigured("Google email history expired. The next check imports the latest 50 Inbox messages; older stored messages are kept."));
            }
            Err(e) => return Err(e),
        };
        let mut values = Vec::new();
        let mut changed = std::collections::BTreeSet::new();
        for entry in page["history"].as_array().into_iter().flatten() {
            for kind in [
                "messagesAdded",
                "messagesDeleted",
                "labelsAdded",
                "labelsRemoved",
            ] {
                for row in entry[kind].as_array().into_iter().flatten() {
                    let message = row["message"]["id"]
                        .as_str()
                        .filter(|s| safe_id(s))
                        .ok_or(ProviderError::Protocol("invalid email id"))?;
                    changed.insert(message.to_string());
                    if changed.len() > 500 {
                        return Err(ProviderError::Protocol(
                            "too many email changes in one page; retry with provider support",
                        ));
                    }
                }
            }
        }
        for message in changed {
            if cancel.load(Ordering::SeqCst) {
                return Err(ProviderError::Cancelled);
            }
            values.push(gmail_current(http, token, &message)?);
        }
        let next = page["nextPageToken"].as_str().filter(|s| !s.is_empty());
        if next.is_none()
            && !page["historyId"]
                .as_str()
                .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err(ProviderError::Protocol("mail history missing"));
        }
        let mut c = providers::lock(db)?;
        connectors::apply_mail_delta_page(&mut c, id, &json!({"value":values}), now)?;
        count += values.len();
        cfg["next_page"] = next.map(Value::from).unwrap_or(Value::Null);
        if next.is_none() {
            cfg["history_id"] = page["historyId"].clone();
        }
        checkpoint(&c, id, GOOGLE, &cfg, now)?;
        if next.is_none() {
            return Ok(count);
        }
    }
    Ok(count)
}
fn fetch_microsoft(
    db: &Mutex<Connection>,
    http: &dyn CloudTransport,
    id: &str,
    token: &Secret,
    cancel: &AtomicBool,
    now: &str,
) -> PRes<usize> {
    let mut count = 0;
    for _ in 0..MAX_PAGES {
        if cancel.load(Ordering::SeqCst) {
            return Err(ProviderError::Cancelled);
        }
        let url = {
            let c = providers::lock(db)?;
            connectors::mail_next_request(&c, id)?
        };
        if !valid_url(MICROSOFT, &url) {
            return Err(ProviderError::Destination);
        }
        let page = http.get(MICROSOFT, &url, token)?;
        if !page["@odata.nextLink"].is_string() && !page["@odata.deltaLink"].is_string() {
            return Err(ProviderError::Protocol(
                "email page has no continuation cursor",
            ));
        }
        let complete = {
            let mut c = providers::lock(db)?;
            let report = connectors::apply_mail_delta_page(&mut c, id, &page, now)?;
            count += report.inserted as usize + report.updated as usize;
            report.complete
        };
        if complete {
            return Ok(count);
        }
    }
    Ok(count)
}
pub fn fetch(
    db: &Mutex<Connection>,
    store: &dyn SecretStore,
    id: &str,
    cancel: &AtomicBool,
    now: &str,
) -> PRes<Value> {
    let result = (|| {
        let (provider, cfg) = {
            let c = providers::lock(db)?;
            let p = crate::email_discovery::require_email(&c, id)?;
            let cfg = providers::load_config(&c, id, &p)?;
            if providers::selected(&c, id, "folder")?.is_empty() {
                return Err(ProviderError::NotConfigured("choose Inbox to read"));
            }
            (p, cfg)
        };
        let token = token_for(db, store, id, &provider, &cfg, &Https)?;
        let count = if provider == GOOGLE {
            fetch_gmail(db, &Https, id, &token, cfg, cancel, now)?
        } else if provider == MICROSOFT {
            fetch_microsoft(db, &Https, id, &token, cancel, now)?
        } else {
            return Err(ProviderError::Destination);
        };
        let more = {
            let c = providers::lock(db)?;
            if provider == GOOGLE {
                providers::load_config(&c, id, GOOGLE)?["next_page"].is_string()
            } else {
                c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM source_cursors WHERE account_id=?1 AND stream=?2)",
                    rusqlite::params![id, format!("{}:next", connectors::MAIL_STREAM)],
                    |r| r.get::<_, bool>(0),
                )?
            }
        };
        Ok(json!({"processed":count,"more_remaining":more}))
    })();
    let c = providers::lock(db)?;
    match &result {
        Ok(_) => {
            let more = result
                .as_ref()
                .ok()
                .and_then(|r| r["more_remaining"].as_bool())
                .unwrap_or(false);
            providers::record_outcome(
                &c,
                id,
                true,
                if more {
                    "Email page saved; more messages are waiting for the next check."
                } else {
                    "Read-only email check completed."
                },
                now,
            )?;
            providers::mark_read_ok(&c, id, Evidence::Live, now)?;
        }
        Err(e) => {
            providers::record_outcome(&c, id, false, &e.to_string(), now)?;
            providers::mark_failed(&c, id, e, now)?;
        }
    }
    result
}
pub fn disconnect(store: &dyn SecretStore, id: &str) -> PRes<()> {
    match store.delete(&refresh_key(id)?) {
        Ok(()) | Err(crate::vault::VaultError::NotFound) => Ok(()),
        Err(_) => Err(ProviderError::VaultUnavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callback_binds_state_and_rejects_duplicates_before_denial() {
        assert_eq!(
            callback_query("GET /callback?state=ok&code=abc HTTP/1.1", "ok").unwrap(),
            "abc"
        );
        assert_eq!(
            callback_query("GET /callback?state=ok&error=access_denied HTTP/1.1", "ok"),
            Err(ProviderError::Denied)
        );
        assert!(callback_query(
            "GET /callback?state=evil&error=access_denied HTTP/1.1",
            "ok"
        )
        .is_err());
        assert!(callback_query("GET /callback?state=ok&state=ok&code=x HTTP/1.1", "ok").is_err());
    }
    #[test]
    fn browser_urls_use_pkce_readonly_scopes_and_fixed_provider() {
        let u = authorize_url(
            GOOGLE,
            "test.apps.googleusercontent.com",
            "",
            "http://127.0.0.1:12345/callback",
            "state",
            "verifier",
        )
        .unwrap();
        let u = url::Url::parse(&u).unwrap();
        assert_eq!(u.host_str(), Some("accounts.google.com"));
        let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(q["scope"], GOOGLE_SCOPE);
        assert_eq!(q["code_challenge"], oauth::pkce_challenge("verifier"));
        assert_eq!(q["code_challenge_method"], "S256");
    }
    #[test]
    fn bearer_tokens_cannot_follow_other_origins_or_paths() {
        for url in [
            "http://gmail.googleapis.com/gmail/v1/users/me/profile",
            "https://gmail.googleapis.com.evil.org/gmail/v1/users/me/profile",
            "https://graph.microsoft.com/v1.0/users/other/messages",
            "https://name@graph.microsoft.com/v1.0/me/messages",
        ] {
            assert!(!valid_url(GOOGLE, url));
            assert!(!valid_url(MICROSOFT, url));
        }
        assert!(valid_url(MICROSOFT, connectors::MAIL_DELTA_START));
    }
    #[test]
    fn normalizes_gmail_text_without_opening_remote_content_or_attachments() {
        let msg=gmail_message(&json!({"id":"abc123","labelIds":["INBOX"],"internalDate":"1792454400000","payload":{"mimeType":"multipart/mixed","headers":[{"name":"Subject","value":"Synthetic class notice"}],"parts":[{"mimeType":"text/plain","body":{"data":URL_SAFE_NO_PAD.encode("Synthetic lesson update")}},{"mimeType":"image/png","body":{"data":URL_SAFE_NO_PAD.encode("ignored attachment")}}]}})).unwrap();
        assert_eq!(msg["body"]["content"], "Synthetic lesson update");
        assert_eq!(msg["subject"], "Synthetic class notice");
    }
    #[test]
    fn identity_is_bound_to_the_signed_in_mailbox_and_missing_scopes_fail() {
        assert!(identity(
            GOOGLE,
            &json!({"emailAddress":"different@example.org"}),
            "owner@example.org"
        )
        .is_err());
        assert!(access(
            &json!({"access_token":"synthetic","scope":"profile"}),
            GOOGLE
        )
        .is_err());
        assert!(access(
            &json!({"access_token":"synthetic","scope":GOOGLE_SCOPE}),
            GOOGLE
        )
        .is_ok());
    }
    #[test]
    fn microsoft_delta_keeps_each_page_and_rejects_untrusted_or_missing_cursors() {
        struct GraphFixture;
        impl CloudTransport for GraphFixture {
            fn token(&self, _: &str, _: &[(&str, &str)]) -> PRes<Value> {
                panic!("No real authentication in fixture");
            }
            fn get(&self, provider: &str, url: &str, _: &Secret) -> PRes<Value> {
                assert_eq!(provider, MICROSOFT);
                assert!(valid_url(provider, url));
                if url.ends_with("/delta") {
                    Ok(
                        json!({"value":[{"id":"synthetic-a","receivedDateTime":"2026-10-20T00:00:00Z","subject":"Synthetic lecture","body":{"contentType":"text","content":"Bring a worksheet"}}],"@odata.nextLink":"https://graph.microsoft.com/v1.0/me/mailFolders/inbox/messages/delta?$skiptoken=next"}),
                    )
                } else {
                    Ok(
                        json!({"value":[],"@odata.deltaLink":"https://graph.microsoft.com/v1.0/me/mailFolders/inbox/messages/delta?$deltatoken=saved"}),
                    )
                }
            }
        }
        struct BadPage;
        impl CloudTransport for BadPage {
            fn token(&self, _: &str, _: &[(&str, &str)]) -> PRes<Value> {
                panic!("No real authentication in fixture");
            }
            fn get(&self, _: &str, _: &str, _: &Secret) -> PRes<Value> {
                Ok(json!({"value":[]}))
            }
        }
        let now = "2026-10-20T00:00:00Z";
        let conn = crate::db::open_in_memory().unwrap();
        let id = connectors::create_account(&conn, Kind::Email, "Microsoft", "Synthetic", now)
            .unwrap()
            .id;
        connectors::transition(
            &conn,
            &id,
            connectors::ConnState::ConsentNeeded,
            Evidence::Fixture,
            None,
            now,
        )
        .unwrap();
        providers::save_config(&conn, &id, MICROSOFT, &json!({}), now).unwrap();
        let db = Mutex::new(conn);
        let token = Secret::new(b"SYNTHETIC-NOT-REAL".to_vec());
        let cancel = AtomicBool::new(false);
        assert_eq!(
            fetch_microsoft(&db, &GraphFixture, &id, &token, &cancel, now).unwrap(),
            1
        );
        {
            let c = providers::lock(&db).unwrap();
            assert!(connectors::mail_next_request(&c, &id)
                .unwrap()
                .contains("$deltatoken=saved"));
            assert_eq!(
                c.query_row("SELECT count(*) FROM source_messages", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
        }
        assert!(matches!(
            fetch_microsoft(&db, &BadPage, &id, &token, &cancel, now),
            Err(ProviderError::Protocol(_))
        ));
    }
    struct FixtureHttp(AtomicBool);
    impl CloudTransport for FixtureHttp {
        fn token(&self, _: &str, _: &[(&str, &str)]) -> PRes<Value> {
            panic!("No OAuth network in this fixture")
        }
        fn get(&self, provider: &str, url: &str, _: &Secret) -> PRes<Value> {
            assert_eq!(provider, GOOGLE);
            if url.ends_with("/profile") {
                return Ok(json!({"emailAddress":"owner@example.org","historyId":"100"}));
            }
            if url.contains("/messages?") {
                return Ok(json!({"messages":[{"id":"abc123"}]}));
            }
            if url.contains("/messages/abc123?") {
                if self.0.load(Ordering::SeqCst) {
                    return Err(ProviderError::Protocol(
                        "email object or history cursor no longer available",
                    ));
                }
                return Ok(
                    json!({"id":"abc123","labelIds":["INBOX"],"internalDate":"1792454400000","payload":{"mimeType":"text/plain","headers":[{"name":"Subject","value":"Synthetic lecture"}],"body":{"data":URL_SAFE_NO_PAD.encode("Bring your worksheet.")}}}),
                );
            }
            if url.contains("startHistoryId=100") {
                self.0.store(true, Ordering::SeqCst);
                return Ok(
                    json!({"history":[{"messagesDeleted":[{"message":{"id":"abc123"}}]}],"historyId":"101"}),
                );
            }
            if url.contains("startHistoryId=101") {
                return Ok(json!({"history":[],"historyId":"101"}));
            }
            panic!("Unexpected synthetic URL")
        }
    }
    #[test]
    fn gmail_initial_history_and_deletion_are_durable_and_idempotent() {
        let now = "2026-10-20T00:00:00Z";
        let conn = crate::db::open_in_memory().unwrap();
        let a = connectors::create_account(&conn, Kind::Email, "Google", "Synthetic Gmail", now)
            .unwrap();
        let id = a.id;
        connectors::transition(
            &conn,
            &id,
            connectors::ConnState::ConsentNeeded,
            Evidence::Fixture,
            None,
            now,
        )
        .unwrap();
        let cfg = json!({"email":"owner@example.org"});
        providers::save_config(&conn, &id, GOOGLE, &cfg, now).unwrap();
        let db = Mutex::new(conn);
        let token = Secret::new(b"SYNTHETIC-NOT-REAL".to_vec());
        let cancel = AtomicBool::new(false);
        let http = FixtureHttp(AtomicBool::new(false));
        assert_eq!(
            fetch_gmail(&db, &http, &id, &token, cfg, &cancel, now).unwrap(),
            1
        );
        let cfg = {
            let c = providers::lock(&db).unwrap();
            let cfg = providers::load_config(&c, &id, GOOGLE).unwrap();
            assert_eq!(cfg["history_id"], "100");
            let n: i64 = c
                .query_row(
                    "SELECT count(*) FROM source_messages WHERE deleted_at IS NULL",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 1);
            cfg
        };
        assert_eq!(
            fetch_gmail(&db, &http, &id, &token, cfg, &cancel, now).unwrap(),
            1
        );
        let cfg = {
            let c = providers::lock(&db).unwrap();
            let cfg = providers::load_config(&c, &id, GOOGLE).unwrap();
            assert_eq!(cfg["history_id"], "101");
            let body: String = c
                .query_row("SELECT body_text FROM source_messages", [], |r| r.get(0))
                .unwrap();
            assert_eq!(body, "");
            cfg
        };
        assert_eq!(
            fetch_gmail(&db, &http, &id, &token, cfg, &cancel, now).unwrap(),
            0
        );
        cancel.store(true, Ordering::SeqCst);
        let cfg = {
            let c = providers::lock(&db).unwrap();
            providers::load_config(&c, &id, GOOGLE).unwrap()
        };
        assert_eq!(
            fetch_gmail(&db, &http, &id, &token, cfg, &cancel, now),
            Err(ProviderError::Cancelled)
        );
    }
}
