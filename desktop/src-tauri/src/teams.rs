//! Microsoft Teams intake via Microsoft Graph v1.0, delegated work/school sign-in
//! as a public client (no client secret): system browser + auth code + PKCE S256,
//! single-use loopback callback, ID token verified with `jsonwebtoken` against the
//! tenant JWKS, refresh token only in the OS vault, access token only in memory.
//! Read-only GETs to explicitly selected chats and channels; HTTP via `ureq`
//! (rustls) with redirects disabled and same-origin paging enforcement.
use crate::connectors::{self, ConnState, Evidence, Kind};
use crate::oauth::{self, AuthFlows, ClientConfig, IdTokenClaims};
use crate::providers::{self, PRes, ProviderError};
use crate::vault::{Secret, SecretKind, SecretStore, VaultKey};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const PROVIDER: &str = "teams_graph";
pub const GRAPH_BASE: &str = "https://graph.microsoft.com";
pub const MAX_PAGES: usize = 20;
pub const MAX_CHANNEL_PAGES: usize = 3;
pub const MAX_THREADS_PER_RUN: usize = 50;
pub const MAX_TEAMS: usize = 50;
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
/// IP-literal redirects require this exact port in the app registration. Unlike
/// localhost, Entra does not ignore their port when matching redirect URIs.
pub const REDIRECT_PORT: u16 = 43821;
pub const REDIRECT_URI: &str = "http://127.0.0.1:43821/callback";
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BYTES: u64 = 5_000_000;

