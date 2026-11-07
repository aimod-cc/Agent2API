use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::server::db::Db;
use super::{now, CodingError};

pub fn sandbox(db: &Db, scope: &str, thread: &str) -> Result<Option<Value>, CodingError> {
    let text: Option<String> = db.with(|conn| conn.query_row(
        "SELECT sandbox FROM coding_thread_settings WHERE scope=?1 AND thread_id=?2", params![scope, thread], |row| row.get(0)).optional())
        .ok_or_else(|| CodingError::new(500, "数据库不可用"))?.map_err(|_| CodingError::new(500, "读取 Codex 会话权限失败"))?;
    text.map(|text| serde_json::from_str(&text).map_err(|_| CodingError::new(500, "Codex 会话权限记录损坏，未放宽权限"))).transpose()
}

pub fn save_sandbox(db: &Db, scope: &str, thread: &str, sandbox: &Value) -> Result<(), CodingError> {
    db.with(|conn| conn.execute(
        "INSERT INTO coding_thread_settings(scope,thread_id,sandbox) VALUES(?1,?2,?3) ON CONFLICT(scope,thread_id) DO UPDATE SET sandbox=excluded.sandbox",
        params![scope, thread, sandbox.to_string()])).ok_or_else(|| CodingError::new(500, "数据库不可用"))?
        .map_err(|_| CodingError::new(500, "保存 Codex 会话权限失败，未发送任务"))?;
    Ok(())
}

fn existing_delivery(old_hash: &str, state: &str, local_id: &str, text: &str) -> Result<Value, CodingError> {
    let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
    if old_hash != hash { return Err(CodingError::new(409, "消息标识已被其他内容使用，未重复发送")); }
    match state {
        "accepted" | "consumed" => Ok(json!({ "accepted": true, "localId": local_id })),
        "rejected" => Err(CodingError::new(409, "这条消息已被 Codex 拒绝，请修改后重新发送")),
        _ => Err(CodingError::uncertain("这条消息的发送结果尚未确认，未重复执行；请检查会话历史")),
    }
}

pub fn delivery(db: &Db, scope: &str, thread: &str, local_id: &str, text: &str) -> Result<Option<Value>, CodingError> {
    let existing: Option<(String, String)> = db.with(|conn| conn.query_row(
        "SELECT content_hash,state FROM coding_submissions WHERE scope=?1 AND thread_id=?2 AND local_id=?3",
        params![scope, thread, local_id], |row| Ok((row.get(0)?, row.get(1)?))).optional())
        .ok_or_else(|| CodingError::new(500, "数据库不可用"))?.map_err(|_| CodingError::new(500, "读取 Codex 消息状态失败"))?;
    existing.map(|(hash, state)| existing_delivery(&hash, &state, local_id, text)).transpose()
}

pub fn reserve(db: &Db, scope: &str, thread: &str, local_id: &str, text: &str) -> Result<Option<Value>, CodingError> {
    let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
    db.with_mut(|conn| -> Result<Option<Value>, CodingError> {
        let tx = conn.transaction().map_err(|_| CodingError::new(500, "保存 Codex 消息状态失败"))?;
        let existing: Option<(String, String)> = tx.query_row(
            "SELECT content_hash,state FROM coding_submissions WHERE scope=?1 AND thread_id=?2 AND local_id=?3",
            params![scope, thread, local_id], |row| Ok((row.get(0)?, row.get(1)?))).optional()
            .map_err(|_| CodingError::new(500, "读取 Codex 消息状态失败"))?;
        if let Some((old_hash, state)) = existing { return existing_delivery(&old_hash, &state, local_id, text).map(Some); }
        tx.execute("INSERT INTO coding_submissions(scope,thread_id,local_id,content_hash,state,updated_at) VALUES(?1,?2,?3,?4,'pending',?5)",
            params![scope, thread, local_id, hash, now()]).map_err(|_| CodingError::new(500, "保存 Codex 消息状态失败，未发送"))?;
        tx.commit().map_err(|_| CodingError::new(500, "保存 Codex 消息状态失败，未发送"))?;
        Ok(None)
    }).ok_or_else(|| CodingError::new(500, "数据库不可用"))?
}

pub fn settle(db: &Db, scope: &str, thread: &str, local_id: &str, state: &str, turn: Option<&str>) -> Result<(), CodingError> {
    db.with(|conn| conn.execute(
        "UPDATE coding_submissions SET state=CASE WHEN state='consumed' THEN state ELSE ?4 END,turn_id=COALESCE(?5,turn_id),updated_at=?6 WHERE scope=?1 AND thread_id=?2 AND local_id=?3",
        params![scope, thread, local_id, state, turn, now()])).ok_or_else(|| CodingError::new(500, "数据库不可用"))?
        .map_err(|_| CodingError::uncertain("Codex 已收到操作，但本地发送状态保存失败；请检查历史，不要重复发送"))?;
    Ok(())
}
