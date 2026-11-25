//! 定时任务插件、任务实例与运行记录的 SQLite 持久化与加密读写。

use std::collections::BTreeMap;
use std::sync::OnceLock;

use rusqlite::{params, OptionalExtension};
use serde_json::Value;

use super::cron_schedule::{next_run_ms, validate_cron, validate_timezone};
use super::types::{
    validate_and_normalize_plugin, TaskActionDef, TaskFieldDef, TaskJobRun, TaskJobView,
    TaskPlugin, TaskStatusFieldDef,
};
use crate::server::core::crypto::{self, SecretScope};
use crate::server::db::Db;
use crate::server::logging;

const MAX_RUNS_PER_JOB: i64 = 50;
const MAX_OUTPUT_CHARS: usize = 100_000;

static DB: OnceLock<Option<Db>> = OnceLock::new();

pub fn install(db: Option<Db>) {
    let _ = DB.set(db);
}

fn database() -> Result<&'static Db, String> {
    DB.get()
        .and_then(Option::as_ref)
        .ok_or_else(|| "定时任务数据库尚未初始化".to_string())
}

fn encrypt_map<T: serde::Serialize>(map: &T) -> Result<String, String> {
    let json = serde_json::to_vec(map).map_err(|error| format!("编码加密数据失败：{error}"))?;
    if json == b"{}" {
        return Ok(String::new());
    }
    crypto::seal(&json, SecretScope::TaskPlugins)
}

fn decrypt_map<T: serde::de::DeserializeOwned + Default>(raw: &str) -> Result<T, String> {
    if raw.trim().is_empty() {
        return Ok(T::default());
    }
    let plain = crypto::open(raw, SecretScope::TaskPlugins)?;
    serde_json::from_slice(&plain).map_err(|error| format!("解析加密数据失败：{error}"))
}

fn plugin_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskPlugin> {
    let domains_json: String = row.get(11)?;
    let fields_json: String = row.get(14)?;
    let actions_json: String = row.get(15)?;
    let status_json: String = row.get(16)?;
    Ok(TaskPlugin {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        version: row.get(3)?,
        source: row.get(4)?,
        signed: row.get::<_, i64>(5)? != 0,
        enabled: row.get::<_, i64>(6)? != 0,
        icon: row.get(7)?,
        default_cron: row.get(8)?,
        default_timezone: row.get(9)?,
        default_timeout: row.get(10)?,
        domains: serde_json::from_str(&domains_json).unwrap_or_default(),
        declares_notify: row.get::<_, i64>(12)? != 0,
        notify_allowed: row.get::<_, i64>(13)? != 0,
        fields: serde_json::from_str::<Vec<TaskFieldDef>>(&fields_json).unwrap_or_default(),
        actions: serde_json::from_str::<Vec<TaskActionDef>>(&actions_json).unwrap_or_default(),
        status_fields: serde_json::from_str::<Vec<TaskStatusFieldDef>>(&status_json)
            .unwrap_or_default(),
        script_body: row.get(17)?,
        created_at: row.get(18)?,
        updated_at: row.get(19)?,
    })
}

pub fn list_plugins() -> Result<Vec<TaskPlugin>, String> {
    let db = database()?;
    db.with(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, name, description, version, source, signed, enabled, icon,
                        default_cron, default_timezone, default_timeout, domains_json,
                        declares_notify, notify_allowed, fields_json, actions_json,
                        status_fields_json, script_body, created_at, updated_at
                 FROM task_plugins
                 ORDER BY updated_at DESC, id ASC",
            )
            .map_err(|error| format!("读取定时任务插件失败：{error}"))?;
        let rows = stmt
            .query_map([], plugin_from_row)
            .map_err(|error| format!("读取定时任务插件失败：{error}"))?;
        let mut out = Vec::new();
        for item in rows {
            out.push(item.map_err(|error| format!("解析定时任务插件失败：{error}"))?);
        }
        Ok(out)
    })
    .ok_or_else(|| "定时任务数据库不可用".to_string())?
}

