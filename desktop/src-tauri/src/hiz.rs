//! HIZ mailbox intake: IMAP over implicit TLS (rustls + webpki roots), the `imap`
//! crate for the protocol and `mail-parser` for MIME. Strictly read-only: EXAMINE,
//! UID SEARCH and UID FETCH with BODY.PEEK only — no STORE/EXPUNGE/APPEND/SMTP,
//! read flags are never changed; attachments are not persisted/opened and remote
//! content is never loaded. Bounded raw MIME fetching can include attachment bytes.
//! The only production destination is the documented HIZ preset; synthetic test
//! endpoints exist for local tests and are not reachable through IPC or settings.
use crate::connectors::{self, Evidence, ImapMessage, Kind};
use crate::providers::{self, PRes, ProviderError};
use crate::vault::{Secret, SecretKind, SecretStore, VaultKey};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const PROVIDER: &str = "hiz_imap";
/// Documented HIZ preset (official HIZ guide, university IT page; not owner-tested).
pub const HIZ_HOST: &str = "mail.hiz-saarland.de";
pub const HIZ_PORT: u16 = 993;
pub const MAX_MESSAGE_BYTES: u32 = 1_000_000;
pub const MAX_BATCH: usize = 25;
pub const MAX_PER_RUN: usize = 500;
pub const MAX_INITIAL: u32 = 500;
pub const MAX_FOLDERS: usize = 20;
pub const MAX_FOLDER_SIZE: u32 = 200_000;
const IO_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HizSettings {
    /// HIZ identifier (login name) as shown by HIZ; never assumed to equal Teams.
    pub username: String,
    /// Most recent messages per folder read on the first fetch (1-500).
    pub initial_count: u32,
}

impl HizSettings {
    pub fn validate(&self) -> PRes<()> {
        let u = self.username.trim();
        if u.is_empty()
            || u.len() > 254
            || !u
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@'))
        {
            return Err(ProviderError::Invalid(
                "HIZ identifier must be 1-254 letters, digits or . - _ @".into(),
            ));
        }
        if !(1..=MAX_INITIAL).contains(&self.initial_count) {
            return Err(ProviderError::Invalid(format!(
                "initial history must be 1-{MAX_INITIAL} messages"
            )));
        }
        Ok(())
    }
}

/// Where the IMAP client connects. Only `preset()` is used by the app.
pub struct HizEndpoint {
    target: Target,
}

enum Target {
    Preset,
    Mail {
        host: String,
    },
    Synthetic {
        addr: SocketAddr,
        server_name: String,
        root_der: Vec<u8>,
    },
}

impl HizEndpoint {
    pub fn preset() -> Self {
        HizEndpoint {
            target: Target::Preset,
        }
    }
    /// Generic email endpoints originate in native discovery/configuration only.
    pub fn for_account(conn: &Connection, id: &str) -> PRes<Self> {
        let provider = crate::email_discovery::require_email(conn, id)?;
        if provider == PROVIDER {
            return Ok(Self::preset());
        }
        if provider != crate::email_discovery::IMAP {
            return Err(ProviderError::Destination);
        }
        let cfg = providers::load_config(conn, id, &provider)?;
        let host = cfg["host"].as_str().ok_or(ProviderError::Destination)?;
        if !crate::email_discovery::valid_host(host) || cfg["port"].as_u64() != Some(993) {
            return Err(ProviderError::Destination);
        }
        Ok(Self {
            target: Target::Mail { host: host.into() },
        })
    }
    /// Local synthetic TLS server for tests (loopback only, custom test root).
    /// Data read through it is labeled SYNTHETIC_FIXTURE, never live.
    #[doc(hidden)]
    pub fn synthetic_for_tests(addr: SocketAddr, server_name: &str, root_der: Vec<u8>) -> Self {
        assert!(
            addr.ip().is_loopback(),
            "synthetic endpoint must be loopback"
        );
        HizEndpoint {
            target: Target::Synthetic {
                addr,
                server_name: server_name.into(),
                root_der,
            },
        }
    }
    pub fn evidence(&self) -> Evidence {
        match self.target {
            Target::Preset | Target::Mail { .. } => Evidence::Live,
            Target::Synthetic { .. } => Evidence::Fixture,
        }
    }
}

