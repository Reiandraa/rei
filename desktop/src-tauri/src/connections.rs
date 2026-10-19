//! IPC for HIZ/Teams connections and opt-in polling. Every command does real Rust
//! work against the production endpoints only; no command returns or accepts a
//! token, and the HIZ password is accepted only by `hiz_sign_in`, used once and
//! stored in the OS vault (never logged, never written to SQLite).
use crate::commands::{AppState, IpcError};
use crate::providers::{self, ProviderError};
use crate::vault::{self, Secret};
use crate::{connectors, hiz, teams};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::Manager;

static POLL_WAKE: AtomicU64 = AtomicU64::new(0);
static SCHEDULER: Mutex<Option<std::thread::Thread>> = Mutex::new(None);

pub fn wake_scheduler() {
    POLL_WAKE.fetch_add(1, Ordering::SeqCst);
    if let Ok(s) = SCHEDULER.lock() {
        if let Some(t) = s.as_ref() {
            t.unpark();
        }
    }
}

type Res<T> = Result<T, IpcError>;

impl From<ProviderError> for IpcError {
    fn from(e: ProviderError) -> Self {
        let code = match e {
            ProviderError::Invalid(_) => "Validation",
            ProviderError::NotConfigured(_)
            | ProviderError::SecretMissing
            | ProviderError::Auth => "ProviderAuthRequired",
            ProviderError::Cancelled => "Cancelled",
            ProviderError::Busy => "Conflict",
            ProviderError::Forbidden | ProviderError::Denied => "ProviderDenied",
            ProviderError::Network | ProviderError::TimedOut => "Offline",
            _ => "Unavailable",
        };
        IpcError::new(code, e.to_string())
    }
}

fn now() -> String {
    crate::time::now_utc_rfc3339()
}

fn store() -> Result<Box<dyn vault::SecretStore>, ProviderError> {
    vault::platform_store().map_err(|_| ProviderError::VaultUnavailable)
}

#[derive(Debug, Serialize)]
pub struct ConnectionsStatus {
    pub accounts: Vec<providers::ProviderAccountStatus>,
    pub polling: providers::Polling,
    pub hiz_host: String,
    pub hiz_port: u16,
    pub os_vault_available: bool,
}

pub fn status_of(conn: &rusqlite::Connection) -> Result<ConnectionsStatus, ProviderError> {
    Ok(ConnectionsStatus {
        accounts: providers::status(conn)?,
        polling: providers::get_polling(conn)?,
        hiz_host: hiz::HIZ_HOST.into(),
        hiz_port: hiz::HIZ_PORT,
        os_vault_available: vault::platform_store().is_ok(),
    })
}

#[tauri::command]
pub fn connections_status(state: tauri::State<'_, AppState>) -> Res<ConnectionsStatus> {
    Ok(status_of(&*providers::lock(&state.db)?)?)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HizConfigureReq {
    pub account_id: Option<String>,
    pub username: String,
    pub initial_count: u32,
}

#[tauri::command]
pub fn hiz_configure(state: tauri::State<'_, AppState>, req: HizConfigureReq) -> Res<String> {
    let c = providers::lock(&state.db)?;
    let s = hiz::HizSettings {
        username: req.username,
        initial_count: req.initial_count,
    };
    Ok(hiz::configure(&c, req.account_id.as_deref(), &s, &now())?)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HizSignInReq {
    pub account_id: String,
    pub password: String,
}

// Never print the request (it carries the password).
impl std::fmt::Debug for HizSignInReq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HizSignInReq({}, <redacted>)", self.account_id)
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, ProviderError> + Send + 'static,
) -> Res<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|_| IpcError::internal())?
        .map_err(IpcError::from)
}

