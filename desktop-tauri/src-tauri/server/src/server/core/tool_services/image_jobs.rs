use rusqlite::{params, OptionalExtension, Row, TransactionBehavior};
use serde_json::{json, Value};

use crate::server::{db::Db, errors::GatewayError, logging};
use super::image_config::{ImageConfig, ImageTarget};

pub const MAX_IMAGE_BYTES: i64 = 32 * 1024 * 1024;
const JOB_COLUMNS: &str = "id, owner, request_id, request_hash, operation, provider, model, account_id, state, created_at, updated_at, expires_at, result, error";

#[derive(Clone)]
pub struct ImageJob {
    pub id: String,
    pub owner: String,
    pub request_id: String,
    pub request_hash: String,
    pub operation: String,
    pub provider: String,
    pub model: String,
    pub account_id: String,
    pub state: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub expires_at: i64,
    pub result: Option<Value>,
    pub error: Option<String>,
}

pub struct ImageFile {
    pub id: String,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

fn storage_error(error: impl std::fmt::Display) -> GatewayError {
    GatewayError::with_status(503, format!("图片任务存储不可用：{error}")).with_code("image_storage_unavailable")
}

fn locked() -> GatewayError {
    storage_error("数据库连接不可用")
}

pub fn random_id() -> Result<String, GatewayError> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(storage_error)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<ImageJob> {
    let result: Option<String> = row.get(12)?;
    Ok(ImageJob { id: row.get(0)?, owner: row.get(1)?, request_id: row.get(2)?, request_hash: row.get(3)?,
        operation: row.get(4)?, provider: row.get(5)?, model: row.get(6)?, account_id: row.get(7)?, state: row.get(8)?,
        created_at: row.get(9)?, updated_at: row.get(10)?, expires_at: row.get(11)?,
        result: result.and_then(|text| serde_json::from_str(&text).ok()), error: row.get(13)? })
}

fn maintain(conn: &rusqlite::Connection, now: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE tool_image_jobs SET state='uncertain', reserved_bytes=0, updated_at=?1, error=?2 WHERE state='running' AND deadline_at<?1",
        params![now, "生成任务已超时或执行进程已停止，上游可能已消耗额度，未重新提交"])?;
    conn.execute("DELETE FROM tool_image_files WHERE job_id IN (SELECT id FROM tool_image_jobs WHERE expires_at<=?1 AND state!='running')", [now])?;
    conn.execute("UPDATE tool_image_jobs SET result=NULL, state='expired', reserved_bytes=0, error='图片结果已过期，未重新生成' WHERE expires_at<=?1 AND state!='running' AND state!='expired'", [now])?;
    // 提交编号在整个保留期内不可复用；过期记录再保留七天，避免迟到重试重新扣费。
    conn.execute("DELETE FROM tool_image_jobs WHERE state='expired' AND expires_at<?1", [now - 7 * 24 * 60 * 60_000])?;
    Ok(())
}

pub fn find_request(db: &Db, owner: &str, request_id: &str) -> Result<Option<ImageJob>, GatewayError> {
    db.with(|conn| {
        maintain(conn, logging::now_ms())?;
        conn.query_row(&format!("SELECT {JOB_COLUMNS} FROM tool_image_jobs WHERE owner=?1 AND request_id=?2"),
            params![owner, request_id], from_row).optional()
    }).ok_or_else(locked)?.map_err(storage_error)
}

pub struct NewJob<'a> {
    pub owner: &'a str,
    pub request_id: &'a str,
    pub request_hash: &'a str,
    pub operation: &'a str,
    pub target: &'a ImageTarget,
    pub count: u32,
    pub timeout: u64,
}

