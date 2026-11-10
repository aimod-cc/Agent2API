use std::sync::OnceLock;
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::server::db::Db;
use crate::server::logging;

use super::types::{self, Detail, Item, Query, Run};
use crate::server::config;
static DB: OnceLock<Option<Db>> = OnceLock::new();
const COLUMNS: &str = "id, task_id, source, retry_of, started_at, finished_at, status, refreshed, skipped, failed, total, error, metadata, duration_ms";

/// 按当前保留设置清理已结束的历史记录（天数与条数上限都以设置为准）。
///
/// 在每轮记录落库后与「设置页把保留值改小」这两个时机调用：后者不主动清理的话，
/// 旧数据要等到下一次维护执行才消失，界面会显得「改了没生效」。
/// 数据库与事务错误向调用方返回，配置已保存时由设置入口明确报告清理失败。
pub fn prune() -> Result<(u64, u64), String> {
    let settings = config::retention_settings();
    let db = database()?;
    db.with_mut(|conn| -> Result<(u64, u64), String> {
        let tx = sql(conn.transaction())?;
        let pruned = prune_with(&tx, settings.maintenance_days, settings.maintenance_count)?;
        sql(tx.commit())?;
        Ok(pruned)
    })
    .ok_or_else(|| "维护记录数据库繁忙".to_string())?
}

/// 一条事务里的清理动作，返回（按天数清理的行数，按条数清理的行数）。
fn prune_with(tx: &rusqlite::Transaction<'_>, retention_days: i64, max_records: i64) -> Result<(u64, u64), String> {
    let cutoff = logging::now_ms().saturating_sub(retention_days.max(1) * 86_400_000);
    let expired = sql(tx.execute(
        "DELETE FROM maintenance_runs WHERE status != 'running' AND started_at < ?1",
        params![cutoff],
    ))?;
    let overflow = sql(tx.execute(
        "DELETE FROM maintenance_runs WHERE status != 'running' AND id NOT IN (SELECT id FROM maintenance_runs WHERE status != 'running' ORDER BY started_at DESC, id DESC LIMIT ?1)",
        params![max_records.max(1)],
    ))?;
    sql(tx.execute(
        "DELETE FROM maintenance_items WHERE NOT EXISTS (SELECT 1 FROM maintenance_runs r WHERE r.id = maintenance_items.run_id)",
        [],
    ))?;
    Ok((expired as u64, overflow as u64))
}

pub fn install(db: Option<Db>) {
    let _ = DB.set(db);
}

/// 手动清理：删掉全部已结束的执行记录与孤儿明细，运行中的记录不动。
/// 返回删除的记录条数。
pub fn clear() -> Result<u64, String> {
    let db = database()?;
    db.with_mut(|conn| -> Result<u64, String> {
        let tx = sql(conn.transaction())?;
        let cleared = sql(tx.execute(
            "DELETE FROM maintenance_runs WHERE status != 'running'",
            [],
        ))?;
        sql(tx.execute(
            "DELETE FROM maintenance_items WHERE NOT EXISTS (SELECT 1 FROM maintenance_runs r WHERE r.id = maintenance_items.run_id)",
            [],
        ))?;
        sql(tx.commit())?;
        Ok(cleared as u64)
    })
    .unwrap_or_else(|| Err("维护记录数据库不可用".to_string()))
}

fn database() -> Result<&'static Db, String> {
    DB.get()
        .and_then(Option::as_ref)
        .ok_or_else(|| "维护记录数据库不可用".to_string())
}

fn sql<T>(result: rusqlite::Result<T>) -> Result<T, String> {
    result.map_err(|error| format!("维护记录读写失败：{error}"))
}

pub struct Recorder {
    pub run: Run,
    clock: Instant,
    heartbeat: tokio::task::JoinHandle<()>,
    failure: Option<String>,
    finished: bool,
}

