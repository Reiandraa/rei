//! Settings persistence with revision checks. Secrets never belong here.
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

pub const ALLOWED_KEYS: &[&str] = &["locale", "theme", "accent", "text_size", "motion"];

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Setting {
    pub key: String,
    pub value: serde_json::Value,
    pub revision: u64,
}

#[derive(Debug, PartialEq)]
pub enum SettingsError {
    Validation(String),
    Conflict { current_revision: u64 },
    Internal(String),
}

impl From<rusqlite::Error> for SettingsError {
    fn from(e: rusqlite::Error) -> Self {
        SettingsError::Internal(e.to_string())
    }
}

fn validate(key: &str, value: &serde_json::Value) -> Result<(), SettingsError> {
    let ok = match key {
        "locale" => matches!(value.as_str(), Some("id" | "en" | "de")),
        "theme" => matches!(value.as_str(), Some("light" | "dark")),
        "accent" => matches!(
            value.as_str(),
            Some("green" | "ocean" | "lavender" | "rose" | "sunrise" | "peach" | "teal" | "sand")
        ),
        "text_size" => matches!(value.as_str(), Some("small" | "standard" | "large")),
        "motion" => matches!(value.as_str(), Some("system" | "reduced")),
        _ => {
            return Err(SettingsError::Validation(format!(
                "unknown setting key: {key}"
            )))
        }
    };
    if ok {
        Ok(())
    } else {
        Err(SettingsError::Validation(format!(
            "invalid value for {key}"
        )))
    }
}

pub fn get(conn: &Connection, key: &str) -> Result<Option<Setting>, SettingsError> {
    if !ALLOWED_KEYS.contains(&key) {
        return Err(SettingsError::Validation(format!(
            "unknown setting key: {key}"
        )));
    }
    let row = conn
        .query_row(
            "SELECT value_json, revision FROM settings WHERE key = ?1",
            [key],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)),
        )
        .optional()?;
    match row {
        None => Ok(None),
        Some((json, revision)) => Ok(Some(Setting {
            key: key.to_string(),
            value: serde_json::from_str(&json)
                .map_err(|e| SettingsError::Internal(e.to_string()))?,
            revision,
        })),
    }
}

/// `expected_revision` is 0 when the key has never been set.
pub fn set(
    conn: &Connection,
    key: &str,
    value: serde_json::Value,
    expected_revision: u64,
    now: &str,
) -> Result<Setting, SettingsError> {
    validate(key, &value)?;
    let current = get(conn, key)?.map(|s| s.revision).unwrap_or(0);
    if current != expected_revision {
        return Err(SettingsError::Conflict {
            current_revision: current,
        });
    }
    let revision = current + 1;
    conn.execute(
        "INSERT INTO settings(key, value_json, revision, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(key) DO UPDATE SET value_json = ?2, revision = ?3, updated_at = ?4",
        params![key, value.to_string(), revision, now],
    )?;
    Ok(Setting {
        key: key.to_string(),
        value,
        revision,
    })
}