pub fn create(db: &Db, config: &ImageConfig, input: NewJob<'_>) -> Result<(ImageJob, bool), GatewayError> {
    let id = random_id()?;
    let now = logging::now_ms();
    let expires = now + i64::from(config.retention_hours.clamp(1, 168)) * 60 * 60_000;
    let deadline = now + (input.timeout as i64 + 120) * 1000;
    let reserve = i64::from(input.count) * MAX_IMAGE_BYTES;
    db.with_mut(|conn| -> Result<(ImageJob, bool), GatewayError> {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage_error)?;
        maintain(&tx, now).map_err(storage_error)?;
        if let Some(job) = tx.query_row(&format!("SELECT {JOB_COLUMNS} FROM tool_image_jobs WHERE owner=?1 AND request_id=?2"),
            params![input.owner, input.request_id], from_row).optional().map_err(storage_error)? {
            tx.commit().map_err(storage_error)?;
            return Ok((job, false));
        }
        let running: u32 = tx.query_row("SELECT COUNT(*) FROM tool_image_jobs WHERE state='running'", [], |row| row.get(0)).map_err(storage_error)?;
        if running >= config.max_concurrency.clamp(1, 8) {
            return Err(GatewayError::with_status(429, "图片生成已达到并发上限，请稍后使用同一提交编号重试").with_code("image_concurrency_limit"));
        }
        let recent: u32 = tx.query_row("SELECT COUNT(*) FROM tool_image_jobs WHERE owner=?1 AND created_at>?2",
            params![input.owner, now - 60_000], |row| row.get(0)).map_err(storage_error)?;
        if recent >= 20 {
            return Err(GatewayError::with_status(429, "图片任务提交过于频繁，请稍后重试").with_code("image_rate_limit"));
        }
        let records: u32 = tx.query_row("SELECT COUNT(*) FROM tool_image_jobs", [], |row| row.get(0)).map_err(storage_error)?;
        if records >= 10_000 {
            return Err(GatewayError::with_status(507, "图片任务记录已满，请等待过期记录清理").with_code("image_storage_full"));
        }
        let used: i64 = tx.query_row("SELECT COALESCE((SELECT SUM(length(bytes)) FROM tool_image_files),0) + COALESCE((SELECT SUM(reserved_bytes) FROM tool_image_jobs),0)",
            [], |row| row.get(0)).map_err(storage_error)?;
        if used.saturating_add(reserve) > i64::from(config.storage_mb.clamp(64, 4096)) * 1024 * 1024 {
            return Err(GatewayError::with_status(507, "图片存储剩余空间不足，尚未请求生图；请清理旧结果或增加存储上限").with_code("image_storage_full"));
        }
        tx.execute("INSERT INTO tool_image_jobs (id,owner,request_id,request_hash,operation,provider,model,account_id,state,created_at,updated_at,deadline_at,expires_at,reserved_bytes) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'running',?9,?9,?10,?11,?12)",
            params![id, input.owner, input.request_id, input.request_hash, input.operation, input.target.provider,
                input.target.model, input.target.account_id, now, deadline, expires, reserve]).map_err(storage_error)?;
        let job = tx.query_row(&format!("SELECT {JOB_COLUMNS} FROM tool_image_jobs WHERE id=?1"), [&id], from_row).map_err(storage_error)?;
        tx.commit().map_err(storage_error)?;
        Ok((job, true))
    }).ok_or_else(locked)?
}

pub fn get(db: &Db, id: &str) -> Result<ImageJob, GatewayError> {
    db.with(|conn| {
        maintain(conn, logging::now_ms())?;
        conn.query_row(&format!("SELECT {JOB_COLUMNS} FROM tool_image_jobs WHERE id=?1"), [id], from_row).optional()
    }).ok_or_else(locked)?.map_err(storage_error)?
        .ok_or_else(|| GatewayError::with_status(404, "图片任务不存在或已过期").with_code("image_job_not_found"))
}

pub fn list(db: &Db) -> Result<Vec<ImageJob>, GatewayError> {
    db.with(|conn| {
        maintain(conn, logging::now_ms())?;
        let mut statement = conn.prepare(&format!("SELECT {JOB_COLUMNS} FROM tool_image_jobs ORDER BY created_at DESC LIMIT 50"))?;
        let rows = statement.query_map([], from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok::<_, rusqlite::Error>(rows)
    }).ok_or_else(locked)?.map_err(storage_error)
}

pub fn finish(db: &Db, id: &str, mut result: Value, files: Vec<ImageFile>) -> Result<(), GatewayError> {
    let total: i64 = files.iter().map(|file| file.bytes.len() as i64).sum();
    let metadata: Vec<_> = files.iter().map(|file| json!({ "id": file.id, "mime": file.mime,
        "width": file.width, "height": file.height, "bytes": file.bytes.len() })).collect();
    result["images"] = json!(metadata);
    let encoded = serde_json::to_string(&result).map_err(storage_error)?;
    db.with_mut(|conn| -> Result<(), GatewayError> {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage_error)?;
        let (reserved, state): (i64, String) = tx.query_row("SELECT reserved_bytes,state FROM tool_image_jobs WHERE id=?1", [id],
            |row| Ok((row.get(0)?, row.get(1)?))).map_err(storage_error)?;
        if state != "running" || total > reserved {
            return Err(storage_error("图片已生成，但任务已结束或图片超过预留空间，请勿重新生图"));
        }
        for file in files {
            tx.execute("INSERT INTO tool_image_files(id,job_id,mime,width,height,bytes) VALUES (?1,?2,?3,?4,?5,?6)",
                params![file.id, id, file.mime, file.width, file.height, file.bytes]).map_err(storage_error)?;
        }
        tx.execute("UPDATE tool_image_jobs SET state='succeeded',reserved_bytes=0,result=?2,updated_at=?3 WHERE id=?1 AND state='running'",
            params![id, encoded, logging::now_ms()]).map_err(storage_error)?;
        tx.commit().map_err(storage_error)
    }).ok_or_else(locked)?
}