/// Public provider destinations only: no loopback/private/link-local addresses.
pub fn is_public(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v) => {
            !(v.is_private()
                || v.is_loopback()
                || v.is_link_local()
                || v.is_unspecified()
                || v.is_broadcast()
                || v.is_documentation()
                || v.is_multicast()
                || v.octets()[0] == 0
                || v.octets()[0] >= 240
                || v.octets()[0] == 198 && matches!(v.octets()[1], 18 | 19)
                || v.octets()[0] == 192 && v.octets()[1] == 0 && v.octets()[2] == 0
                || v.octets()[0] == 100 && (v.octets()[1] & 0xC0) == 64)
        }
        std::net::IpAddr::V6(v) => {
            (v.segments()[0] & 0xe000) == 0x2000
                && !(v.is_loopback()
                    || v.is_unspecified()
                    || v.is_multicast()
                    || v.segments()[0] == 0x2002
                    || v.segments()[0] == 0x2001 && matches!(v.segments()[1], 0 | 0xdb8)
                    || (v.segments()[0] & 0xfe00) == 0xfc00
                    || (v.segments()[0] & 0xffc0) == 0xfe80)
        }
    }
}

type Tls = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;

fn tls_config(roots: rustls::RootCertStore) -> PRes<Arc<rustls::ClientConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| ProviderError::Tls)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(cfg))
}

fn connect(ep: &HizEndpoint) -> PRes<imap::Client<Tls>> {
    let (addr, name, roots) = match &ep.target {
        Target::Preset | Target::Mail { .. } => {
            let host = match &ep.target {
                Target::Mail { host } => host.as_str(),
                _ => HIZ_HOST,
            };
            let addrs: Vec<SocketAddr> = (host, HIZ_PORT)
                .to_socket_addrs()
                .map_err(|_| ProviderError::Network)?
                .collect();
            let addr = *addrs.first().ok_or(ProviderError::Network)?;
            if addrs.iter().any(|a| !is_public(a.ip())) {
                return Err(ProviderError::Destination);
            }
            let roots =
                rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            (addr, host.to_string(), roots)
        }
        Target::Synthetic {
            addr,
            server_name,
            root_der,
        } => {
            let mut roots = rustls::RootCertStore::empty();
            roots
                .add(rustls::pki_types::CertificateDer::from(root_der.clone()))
                .map_err(|_| ProviderError::Tls)?;
            (*addr, server_name.clone(), roots)
        }
    };
    let tcp = TcpStream::connect_timeout(&addr, IO_TIMEOUT).map_err(|_| ProviderError::Network)?;
    tcp.set_read_timeout(Some(IO_TIMEOUT))
        .and_then(|_| tcp.set_write_timeout(Some(IO_TIMEOUT)))
        .map_err(|_| ProviderError::Network)?;
    let sni = rustls::pki_types::ServerName::try_from(name).map_err(|_| ProviderError::Tls)?;
    let conn =
        rustls::ClientConnection::new(tls_config(roots)?, sni).map_err(|_| ProviderError::Tls)?;
    let mut tls = rustls::StreamOwned::new(conn, tcp);
    // Complete the handshake before any IMAP byte so certificate errors are TLS errors.
    while tls.conn.is_handshaking() {
        tls.conn
            .complete_io(&mut tls.sock)
            .map_err(|_| ProviderError::Tls)?;
    }
    let mut client = imap::Client::new(tls);
    client.read_greeting().map_err(map_imap)?;
    Ok(client)
}

