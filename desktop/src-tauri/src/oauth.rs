//! OAuth abstraction for the Microsoft identity platform (docs/INTERFACES.md §7):
//! public client, auth code + PKCE S256, 128-bit+ `state` and `nonce`, loopback
//! redirect, single-use flows that expire after 5 minutes, and id_token claim checks.
//! No network I/O happens here; the connectors phase performs the token exchange and
//! JWKS signature verification (NOT_YET_COMPLETE) before calling `validate_claims`.
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;

pub const FLOW_TTL_SECS: i64 = 300;
pub const AUTHORITY: &str = "https://login.microsoftonline.com";

/// Read-only scopes allowed in v1. Anything else (e.g. `Mail.Send`, `*.ReadWrite`)
/// is rejected before an authorization URL is ever built.
pub const ALLOWED_SCOPES: &[&str] = &[
    "openid",
    "profile",
    "offline_access",
    "User.Read",
    "Mail.Read",
    "Calendars.Read",
    "Chat.Read",
    "ChannelMessage.Read.All",
    "Team.ReadBasic.All",
    "Channel.ReadBasic.All",
];

#[derive(Debug, PartialEq, Eq)]
pub enum OAuthError {
    ScopeNotAllowed(String),
    RedirectNotLoopback,
    InvalidClientId,
    UnknownState,
    Expired,
    ProviderError,
    Issuer,
    Audience,
    Nonce,
    Tenant,
    TokenExpired,
}