pub const CHAT_SCOPES: &[&str] = &["openid", "profile", "offline_access", "Chat.Read"];
pub const CHANNEL_SCOPES: &[&str] = &[
    "ChannelMessage.Read.All",
    "Team.ReadBasic.All",
    "Channel.ReadBasic.All",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TeamsSettings {
    /// Owner-registered public-client application (client) ID. No secret exists.
    pub client_id: String,
    /// `organizations` or the tenant GUID.
    pub tenant: String,
    /// Request channel permissions too (incremental consent; may need tenant admin).
    pub channels: bool,
}

fn is_guid(s: &str) -> bool {
    s.len() == 36
        && s.chars().enumerate().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

impl TeamsSettings {
    pub fn validate(&self) -> PRes<()> {
        if !is_guid(self.client_id.trim()) {
            return Err(ProviderError::Invalid(
                "application (client) ID must be a GUID from your app registration".into(),
            ));
        }
        let t = self.tenant.trim();
        if t != "organizations" && !is_guid(t) {
            return Err(ProviderError::Invalid(
                "tenant must be 'organizations' or a tenant GUID".into(),
            ));
        }
        Ok(())
    }
    fn client(&self) -> ClientConfig {
        ClientConfig {
            client_id: self.client_id.trim().to_ascii_lowercase(),
            tenant: self.tenant.trim().to_ascii_lowercase(),
            pinned_tenant_id: is_guid(self.tenant.trim())
                .then(|| self.tenant.trim().to_ascii_lowercase()),
        }
    }
    pub fn scopes(&self) -> Vec<&'static str> {
        let mut s = CHAT_SCOPES.to_vec();
        if self.channels {
            s.extend_from_slice(CHANNEL_SCOPES);
        }
        s
    }
}

/// Provider hosts. Only `production()` is used by the app; the synthetic variant
/// targets a loopback test server and its data can never count as live.
pub struct GraphEndpoint {
    graph: String,
    login: String,
    evidence: Evidence,
}

impl GraphEndpoint {
    pub fn production() -> Self {
        GraphEndpoint {
            graph: GRAPH_BASE.into(),
            login: oauth::AUTHORITY.into(),
            evidence: Evidence::Live,
        }
    }
    #[doc(hidden)]
    pub fn synthetic_for_tests(base: &str) -> Self {
        assert!(
            base.starts_with("http://127.0.0.1:"),
            "synthetic endpoint must be loopback"
        );
        GraphEndpoint {
            graph: base.trim_end_matches('/').into(),
            login: base.trim_end_matches('/').into(),
            evidence: Evidence::Fixture,
        }
    }
    pub fn evidence(&self) -> Evidence {
        self.evidence
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(HTTP_TIMEOUT)
        .user_agent("REI-OS/0.1")
        .build()
}

fn read_json(r: ureq::Response) -> PRes<Value> {
    use std::io::Read;
    let mut buf = Vec::new();
    r.into_reader()
        .take(MAX_RESPONSE_BYTES)
        .read_to_end(&mut buf)
        .map_err(|_| ProviderError::Network)?;
    serde_json::from_slice(&buf).map_err(|_| ProviderError::Protocol("invalid JSON"))
}

// ---------- tokens ----------

struct Access {
    token: Secret,
    expires_at: Instant,
}

static ACCESS: Mutex<Option<HashMap<String, Access>>> = Mutex::new(None);
static FLOWS: std::sync::LazyLock<AuthFlows> = std::sync::LazyLock::new(AuthFlows::default);

fn refresh_key(identity: &str) -> PRes<VaultKey> {
    VaultKey::new(identity, SecretKind::TeamsRefresh)
        .map_err(|_| ProviderError::Invalid("account id".into()))
}

fn set_access(identity: &str, token: String, expires_in: u64) {
    if let Ok(mut m) = ACCESS.lock() {
        m.get_or_insert_with(HashMap::new).insert(
            identity.into(),
            Access {
                token: Secret::new(token.into_bytes()),
                expires_at: Instant::now() + Duration::from_secs(expires_in.clamp(60, 7200) - 30),
            },
        );
    }
}

fn get_access(identity: &str) -> Option<String> {
    let m = ACCESS.lock().ok()?;
    let a = m.as_ref()?.get(identity)?;
    (a.expires_at > Instant::now())
        .then(|| String::from_utf8(a.token.expose().to_vec()).ok())
        .flatten()
}

pub fn forget_access(identity: &str) {
    if let Ok(mut m) = ACCESS.lock() {
        if let Some(m) = m.as_mut() {
            m.remove(identity);
        }
    }
}

/// Maps an OAuth error response without echoing its free-text description.
fn token_error(body: &Value) -> ProviderError {
    let code = body["error"].as_str().unwrap_or("");
    let desc = body["error_description"].as_str().unwrap_or("");
    if code == "access_denied" {
        ProviderError::Denied
    } else if code == "consent_required"
        || desc.contains("AADSTS65001")
        || desc.contains("AADSTS90094")
    {
        ProviderError::Forbidden
    } else if code == "invalid_grant" || code == "interaction_required" {
        ProviderError::Auth
    } else {
        ProviderError::Protocol("token endpoint error")
    }
}

fn token_request(ep: &GraphEndpoint, cfg: &ClientConfig, form: &[(&str, &str)]) -> PRes<Value> {
    let url = format!("{}/{}/oauth2/v2.0/token", ep.login, cfg.tenant);
    match agent().post(&url).send_form(form) {
        Ok(r) if r.status() == 200 => read_json(r),
        Ok(_) => Err(ProviderError::Destination),
        Err(ureq::Error::Status(_, r)) => Err(token_error(&read_json(r).unwrap_or(Value::Null))),
        Err(_) => Err(ProviderError::Network),
    }
}

/// Verifies the ID token signature (RS256, tenant JWKS), audience, expiry, issuer,
/// nonce and pinned tenant. Returns the verified (tenant ID, object ID).
pub fn verify_id_token(
    ep: &GraphEndpoint,
    cfg: &ClientConfig,
    id_token: &str,
    nonce: &str,
    now_unix: i64,
) -> PRes<(String, String)> {
    use jsonwebtoken::{decode, decode_header, jwk::JwkSet, Algorithm, DecodingKey, Validation};
    let header = decode_header(id_token).map_err(|_| ProviderError::Protocol("ID token"))?;
    if header.alg != Algorithm::RS256 {
        return Err(ProviderError::Protocol("ID token algorithm"));
    }
    let kid = header
        .kid
        .ok_or(ProviderError::Protocol("ID token key id"))?;
    let url = format!("{}/{}/discovery/v2.0/keys", ep.login, cfg.tenant);
    let jwks: JwkSet = match agent().get(&url).call() {
        Ok(r) if r.status() == 200 => {
            serde_json::from_value(read_json(r)?).map_err(|_| ProviderError::Protocol("JWKS"))?
        }
        Ok(_) => return Err(ProviderError::Destination),
        Err(_) => return Err(ProviderError::Network),
    };
    let jwk = jwks
        .find(&kid)
        .ok_or(ProviderError::Protocol("unknown signing key"))?;
    let key = DecodingKey::from_jwk(jwk).map_err(|_| ProviderError::Protocol("JWKS"))?;
    let mut v = Validation::new(Algorithm::RS256);
    v.set_audience(&[cfg.client_id.as_str()]);
    v.leeway = 60;
    // Expiry is checked against the caller's clock in `oauth::validate_claims`.
    v.validate_exp = false;
    let data = decode::<IdTokenClaims>(id_token, &key, &v)
        .map_err(|_| ProviderError::Protocol("ID token signature or claims rejected"))?;
    oauth::validate_claims(&data.claims, cfg, nonce, now_unix)
        .map_err(|_| ProviderError::Protocol("ID token issuer, tenant or nonce rejected"))?;
    if !is_guid(&data.claims.oid) {
        return Err(ProviderError::Protocol("ID token has no user object ID"));
    }
    Ok((
        data.claims.tid.to_ascii_lowercase(),
        data.claims.oid.to_ascii_lowercase(),
    ))
}

/// Uses the vault refresh token to obtain a new access token; stores the rotated
/// refresh token before the old one is discarded.
fn refresh(
    ep: &GraphEndpoint,
    store: &dyn SecretStore,
    identity: &str,
    s: &TeamsSettings,
) -> PRes<String> {
    let key = refresh_key(identity)?;
    let rt = store.get(&key).map_err(|e| match e {
        crate::vault::VaultError::NotFound => ProviderError::SecretMissing,
        _ => ProviderError::VaultUnavailable,
    })?;
    let rt_str = std::str::from_utf8(rt.expose()).map_err(|_| ProviderError::SecretMissing)?;
    let cfg = s.client();
    let scope = s.scopes().join(" ");
    let body = token_request(
        ep,
        &cfg,
        &[
            ("client_id", &cfg.client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", rt_str),
            ("scope", &scope),
        ],
    )?;
    if let Some(new_rt) = body["refresh_token"].as_str() {
        store
            .rotate(&key, &Secret::new(new_rt.as_bytes().to_vec()))
            .map_err(|_| ProviderError::VaultUnavailable)?;
    }
    let at = body["access_token"]
        .as_str()
        .ok_or(ProviderError::Protocol("no access token"))?
        .to_string();
    set_access(
        identity,
        at.clone(),
        body["expires_in"].as_u64().unwrap_or(3600),
    );
    Ok(at)
}

// ---------- setup and sign-in ----------

/// Creates the Teams chat account and its linked channel account.
pub fn configure(
    conn: &Connection,
    account_id: Option<&str>,
    s: &TeamsSettings,
    now: &str,
) -> PRes<String> {
    s.validate()?;
    let _run = account_id
        .map(|id| providers::operation_guard(conn, id))
        .transpose()?;
    let chat_id = match account_id {
        Some(id) => {
            if providers::require_provider(conn, id, PROVIDER)? != Kind::TeamsChat {
                return Err(ProviderError::Invalid(
                    "configure Teams using its primary chat account".into(),
                ));
            }
            for a in linked_accounts(conn, id)? {
                providers::reopen(conn, &a, now)?;
            }
            id.to_string()
        }
        None => {
            let a = connectors::create_account(
                conn,
                Kind::TeamsChat,
                "MicrosoftGraph",
                "Teams chats",
                now,
            )?;
            connectors::transition(
                conn,
                &a.id,
                ConnState::ConsentNeeded,
                Evidence::None,
                None,
                now,
            )?;
            let c = connectors::create_account(
                conn,
                Kind::TeamsChannel,
                "MicrosoftGraph",
                "Teams channels",
                now,
            )?;
            connectors::transition(
                conn,
                &c.id,
                ConnState::ConsentNeeded,
                Evidence::None,
                None,
                now,
            )?;
            let cfg = serde_json::json!({ "identity": a.id, "role": "channels" });
            providers::save_config(conn, &c.id, PROVIDER, &cfg, now)?;
            a.id
        }
    };
    let mut cfg = serde_json::json!({
        "client_id": s.client_id.trim().to_ascii_lowercase(),
        "tenant": s.tenant.trim().to_ascii_lowercase(),
        "channels": s.channels,
        "role": "chats",
        "redirect_uri": REDIRECT_URI,
    });
    if account_id.is_some() {
        // Once a sign-in bound this account to a verified tenant/user, the app
        // registration and tenant cannot be swapped underneath its stored refresh
        // token, discovered sources and messages: disconnect first.
        let prev = providers::load_config(conn, &chat_id, PROVIDER)?;
        if let Some(oid) = prev["verified_oid"].as_str() {
            if prev["client_id"] != cfg["client_id"] || prev["tenant"] != cfg["tenant"] {
                return Err(ProviderError::Invalid(
                    "this account is bound to a signed-in Microsoft identity; disconnect it before changing the app ID or tenant".into(),
                ));
            }
            cfg["verified_oid"] = oid.into();
            cfg["verified_tid"] = prev["verified_tid"].clone();
        }
    }
    providers::save_config(conn, &chat_id, PROVIDER, &cfg, now)?;
    Ok(chat_id)
}

/// Resolves any Teams account (chat or channel) to its identity (chat account) and settings.
fn identity_of(conn: &Connection, account_id: &str) -> PRes<(String, TeamsSettings)> {
    providers::require_provider(conn, account_id, PROVIDER)?;
    let v = providers::load_config(conn, account_id, PROVIDER)?;
    let identity = v["identity"].as_str().unwrap_or(account_id).to_string();
    let c = providers::load_config(conn, &identity, PROVIDER)?;
    let s = TeamsSettings {
        client_id: c["client_id"].as_str().unwrap_or("").into(),
        tenant: c["tenant"].as_str().unwrap_or("").into(),
        channels: c["channels"].as_bool().unwrap_or(false),
    };
    s.validate()?;
    Ok((identity, s))
}

fn linked_accounts(conn: &Connection, identity: &str) -> PRes<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT account_id FROM provider_config WHERE provider='teams_graph'
         AND (account_id=?1 OR json_extract(config,'$.identity')=?1) ORDER BY account_id",
    )?;
    let rows = st.query_map([identity], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<String>>>()?)
}

/// Waits for exactly one loopback callback with the expected path; returns its query.
fn await_callback(
    listener: TcpListener,
    cancel: &AtomicBool,
    deadline: Instant,
) -> PRes<Vec<(String, String)>> {
    listener
        .set_nonblocking(true)
        .map_err(|_| ProviderError::Network)?;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(ProviderError::Cancelled);
        }
        if Instant::now() > deadline {
            return Err(ProviderError::TimedOut);
        }
        match listener.accept() {
            Ok((mut sock, peer)) => {
                if !peer.ip().is_loopback() {
                    continue;
                }
                sock.set_nonblocking(false).ok();
                sock.set_read_timeout(Some(Duration::from_secs(5))).ok();
                let mut line = String::new();
                let _ = BufReader::new(std::io::Read::take(&sock, 8192)).read_line(&mut line);
                let path = line.split_whitespace().nth(1).unwrap_or("");
                let Some(q) = path.strip_prefix("/callback?") else {
                    let _ = sock.write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    continue;
                };
                let pairs: Vec<(String, String)> = url::form_urlencoded::parse(q.as_bytes())
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect();
                let page = "<!doctype html><title>REI OS</title><p>Response received. Return to REI OS to check the sign-in result.</p>";
                let _ = write!(
                    sock,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{page}",
                    page.len()
                );
                return Ok(pairs);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return Err(ProviderError::Network),
        }
    }
}

