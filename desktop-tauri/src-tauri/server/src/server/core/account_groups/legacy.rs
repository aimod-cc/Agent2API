//! One-time cleanup for Keys bound to account groups discarded by schema v17.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

const MARKER: &str = "accountGroupsLegacyCleared";
const API_KEYS: &str = "apiKeys";

/// Disable legacy account-group Keys once and record completion atomically.
///
/// Returns `true` only when this call changed at least one Key. A missing
/// `apiKeys` row still records the marker: there is nothing to disable, and the
/// legacy single `apiKey` field is intentionally left for the existing reader.
pub fn disable_legacy_group_keys_once(conn: &Connection) -> rusqlite::Result<bool> {
    let tx = conn.unchecked_transaction()?;
    let completed = tx
        .query_row("SELECT 1 FROM kv WHERE key = ?1", [MARKER], |row| {
            row.get::<_, i64>(0)
        })
        .optional()?
        .is_some();
    if completed {
        return Ok(false);
    }

    let api_keys = tx
        .query_row("SELECT value FROM kv WHERE key = ?1", [API_KEYS], |row| {
            row.get::<_, Option<String>>(0)
        })
        .optional()?
        .flatten();
    let changed = match api_keys {
        Some(text) => {
            let mut entries: Value = serde_json::from_str(&text).map_err(invalid_json)?;
            let changed = disable_group_entries(&mut entries);
            if changed {
                tx.execute(
                    "UPDATE kv SET value = ?1 WHERE key = ?2",
                    params![entries.to_string(), API_KEYS],
                )?;
            }
            changed
        }
        None => false,
    };
    tx.execute("INSERT INTO kv (key, value) VALUES (?1, 'true')", [MARKER])?;
    tx.commit()?;
    Ok(changed)
}

fn disable_group_entries(entries: &mut Value) -> bool {
    let Some(items) = entries.as_array_mut() else {
        return false;
    };
    let mut changed = false;
    for item in items {
        let Some(entry) = item.as_object_mut() else {
            continue;
        };
        if entry.get("routeMode").and_then(Value::as_str) != Some("account_group")
            || matches!(entry.get("enabled"), Some(Value::Bool(false)))
        {
            continue;
        }
        entry.insert("enabled".to_string(), Value::Bool(false));
        changed = true;
    }
    changed
}

fn invalid_json(error: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}