pub fn get_plugin(id: &str) -> Result<Option<TaskPlugin>, String> {
    let db = database()?;
    db.with(|conn| {
        conn.query_row(
            "SELECT id, name, description, version, source, signed, enabled, icon,
                    default_cron, default_timezone, default_timeout, domains_json,
                    declares_notify, notify_allowed, fields_json, actions_json,
                    status_fields_json, script_body, created_at, updated_at
             FROM task_plugins WHERE id = ?1",
            [id],
            plugin_from_row,
        )
        .optional()
        .map_err(|error| format!("查询定时任务插件失败：{error}"))
    })
    .ok_or_else(|| "定时任务数据库不可用".to_string())?
}

pub fn upsert_plugin(plugin: TaskPlugin, preserve_permissions: bool) -> Result<TaskPlugin, String> {
    let mut plugin = validate_and_normalize_plugin(plugin)?;
    let now = logging::now_ms();
    let existing = get_plugin(&plugin.id)?;
    if let Some(prev) = existing {
        plugin.created_at = prev.created_at;
        if preserve_permissions {
            plugin.enabled = prev.enabled;
            plugin.notify_allowed = prev.notify_allowed;
        }
    } else {
        plugin.created_at = now;
    }
    plugin.updated_at = now;

    let domains_json = serde_json::to_string(&plugin.domains).unwrap_or_else(|_| "[]".into());
    let fields_json = serde_json::to_string(&plugin.fields).unwrap_or_else(|_| "[]".into());
    let actions_json = serde_json::to_string(&plugin.actions).unwrap_or_else(|_| "[]".into());
    let status_json = serde_json::to_string(&plugin.status_fields).unwrap_or_else(|_| "[]".into());

    let db = database()?;
    db.with(|conn| {
        conn.execute(
            "INSERT INTO task_plugins (
                id, name, description, version, source, signed, enabled, icon,
                default_cron, default_timezone, default_timeout, domains_json,
                declares_notify, notify_allowed, fields_json, actions_json,
                status_fields_json, script_body, created_at, updated_at
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)
             ON CONFLICT(id) DO UPDATE SET
                name=excluded.name, description=excluded.description, version=excluded.version,
                source=excluded.source, signed=excluded.signed, enabled=excluded.enabled,
                icon=excluded.icon, default_cron=excluded.default_cron,
                default_timezone=excluded.default_timezone, default_timeout=excluded.default_timeout,
                domains_json=excluded.domains_json, declares_notify=excluded.declares_notify,
                notify_allowed=excluded.notify_allowed, fields_json=excluded.fields_json,
                actions_json=excluded.actions_json, status_fields_json=excluded.status_fields_json,
                script_body=excluded.script_body, updated_at=excluded.updated_at",
            params![
                plugin.id,
                plugin.name,
                plugin.description,
                plugin.version,
                plugin.source,
                i64::from(plugin.signed),
                i64::from(plugin.enabled),
                plugin.icon,
                plugin.default_cron,
                plugin.default_timezone,
                plugin.default_timeout,
                domains_json,
                i64::from(plugin.declares_notify),
                i64::from(plugin.notify_allowed),
                fields_json,
                actions_json,
                status_json,
                plugin.script_body,
                plugin.created_at,
                plugin.updated_at,
            ],
        )
        .map_err(|error| format!("保存定时任务插件失败：{error}"))
    })
    .ok_or_else(|| "定时任务数据库不可用".to_string())??;

    Ok(plugin)
}

pub fn patch_plugin(
    id: &str,
    enabled: Option<bool>,
    notify_allowed: Option<bool>,
) -> Result<TaskPlugin, String> {
    let mut plugin = get_plugin(id)?.ok_or_else(|| "定时任务插件不存在".to_string())?;
    if let Some(value) = enabled {
        plugin.enabled = value;
    }
    if let Some(value) = notify_allowed {
        plugin.notify_allowed = value;
    }
    upsert_plugin(plugin, false)
}