/// Full interactive sign-in. `open_browser` receives the authorization URL (which
/// contains only public PKCE challenge/state values, never a token or secret).
pub fn sign_in(
    db: &Mutex<Connection>,
    store: &dyn SecretStore,
    ep: &GraphEndpoint,
    account_id: &str,
    open_browser: &dyn Fn(&str) -> PRes<()>,
    cancel: &AtomicBool,
    now_unix: i64,
) -> PRes<String> {
    let now = crate::time::format_unix(now_unix);
    let (identity, s, bound) = {
        let c = providers::lock(db)?;
        let (identity, s) = identity_of(&c, account_id)?;
        let cfg = providers::load_config(&c, &identity, PROVIDER)?;
        let bound = match (cfg["verified_tid"].as_str(), cfg["verified_oid"].as_str()) {
            (Some(t), Some(o)) => Some((t.to_string(), o.to_string())),
            _ => None,
        };
        (identity, s, bound)
    };
    let res = sign_in_inner(
        store,
        ep,
        &identity,
        &s,
        bound.as_ref(),
        open_browser,
        cancel,
        now_unix,
    );
    let c = providers::lock(db)?;
    match res {
        Ok((tid, oid)) => {
            if bound.is_none() {
                let mut cfg = providers::load_config(&c, &identity, PROVIDER)?;
                cfg["verified_tid"] = tid.clone().into();
                cfg["verified_oid"] = oid.into();
                providers::save_config(&c, &identity, PROVIDER, &cfg, &now)?;
            }
            for a in linked_accounts(&c, &identity)? {
                providers::set_secret_saved(&c, &a, true, &now)?;
                providers::record_authenticated(&c, &a, &now)?;
            }
            Ok(tid)
        }
        Err(e) => {
            providers::record_outcome(&c, &identity, false, &e.to_string(), &now)?;
            if e == ProviderError::Denied {
                providers::mark_failed(&c, &identity, &e, &now)?;
            }
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn sign_in_inner(
    store: &dyn SecretStore,
    ep: &GraphEndpoint,
    identity: &str,
    s: &TeamsSettings,
    bound: Option<&(String, String)>,
    open_browser: &dyn Fn(&str) -> PRes<()>,
    cancel: &AtomicBool,
    now_unix: i64,
) -> PRes<(String, String)> {
    let started = Instant::now();
    let cfg = s.client();
    let bind = if ep.evidence() == Evidence::Live {
        format!("127.0.0.1:{REDIRECT_PORT}")
    } else {
        "127.0.0.1:0".into()
    };
    let listener = TcpListener::bind(bind).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AddrInUse {
            ProviderError::NotConfigured(
                "Teams sign-in port 43821 is busy; finish the other sign-in and retry",
            )
        } else {
            ProviderError::Network
        }
    })?;
    let port = listener
        .local_addr()
        .map_err(|_| ProviderError::Network)?
        .port();
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let start = FLOWS
        .start(&cfg, &s.scopes(), &redirect, now_unix)
        .map_err(|_| ProviderError::Invalid("sign-in request".into()))?;
    open_browser(&start.url)?;
    let q = await_callback(listener, cancel, Instant::now() + LOGIN_TIMEOUT)?;
    validate_callback_state(&q, &start.state)?;
    let get = |k: &str| q.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
    // A denial counts only when it carries this flow's state: any local process can
    // reach the loopback port, so an unmatched `error=access_denied` is not a denial.
    let ex = FLOWS
        .complete(
            get("state").unwrap_or(""),
            get("code"),
            get("error"),
            now_unix + started.elapsed().as_secs() as i64,
        )
        .map_err(|e| match e {
            oauth::OAuthError::ProviderError if get("error") == Some("access_denied") => {
                ProviderError::Denied
            }
            oauth::OAuthError::ProviderError => ProviderError::Forbidden,
            _ => ProviderError::Protocol("sign-in callback state rejected"),
        })?;
    let scope = s.scopes().join(" ");
    let body = token_request(
        ep,
        &cfg,
        &[
            ("client_id", &cfg.client_id),
            ("grant_type", "authorization_code"),
            ("code", &ex.code),
            ("redirect_uri", &ex.redirect_uri),
            ("code_verifier", &ex.verifier),
            ("scope", &scope),
        ],
    )?;
    let id_token = body["id_token"]
        .as_str()
        .ok_or(ProviderError::Protocol("no ID token"))?;
    // The owner may take minutes in the browser: check expiry at the current time.
    let verify_at = now_unix + started.elapsed().as_secs() as i64;
    let (tid, oid) = verify_id_token(ep, &cfg, id_token, &ex.nonce, verify_at)?;
    if let Some((bt, bo)) = bound {
        if *bt != tid || *bo != oid {
            return Err(ProviderError::Protocol(
                "a different Microsoft account signed in; disconnect this account first",
            ));
        }
    }
    let rt = body["refresh_token"]
        .as_str()
        .ok_or(ProviderError::Protocol("no refresh token"))?;
    let at = body["access_token"]
        .as_str()
        .ok_or(ProviderError::Protocol("no access token"))?;
    store
        .rotate(
            &refresh_key(identity)?,
            &Secret::new(rt.as_bytes().to_vec()),
        )
        .map_err(|_| ProviderError::VaultUnavailable)?;
    set_access(
        identity,
        at.to_string(),
        body["expires_in"].as_u64().unwrap_or(3600),
    );
    Ok((tid, oid))
}

// ---------- Graph GET ----------

fn validate_callback_state(pairs: &[(String, String)], expected: &str) -> PRes<()> {
    let states: Vec<&str> = pairs
        .iter()
        .filter(|(k, _)| k == "state")
        .map(|(_, v)| v.as_str())
        .collect();
    let count = |key: &str| pairs.iter().filter(|(k, _)| k == key).count();
    if states != [expected] || count("code") > 1 || count("error") > 1 {
        return Err(ProviderError::Protocol("sign-in callback state rejected"));
    }
    Ok(())
}

struct Graph<'a> {
    ep: &'a GraphEndpoint,
    store: &'a dyn SecretStore,
    identity: &'a str,
    settings: &'a TeamsSettings,
    cancel: &'a AtomicBool,
    refreshed: bool,
}