fn map_imap(e: imap::Error) -> ProviderError {
    match e {
        imap::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => ProviderError::TimedOut,
        imap::Error::Io(_) | imap::Error::ConnectionLost => ProviderError::Network,
        imap::Error::No(_) | imap::Error::Bad(_) => {
            ProviderError::Protocol("server refused a command")
        }
        _ => ProviderError::Protocol("malformed IMAP response"),
    }
}

fn login(ep: &HizEndpoint, user: &str, password: &Secret) -> PRes<imap::Session<Tls>> {
    let client = connect(ep)?;
    let pw = std::str::from_utf8(password.expose()).map_err(|_| ProviderError::SecretMissing)?;
    client.login(user, pw).map_err(|(e, _)| match e {
        imap::Error::No(_) => ProviderError::Auth,
        other => map_imap(other),
    })
}

fn password_key(account_id: &str) -> PRes<VaultKey> {
    VaultKey::new(account_id, SecretKind::HizPassword)
        .map_err(|_| ProviderError::Invalid("account id".into()))
}

// ---------- setup ----------

/// Creates (or updates) the HIZ account with non-secret settings.
pub fn configure(
    conn: &Connection,
    account_id: Option<&str>,
    s: &HizSettings,
    now: &str,
) -> PRes<String> {
    s.validate()?;
    let _run = account_id
        .map(|id| providers::operation_guard(conn, id))
        .transpose()?;
    if let Some(id) = account_id {
        let previous = settings_of(conn, id)?;
        let saved: bool = conn.query_row(
            "SELECT secret_saved FROM provider_config WHERE account_id=?1",
            [id],
            |r| r.get(0),
        )?;
        if saved && previous.username != s.username.trim() {
            return Err(ProviderError::Invalid(
                "disconnect before changing the HIZ identifier".into(),
            ));
        }
    }
    let id = match account_id {
        Some(id) => {
            providers::require_provider(conn, id, PROVIDER)?;
            providers::reopen(conn, id, now)?;
            id.to_string()
        }
        None => {
            let a = connectors::create_account(conn, Kind::Email, "HIZ", "HIZ mailbox", now)?;
            connectors::transition(
                conn,
                &a.id,
                connectors::ConnState::ConsentNeeded,
                Evidence::None,
                None,
                now,
            )?;
            a.id
        }
    };
    let cfg = serde_json::json!({
        "username": s.username.trim(),
        "initial_count": s.initial_count,
        "host": HIZ_HOST,
        "port": HIZ_PORT,
        "security": "IMAP over TLS (implicit, port 993)",
    });
    providers::save_config(conn, &id, PROVIDER, &cfg, now)?;
    Ok(id)
}

/// Configure an implicitly TLS-protected generic IMAP account after native discovery.
pub fn configure_generic(
    conn: &Connection,
    email: &str,
    name: &str,
    username: &str,
    host: &str,
    now: &str,
) -> PRes<String> {
    crate::email_discovery::parts(email)?;
    if !crate::email_discovery::valid_host(host)
        || username.is_empty()
        || username.len() > 254
        || username.chars().any(|c| c.is_control())
    {
        return Err(ProviderError::Invalid(
            "invalid discovered email settings".into(),
        ));
    }
    let account = connectors::create_account(conn, Kind::Email, "IMAP", name, now)?;
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
        crate::email_discovery::IMAP,
        &serde_json::json!({"email":email,"username":username,"initial_count":50,"host":host,"port":993,"security":"IMAP over TLS"}),
        now,
    )?;
    Ok(account.id)
}

fn settings_of(conn: &Connection, account_id: &str) -> PRes<HizSettings> {
    let provider = crate::email_discovery::require_email(conn, account_id)?;
    if !matches!(provider.as_str(), PROVIDER | crate::email_discovery::IMAP) {
        return Err(ProviderError::Destination);
    }
    let v = providers::load_config(conn, account_id, &provider)?;
    Ok(HizSettings {
        username: v["username"].as_str().unwrap_or("").to_string(),
        initial_count: v["initial_count"].as_u64().unwrap_or(50) as u32,
    })
}