pub fn delete_plugin(id: &str) -> Result<(), String> {
    let db = database()?;
    let affected = db
        .with(|conn| {
            conn.execute("DELETE FROM task_plugins WHERE id = ?1", [id])
                .map_err(|error| format!("卸载定时任务插件失败：{error}"))
        })
        .ok_or_else(|| "定时任务数据库不可用".to_string())??;
    if affected == 0 {
        return Err("定时任务插件不存在".to_string());
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct JobRecord {
    pub view: TaskJobView,
    pub secrets_enc: String,
    pub storage_enc: String,
}

fn job_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobRecord> {
    let values_json: String = row.get(8)?;
    let secrets_enc: String = row.get(9)?;
    let storage_enc: String = row.get(10)?;
    let status_json: String = row.get(11)?;
    let saved_secrets: BTreeMap<String, bool> = decrypt_map::<BTreeMap<String, String>>(&secrets_enc)
        .map(|map| {
            map.into_iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, _)| (k, true))
                .collect()
        })
        .unwrap_or_default();
    Ok(JobRecord {
        view: TaskJobView {
            id: row.get(0)?,
            plugin_id: row.get(1)?,
            name: row.get(2)?,
            cron_expression: row.get(3)?,
            timezone: row.get(4)?,
            timeout_seconds: row.get(5)?,
            proxy_id: row.get(6)?,
            enabled: row.get::<_, i64>(7)? != 0,
            values: serde_json::from_str(&values_json).unwrap_or_default(),
            saved_secrets,
            status_summary: serde_json::from_str(&status_json).unwrap_or_default(),
            next_run_at: row.get(12)?,
            last_run_at: row.get(13)?,
            last_status: row.get(14)?,
            last_duration_ms: row.get(15)?,
            last_error: row.get(16)?,
            created_at: row.get(17)?,
            updated_at: row.get(18)?,
        },
        secrets_enc,
        storage_enc,
    })
}

pub fn list_jobs() -> Result<Vec<TaskJobView>, String> {
    let db = database()?;
    db.with(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, plugin_id, name, cron_expression, timezone, timeout_seconds,
                        proxy_id, enabled, values_json, secrets_enc, storage_enc,
                        status_summary_json, next_run_at, last_run_at, last_status,
                        last_duration_ms, last_error, created_at, updated_at
                 FROM task_jobs ORDER BY created_at DESC, id DESC",
            )
            .map_err(|error| format!("读取定时任务列表失败：{error}"))?;
        let rows = stmt
            .query_map([], job_from_row)
            .map_err(|error| format!("读取定时任务列表失败：{error}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|error| format!("解析定时任务失败：{error}"))?.view);
        }
        Ok(out)
    })
    .ok_or_else(|| "定时任务数据库不可用".to_string())?
}

pub fn get_job_record(id: &str) -> Result<Option<JobRecord>, String> {
    let db = database()?;
    db.with(|conn| {
        conn.query_row(
            "SELECT id, plugin_id, name, cron_expression, timezone, timeout_seconds,
                    proxy_id, enabled, values_json, secrets_enc, storage_enc,
                    status_summary_json, next_run_at, last_run_at, last_status,
                    last_duration_ms, last_error, created_at, updated_at
             FROM task_jobs WHERE id = ?1",
            [id],
            job_from_row,
        )
        .optional()
        .map_err(|error| format!("查询定时任务失败：{error}"))
    })
    .ok_or_else(|| "定时任务数据库不可用".to_string())?
}

pub fn decrypt_job_secrets(record: &JobRecord) -> Result<BTreeMap<String, String>, String> {
    decrypt_map(&record.secrets_enc)
}

pub fn decrypt_job_storage(record: &JobRecord) -> Result<BTreeMap<String, String>, String> {
    decrypt_map(&record.storage_enc)
}

pub struct JobUpsertInput {
    pub id: Option<String>,
    pub plugin_id: String,
    pub name: String,
    pub cron_expression: String,
    pub timezone: String,
    pub timeout_seconds: i64,
    pub proxy_id: String,
    pub enabled: bool,
    pub values: BTreeMap<String, Value>,
    pub clear_secrets: Vec<String>,
}

