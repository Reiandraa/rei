//! Typed IPC commands. Names must match schemas/ipc-v1.json and capabilities/default.json.
use crate::{db, planner, proposals, settings};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

pub const IPC_VERSION: u32 = 1;

/// Every command exposed to the renderer. Single source of truth for contract tests.
pub const COMMANDS: &[&str] = &[
    "app_status",
    "ai_settings",
    "local_ai_setup",
    "email_discover",
    "email_configure",
    "email_sign_in",
    "assistant_summary_get",
    "assistant_summary_refresh",
    "assistant_memory",
    "health",
    "settings_get",
    "settings_set",
    "task_list",
    "task_get",
    "task_create",
    "task_update",
    "task_complete",
    "task_delete",
    "occurrence_edit",
    "schedule_change_apply",
    "schedule_change_undo",
    "agenda_list",
    "calendar_day_labels",
    "event_list",
    "event_create",
    "event_update",
    "event_delete",
    "proposal_list",
    "proposal_approve",
    "proposal_reject",
    "planner_suggest_reschedules",
    "inbox_overview",
    "source_list",
    "source_get",
    "source_import_manual",
    "model_status",
    "source_analyze",
    "source_analyze_cancel",
    "candidate_approve",
    "candidate_reject",
    "assistant_workflow_set",
    "connections_status",
    "hiz_configure",
    "hiz_sign_in",
    "teams_configure",
    "teams_sign_in",
    "teams_discover",
    "connection_select_sources",
    "connection_fetch_now",
    "connection_fetch_cancel",
    "connection_disconnect",
    "polling_set",
    "auto_settings_get",
    "auto_settings_set",
    "auto_summary",
    "auto_queue_cancel",
    "auto_queue_retry",
    "candidate_batch_approve",
    "task_plan",
    "task_unplan",
    "task_reopen",
    "task_undo",
    "model_list",
];

pub struct AppState {
    pub db: Mutex<Connection>,
    pub device_id: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct IpcError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_revision: Option<u64>,
}

impl IpcError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        IpcError {
            code: code.into(),
            message: message.into(),
            current_revision: None,
        }
    }

    pub fn internal() -> Self {
        IpcError {
            code: "Internal".into(),
            message: "internal error".into(),
            current_revision: None,
        }
    }
}