impl Graph<'_> {
    /// Only same-origin `/v1.0/` URLs; nextLinks are checked by the same rule.
    fn check_url(&self, url: &str) -> PRes<()> {
        let rest = url
            .strip_prefix(&self.ep.graph)
            .filter(|r| r.starts_with("/v1.0/"))
            .ok_or(ProviderError::Destination)?;
        // The exact origin prefix is stripped above, so '@' here is path data (Graph
        // chat IDs contain it), not URL userinfo.
        if rest.contains("..")
            || rest.contains('\\')
            || rest.contains(char::is_whitespace)
            || rest.contains('#')
        {
            return Err(ProviderError::Destination);
        }
        Ok(())
    }

    fn get(&mut self, url: &str) -> PRes<Value> {
        self.check_url(url)?;
        let mut attempt = 0;
        loop {
            if self.cancel.load(Ordering::SeqCst) {
                return Err(ProviderError::Cancelled);
            }
            let token = match get_access(self.identity) {
                Some(t) => t,
                None => {
                    self.refreshed = true;
                    refresh(self.ep, self.store, self.identity, self.settings)?
                }
            };
            let resp = agent()
                .get(url)
                .set("Authorization", &format!("Bearer {token}"))
                .set("Accept", "application/json")
                .call();
            match resp {
                Ok(r) if r.status() == 200 => return read_json(r),
                Ok(_) => return Err(ProviderError::Destination), // 3xx: redirects are refused
                Err(ureq::Error::Status(401, _)) if !self.refreshed => {
                    forget_access(self.identity);
                    self.refreshed = true;
                    refresh(self.ep, self.store, self.identity, self.settings)?;
                }
                Err(ureq::Error::Status(401, _)) => return Err(ProviderError::Auth),
                Err(ureq::Error::Status(403, _)) => return Err(ProviderError::Forbidden),
                Err(ureq::Error::Status(404, _)) => {
                    return Err(ProviderError::Protocol("selected source no longer exists"))
                }
                Err(ureq::Error::Status(code, r)) if code == 429 || code >= 500 => {
                    let after = r
                        .header("Retry-After")
                        .and_then(|v| v.trim().parse::<u64>().ok());
                    let d = connectors::retry_delay(attempt, after, 0.5)
                        .ok_or(ProviderError::Protocol("provider kept throttling"))?;
                    providers::sleep_cancellable(d, self.cancel)?;
                    attempt += 1;
                }
                Err(ureq::Error::Status(_, _)) => {
                    return Err(ProviderError::Protocol("unexpected HTTP status"))
                }
                Err(_) => return Err(ProviderError::Network),
            }
        }
    }

    /// Collects up to `max` pages (newest first as Graph returns them).
    fn pages(&mut self, first: String, max: usize) -> PRes<(Vec<Value>, bool)> {
        let mut out = Vec::new();
        let mut next = Some(first);
        while let Some(u) = next.take() {
            if out.len() >= max {
                return Ok((out, true));
            }
            let mut page = self.get(&u)?;
            if let Some(n) = page["@odata.nextLink"].as_str() {
                self.check_url(n)?;
                next = Some(n.to_string());
            }
            if let Some(o) = page.as_object_mut() {
                o.remove("@odata.nextLink");
            }
            out.push(page);
        }
        Ok((out, false))
    }
}

