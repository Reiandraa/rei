//! Email-first discovery. Passwords never enter this module. Only a validated
//! domain goes to Thunderbird's fixed HTTPS directory; no guessed mail servers.
use crate::{
    commands::{AppState, IpcError},
    hiz,
    providers::{self, PRes, ProviderError},
    time,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::Read,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

pub const IMAP: &str = "imap_email";
pub const GOOGLE: &str = "google_mail";
pub const MICROSOFT: &str = "microsoft_mail";
const MAX_CONFIG: u64 = 256 * 1024;
static PROFILES: OnceLock<Mutex<HashMap<String, (Instant, Profile)>>> = OnceLock::new();
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Profile {
    pub profile_id: String,
    pub kind: String,
    pub email: String,
    pub display_name: String,
    pub username: String,
    pub server: Option<String>,
    pub publisher_ready: bool,
    pub detail: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoverReq {
    pub email: String,
    pub preferred: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigureReq {
    pub profile_id: String,
    pub username: Option<String>,
}

pub fn is_email_provider(p: &str) -> bool {
    matches!(p, hiz::PROVIDER | IMAP | GOOGLE | MICROSOFT)
}
pub fn require_email(conn: &rusqlite::Connection, id: &str) -> PRes<String> {
    let p: String = conn.query_row(
        "SELECT provider FROM provider_config WHERE account_id=?1",
        [id],
        |r| r.get(0),
    )?;
    if !is_email_provider(&p)
        || providers::require_provider(conn, id, &p)? != crate::connectors::Kind::Email
    {
        return Err(ProviderError::Invalid("not an email account".into()));
    }
    Ok(p)
}
pub fn parts(email: &str) -> PRes<(&str, &str)> {
    if email.chars().any(|c| c.is_control()) {
        return Err(ProviderError::Invalid(
            "enter a complete email address".into(),
        ));
    }
    let (local, domain) = email
        .trim()
        .split_once('@')
        .ok_or_else(|| ProviderError::Invalid("enter a complete email address".into()))?;
    if local.is_empty()
        || email.len() > 254
        || !local
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-=%".contains(&b))
        || !valid_host(domain)
    {
        return Err(ProviderError::Invalid(
            "enter a complete email address".into(),
        ));
    }
    Ok((local, domain))
}
pub fn valid_host(host: &str) -> bool {
    host.len() <= 253
        && host.contains('.')
        && host.parse::<std::net::IpAddr>().is_err()
        && host.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        && !host.to_ascii_lowercase().ends_with(".localhost")
        && !host.to_ascii_lowercase().ends_with(".local")
}
fn profile(kind: &str, email: &str, name: &str, username: &str, server: Option<String>) -> Profile {
    let ready = match kind {
        "google" => crate::mail_cloud::publisher_ready(GOOGLE),
        "microsoft" => crate::mail_cloud::publisher_ready(MICROSOFT),
        "unsupported" => false,
        _ => true,
    };
    Profile {
        profile_id: uuid::Uuid::new_v4().to_string(),
        kind: kind.into(),
        email: email.into(),
        display_name: name.into(),
        username: username.into(),
        server,
        publisher_ready: ready,
        detail: String::new(),
    }
}
#[derive(Deserialize)]
struct ClientConfig {
    #[serde(rename = "emailProvider")]
    provider: EmailProvider,
}
#[derive(Deserialize)]
struct EmailProvider {
    #[serde(rename = "displayName", default)]
    name: String,
    #[serde(rename = "incomingServer", default)]
    incoming: Vec<Incoming>,
}
#[derive(Deserialize)]
struct Incoming {
    #[serde(rename = "@type", default)]
    kind: String,
    hostname: String,
    port: u16,
    #[serde(rename = "socketType")]
    socket: String,
    #[serde(rename = "authentication", default)]
    auth: Vec<String>,
    username: String,
}
pub fn parse_config(email: &str, raw: &str) -> PRes<Profile> {
    let (local, domain) = parts(email)?;
    let upper = raw.to_ascii_uppercase();
    if raw.len() > MAX_CONFIG as usize || upper.contains("<!DOCTYPE") || upper.contains("<!ENTITY")
    {
        return Err(ProviderError::Protocol("unsafe email configuration"));
    }
    let config: ClientConfig = quick_xml::de::from_str(raw)
        .map_err(|_| ProviderError::Protocol("invalid email configuration"))?;
    for server in config.provider.incoming {
        if server.kind != "imap"
            || server.port != 993
            || server.socket != "SSL"
            || !valid_host(&server.hostname)
        {
            continue;
        }
        let host = server.hostname.to_ascii_lowercase();
        if matches!(host.as_str(), "imap.gmail.com" | "imap.googlemail.com") {
            return Ok(profile("google", email, "Google", email, None));
        }
        if matches!(
            host.as_str(),
            "outlook.office365.com" | "imap-mail.outlook.com"
        ) {
            return Ok(profile("microsoft", email, "Microsoft", email, None));
        }
        // OAuth-only configurations are not downgraded to a password form.
        if !server
            .auth
            .iter()
            .any(|a| a == "password-cleartext" || a == "password-encrypted")
            || server.auth.iter().any(|a| a == "OAuth2")
        {
            continue;
        }
        // The IMAP adapter implements LOGIN over verified TLS, not challenge auth.
        if !server.auth.iter().any(|a| a == "password-cleartext") {
            continue;
        }
        let user = server
            .username
            .replace("%EMAILADDRESS%", email)
            .replace("%EMAILLOCALPART%", local)
            .replace("%EMAILDOMAIN%", domain);
        if user.contains('%')
            || user.is_empty()
            || user.len() > 254
            || user.chars().any(|c| c.is_control())
        {
            continue;
        }
        let name = if config.provider.name.is_empty() {
            domain.to_string()
        } else {
            config.provider.name.chars().take(100).collect()
        };
        return Ok(profile("imap", email, &name, &user, Some(host)));
    }
    Err(ProviderError::NotConfigured(
        "no supported secure IMAP or Google/Microsoft login found",
    ))
}
fn discover(req: DiscoverReq) -> PRes<Profile> {
    let email = req.email.trim();
    if let Some(preferred) = req.preferred.as_deref() {
        if !email.is_empty() {
            parts(email)?;
        }
        return match preferred {
            "google" => Ok(profile("google", email, "Google", email, None)),
            "microsoft" => Ok(profile("microsoft", email, "Microsoft", email, None)),
            _ => Err(ProviderError::Invalid(
                "unknown email sign-in option".into(),
            )),
        };
    }
    let (local, domain) = parts(email)?;
    let domain = domain.to_ascii_lowercase();
    if matches!(domain.as_str(), "uni-saarland.de" | "stud.uni-saarland.de") {
        let id = if local.len() == 8
            && local[..3].bytes().all(|b| b.is_ascii_alphabetic())
            && local[3..].bytes().all(|b| b.is_ascii_digit())
        {
            local
        } else {
            ""
        };
        return Ok(profile(
            "hiz",
            email,
            "Saarland University · HIZ",
            id,
            Some(hiz::HIZ_HOST.into()),
        ));
    }
    if matches!(domain.as_str(), "gmail.com" | "googlemail.com") {
        return Ok(profile("google", email, "Google", email, None));
    }
    if [
        "outlook.com",
        "hotmail.com",
        "live.com",
        "msn.com",
        "outlook.de",
        "hotmail.de",
    ]
    .contains(&domain.as_str())
    {
        return Ok(profile("microsoft", email, "Microsoft", email, None));
    }
    // The directory receives the domain only. No email local-part or password.
    let response = ureq::AgentBuilder::new().redirects(0).try_proxy_from_env(false).timeout(Duration::from_secs(12)).build()
        .get(&format!("https://autoconfig.thunderbird.net/v1.1/{domain}")).call().map_err(|_| ProviderError::NotConfigured("automatic settings were not found; try Continue with Google or Microsoft if your email uses them"))?;
    let mut raw = String::new();
    response
        .into_reader()
        .take(MAX_CONFIG + 1)
        .read_to_string(&mut raw)
        .map_err(|_| ProviderError::Network)?;
    parse_config(email, &raw)
}
fn profiles() -> &'static Mutex<HashMap<String, (Instant, Profile)>> {
    PROFILES.get_or_init(|| Mutex::new(HashMap::new()))
}
#[tauri::command]
pub async fn email_discover(req: DiscoverReq) -> Result<Profile, IpcError> {
    let p = tauri::async_runtime::spawn_blocking(move || discover(req))
        .await
        .map_err(|_| IpcError::internal())?
        .map_err(IpcError::from)?;
    let mut cache = profiles().lock().map_err(|_| IpcError::internal())?;
    cache.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(600));
    if cache.len() >= 64 {
        return Err(IpcError::new(
            "RateLimited",
            "Too many pending email setups. Wait and retry.",
        ));
    }
    cache.insert(p.profile_id.clone(), (Instant::now(), p.clone()));
    Ok(p)
}
#[tauri::command]
pub fn email_configure(
    state: tauri::State<'_, AppState>,
    req: ConfigureReq,
) -> Result<String, IpcError> {
    let p = {
        let cache = profiles().lock().map_err(|_| IpcError::internal())?;
        let (at, p) = cache.get(&req.profile_id).ok_or_else(|| {
            IpcError::new(
                "Validation",
                "Check your email address again before connecting.",
            )
        })?;
        if at.elapsed() > Duration::from_secs(600) {
            return Err(IpcError::new(
                "Validation",
                "Email setup expired. Check your address again.",
            ));
        }
        p.clone()
    };
    let conn = providers::lock(&state.db)?;
    let now = time::now_utc_rfc3339();
    let id = if p.kind == "hiz" {
        hiz::configure(
            &conn,
            None,
            &hiz::HizSettings {
                username: req.username.unwrap_or(p.username),
                initial_count: 50,
            },
            &now,
        )?
    } else if p.kind == "imap" {
        hiz::configure_generic(
            &conn,
            &p.email,
            &p.display_name,
            &req.username.unwrap_or(p.username),
            p.server.as_deref().ok_or_else(IpcError::internal)?,
            &now,
        )?
    } else {
        crate::mail_cloud::configure(
            &conn,
            if p.kind == "google" {
                GOOGLE
            } else {
                MICROSOFT
            },
            &p.email,
            &now,
        )?
    };
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn xml(host: &str, auth: &str) -> String {
        format!(
            r#"<clientConfig version="1.1"><emailProvider id="example.org"><displayName>Example University</displayName><incomingServer type="imap"><hostname>{host}</hostname><port>993</port><socketType>SSL</socketType><authentication>{auth}</authentication><username>%EMAILADDRESS%</username></incomingServer></emailProvider></clientConfig>"#
        )
    }
    #[test]
    fn secure_config_uses_full_address_and_never_treats_oauth_as_password() {
        let p = parse_config(
            "student@example.org",
            &xml("mail.example.org", "password-cleartext"),
        )
        .unwrap();
        assert_eq!(p.kind, "imap");
        assert_eq!(p.username, "student@example.org");
        let p = parse_config("student@example.org", &xml("imap.gmail.com", "OAuth2")).unwrap();
        assert_eq!(p.kind, "google");
        assert!(p.server.is_none());
        assert!(parse_config("student@example.org", &xml("mail.example.org", "OAuth2")).is_err());
    }
    #[test]
    fn discovered_account_keeps_its_native_destination_and_secret_is_not_saved_by_configuration() {
        let conn = crate::db::open_in_memory().unwrap();
        let id = hiz::configure_generic(
            &conn,
            "synthetic+student@example.org",
            "Example",
            "synthetic+student@example.org",
            "mail.example.org",
            "2026-10-20T00:00:00Z",
        )
        .unwrap();
        assert_eq!(require_email(&conn, &id).unwrap(), IMAP);
        assert!(hiz::HizEndpoint::for_account(&conn, &id).is_ok());
        let saved: bool = conn
            .query_row(
                "SELECT secret_saved FROM provider_config WHERE account_id=?1",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!saved);
        conn.execute(
            "UPDATE provider_config SET config=?1 WHERE account_id=?2",
            rusqlite::params![r#"{"host":"127.0.0.1","port":993}"#, id],
        )
        .unwrap();
        assert!(matches!(
            hiz::HizEndpoint::for_account(&conn, &id),
            Err(ProviderError::Destination)
        ));
    }
    #[test]
    fn rejects_insecure_ports_hosts_entities_and_address_injection() {
        assert!(parse_config(
            "student@example.org",
            &xml("127.0.0.1", "password-cleartext")
        )
        .is_err());
        assert!(parse_config(
            "student@example.org",
            &xml("mail.example.org", "password-cleartext").replace("993", "143")
        )
        .is_err());
        assert!(parse_config("student@example.org", "<!DOCTYPE x []>").is_err());
        for mail in [
            "x@example.org/evil",
            "x@localhost",
            "x@example.org\r\n",
            "x@example.org@evil.org",
        ] {
            assert!(parts(mail).is_err(), "{mail:?}");
        }
    }
}