/// Signs in once with a transiently entered password, lists folders, and only on
/// success stores the password in the OS vault. A vault failure leaves nothing saved.
pub fn sign_in(
    db: &Mutex<Connection>,
    store: &dyn SecretStore,
    ep: &HizEndpoint,
    account_id: &str,
    password: Secret,
    now: &str,
) -> PRes<Vec<String>> {
    let user = {
        let c = providers::lock(db)?;
        settings_of(&c, account_id)?;
        settings_of(&c, account_id)?.username
    };
    let res = login(ep, &user, &password).and_then(|mut s| {
        let folders = list_folders(&mut s);
        let _ = s.logout();
        folders
    });
    let c = providers::lock(db)?;
    match res {
        Ok(folders) => {
            store
                .put(&password_key(account_id)?, &password)
                .map_err(|_| ProviderError::VaultUnavailable)?;
            providers::set_secret_saved(&c, account_id, true, now)?;
            providers::record_authenticated(&c, account_id, now)?;
            let found: Vec<(String, String, String)> = folders
                .iter()
                .map(|f| ("folder".to_string(), f.clone(), f.clone()))
                .collect();
            providers::upsert_discovered(&c, account_id, &found, now)?;
            Ok(folders)
        }
        Err(e) => {
            providers::record_outcome(&c, account_id, false, &e.to_string(), now)?;
            Err(e)
        }
    }
}

fn list_folders(s: &mut imap::Session<Tls>) -> PRes<Vec<String>> {
    let names = s.list(Some(""), Some("*")).map_err(map_imap)?;
    let mut out: Vec<String> = names
        .iter()
        .filter(|n| {
            !n.attributes()
                .iter()
                .any(|a| matches!(a, imap_proto::NameAttribute::NoSelect))
        })
        .map(|n| n.name().to_string())
        .filter(|n| valid_folder(n))
        .take(providers::MAX_SOURCES)
        .collect();
    out.sort();
    Ok(out)
}

/// Folder names are sent as quoted IMAP strings by the library; additionally refuse
/// control characters and quotes so a hostile server listing cannot inject commands.
pub fn valid_folder(f: &str) -> bool {
    !f.is_empty() && f.len() <= 200 && !f.chars().any(|c| c.is_control() || c == '"' || c == '\\')
}

// ---------- fetch ----------

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct FetchReport {
    pub folders: u32,
    pub inserted: u32,
    pub updated: u32,
    pub unchanged: u32,
    pub removed: u32,
    pub skipped: u32,
    pub oversized_headers_only: u32,
    /// More messages remain beyond this run's bound; the next fetch continues.
    pub more_remaining: bool,
}