pub fn save_job(input: JobUpsertInput) -> Result<TaskJobView, String> {
    let plugin = get_plugin(&input.plugin_id)?
        .ok_or_else(|| format!("定时任务插件「{}」不存在或未安装", input.plugin_id))?;
    let name: String = input.name.trim().chars().take(100).collect();
    if name.is_empty() {
        return Err("任务名称不能为空".to_string());
    }
    let cron = validate_cron(&input.cron_expression)?;
    let timezone = validate_timezone(&input.timezone)?;
    if !(1..=300).contains(&input.timeout_seconds) {
        return Err("超时时间必须在 1–300 秒之间".to_string());
    }
    let proxy_id = input.proxy_id.trim().to_string();
    if !proxy_id.is_empty() && proxy_id != "direct" {
        crate::server::core::proxy_pool::resolve_reference(&proxy_id)?;
    }
    let proxy_id = if proxy_id == "direct" { String::new() } else { proxy_id };

    let now = logging::now_ms();
    let existing = match input.id.as_deref() {
        Some(id) if !id.trim().is_empty() => get_job_record(id)?,
        _ => None,
    };
    let job_id = existing
        .as_ref()
        .map(|rec| rec.view.id.clone())
        .unwrap_or_else(|| format!("job-{now}-{}", rand_suffix()));

    let secret_keys: std::collections::HashSet<&str> = plugin
        .fields
        .iter()
        .filter(|f| f.secret || f.field_type == "password")
        .map(|f| f.key.as_str())
        .collect();

    let mut plain_values = BTreeMap::new();
    let mut secrets: BTreeMap<String, String> = existing
        .as_ref()
        .map(decrypt_job_secrets)
        .transpose()?
        .unwrap_or_default();

    for key in &input.clear_secrets {
        secrets.remove(key);
    }

    for (key, value) in input.values {
        if secret_keys.contains(key.as_str()) {
            if let Some(text) = value.as_str() {
                if !text.is_empty() {
                    secrets.insert(key, text.to_string());
                }
            }
        } else {
            plain_values.insert(key, value);
        }
    }

    let values_json = serde_json::to_string(&plain_values).unwrap_or_else(|_| "{}".into());
    let secrets_enc = encrypt_map(&secrets)?;
    let storage_enc = existing
        .as_ref()
        .map(|rec| rec.storage_enc.clone())
        .unwrap_or_default();
    let status_summary_json = existing
        .as_ref()
        .and_then(|rec| serde_json::to_string(&rec.view.status_summary).ok())
        .unwrap_or_else(|| "{}".into());
    let next_run = if input.enabled {
        Some(next_run_ms(&cron, &timezone, now)?)
    } else {
        None
    };
    let created_at = existing.as_ref().map(|rec| rec.view.created_at).unwrap_or(now);

    let db = database()?;
    db.with(|conn| {
        conn.execute(
            "INSERT INTO task_jobs (
                id, plugin_id, name, cron_expression, timezone, timeout_seconds,
                proxy_id, enabled, values_json, secrets_enc, storage_enc,
                status_summary_json, next_run_at, created_at, updated_at
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
             ON CONFLICT(id) DO UPDATE SET
                name=excluded.name, cron_expression=excluded.cron_expression,
                timezone=excluded.timezone, timeout_seconds=excluded.timeout_seconds,
                proxy_id=excluded.proxy_id, enabled=excluded.enabled,
                values_json=excluded.values_json, secrets_enc=excluded.secrets_enc,
                next_run_at=excluded.next_run_at, updated_at=excluded.updated_at",
            params![
                job_id,
                plugin.id,
                name,
                cron,
                timezone,
                input.timeout_seconds,
                proxy_id,
                i64::from(input.enabled),
                values_json,
                secrets_enc,
                storage_enc,
                status_summary_json,
                next_run,
                created_at,
                now,
            ],
        )
        .map_err(|error| format!("保存定时任务失败：{error}"))
    })
    .ok_or_else(|| "定时任务数据库不可用".to_string())??;

    get_job_record(&job_id)?
        .map(|rec| rec.view)
        .ok_or_else(|| "保存后读取定时任务失败".to_string())
}