/// Graph IDs are inserted into URL paths; only the documented ID charset passes.
pub fn valid_graph_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 400
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '@' | '.' | '_' | '-' | '='))
}

fn path_id(id: &str) -> PRes<String> {
    if !valid_graph_id(id) {
        return Err(ProviderError::Invalid("invalid Graph identifier".into()));
    }
    Ok(url::form_urlencoded::byte_serialize(id.as_bytes()).collect())
}

/// Discovers the owner's chats and (if channel consent was requested) joined
/// teams/channels. Nothing is selected automatically.
pub fn discover(
    db: &Mutex<Connection>,
    store: &dyn SecretStore,
    ep: &GraphEndpoint,
    account_id: &str,
    cancel: &AtomicBool,
    now: &str,
) -> PRes<u32> {
    let (identity, s) = identity_of(&*providers::lock(db)?, account_id)?;
    let mut g = Graph {
        ep,
        store,
        identity: &identity,
        settings: &s,
        cancel,
        refreshed: false,
    };
    let mut chats = Vec::new();
    let (pages, _) = g.pages(format!("{}/v1.0/me/chats?$top=50", ep.graph), 10)?;
    for it in pages
        .iter()
        .flat_map(|p| p["value"].as_array().cloned().unwrap_or_default())
    {
        let Some(id) = it["id"].as_str().filter(|i| valid_graph_id(i)) else {
            continue;
        };
        let name = it["topic"]
            .as_str()
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("{} chat", it["chatType"].as_str().unwrap_or("Teams")));
        chats.push(("chat".to_string(), id.to_string(), name));
    }
    let mut channels = Vec::new();
    if s.channels {
        let (teams, _) = g.pages(format!("{}/v1.0/me/joinedTeams", ep.graph), 3)?;
        let teams: Vec<Value> = teams
            .iter()
            .flat_map(|p| p["value"].as_array().cloned().unwrap_or_default())
            .take(MAX_TEAMS)
            .collect();
        for t in teams {
            let Some(tid) = t["id"].as_str().filter(|i| valid_graph_id(i)) else {
                continue;
            };
            let tname = t["displayName"].as_str().unwrap_or("Team");
            let (cp, _) = g.pages(
                format!("{}/v1.0/teams/{}/channels", ep.graph, path_id(tid)?),
                3,
            )?;
            for c in cp
                .iter()
                .flat_map(|p| p["value"].as_array().cloned().unwrap_or_default())
            {
                let Some(cid) = c["id"].as_str().filter(|i| valid_graph_id(i)) else {
                    continue;
                };
                channels.push((
                    "channel".to_string(),
                    format!("{tid}/{cid}"),
                    format!(
                        "{tname} / {}",
                        c["displayName"].as_str().unwrap_or("Channel")
                    ),
                ));
            }
        }
    }
    let c = providers::lock(db)?;
    let linked = linked_accounts(&c, &identity)?;
    providers::upsert_discovered(&c, &identity, &chats, now)?;
    if let Some(ch) = linked.iter().find(|a| *a != &identity) {
        providers::upsert_discovered(&c, ch, &channels, now)?;
    }
    Ok((chats.len() + channels.len()) as u32)
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct FetchReport {
    pub sources: u32,
    pub inserted: u32,
    pub updated: u32,
    pub unchanged: u32,
    pub removed: u32,
    pub skipped: u32,
    pub threads_reconciled: u32,
    /// A page bound was reached; older history was not read in this run.
    pub truncated: bool,
}