/// Stable identity: Message-ID survives UIDVALIDITY resets and folder moves. The
/// Message-ID header is sender-controlled, so it is bound to a fingerprint of the
/// parsed content: a hostile mail reusing a legitimate Message-ID gets its own
/// identity instead of overwriting (or resurrecting) the stored original.
pub fn object_id(
    message_id: Option<&str>,
    fingerprint: &str,
    folder: &str,
    uv: u32,
    uid: u32,
) -> String {
    match message_id.map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => {
            let mut h = Sha256::new();
            h.update(m.as_bytes());
            h.update([0u8]);
            h.update(fingerprint.as_bytes());
            format!("mid:{}", &hex(&h.finalize())[..40])
        }
        None => format!(
            "uid:{}:{uv}:{uid}",
            &hex(&Sha256::digest(folder.as_bytes()))[..16]
        ),
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Parses raw RFC 5322 bytes with mail-parser (MIME, charsets, encoded words).
pub fn parse_message(
    raw: &[u8],
    folder: &str,
    uv: u32,
    uid: u32,
    headers_only: bool,
    fallback_date: &str,
) -> Option<ImapMessage> {
    let m = mail_parser::MessageParser::default().parse(raw)?;
    // Hostile/malformed input without any RFC 5322 header is skipped, not stored.
    if m.from().is_none() && m.date().is_none() && m.subject().is_none() && m.message_id().is_none()
    {
        return None;
    }
    let sender = m
        .from()
        .and_then(|a| a.first())
        .map(|a| match (a.name(), a.address()) {
            (Some(n), Some(ad)) => format!("{n} <{ad}>"),
            (Some(n), None) => n.to_string(),
            (None, Some(ad)) => ad.to_string(),
            (None, None) => String::new(),
        });
    let received_at = m
        .date()
        .map(|d| d.to_timestamp())
        .filter(|t| (0..=253_402_300_799).contains(t))
        .map(crate::time::format_unix)
        .unwrap_or_else(|| fallback_date.to_string());
    let body = if headers_only {
        "[Message larger than the intake limit: only headers were read. Open it in your mail client.]".to_string()
    } else {
        m.body_text(0).map(|b| b.into_owned()).unwrap_or_default()
    };
    let sender = sender.filter(|s| !s.is_empty());
    let subject = m.subject().map(str::to_string);
    let fingerprint = format!(
        "{}\0{}\0{}\0{}",
        sender.as_deref().unwrap_or(""),
        received_at,
        subject.as_deref().unwrap_or(""),
        body
    );
    Some(ImapMessage {
        uid,
        object_id: object_id(m.message_id(), &fingerprint, folder, uv, uid),
        sender,
        subject,
        body,
        received_at,
    })
}

struct Snapshot {
    user: String,
    initial: u32,
    folders: Vec<String>,
    cursors: Vec<Option<(u32, u32)>>,
}

fn snapshot(conn: &Connection, account_id: &str) -> PRes<Snapshot> {
    let s = settings_of(conn, account_id)?;
    let folders = providers::selected(conn, account_id, "folder")?;
    if folders.is_empty() {
        return Err(ProviderError::NotConfigured("select at least one folder"));
    }
    let cursors = folders
        .iter()
        .map(|f| connectors::imap_cursor(conn, account_id, f))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Snapshot {
        user: s.username,
        initial: s.initial_count,
        folders: folders.into_iter().take(MAX_FOLDERS).collect(),
        cursors,
    })
}