pub fn patch_job_inline(
    id: &str,
    enabled: Option<bool>,
    proxy_id: Option<String>,
) -> Result<TaskJobView, String> {
    let rec = get_job_record(id)?.ok_or_else(|| "定时任务不存在".to_string())?;
    let next_enabled = enabled.unwrap_or(rec.view.enabled);
    let next_proxy = match proxy_id {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed == "direct" {
                String::new()
            } else {
                crate::server::core::proxy_pool::resolve_reference(trimmed)?;
                trimmed.to_string()
            }
        }
        None => rec.view.proxy_id.clone(),
    };
    let now = logging::now_ms();
    let next_run = if next_enabled {
        Some(next_run_ms(&rec.view.cron_expression, &rec.view.timezone, now)?)
    } else {
        None
    };
    let db = database()?;
    db.with(|conn| {
        conn.execute(
            "UPDATE task_jobs SET enabled=?1, proxy_id=?2, next_run_at=?3, updated_at=?4 WHERE id=?5",
            params![i64::from(next_enabled), next_proxy, next_run, now, id],
        )
        .map_err(|error| format!("更新定时任务失败：{error}"))
    })
    .ok_or_else(|| "定时任务数据库不可用".to_string())??;
    get_job_record(id)?
        .map(|r| r.view)
        .ok_or_else(|| "定时任务不存在".to_string())
}

pub fn delete_job(id: &str) -> Result<(), String> {
    let db = database()?;
    let affected = db
        .with(|conn| {
            conn.execute("DELETE FROM task_jobs WHERE id = ?1", [id])
                .map_err(|error| format!("删除定时任务失败：{error}"))
        })
        .ok_or_else(|| "定时任务数据库不可用".to_string())??;
    if affected == 0 {
        return Err("定时任务不存在".to_string());
    }
    Ok(())
}

pub fn duplicate_job(id: &str) -> Result<TaskJobView, String> {
    let rec = get_job_record(id)?.ok_or_else(|| "定时任务不存在".to_string())?;
    let base: String = rec.view.name.chars().take(95).collect();
    let name: String = format!("{base} (副本)").chars().take(100).collect();
    let now = logging::now_ms();
    let job_id = format!("job-{now}-{}", rand_suffix());
    let values_json = serde_json::to_string(&rec.view.values).unwrap_or_else(|_| "{}".into());

    let db = database()?;
    db.with(|conn| {
        conn.execute(
            "INSERT INTO task_jobs (
                id, plugin_id, name, cron_expression, timezone, timeout_seconds,
                proxy_id, enabled, values_json, secrets_enc, storage_enc,
                status_summary_json, next_run_at, created_at, updated_at
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,0,?8,'','','{}',NULL,?9,?10)",
            params![
                job_id,
                rec.view.plugin_id,
                name,
                rec.view.cron_expression,
                rec.view.timezone,
                rec.view.timeout_seconds,
                rec.view.proxy_id,
                values_json,
                now,
                now,
            ],
        )
        .map_err(|error| format!("复制定时任务失败：{error}"))
    })
    .ok_or_else(|| "定时任务数据库不可用".to_string())??;

    get_job_record(&job_id)?
        .map(|rec| rec.view)
        .ok_or_else(|| "复制后读取定时任务失败".to_string())
}

pub fn clear_runs(job_id: &str) -> Result<(), String> {
    if get_job_record(job_id)?.is_none() {
        return Err("定时任务不存在".to_string());
    }
    let db = database()?;
    db.with(|conn| {
        conn.execute("DELETE FROM task_job_runs WHERE job_id = ?1", [job_id])
            .map_err(|error| format!("清空运行记录失败：{error}"))
    })
    .ok_or_else(|| "定时任务数据库不可用".to_string())??;
    Ok(())
}