fn add(rep: &mut FetchReport, s: &connectors::SyncSummary) {
    rep.inserted += s.inserted;
    rep.updated += s.updated;
    rep.unchanged += s.unchanged;
    rep.removed += s.removed;
    rep.skipped += s.skipped;
}

/// Fetches selected chats (chat account) or selected channels with replies
/// (channel account). Pages are applied oldest-first so a monotonic cursor never
/// skips unread older pages when a run is cancelled.
pub fn fetch(
    db: &Mutex<Connection>,
    store: &dyn SecretStore,
    ep: &GraphEndpoint,
    account_id: &str,
    cancel: &AtomicBool,
    now: &str,
) -> PRes<FetchReport> {
    let res = fetch_inner(db, store, ep, account_id, cancel, now);
    let c = providers::lock(db)?;
    match &res {
        Ok(r) => {
            providers::record_outcome(
                &c,
                account_id,
                true,
                &format!(
                    "Read {} source(s): {} new, {} updated, {} removed{}",
                    r.sources,
                    r.inserted,
                    r.updated,
                    r.removed,
                    if r.truncated {
                        "; page limit reached, older messages in this window were not read"
                    } else {
                        ""
                    }
                ),
                now,
            )?;
            providers::mark_read_ok(&c, account_id, ep.evidence(), now)?;
        }
        Err(e) => {
            providers::record_outcome(&c, account_id, false, &e.to_string(), now)?;
            providers::mark_failed(&c, account_id, e, now)?;
        }
    }
    res
}

