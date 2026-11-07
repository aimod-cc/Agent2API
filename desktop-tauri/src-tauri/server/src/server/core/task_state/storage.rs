use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::{json, Value};

use super::{database, TaskState};
use crate::server::logging;

const KV_KEY: &str = "backgroundTaskState";
// 本机诊断快照不参与配置或云同步；只保留最近一次，避免无限增长。
const RECOVERY_KEY: &str = "backgroundTaskStateRecovery";

type States = HashMap<String, TaskState>;

#[derive(Default)]
struct StoredStates {
    states: States,
    damaged: Option<Damage>,
    migrated: bool,
}

struct Damage {
    raw: String,
    error: String,
}

fn read_states(conn: &Connection) -> Result<StoredStates, String> {
    let text: Option<String> = conn
        .query_row("SELECT value FROM kv WHERE key = ?1", [KV_KEY], |row| row.get(0))
        .optional()
        .map_err(|error| format!("读取任务状态失败: {error}"))?;
    let Some(text) = text else { return Ok(StoredStates::default()) };
    let mut stored = match serde_json::from_str::<States>(&text) {
        Ok(states) => StoredStates { states, ..StoredStates::default() },
        Err(error) => {
            let states = match serde_json::from_str::<serde_json::Map<String, Value>>(&text) {
                Ok(entries) => entries
                    .into_iter()
                    .filter_map(|(key, value)| {
                        serde_json::from_value::<TaskState>(value).ok().map(|state| (key, state))
                    })
                    .collect(),
                Err(_) => States::new(),
            };
            StoredStates { states, damaged: Some(Damage { raw: text, error: error.to_string() }), migrated: false }
        }
    };
    for (key, state) in &mut stored.states {
        if source_managed(key) {
            stored.migrated |= state.use_source_backoff();
        }
    }
    Ok(stored)
}

fn source_managed(key: &str) -> bool {
    ["updateCheck:", "pluginCatalog:", "pluginRequirement:", "pluginGithub:"]
        .iter().any(|prefix| key.starts_with(prefix))
}

fn write_states(conn: &Connection, states: &States) -> Result<(), String> {
    let text = serde_json::to_string(states)
        .map_err(|error| format!("编码任务状态失败: {error}"))?;
    conn.execute(
        "INSERT INTO kv (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![KV_KEY, text],
    )
    .map_err(|error| format!("保存任务状态失败: {error}"))?;
    Ok(())
}

fn preserve_damage(tx: &Transaction<'_>, stored: &StoredStates) -> Result<Option<String>, String> {
    let Some(damage) = &stored.damaged else { return Ok(None) };
    let snapshot = serde_json::to_string(&json!({
        "recoveredAt": logging::now_ms(),
        "error": damage.error,
        "raw": damage.raw,
        "retainedTasks": stored.states.len(),
    }))
    .map_err(|error| format!("编码损坏任务状态备份失败: {error}"))?;
    tx.execute(
        "INSERT INTO kv (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![RECOVERY_KEY, snapshot],
    )
    .map_err(|error| format!("备份损坏任务状态失败，未重置原数据: {error}"))?;
    Ok(Some(format!(
        "后台任务状态格式异常，已自动恢复，保留 {} 条有效任务；原文已备份至 kv.{RECOVERY_KEY}",
        stored.states.len(),
    )))
}

fn log_recovery(message: Option<String>) {
    if let Some(message) = message {
        logging::log_with_level("[Tasks]", &message, "warn");
    }
}

pub(super) fn read_all() -> Result<States, String> {
    let db = database()?;
    let stored = db
        .with(read_states)
        .ok_or_else(|| "任务状态数据库不可用".to_string())??;
    if stored.damaged.is_none() && !stored.migrated {
        return Ok(stored.states);
    }
    let (states, recovered) = db
        .with_mut(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| format!("锁定任务状态失败: {error}"))?;
            // 读锁交还后其他实例可能已修复或更新，必须在写事务内重新读取。
            let stored = read_states(&tx)?;
            let recovered = preserve_damage(&tx, &stored)?;
            if recovered.is_some() || stored.migrated {
                write_states(&tx, &stored.states)?;
            }
            tx.commit().map_err(|error| format!("提交任务状态失败: {error}"))?;
            Ok::<_, String>((stored.states, recovered))
        })
        .ok_or_else(|| "任务状态数据库不可用".to_string())??;
    log_recovery(recovered);
    Ok(states)
}

pub(super) fn change(
    key: &str,
    mutate: impl FnOnce(&mut TaskState),
) -> Result<TaskState, String> {
    let (result, recovered) = database()?
        .with_mut(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| format!("锁定任务状态失败: {error}"))?;
            let mut stored = read_states(&tx)?;
            let recovered = preserve_damage(&tx, &stored)?;
            let state = stored.states.entry(key.to_string()).or_default();
            if source_managed(key) {
                state.use_source_backoff();
            }
            mutate(state);
            let result = state.clone();
            write_states(&tx, &stored.states)?;
            tx.commit().map_err(|error| format!("提交任务状态失败: {error}"))?;
            Ok::<_, String>((result, recovered))
        })
        .ok_or_else(|| "任务状态数据库不可用".to_string())??;
    log_recovery(recovered);
    Ok(result)
}