impl Recorder {
    pub fn start(task_id: &str, source: &str, retry_of: Option<&str>) -> Result<Self, String> {
        let now = logging::now_ms();
        let mut random = [0_u8; 12];
        getrandom::getrandom(&mut random).map_err(|error| format!("无法创建维护记录：{error}"))?;
        let id = format!(
            "{now}-{}",
            random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let run = Run {
            id,
            task_id: task_id.to_string(),
            source: source.to_string(),
            retry_of: retry_of.map(str::to_string),
            started_at: now,
            finished_at: None,
            duration_ms: None,
            status: "running".to_string(),
            refreshed: 0,
            skipped: 0,
            failed: 0,
            total: 0,
            error: None,
            metadata: None,
        };
        database()?.with(|conn| sql(conn.execute(
            "INSERT INTO maintenance_runs (id, task_id, source, retry_of, started_at, heartbeat_at, status) VALUES (?1, ?2, ?3, ?4, ?5, ?5, 'running')",
            params![run.id, run.task_id, run.source, run.retry_of, now],
        ))).ok_or_else(|| "维护记录数据库繁忙".to_string())??;
        let run_id = run.id.clone();
        let heartbeat = crate::spawn_task(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let Ok(db) = database() else {
                    break;
                };
                let result = db.with(|conn| {
                    conn.execute(
                    "UPDATE maintenance_runs SET heartbeat_at=?2 WHERE id=?1 AND status='running'",
                    params![run_id, logging::now_ms()],
                )
                });
                if matches!(result, Some(Ok(0))) {
                    break;
                }
            }
        });
        Ok(Self {
            run,
            clock: Instant::now(),
            heartbeat,
            failure: None,
            finished: false,
        })
    }

    pub fn finish(mut self, results: Vec<Item>, metadata: Option<Item>) -> Result<Detail, String> {
        self.failure =
            Some("维护已执行，但完整结果未能保存；请先核对账号状态，不要反复执行".to_string());
        let (refreshed, skipped, failed) = types::summary(&results);
        self.run.finished_at = Some(logging::now_ms());
        self.run.duration_ms = Some(self.clock.elapsed().as_millis().min(i64::MAX as u128) as i64);
        self.run.status = types::status(refreshed, failed, metadata.as_ref()).to_string();
        self.run.refreshed = refreshed;
        self.run.skipped = skipped;
        self.run.failed = failed;
        self.run.total = results.len();
        self.run.metadata = metadata;
        let encoded_metadata = self
            .run
            .metadata
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| error.to_string())?;
        let encoded_items = results
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let result = database()?.with_mut(|conn| -> Result<(), String> {
            let tx = sql(conn.transaction())?;
            for (position, (item, data)) in results.iter().zip(encoded_items.iter()).enumerate() {
                sql(tx.execute(
                    "INSERT INTO maintenance_items (run_id, position, provider, account_id, account_name, status, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![self.run.id, position as i64, item.provider, item.account_id, item.account_name, item.status, data],
                ))?;
            }
            sql(tx.execute(
                "UPDATE maintenance_runs SET finished_at=?2, status=?3, refreshed=?4, skipped=?5, failed=?6, total=?7, metadata=?8, duration_ms=?9, error=NULL WHERE id=?1",
                params![self.run.id, self.run.finished_at, self.run.status, refreshed as i64, skipped as i64, failed as i64, results.len() as i64, encoded_metadata, self.run.duration_ms],
            ))?;
            recover_interrupted(&tx)?;
            let settings = config::retention_settings();
            prune_with(&tx, settings.maintenance_days, settings.maintenance_count)?;
            sql(tx.commit())
        }).ok_or_else(|| "维护记录数据库繁忙".to_string()).and_then(|result| result);
        if let Err(error) = result {
            let message = types::safe_message(&format!(
                "维护已执行，但结果保存失败：{error}；请勿因记录失败反复执行"
            ));
            self.failure = Some(message.clone());
            return Err(message);
        }
        self.heartbeat.abort();
        self.finished = true;
        Ok(Detail {
            run: self.run.clone(),
            results,
        })
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.heartbeat.abort();
        if self.finished {
            return;
        }
        if let Ok(db) = database() {
            let message = self
                .failure
                .as_deref()
                .unwrap_or("执行中断，结果可能已部分生效，请先核对账号状态");
            let _ = db.with(|conn| conn.execute(
                "UPDATE maintenance_runs SET status='interrupted', finished_at=?2, error=?3 WHERE id=?1 AND status='running'",
                params![self.run.id, logging::now_ms(), message],
            ));
        }
    }
}

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Run> {
    let started_at: i64 = row.get(4)?;
    let finished_at: Option<i64> = row.get(5)?;
    let metadata: Option<String> = row.get(12)?;
    let metadata = metadata
        .map(|text| {
            serde_json::from_str::<Item>(&text).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    12,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })
        .transpose()?;
    Ok(Run {
        id: row.get(0)?,
        task_id: row.get(1)?,
        source: row.get(2)?,
        retry_of: row.get(3)?,
        started_at,
        finished_at,
        duration_ms: row.get(13)?,
        status: row.get(6)?,
        refreshed: row.get::<_, i64>(7)?.max(0) as usize,
        skipped: row.get::<_, i64>(8)?.max(0) as usize,
        failed: row.get::<_, i64>(9)?.max(0) as usize,
        total: row.get::<_, i64>(10)?.max(0) as usize,
        error: row.get(11)?,
        metadata,
    })
}