impl From<settings::SettingsError> for IpcError {
    fn from(e: settings::SettingsError) -> Self {
        match e {
            settings::SettingsError::Validation(m) => IpcError {
                code: "Validation".into(),
                message: m,
                current_revision: None,
            },
            settings::SettingsError::Conflict { current_revision } => IpcError {
                code: "Conflict".into(),
                message: "revision mismatch".into(),
                current_revision: Some(current_revision),
            },
            settings::SettingsError::Internal(_) => IpcError::internal(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct AppStatus {
    pub ipc_version: u32,
    pub schema_version: u32,
    pub lock_state: String,
    pub device_id: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Health {
    pub database_ok: bool,
    pub schema_version: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsGetReq {
    pub key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsSetReq {
    pub key: String,
    pub value: serde_json::Value,
    pub expected_revision: u64,
}

pub fn status_impl(state: &AppState) -> Result<AppStatus, IpcError> {
    let conn = state.db.lock().map_err(|_| IpcError::internal())?;
    let schema_version = db::schema_version(&conn).map_err(|_| IpcError::internal())?;
    Ok(AppStatus {
        ipc_version: IPC_VERSION,
        schema_version,
        // No vault yet (security phase); nothing can lock the app.
        lock_state: "unlocked".into(),
        device_id: state.device_id.clone(),
    })
}

pub fn health_impl(state: &AppState) -> Result<Health, IpcError> {
    let conn = state.db.lock().map_err(|_| IpcError::internal())?;
    let database_ok = conn
        .query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
        .map(|s| s == "ok")
        .unwrap_or(false);
    let schema_version = db::schema_version(&conn).map_err(|_| IpcError::internal())?;
    Ok(Health {
        database_ok,
        schema_version,
    })
}

pub fn settings_get_impl(
    state: &AppState,
    req: SettingsGetReq,
) -> Result<Option<settings::Setting>, IpcError> {
    let conn = state.db.lock().map_err(|_| IpcError::internal())?;
    Ok(settings::get(&conn, &req.key)?)
}

pub fn settings_set_impl(
    state: &AppState,
    req: SettingsSetReq,
    now: &str,
) -> Result<settings::Setting, IpcError> {
    let conn = state.db.lock().map_err(|_| IpcError::internal())?;
    Ok(settings::set(
        &conn,
        &req.key,
        req.value,
        req.expected_revision,
        now,
    )?)
}

#[tauri::command]
pub fn app_status(state: tauri::State<'_, AppState>) -> Result<AppStatus, IpcError> {
    status_impl(&state)
}

#[tauri::command]
pub fn health(state: tauri::State<'_, AppState>) -> Result<Health, IpcError> {
    health_impl(&state)
}

#[tauri::command]
pub fn settings_get(
    state: tauri::State<'_, AppState>,
    req: SettingsGetReq,
) -> Result<Option<settings::Setting>, IpcError> {
    settings_get_impl(&state, req)
}

#[tauri::command]
pub fn settings_set(
    state: tauri::State<'_, AppState>,
    req: SettingsSetReq,
) -> Result<settings::Setting, IpcError> {
    settings_set_impl(&state, req, &crate::time::now_utc_rfc3339())
}

impl From<rusqlite::Error> for IpcError {
    fn from(_: rusqlite::Error) -> Self {
        IpcError::internal()
    }
}

/// Runs one planner/proposal operation on the locked connection with the current clock.
pub fn run<T>(
    state: &AppState,
    now: &str,
    f: impl FnOnce(&mut Connection, &planner::Ctx) -> planner::Res<T>,
) -> Result<T, IpcError> {
    let mut conn = state.db.lock().map_err(|_| IpcError::internal())?;
    let ctx = planner::Ctx {
        device_id: &state.device_id,
        now,
    };
    f(&mut conn, &ctx)
}

#[tauri::command]
pub fn task_list(
    state: tauri::State<'_, AppState>,
    req: planner::TaskListReq,
) -> Result<Vec<planner::Task>, IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, _ctx| {
        planner::task_list(conn, req)
    })
}

#[tauri::command]
pub fn task_get(
    state: tauri::State<'_, AppState>,
    req: planner::IdReq,
) -> Result<planner::Task, IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, _ctx| {
        planner::task_get(conn, req)
    })
}

#[tauri::command]
pub fn task_create(
    state: tauri::State<'_, AppState>,
    req: planner::TaskCreateReq,
) -> Result<planner::Task, IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, ctx| {
        planner::task_create(conn, ctx, req)
    })
}

#[tauri::command]
pub fn task_update(
    state: tauri::State<'_, AppState>,
    req: planner::TaskUpdateReq,
) -> Result<planner::Task, IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, ctx| {
        planner::task_update(conn, ctx, req)
    })
}

#[tauri::command]
pub fn task_complete(
    state: tauri::State<'_, AppState>,
    req: planner::RevisionReq,
) -> Result<planner::Task, IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, ctx| {
        planner::task_complete(conn, ctx, req)
    })
}

#[tauri::command]
pub fn task_plan(
    state: tauri::State<'_, AppState>,
    req: planner::PlanReq,
) -> Result<planner::Task, IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, ctx| {
        planner::task_plan(conn, ctx, req)
    })
}

#[tauri::command]
pub fn task_unplan(
    state: tauri::State<'_, AppState>,
    req: planner::RevisionReq,
) -> Result<planner::Task, IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, ctx| {
        planner::task_unplan(conn, ctx, req)
    })
}

#[tauri::command]
pub fn task_reopen(
    state: tauri::State<'_, AppState>,
    req: planner::RevisionReq,
) -> Result<planner::Task, IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, ctx| {
        planner::task_reopen(conn, ctx, req)
    })
}

#[tauri::command]
pub fn task_undo(
    state: tauri::State<'_, AppState>,
    req: planner::RevisionReq,
) -> Result<planner::Task, IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, ctx| {
        planner::task_undo(conn, ctx, req)
    })
}

#[tauri::command]
pub fn task_delete(
    state: tauri::State<'_, AppState>,
    req: planner::DeleteReq,
) -> Result<(), IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, ctx| {
        planner::task_delete(conn, ctx, req)
    })
}

#[tauri::command]
pub fn occurrence_edit(
    state: tauri::State<'_, AppState>,
    req: planner::OccurrenceEditReq,
) -> Result<planner::Task, IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, ctx| {
        planner::occurrence_edit(conn, ctx, req)
    })
}