pub fn fail(db: &Db, id: &str, uncertain: bool, message: &str) -> Result<(), GatewayError> {
    let message: String = message.chars().take(2000).collect();
    db.with(|conn| conn.execute("UPDATE tool_image_jobs SET state=?2,reserved_bytes=0,error=?3,updated_at=?4 WHERE id=?1 AND state='running'",
        params![id, if uncertain { "uncertain" } else { "failed" }, message, logging::now_ms()]))
        .ok_or_else(locked)?.map_err(storage_error).map(|_| ())
}

pub fn read_file(db: &Db, id: &str) -> Result<(ImageJob, ImageFile), GatewayError> {
    if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(GatewayError::with_status(404, "生成图片不存在或已过期"));
    }
    let job_id = db.with(|conn| conn.query_row("SELECT job_id FROM tool_image_files WHERE id=?1", [id], |row| row.get::<_, String>(0)).optional())
        .ok_or_else(locked)?.map_err(storage_error)?
        .ok_or_else(|| GatewayError::with_status(404, "生成图片不存在或已过期"))?;
    let job = get(db, &job_id)?;
    if job.state != "succeeded" || job.expires_at <= logging::now_ms() {
        return Err(GatewayError::with_status(404, "生成图片不存在或已过期"));
    }
    let file = db.with(|conn| conn.query_row("SELECT mime,width,height,bytes FROM tool_image_files WHERE id=?1", [id], |row| {
        Ok(ImageFile { id: id.to_string(), mime: row.get(0)?, width: row.get(1)?, height: row.get(2)?, bytes: row.get(3)? })
    }).optional()).ok_or_else(locked)?.map_err(storage_error)?
        .ok_or_else(|| GatewayError::with_status(404, "生成图片不存在或已过期"))?;
    Ok((job, file))
}

pub fn delete(db: &Db, id: &str) -> Result<(), GatewayError> {
    db.with_mut(|conn| -> Result<(), GatewayError> {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage_error)?;
        let state: Option<String> = tx.query_row("SELECT state FROM tool_image_jobs WHERE id=?1", [id], |row| row.get(0))
            .optional().map_err(storage_error)?;
        if state.as_deref() == Some("running") {
            return Err(GatewayError::with_status(409, "生成中的任务不能清理，请等待结束"));
        }
        tx.execute("DELETE FROM tool_image_files WHERE job_id=?1", [id]).map_err(storage_error)?;
        tx.execute("UPDATE tool_image_jobs SET state='expired',result=NULL,error='图片结果已清理，未重新生成',updated_at=?2 WHERE id=?1",
            params![id, logging::now_ms()]).map_err(storage_error)?;
        tx.commit().map_err(storage_error)
    }).ok_or_else(locked)?
}

pub fn storage_usage(db: &Db) -> Result<Value, GatewayError> {
    db.with(|conn| {
        maintain(conn, logging::now_ms())?;
        conn.query_row("SELECT COALESCE((SELECT SUM(length(bytes)) FROM tool_image_files),0),COALESCE((SELECT SUM(reserved_bytes) FROM tool_image_jobs),0)",
            [], |row| Ok(json!({ "used_bytes": row.get::<_, i64>(0)?, "reserved_bytes": row.get::<_, i64>(1)? })))
    }).ok_or_else(locked)?.map_err(storage_error)
}