pub fn mark_run_started(
    job_id: &str,
    trigger_type: &str,
    trigger_label: &str,
) -> Result<i64, String> {
    let now = logging::now_ms();
    let db = database()?;
    db.with(|conn| {
        conn.execute(
            "INSERT INTO task_job_runs (job_id, trigger_type, trigger_label, status, started_at)
             VALUES (?1, ?2, ?3, 'running', ?4)",
            params![job_id, trigger_type, trigger_label, now],
        )
        .map_err(|error| format!("写入运行记录失败：{error}"))?;
        let run_id = conn.last_insert_rowid();
        if trigger_type != "action" {
            conn.execute(
                "UPDATE task_jobs SET last_run_at=?1, last_status='running', last_duration_ms=NULL, last_error=NULL WHERE id=?2",
                params![now, job_id],
            )
            .map_err(|error| format!("更新任务状态失败：{error}"))?;
        }
        Ok(run_id)
    })
    .ok_or_else(|| "定时任务数据库不可用".to_string())?
}

#[allow(clippy::too_many_arguments)]
pub fn mark_run_finished(
    job_id: &str,
    run_id: i64,
    trigger_type: &str,
    success: bool,
    duration_ms: i64,
    output: &str,
    error: Option<&str>,
    status_patch: Option<&BTreeMap<String, String>>,
    new_storage: Option<&BTreeMap<String, String>>,
) -> Result<(), String> {
    let now = logging::now_ms();
    let status = if success { "success" } else { "failed" };
    let truncated_output: String = output.chars().take(MAX_OUTPUT_CHARS).collect();
    let truncated_error = error.map(|e| e.chars().take(16_000).collect::<String>());
    let storage_enc = match new_storage {
        Some(map) => Some(encrypt_map(map)?),
        None => None,
    };

    let existing = get_job_record(job_id)?;
    let merged_summary_json = if let Some(patch) = status_patch {
        let mut summary = existing
            .as_ref()
            .map(|r| r.view.status_summary.clone())
            .unwrap_or_default();
        for (k, v) in patch {
            summary.insert(k.clone(), v.clone());
        }
        Some(serde_json::to_string(&summary).unwrap_or_else(|_| "{}".into()))
    } else {
        None
    };

    let db = database()?;
    db.with(|conn| {
        conn.execute(
            "UPDATE task_job_runs
             SET status=?1, finished_at=?2, duration_ms=?3, output=?4, error=?5
             WHERE id=?6",
            params![status, now, duration_ms, truncated_output, truncated_error, run_id],
        )
        .map_err(|err| format!("更新运行记录失败：{err}"))?;

        if trigger_type != "action" {
            conn.execute(
                "UPDATE task_jobs
                 SET last_status=?1, last_duration_ms=?2, last_error=?3, updated_at=?4
                 WHERE id=?5",
                params![status, duration_ms, truncated_error, now, job_id],
            )
            .map_err(|err| format!("更新任务终态失败：{err}"))?;
        }
        if let Some(ref summary_json) = merged_summary_json {
            conn.execute(
                "UPDATE task_jobs SET status_summary_json=?1, updated_at=?2 WHERE id=?3",
                params![summary_json, now, job_id],
            )
            .map_err(|err| format!("更新任务状态摘要失败：{err}"))?;
        }
        if let Some(ref enc) = storage_enc {
            conn.execute(
                "UPDATE task_jobs SET storage_enc=?1, updated_at=?2 WHERE id=?3",
                params![enc, now, job_id],
            )
            .map_err(|err| format!("保存任务加密仓库失败：{err}"))?;
        }
        // 只保留最近 MAX_RUNS_PER_JOB 条运行记录
        conn.execute(
            "DELETE FROM task_job_runs WHERE job_id=?1 AND id NOT IN (
                SELECT id FROM task_job_runs WHERE job_id=?1 ORDER BY id DESC LIMIT ?2
             )",
            params![job_id, MAX_RUNS_PER_JOB],
        )
        .map_err(|err| format!("清理旧运行记录失败：{err}"))?;
        Ok(())
    })
    .ok_or_else(|| "定时任务数据库不可用".to_string())?
}