#[tauri::command]
pub fn agenda_list(
    state: tauri::State<'_, AppState>,
    req: planner::RangeReq,
) -> Result<Vec<planner::AgendaItem>, IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, _ctx| {
        planner::agenda_list(conn, req)
    })
}

#[tauri::command]
pub fn event_list(
    state: tauri::State<'_, AppState>,
    req: planner::RangeReq,
) -> Result<Vec<planner::Event>, IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, _ctx| {
        planner::event_list(conn, req)
    })
}

#[tauri::command]
pub fn event_create(
    state: tauri::State<'_, AppState>,
    req: planner::EventCreateReq,
) -> Result<planner::Event, IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, ctx| {
        planner::event_create(conn, ctx, req)
    })
}

#[tauri::command]
pub fn event_update(
    state: tauri::State<'_, AppState>,
    req: planner::EventUpdateReq,
) -> Result<planner::Event, IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, ctx| {
        planner::event_update(conn, ctx, req)
    })
}

#[tauri::command]
pub fn event_delete(
    state: tauri::State<'_, AppState>,
    req: planner::DeleteReq,
) -> Result<(), IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, ctx| {
        planner::event_delete(conn, ctx, req)
    })
}

#[tauri::command]
pub fn proposal_list(
    state: tauri::State<'_, AppState>,
    req: proposals::ProposalListReq,
) -> Result<Vec<proposals::Proposal>, IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, ctx| {
        proposals::list(conn, ctx, req)
    })
}

#[tauri::command]
pub fn proposal_approve(
    state: tauri::State<'_, AppState>,
    req: proposals::ProposalApproveReq,
) -> Result<proposals::Proposal, IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, ctx| {
        proposals::approve(conn, ctx, req)
    })
}

#[tauri::command]
pub fn proposal_reject(
    state: tauri::State<'_, AppState>,
    req: proposals::ProposalIdReq,
) -> Result<proposals::Proposal, IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, ctx| {
        proposals::reject(conn, ctx, req)
    })
}

#[tauri::command]
pub fn planner_suggest_reschedules(
    state: tauri::State<'_, AppState>,
    req: proposals::SuggestReq,
) -> Result<Vec<proposals::Proposal>, IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, ctx| {
        proposals::suggest_reschedules(conn, ctx, req)
    })
}

#[tauri::command]
pub fn schedule_change_apply(
    state: tauri::State<'_, AppState>,
    req: crate::scheduled::ApplyReq,
) -> Result<crate::scheduled::Applied, IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, ctx| {
        crate::scheduled::apply(conn, ctx, req)
    })
}
#[tauri::command]
pub fn schedule_change_undo(
    state: tauri::State<'_, AppState>,
    req: crate::scheduled::UndoReq,
) -> Result<crate::scheduled::Applied, IpcError> {
    run(&state, &crate::time::now_utc_rfc3339(), |conn, ctx| {
        crate::scheduled::undo(conn, ctx, req)
    })
}

#[tauri::command]
pub async fn calendar_day_labels(
    state: tauri::State<'_, AppState>,
    req: crate::calendar_labels::MonthReq,
) -> Result<crate::calendar_labels::Labels, IpcError> {
    let plan = {
        let conn = state.db.lock().map_err(|_| IpcError::internal())?;
        crate::calendar_labels::prepare(&conn, &req.month)?
    };
    let prep = match plan {
        crate::calendar_labels::Plan::Ready(r) => return Ok(r),
        crate::calendar_labels::Plan::Run(p) => p,
    };
    let Some(_slot) = crate::inbox::try_begin_inference() else {
        return Ok(crate::calendar_labels::Labels {
            state: "busy".into(),
            labels: prep.cached_labels.clone(),
        });
    };
    let model = prep.model.clone();
    let messages = prep.messages.clone();
    let infer = {
        let conn = state.db.lock().map_err(|_| IpcError::internal())?;
        crate::ai::infer_with_timeout(&conn, &model, None, 30)
    };
    let raw = tauri::async_runtime::spawn_blocking(move || {
        infer?.infer(&messages, crate::inbox::cancel_flag())
    })
    .await;
    let conn = state.db.lock().map_err(|_| IpcError::internal())?;
    match raw {
        Ok(Ok(raw)) => match crate::calendar_labels::finish(&conn, &prep, &raw) {
            Ok(labels) => Ok(labels),
            Err(_) => crate::calendar_labels::unavailable(&conn, &prep),
        },
        _ => crate::calendar_labels::unavailable(&conn, &prep),
    }
}
