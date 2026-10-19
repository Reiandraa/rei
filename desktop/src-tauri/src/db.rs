//! SQLite access and versioned migrations (docs/INTERFACES.md §3).
use rusqlite::Connection;
use std::path::Path;

pub const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_core", include_str!("../migrations/0001_core.sql")),
    ("0002_memory", include_str!("../migrations/0002_memory.sql")),
    (
        "0003_connectors",
        include_str!("../migrations/0003_connectors.sql"),
    ),
    ("0004_sync", include_str!("../migrations/0004_sync.sql")),
    (
        "0005_study_life",
        include_str!("../migrations/0005_study_life.sql"),
    ),
    (
        "0006_notifications",
        include_str!("../migrations/0006_notifications.sql"),
    ),
    (
        "0007_academic_inbox",
        include_str!("../migrations/0007_academic_inbox.sql"),
    ),
    (
        "0008_provider_connectors",
        include_str!("../migrations/0008_provider_connectors.sql"),
    ),
    (
        "0009_auto_academic",
        include_str!("../migrations/0009_auto_academic.sql"),
    ),
    (
        "0010_ui_planner_dates",
        include_str!("../migrations/0010_ui_planner_dates.sql"),
    ),
    (
        "0011_assistant_workflow",
        include_str!("../migrations/0011_assistant_workflow.sql"),
    ),
    (
        "0012_weekday_times",
        include_str!("../migrations/0012_weekday_times.sql"),
    ),
    (
        "0013_schedule_adjustments",
        include_str!("../migrations/0013_schedule_adjustments.sql"),
    ),
    (
        "0014_routine_history",
        include_str!("../migrations/0014_routine_history.sql"),
    ),
    (
        "0015_calendar_label_cache",
        include_str!("../migrations/0015_calendar_label_cache.sql"),
    ),
    (
        "0016_assistant_digest",
        include_str!("../migrations/0016_assistant_digest.sql"),
    ),
];

#[derive(Debug)]
pub enum DbError {
    Sqlite(rusqlite::Error),
    /// Database was written by a newer app version; downgrade is rejected.
    Downgrade {
        found: u32,
        supported: u32,
    },
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbError::Sqlite(e) => write!(f, "database error: {e}"),
            DbError::Downgrade { found, supported } => write!(
                f,
                "database schema {found} is newer than supported {supported}; downgrade rejected"
            ),
        }
    }
}

impl std::error::Error for DbError {}

impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        DbError::Sqlite(e)
    }
}

pub fn supported_version() -> u32 {
    MIGRATIONS.len() as u32
}

pub fn open(path: &Path) -> Result<Connection, DbError> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    configure(&conn)?;
    migrate(&mut conn)?;
    Ok(conn)
}

pub fn open_in_memory() -> Result<Connection, DbError> {
    let mut conn = Connection::open_in_memory()?;
    configure(&conn)?;
    migrate(&mut conn)?;
    Ok(conn)
}

fn configure(conn: &Connection) -> Result<(), DbError> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_millis(5000))?;
    Ok(())
}

pub fn schema_version(conn: &Connection) -> Result<u32, DbError> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

/// Applies pending migrations, each in its own transaction together with the
/// `user_version` bump, so a failed migration leaves the previous version intact.
pub fn migrate(conn: &mut Connection) -> Result<u32, DbError> {
    let current = schema_version(conn)?;
    let supported = supported_version();
    if current > supported {
        return Err(DbError::Downgrade {
            found: current,
            supported,
        });
    }
    for (i, (_name, sql)) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as u32)?;
        tx.commit()?;
    }
    schema_version(conn)
}
