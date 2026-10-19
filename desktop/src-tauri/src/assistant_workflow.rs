//! One explicit owner action joins saved sign-in, source scope, fetching and local analysis.
//! No credentials, model output, provider I/O or account data leave this module through IPC.
use crate::auto_academic::{self, AutoSetReq, AutoSettings};
use crate::commands::{AppState, IpcError};
use crate::{connections, hiz, providers, time};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

type Res<T> = Result<T, IpcError>;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowReq {
    pub enabled: bool,
    pub account_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowResult {
    pub settings: AutoSettings,
    pub polling: providers::Polling,
    pub selected_inbox: bool,
}

/// Atomic activation; a missing sign-in/Inbox never leaves half of the pipeline enabled.
/// Existing source choices, model, zone, credentials, messages and owner decisions survive.
pub fn set_workflow(conn: &mut Connection, now: &str, req: &WorkflowReq) -> Res<WorkflowResult> {
    let mut selected_inbox = false;
    let mut selected_ref = None;
    let _operation = if req.enabled {
        let id = req.account_id.as_deref().ok_or_else(|| {
            IpcError::new(
                "ProviderAuthRequired",
                "Connect your university email first.",
            )
        })?;
        providers::require_provider(conn, id, hiz::PROVIDER)?;
        let (saved, state): (bool, String) = conn.query_row(
            "SELECT p.secret_saved,a.state FROM provider_config p JOIN connector_accounts a ON a.id=p.account_id WHERE p.account_id=?1",
            [id], |r| Ok((r.get(0)?,r.get(1)?)))?;
        if !saved
            || matches!(
                state.as_str(),
                "not_configured" | "expired" | "permission_denied"
            )
        {
            return Err(IpcError::new(
                "ProviderAuthRequired",
                "Sign in to your university email again.",
            ));
        }
        let selected = providers::selected(conn, id, "folder")?;
        // A fetch can be running for an already-selected account. Enabling analysis should
        // not force a second sign-in or interrupt it. A scope change needs the normal guard.
        if selected.is_empty() {
            let guard = providers::operation_guard(conn, id)?;
            selected_ref = conn.query_row("SELECT source_ref FROM provider_sources WHERE account_id=?1 AND source_kind='folder' AND upper(source_ref)='INBOX' LIMIT 1", [id], |r| r.get::<_,String>(0)).optional()?;
            if selected_ref.is_none() {
                return Err(IpcError::new(
                    "Validation",
                    "Inbox was not found. Choose a mail folder in Advanced settings.",
                ));
            }
            Some(guard)
        } else {
            None
        }
    } else {
        None
    };
    let current = auto_academic::get_settings(conn)?;
    let old_poll = providers::get_polling(conn)?;
    let tx = conn.transaction()?;
    if let (Some(id), Some(folder)) = (&req.account_id, selected_ref) {
        tx.execute("UPDATE provider_sources SET selected=1 WHERE account_id=?1 AND source_kind='folder' AND source_ref=?2", rusqlite::params![id,folder])?;
        selected_inbox = true;
    }
    let polling = providers::set_polling(
        &tx,
        &providers::Polling {
            enabled: req.enabled,
            interval_minutes: old_poll.interval_minutes,
        },
        now,
    )?;
    let settings = auto_academic::set_settings(
        &tx,
        now,
        &AutoSetReq {
            enabled: if req.enabled { true } else { current.enabled },
            paused: !req.enabled,
            model: current.model,
            date_zone: None,
        },
    )?;
    tx.commit()?;
    if req.enabled {
        connections::wake_scheduler();
        auto_academic::wake_worker();
    } else {
        // Pause intake as well as inference; ongoing provider I/O stops at the next
        // cancellation check. Do not cancel durable jobs or erase their attempt history.
        for (id, _) in providers::pollable_accounts(conn)? {
            if let Ok(identity) = providers::operation_identity(conn, &id) {
                providers::cancel(&identity);
            }
        }
    }
    Ok(WorkflowResult {
        settings,
        polling,
        selected_inbox,
    })
}

#[tauri::command]
pub fn assistant_workflow_set(
    state: tauri::State<'_, AppState>,
    req: WorkflowReq,
) -> Res<WorkflowResult> {
    set_workflow(
        &mut *providers::lock(&state.db)?,
        &time::now_utc_rfc3339(),
        &req,
    )
}