pub fn list_runs(job_id: &str, limit: i64) -> Result<Vec<TaskJobRun>, String> {
    let limit = limit.clamp(1, 100);
    let db = database()?;
    db.with(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, job_id, trigger_type, trigger_label, status, started_at,
                        finished_at, duration_ms, COALESCE(output, ''), error
                 FROM task_job_runs
                 WHERE job_id = ?1
                 ORDER BY id DESC LIMIT ?2",
            )
            .map_err(|error| format!("读取运行记录失败：{error}"))?;
        let rows = stmt
            .query_map(params![job_id, limit], |row| {
                Ok(TaskJobRun {
                    id: row.get(0)?,
                    job_id: row.get(1)?,
                    trigger_type: row.get(2)?,
                    trigger_label: row.get(3)?,
                    status: row.get(4)?,
                    started_at: row.get(5)?,
                    finished_at: row.get(6)?,
                    duration_ms: row.get(7)?,
                    output: row.get(8)?,
                    error: row.get(9)?,
                })
            })
            .map_err(|error| format!("读取运行记录失败：{error}"))?;
        let mut out = Vec::new();
        for item in rows {
            out.push(item.map_err(|error| format!("解析运行记录失败：{error}"))?);
        }
        Ok(out)
    })
    .ok_or_else(|| "定时任务数据库不可用".to_string())?
}

pub fn list_due_jobs(now_ms: i64) -> Result<Vec<TaskJobView>, String> {
    let db = database()?;
    db.with(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT j.id, j.plugin_id, j.name, j.cron_expression, j.timezone, j.timeout_seconds,
                        j.proxy_id, j.enabled, j.values_json, j.secrets_enc, j.storage_enc,
                        j.status_summary_json, j.next_run_at, j.last_run_at, j.last_status,
                        j.last_duration_ms, j.last_error, j.created_at, j.updated_at
                 FROM task_jobs j
                 INNER JOIN task_plugins p ON p.id = j.plugin_id
                 WHERE j.enabled = 1 AND p.enabled = 1
                   AND j.next_run_at IS NOT NULL AND j.next_run_at <= ?1
                 ORDER BY j.next_run_at ASC LIMIT 20",
            )
            .map_err(|error| format!("查询到期定时任务失败：{error}"))?;
        let rows = stmt
            .query_map([now_ms], job_from_row)
            .map_err(|error| format!("查询到期定时任务失败：{error}"))?;
        let mut out = Vec::new();
        for item in rows {
            out.push(item.map_err(|error| format!("解析到期定时任务失败：{error}"))?.view);
        }
        Ok(out)
    })
    .ok_or_else(|| "定时任务数据库不可用".to_string())?
}

pub fn update_next_run(job_id: &str, next_run: Option<i64>) -> Result<(), String> {
    let db = database()?;
    db.with(|conn| {
        conn.execute(
            "UPDATE task_jobs SET next_run_at=?1 WHERE id=?2",
            params![next_run, job_id],
        )
        .map_err(|error| format!("更新下次执行时间失败：{error}"))
    })
    .ok_or_else(|| "定时任务数据库不可用".to_string())??;
    Ok(())
}

pub fn recover_on_startup() {
    let Ok(db) = database() else { return };
    let now = logging::now_ms();
    let _ = db.with(|conn| {
        let _ = conn.execute(
            "UPDATE task_job_runs
             SET status='failed', finished_at=?1,
                 error=COALESCE(error, '服务重启，执行已中断')
             WHERE status='running'",
            [now],
        );
        let _ = conn.execute(
            "UPDATE task_jobs
             SET last_status='failed',
                 last_error=COALESCE(last_error, '服务重启，执行已中断')
             WHERE last_status='running'",
            [],
        );
    });
    if let Ok(jobs) = list_jobs() {
        for job in jobs {
            if job.enabled {
                if let Ok(next) = next_run_ms(&job.cron_expression, &job.timezone, now) {
                    let _ = update_next_run(&job.id, Some(next));
                }
            }
        }
    }
}

fn rand_suffix() -> String {
    let mut buf = [0u8; 4];
    let _ = getrandom::getrandom(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}