pub fn b64url(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().fold(0u32, |acc, b| (acc << 8) | *b as u32) << (8 * (3 - chunk.len()));
        for i in 0..=chunk.len() {
            out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    out
}

/// 32 bytes from the OS CSPRNG (uuid v4 uses `getrandom`), hashed to spread the
/// 3×122 random bits uniformly.
fn random32() -> [u8; 32] {
    let mut h = Sha256::new();
    for _ in 0..3 {
        h.update(uuid::Uuid::new_v4().as_bytes());
    }
    h.finalize().into()
}

pub fn pkce_challenge(verifier: &str) -> String {
    b64url(&Sha256::digest(verifier.as_bytes()))
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Only `http://127.0.0.1:{port}/callback` or `http://[::1]:{port}/callback`.
pub fn validate_redirect(uri: &str) -> Result<(), OAuthError> {
    let rest = uri
        .strip_prefix("http://127.0.0.1:")
        .or_else(|| uri.strip_prefix("http://[::1]:"))
        .ok_or(OAuthError::RedirectNotLoopback)?;
    let port = rest
        .strip_suffix("/callback")
        .ok_or(OAuthError::RedirectNotLoopback)?;
    match port.parse::<u16>() {
        Ok(p) if p >= 1024 && port.bytes().all(|b| b.is_ascii_digit()) => Ok(()),
        _ => Err(OAuthError::RedirectNotLoopback),
    }
}

#[derive(Clone, Debug)]
pub struct ClientConfig {
    pub client_id: String,
    /// `organizations`, `common` or a pinned tenant GUID.
    pub tenant: String,
    pub pinned_tenant_id: Option<String>,
}

struct Pending {
    state: String,
    nonce: String,
    verifier: String,
    redirect_uri: String,
    created_at: i64,
}

/// Returned to the caller that opens the system browser. The verifier stays in Rust.
#[derive(Debug)]
pub struct AuthStart {
    pub url: String,
    pub state: String,
}

/// What a successful callback yields for the token exchange (still Rust-only).
#[derive(Debug)]
pub struct CodeExchange {
    pub code: String,
    pub verifier: String,
    pub redirect_uri: String,
    pub nonce: String,
}

#[derive(Default)]
pub struct AuthFlows {
    pending: Mutex<HashMap<String, Pending>>,
}

impl AuthFlows {
    pub fn start(
        &self,
        cfg: &ClientConfig,
        scopes: &[&str],
        redirect_uri: &str,
        now_unix: i64,
    ) -> Result<AuthStart, OAuthError> {
        if let Some(bad) = scopes.iter().find(|s| !ALLOWED_SCOPES.contains(s)) {
            return Err(OAuthError::ScopeNotAllowed((*bad).to_string()));
        }
        validate_redirect(redirect_uri)?;
        if cfg.client_id.is_empty()
            || !cfg
                .client_id
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == '-')
        {
            return Err(OAuthError::InvalidClientId);
        }
        let state = b64url(&random32());
        let nonce = b64url(&random32());
        let verifier = b64url(&random32());
        let url = format!(
            "{AUTHORITY}/{}/oauth2/v2.0/authorize?client_id={}&response_type=code&redirect_uri={}&response_mode=query&scope={}&state={}&nonce={}&code_challenge={}&code_challenge_method=S256",
            pct(&cfg.tenant),
            pct(&cfg.client_id),
            pct(redirect_uri),
            pct(&scopes.join(" ")),
            state,
            nonce,
            pkce_challenge(&verifier)
        );
        let mut p = self.pending.lock().expect("auth flow lock");
        p.retain(|_, v| now_unix - v.created_at <= FLOW_TTL_SECS);
        p.insert(
            state.clone(),
            Pending {
                state: state.clone(),
                nonce,
                verifier,
                redirect_uri: redirect_uri.to_string(),
                created_at: now_unix,
            },
        );
        Ok(AuthStart { url, state })
    }

    /// Consumes the flow (single use) whether or not validation succeeds.
    pub fn complete(
        &self,
        state: &str,
        code: Option<&str>,
        error: Option<&str>,
        now_unix: i64,
    ) -> Result<CodeExchange, OAuthError> {
        let mut p = self.pending.lock().expect("auth flow lock");
        let key = p
            .values()
            .find(|v| ct_eq(v.state.as_bytes(), state.as_bytes()))
            .map(|v| v.state.clone())
            .ok_or(OAuthError::UnknownState)?;
        let flow = p.remove(&key).ok_or(OAuthError::UnknownState)?;
        if now_unix - flow.created_at > FLOW_TTL_SECS || now_unix < flow.created_at {
            return Err(OAuthError::Expired);
        }
        match (code, error) {
            (Some(c), None) if !c.is_empty() => Ok(CodeExchange {
                code: c.to_string(),
                verifier: flow.verifier,
                redirect_uri: flow.redirect_uri,
                nonce: flow.nonce,
            }),
            _ => Err(OAuthError::ProviderError),
        }
    }
}

/// id_token claims after signature verification.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct IdTokenClaims {
    pub iss: String,
    pub aud: String,
    pub nonce: String,
    pub tid: String,
    /// Immutable user object ID; binds a Teams account to one signed-in user.
    #[serde(default)]
    pub oid: String,
    pub exp: i64,
}

pub fn validate_claims(
    claims: &IdTokenClaims,
    cfg: &ClientConfig,
    expected_nonce: &str,
    now_unix: i64,
) -> Result<(), OAuthError> {
    if claims.iss != format!("{AUTHORITY}/{}/v2.0", claims.tid) || claims.tid.is_empty() {
        return Err(OAuthError::Issuer);
    }
    if claims.aud != cfg.client_id {
        return Err(OAuthError::Audience);
    }
    if !ct_eq(claims.nonce.as_bytes(), expected_nonce.as_bytes()) {
        return Err(OAuthError::Nonce);
    }
    if let Some(pin) = &cfg.pinned_tenant_id {
        if !claims.tid.eq_ignore_ascii_case(pin) {
            return Err(OAuthError::Tenant);
        }
    }
    if claims.exp <= now_unix {
        return Err(OAuthError::TokenExpired);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64url_and_pkce_match_rfc7636_appendix_b() {
        assert_eq!(b64url(b"f"), "Zg");
        assert_eq!(b64url(b"foob"), "Zm9vYg");
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