fn recover_interrupted(conn: &Connection) -> Result<(), String> {
    // 按轮次心跳恢复，不能用同类任务的新租约掩盖旧轮次中断。
    let cutoff = logging::now_ms().saturating_sub(180_000);
    sql(conn.execute(
        "UPDATE maintenance_runs SET status='interrupted', finished_at=heartbeat_at, error='上次执行中断，没有完整结果；请核对账号状态' WHERE status='running' AND heartbeat_at < ?1",
        [cutoff],
    ))?;
    Ok(())
}

pub fn list(query: &Query) -> Result<Value, String> {
    let limit = if query.limit == 0 {
        20
    } else {
        query.limit.min(100)
    } as i64;
    let offset = query.offset.min(i64::MAX as usize) as i64;
    let keyword = query
        .keyword
        .trim()
        .to_lowercase()
        .chars()
        .take(200)
        .collect::<String>();
    database()?.with(|conn| -> Result<Value, String> {
        recover_interrupted(conn)?;
        let clause = "(?1='' OR r.task_id=?1) AND (?2='' OR (?2='automatic' AND r.source IN ('automatic','passive')) OR (?2='manual' AND r.source='manual')) AND (?3='' OR (?3='failed' AND r.status IN ('partial','failed','interrupted')) OR (?3='success' AND r.status IN ('completed','skipped'))) AND (?4='' OR EXISTS (SELECT 1 FROM maintenance_items i WHERE i.run_id=r.id AND (instr(lower(i.provider),?4)>0 OR instr(lower(coalesce(i.account_name,'')),?4)>0 OR instr(lower(coalesce(i.account_id,'')),?4)>0 OR instr(lower(i.data),?4)>0)))";
        let total: i64 = sql(conn.query_row(&format!("SELECT count(*) FROM maintenance_runs r WHERE {clause}"), params![query.task_id, query.source, query.status, keyword], |row| row.get(0)))?;
        let fields = COLUMNS.split(", ").map(|column| format!("r.{column}")).collect::<Vec<_>>().join(", ");
        let mut statement = sql(conn.prepare(&format!("SELECT {fields} FROM maintenance_runs r WHERE {clause} ORDER BY r.started_at DESC, r.id DESC LIMIT ?5 OFFSET ?6")))?;
        let rows = sql(statement.query_map(params![query.task_id, query.source, query.status, keyword, limit, offset], read_row))?;
        let items = sql(rows.collect::<rusqlite::Result<Vec<_>>>())?;
        Ok(json!({ "items": items, "total": total, "offset": offset, "limit": limit }))
    }).ok_or_else(|| "维护记录数据库繁忙".to_string())?
}

pub fn latest() -> Result<Value, String> {
    database()?.with(|conn| -> Result<Value, String> {
        let mut latest = serde_json::Map::new();
        for task_id in [super::CREDENTIAL, super::MODEL] {
            let run = sql(conn.query_row(&format!("SELECT {COLUMNS} FROM maintenance_runs WHERE task_id=?1 ORDER BY started_at DESC, id DESC LIMIT 1"), [task_id], read_row).optional())?;
            if let Some(run) = run { latest.insert(task_id.to_string(), json!(run)); }
        }
        Ok(Value::Object(latest))
    }).ok_or_else(|| "维护记录数据库繁忙".to_string())?
}

pub fn detail(id: &str) -> Result<Detail, String> {
    database()?
        .with(|conn| -> Result<Detail, String> {
            recover_interrupted(conn)?;
            let run = sql(conn
                .query_row(
                    &format!("SELECT {COLUMNS} FROM maintenance_runs WHERE id=?1"),
                    [id],
                    read_row,
                )
                .optional())?
            .ok_or_else(|| "维护记录不存在或已清理".to_string())?;
            let mut statement = sql(conn
                .prepare("SELECT data FROM maintenance_items WHERE run_id=?1 ORDER BY position"))?;
            let rows = sql(statement.query_map([id], |row| row.get::<_, String>(0)))?;
            let results = sql(rows.collect::<rusqlite::Result<Vec<_>>>())?
                .into_iter()
                .map(|text| {
                    serde_json::from_str::<Item>(&text)
                        .map_err(|error| format!("维护明细读取失败：{error}"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Detail { run, results })
        })
        .ok_or_else(|| "维护记录数据库繁忙".to_string())?
}