#[tauri::command]
pub async fn hiz_sign_in(app: tauri::AppHandle, req: HizSignInReq) -> Res<Vec<String>> {
    let pw = Secret::new(req.password.into_bytes());
    let id = req.account_id;
    blocking(move || {
        let _run = providers::RunGuard::start(&id)?;
        let st = app.state::<AppState>();
        hiz::sign_in(
            &st.db,
            &*store()?,
            &hiz::HizEndpoint::preset(),
            &id,
            pw,
            &now(),
        )
    })
    .await
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamsConfigureReq {
    pub account_id: Option<String>,
    pub client_id: String,
    pub tenant: String,
    pub channels: bool,
}

#[tauri::command]
pub fn teams_configure(state: tauri::State<'_, AppState>, req: TeamsConfigureReq) -> Res<String> {
    let c = providers::lock(&state.db)?;
    let s = teams::TeamsSettings {
        client_id: req.client_id,
        tenant: req.tenant,
        channels: req.channels,
    };
    Ok(teams::configure(&c, req.account_id.as_deref(), &s, &now())?)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountReq {
    pub account_id: String,
}

#[tauri::command]
pub async fn teams_sign_in(app: tauri::AppHandle, req: AccountReq) -> Res<String> {
    blocking(move || {
        let st = app.state::<AppState>();
        let run = providers::operation_guard(&*providers::lock(&st.db)?, &req.account_id)?;
        let opener = |url: &str| open::that_detached(url).map_err(|_| ProviderError::Network);
        let t = jiff::Timestamp::now().as_second();
        teams::sign_in(
            &st.db,
            &*store()?,
            &teams::GraphEndpoint::production(),
            &req.account_id,
            &opener,
            &run.cancel,
            t,
        )?;
        Ok("signed_in".to_string())
    })
    .await
}

#[tauri::command]
pub async fn teams_discover(app: tauri::AppHandle, req: AccountReq) -> Res<u32> {
    blocking(move || {
        let st = app.state::<AppState>();
        let run = providers::operation_guard(&*providers::lock(&st.db)?, &req.account_id)?;
        teams::discover(
            &st.db,
            &*store()?,
            &teams::GraphEndpoint::production(),
            &req.account_id,
            &run.cancel,
            &now(),
        )
    })
    .await
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedSource {
    pub source_kind: String,
    pub source_ref: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectReq {
    pub account_id: String,
    pub selected: Vec<SelectedSource>,
}

#[tauri::command]
pub fn connection_select_sources(state: tauri::State<'_, AppState>, req: SelectReq) -> Res<u32> {
    let mut c = providers::lock(&state.db)?;
    let sel: Vec<(String, String)> = req
        .selected
        .into_iter()
        .map(|s| (s.source_kind, s.source_ref))
        .collect();
    Ok(providers::set_selected(&mut c, &req.account_id, &sel)?)
}

/// One fetch for one account against the production provider.
pub fn fetch_account(st: &AppState, account_id: &str) -> Result<serde_json::Value, ProviderError> {
    fetch_account_inner(st, account_id, false)
}

fn fetch_account_inner(
    st: &AppState,
    account_id: &str,
    scheduled: bool,
) -> Result<serde_json::Value, ProviderError> {
    let run = {
        let c = providers::lock(&st.db)?;
        let run = providers::operation_guard(&c, account_id)?;
        if scheduled && !providers::get_polling(&c)?.enabled {
            return Err(ProviderError::Cancelled);
        }
        run
    };
    if run.cancelled() {
        return Err(ProviderError::Cancelled);
    }
    let provider: String = {
        let c = providers::lock(&st.db)?;
        c.query_row(
            "SELECT provider FROM provider_config WHERE account_id=?1",
            [account_id],
            |r| r.get(0),
        )
        .map_err(|_| ProviderError::Invalid("unknown account".into()))?
    };
    let s = store()?;
    let v = match provider.as_str() {
        hiz::PROVIDER => serde_json::to_value(hiz::fetch(
            &st.db,
            &*s,
            &hiz::HizEndpoint::preset(),
            account_id,
            &run.cancel,
            &now(),
        )?),
        teams::PROVIDER => serde_json::to_value(teams::fetch(
            &st.db,
            &*s,
            &teams::GraphEndpoint::production(),
            account_id,
            &run.cancel,
            &now(),
        )?),
        _ => return Err(ProviderError::Invalid("unknown provider".into())),
    };
    let value = v.map_err(|_| ProviderError::Db)?;
    crate::auto_academic::wake_worker();
    Ok(value)
}

#[tauri::command]
pub async fn connection_fetch_now(
    app: tauri::AppHandle,
    req: AccountReq,
) -> Res<serde_json::Value> {
    blocking(move || fetch_account(&app.state::<AppState>(), &req.account_id)).await
}

#[tauri::command]
pub fn connection_fetch_cancel(state: tauri::State<'_, AppState>, req: AccountReq) -> Res<bool> {
    let identity = providers::operation_identity(&*providers::lock(&state.db)?, &req.account_id)?;
    Ok(providers::cancel(&identity))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisconnectReq {
    pub account_id: String,
    /// Explicit owner choice: also delete locally stored messages from this account.
    pub purge: bool,
}

/// Disconnect: removes the saved sign-in from the OS vault, clears cursors and,
/// only when chosen, purges stored messages. Approved tasks are never deleted.
pub fn disconnect(
    conn: &mut rusqlite::Connection,
    store: &dyn vault::SecretStore,
    account_id: &str,
    purge: bool,
) -> Result<(), ProviderError> {
    // Reject concurrent disconnect rather than removing a secret while an older
    // sign-in is about to save it again. Cancel and wait for the operation first.
    let _run = providers::operation_guard(conn, account_id)?;
    let provider: String = conn
        .query_row(
            "SELECT provider FROM provider_config WHERE account_id=?1",
            [account_id],
            |r| r.get(0),
        )
        .map_err(|_| ProviderError::Invalid("unknown account".into()))?;
    let accounts = if provider == teams::PROVIDER {
        let l = teams::linked(conn, account_id)?;
        teams::sign_out(store, conn, account_id)?;
        l
    } else {
        hiz::forget_password(store, account_id)?;
        vec![account_id.to_string()]
    };
    let t = now();
    for a in &accounts {
        connectors::disconnect(conn, a, purge, &t)?;
        conn.execute("DELETE FROM imap_uids WHERE account_id=?1", [a])?;
        conn.execute(
            "UPDATE provider_sources SET selected=0 WHERE account_id=?1",
            [a],
        )?;
        providers::set_secret_saved(conn, a, false, &t)?;
        providers::record_outcome(
            conn,
            a,
            false,
            if purge {
                "Disconnected; stored messages deleted"
            } else {
                "Disconnected; stored messages kept"
            },
            &t,
        )?;
    }
    Ok(())
}

#[tauri::command]
pub fn connection_disconnect(state: tauri::State<'_, AppState>, req: DisconnectReq) -> Res<bool> {
    let mut c = providers::lock(&state.db)?;
    disconnect(&mut c, &*store()?, &req.account_id, req.purge)?;
    Ok(true)
}

#[tauri::command]
pub fn polling_set(
    state: tauri::State<'_, AppState>,
    req: providers::Polling,
) -> Res<providers::Polling> {
    Ok(providers::set_polling(
        &*providers::lock(&state.db)?,
        &req,
        &now(),
    )?)
}

/// Opt-in in-app polling: one account at a time (bounded concurrency), backoff
/// after failures, only while the desktop app runs. No OS scheduled task/service.
pub fn start_scheduler(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        if let Ok(mut t) = SCHEDULER.lock() {
            *t = Some(std::thread::current());
        }
        let mut failures = 0u32;
        let mut seen = POLL_WAKE.load(Ordering::SeqCst);
        let mut was_enabled = false;
        let mut next_due = std::time::Instant::now();
        loop {
            let st = app.state::<AppState>();
            let (p, accounts) = match providers::lock(&st.db).and_then(|c| {
                Ok((
                    providers::get_polling(&c)?,
                    providers::pollable_accounts(&c)?,
                ))
            }) {
                Ok(x) => x,
                Err(_) => {
                    std::thread::park_timeout(std::time::Duration::from_secs(20));
                    continue;
                }
            };
            let wake = POLL_WAKE.load(Ordering::SeqCst);
            if wake != seen || (p.enabled && !was_enabled) {
                next_due = std::time::Instant::now();
            }
            seen = wake;
            was_enabled = p.enabled;
            if !p.enabled || std::time::Instant::now() < next_due {
                std::thread::park_timeout(std::time::Duration::from_secs(20));
                continue;
            }
            let mut more = false;
            let mut ok = true;
            for (id, _) in accounts {
                match fetch_account_inner(&st, &id, true) {
                    Ok(report) => {
                        more |= report
                            .get("more_remaining")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                    }
                    Err(ProviderError::Busy) => more = true,
                    Err(_) => ok = false,
                }
                // Re-check after every account so disabling stops the round.
                if !providers::lock(&st.db)
                    .and_then(|c| providers::get_polling(&c))
                    .map(|p| p.enabled)
                    .unwrap_or(false)
                {
                    break;
                }
            }
            failures = if ok { 0 } else { failures + 1 };
            next_due = std::time::Instant::now()
                + if ok && more {
                    std::time::Duration::from_secs(30)
                } else {
                    providers::poll_delay(&p, failures)
                };
            std::thread::park_timeout(std::time::Duration::from_secs(20));
        }
    });
}
