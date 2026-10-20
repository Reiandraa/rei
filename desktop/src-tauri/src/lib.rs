pub mod ai;
pub mod assistant;
pub mod assistant_workflow;
pub mod auto_academic;
pub mod calendar_labels;
pub mod commands;
pub mod connections;
pub mod connectors;
pub mod crypto;
pub mod dates;
pub mod db;
pub mod files;
pub mod guard;
pub mod hiz;
pub mod inbox;
pub mod life;
pub mod local_setup;
pub mod memory;
pub mod notifications;
pub mod oauth;
pub mod ollama;
pub mod planner;
pub mod projects;
pub mod proposals;
pub mod providers;
pub mod records;
pub mod recurrence;
pub mod schedule_edit;
pub mod scheduled;
pub mod settings;
pub mod study;
pub mod sync;
pub mod teams;
pub mod time;
pub mod tools;
pub mod vault;

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let conn = db::open(&dir.join("rei.sqlite"))?;
            let device_id = device_id(&conn)?;
            app.manage(commands::AppState {
                db: std::sync::Mutex::new(conn),
                device_id,
            });
            // Opt-in polling thread: idle unless the owner enables it in Connections.
            connections::start_scheduler(app.handle().clone());
            // Opt-in automatic local analysis worker: idle unless enabled in Today.
            auto_academic::start_worker(app.handle().clone());
            assistant::start_worker(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_status,
            ai::ai_settings,
            local_setup::local_ai_setup,
            assistant::assistant_summary_get,
            assistant::assistant_summary_refresh,
            assistant::assistant_memory,
            commands::health,
            commands::settings_get,
            commands::settings_set,
            commands::task_list,
            commands::task_get,
            commands::task_create,
            commands::task_update,
            commands::task_complete,
            commands::task_delete,
            commands::occurrence_edit,
            commands::schedule_change_apply,
            commands::schedule_change_undo,
            commands::agenda_list,
            commands::calendar_day_labels,
            commands::event_list,
            commands::event_create,
            commands::event_update,
            commands::event_delete,
            commands::proposal_list,
            commands::proposal_approve,
            commands::proposal_reject,
            commands::planner_suggest_reschedules,
            inbox::inbox_overview,
            inbox::source_list,
            inbox::source_get,
            inbox::source_import_manual,
            inbox::model_status,
            inbox::source_analyze,
            inbox::source_analyze_cancel,
            inbox::candidate_approve,
            inbox::candidate_reject,
            assistant_workflow::assistant_workflow_set,
            connections::connections_status,
            connections::hiz_configure,
            connections::hiz_sign_in,
            connections::teams_configure,
            connections::teams_sign_in,
            connections::teams_discover,
            connections::connection_select_sources,
            connections::connection_fetch_now,
            connections::connection_fetch_cancel,
            connections::connection_disconnect,
            connections::polling_set,
            auto_academic::auto_settings_get,
            auto_academic::auto_settings_set,
            auto_academic::auto_summary,
            auto_academic::auto_queue_cancel,
            auto_academic::auto_queue_retry,
            auto_academic::candidate_batch_approve,
            commands::task_plan,
            commands::task_unplan,
            commands::task_reopen,
            commands::task_undo,
            inbox::model_list
        ])
        .run(tauri::generate_context!())
        .expect("error while running REI OS");
}

/// Stable per-install device ID, generated once in Rust and stored in `app_meta`.
pub fn device_id(conn: &rusqlite::Connection) -> rusqlite::Result<String> {
    if let Ok(id) = conn.query_row(
        "SELECT value FROM app_meta WHERE key = 'device_id'",
        [],
        |r| r.get(0),
    ) {
        return Ok(id);
    }
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO app_meta(key, value) VALUES ('device_id', ?1)",
        [&id],
    )?;
    Ok(id)
}