/// Fetches new messages from the selected folders. The DB lock is held only for
/// snapshot and per-batch commits, never during network I/O.
pub fn fetch(
    db: &Mutex<Connection>,
    store: &dyn SecretStore,
    ep: &HizEndpoint,
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
                    "Read {} folder(s): {} new, {} updated, {} removed{}",
                    r.folders,
                    r.inserted,
                    r.updated,
                    r.removed,
                    if r.more_remaining {
                        "; more messages remain beyond this run's limit, fetch again"
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
    ep: &HizEndpoint,
    account_id: &str,
    cancel: &AtomicBool,
    now: &str,
) -> PRes<FetchReport> {
    if cancel.load(Ordering::SeqCst) {
        return Err(ProviderError::Cancelled);
    }
    let snap = snapshot(&*providers::lock(db)?, account_id)?;
    let password = store.get(&password_key(account_id)?).map_err(|e| match e {
        crate::vault::VaultError::NotFound => ProviderError::SecretMissing,
        _ => ProviderError::VaultUnavailable,
    })?;
    if cancel.load(Ordering::SeqCst) {
        return Err(ProviderError::Cancelled);
    }
    let mut s = login(ep, &snap.user, &password)?;
    drop(password);
    let mut rep = FetchReport::default();
    let mut budget = MAX_PER_RUN;
    for (folder, cursor) in snap.folders.iter().zip(snap.cursors.iter()) {
        if cancel.load(Ordering::SeqCst) {
            return Err(ProviderError::Cancelled);
        }
        let mb = s.examine(folder).map_err(map_imap)?;
        if mb.exists > MAX_FOLDER_SIZE {
            return Err(ProviderError::Protocol("folder exceeds the supported size"));
        }
        let uv = mb
            .uid_validity
            .ok_or(ProviderError::Protocol("missing UIDVALIDITY"))?;
        let all: HashSet<u32> = s.uid_search("ALL").map_err(map_imap)?;
        let mut uids: Vec<u32> = all.iter().copied().collect();
        uids.sort_unstable();
        let want: Vec<u32> = match cursor {
            Some((v, last)) if *v == uv => uids.into_iter().filter(|u| u > last).collect(),
            // First fetch or UIDVALIDITY reset: owner-chosen bounded recent history.
            _ => {
                let n = uids.len().saturating_sub(snap.initial as usize);
                uids.split_off(n)
            }
        };
        if want.len() > budget {
            rep.more_remaining = true;
        }
        let want: Vec<u32> = want.into_iter().take(budget).collect();
        budget -= want.len();
        for chunk in want.chunks(MAX_BATCH) {
            if cancel.load(Ordering::SeqCst) {
                return Err(ProviderError::Cancelled);
            }
            let batch = fetch_chunk(&mut s, folder, uv, chunk, now, &mut rep)?;
            let sum = connectors::apply_imap_batch(
                &mut *providers::lock(db)?,
                account_id,
                folder,
                uv,
                &batch,
                chunk.iter().copied().max().unwrap_or(0),
                now,
            )?;
            rep.inserted += sum.inserted;
            rep.updated += sum.updated;
            rep.unchanged += sum.unchanged;
        }
        if want.is_empty() {
            // Records UIDVALIDITY and that the folder was read, even with no new mail.
            connectors::apply_imap_batch(
                &mut *providers::lock(db)?,
                account_id,
                folder,
                uv,
                &[],
                0,
                now,
            )?;
        }
        rep.removed += connectors::reconcile_imap_deletions(
            &mut *providers::lock(db)?,
            account_id,
            folder,
            uv,
            &all,
            now,
        )?;
        rep.folders += 1;
    }
    let _ = s.logout();
    Ok(rep)
}

fn uid_set(uids: &[u32]) -> String {
    uids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn fetch_chunk(
    s: &mut imap::Session<Tls>,
    folder: &str,
    uv: u32,
    chunk: &[u32],
    now: &str,
    rep: &mut FetchReport,
) -> PRes<Vec<ImapMessage>> {
    let sizes = s
        .uid_fetch(uid_set(chunk), "(UID RFC822.SIZE)")
        .map_err(map_imap)?;
    let mut small = Vec::new();
    let mut big = Vec::new();
    for f in sizes.iter() {
        let Some(uid) = f.uid.filter(|u| chunk.contains(u)) else {
            continue;
        };
        if f.size.unwrap_or(u32::MAX) <= MAX_MESSAGE_BYTES {
            small.push(uid);
        } else {
            big.push(uid);
        }
    }
    let mut out = Vec::new();
    for (set, query, headers_only) in [
        (&small, "(UID BODY.PEEK[])", false),
        (&big, "(UID BODY.PEEK[HEADER])", true),
    ] {
        if set.is_empty() {
            continue;
        }
        let fetched = s.uid_fetch(uid_set(set), query).map_err(map_imap)?;
        for f in fetched.iter() {
            let Some(uid) = f.uid.filter(|u| set.contains(u)) else {
                continue;
            };
            let raw = if headers_only { f.header() } else { f.body() };
            match raw
                .filter(|r| r.len() <= MAX_MESSAGE_BYTES as usize + 65_536)
                .and_then(|r| parse_message(r, folder, uv, uid, headers_only, now))
            {
                Some(m) => {
                    if headers_only {
                        rep.oversized_headers_only += 1;
                    }
                    out.push(m);
                }
                None => rep.skipped += 1,
            }
        }
    }
    out.sort_by_key(|m| m.uid);
    Ok(out)
}

/// Removes the saved HIZ password from the OS vault (used by disconnect).
pub fn forget_password(store: &dyn SecretStore, account_id: &str) -> PRes<()> {
    store
        .delete(&password_key(account_id)?)
        .map_err(|_| ProviderError::VaultUnavailable)
}