fn fetch_inner(
    db: &Mutex<Connection>,
    store: &dyn SecretStore,
    ep: &GraphEndpoint,
    account_id: &str,
    cancel: &AtomicBool,
    now: &str,
) -> PRes<FetchReport> {
    let (identity, s, kind, chats, channels) = {
        let c = providers::lock(db)?;
        let (identity, s) = identity_of(&c, account_id)?;
        let kind = connectors::get_account(&c, account_id)?.kind;
        let chats = providers::selected(&c, account_id, "chat")?;
        let channels = providers::selected(&c, account_id, "channel")?;
        (identity, s, kind, chats, channels)
    };
    if chats.is_empty() && channels.is_empty() {
        return Err(ProviderError::NotConfigured(
            "select at least one chat or channel",
        ));
    }
    let mut g = Graph {
        ep,
        store,
        identity: &identity,
        settings: &s,
        cancel,
        refreshed: false,
    };
    let mut rep = FetchReport::default();
    if kind == Kind::TeamsChat {
        for chat in &chats {
            let since = connectors::chat_poll_since(&*providers::lock(db)?, account_id, chat)?;
            let url = connectors::chat_messages_url(chat, since.as_deref())?;
            let url = format!("{}{}", ep.graph, url.trim_start_matches(GRAPH_BASE));
            let (pages, truncated) = g.pages(url, MAX_PAGES)?;
            rep.truncated |= truncated;
            for p in pages.iter().rev() {
                let sum = connectors::apply_chat_messages_page(
                    &mut *providers::lock(db)?,
                    account_id,
                    chat,
                    p,
                    now,
                )?;
                add(&mut rep, &sum);
            }
            rep.sources += 1;
        }
    } else {
        for r in &channels {
            let (team, chan) = r
                .split_once('/')
                .ok_or(ProviderError::Invalid("channel reference".into()))?;
            let url = format!(
                "{}/v1.0/teams/{}/channels/{}/messages?$top=50",
                ep.graph,
                path_id(team)?,
                path_id(chan)?
            );
            let (pages, truncated) = g.pages(url, MAX_CHANNEL_PAGES)?;
            rep.truncated |= truncated;
            for p in pages.iter().rev() {
                let sum = connectors::apply_channel_messages_page(
                    &mut *providers::lock(db)?,
                    account_id,
                    team,
                    chan,
                    None,
                    p,
                    now,
                )?;
                add(&mut rep, &sum);
            }
            let threads = connectors::channel_threads_to_poll(
                &*providers::lock(db)?,
                account_id,
                team,
                chan,
                (MAX_THREADS_PER_RUN + 1) as u32,
            )?;
            rep.truncated |= threads.len() > MAX_THREADS_PER_RUN;
            for root in threads.iter().take(MAX_THREADS_PER_RUN) {
                let url = format!(
                    "{}/v1.0/teams/{}/channels/{}/messages/{}/replies?$top=50",
                    ep.graph,
                    path_id(team)?,
                    path_id(chan)?,
                    path_id(root)?
                );
                let (pages, truncated) = g.pages(url, MAX_CHANNEL_PAGES)?;
                rep.truncated |= truncated;
                for p in pages.iter().rev() {
                    let sum = connectors::apply_channel_messages_page(
                        &mut *providers::lock(db)?,
                        account_id,
                        team,
                        chan,
                        Some(root),
                        p,
                        now,
                    )?;
                    add(&mut rep, &sum);
                }
                rep.threads_reconciled += 1;
            }
            rep.truncated |= threads.len() > MAX_THREADS_PER_RUN;
            rep.sources += 1;
        }
    }
    Ok(rep)
}

/// Local sign-out: removes the refresh token from the OS vault and the in-memory
/// access token. Provider-side session revocation is done by the owner.
pub fn sign_out(store: &dyn SecretStore, conn: &Connection, account_id: &str) -> PRes<()> {
    let (identity, _) = identity_of(conn, account_id)?;
    forget_access(&identity);
    store
        .delete(&refresh_key(&identity)?)
        .map_err(|_| ProviderError::VaultUnavailable)
}

pub fn identity_for(conn: &Connection, account_id: &str) -> PRes<String> {
    Ok(identity_of(conn, account_id)?.0)
}

pub fn linked(conn: &Connection, account_id: &str) -> PRes<Vec<String>> {
    let id = identity_for(conn, account_id)?;
    linked_accounts(conn, &id)
}

#[cfg(test)]
mod callback_security_tests {
    use super::*;

    #[test]
    fn callback_is_bound_to_its_own_flow_not_any_pending_state() {
        let q = vec![
            ("state".into(), "different-valid-flow".into()),
            ("code".into(), "synthetic".into()),
        ];
        assert_eq!(
            validate_callback_state(&q, "this-flow"),
            Err(ProviderError::Protocol("sign-in callback state rejected"))
        );
        assert!(validate_callback_state(&q, "different-valid-flow").is_ok());
    }

    #[test]
    fn duplicate_callback_state_code_and_error_are_rejected() {
        for key in ["state", "code", "error"] {
            let mut q = vec![
                ("state".into(), "expected".into()),
                ("code".into(), "synthetic".into()),
                ("error".into(), "synthetic".into()),
            ];
            q.push((key.into(), "duplicate".into()));
            assert!(validate_callback_state(&q, "expected").is_err(), "{key}");
        }
    }
}
