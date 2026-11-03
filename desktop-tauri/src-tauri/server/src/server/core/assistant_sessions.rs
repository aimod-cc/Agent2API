//! AI 助手会话按整条消息流保存；图片复制和会话分叉在同一事务内完成。

use std::collections::HashMap;
use std::sync::OnceLock;

use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

use crate::server::db::Db;
use crate::server::logging;

static DB: OnceLock<Option<Db>> = OnceLock::new();
const MAX_TITLE_CHARS: usize = 120;
const MAX_ID_CHARS: usize = 128;
const MAX_MESSAGES_CHARS: usize = 8 * 1024 * 1024;

pub fn install(db: Option<Db>) { let _ = DB.set(db); }
fn database() -> Option<&'static Db> { DB.get().and_then(Option::as_ref) }
fn truncate(text: &str, limit: usize) -> String { text.chars().take(limit).collect() }
fn field<'a>(value: &'a Value, key: &str) -> &'a str { value.get(key).and_then(Value::as_str).unwrap_or("").trim() }

fn summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": row.get::<_, String>(0)?, "title": row.get::<_, String>(1)?,
        "model": row.get::<_, Option<String>>(2)?, "reasoning": row.get::<_, Option<String>>(3)?,
        "createdAt": row.get::<_, i64>(4)?, "updatedAt": row.get::<_, i64>(5)?,
        "titleGenerated": row.get::<_, bool>(6)?,
    }))
}

pub fn list() -> Vec<Value> {
    let Some(db) = database() else { return Vec::new() };
    db.with(|conn| {
        let mut statement = conn.prepare("SELECT id, title, model, reasoning, created_at, updated_at, title_generated FROM assistant_sessions ORDER BY updated_at DESC")?;
        let rows = statement.query_map([], summary)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok::<_, rusqlite::Error>(rows)
    }).and_then(Result::ok).unwrap_or_default()
}

pub fn get(id: &str) -> Option<Value> {
    let db = database()?;
    db.with(|conn| conn.query_row(
        "SELECT id, title, model, reasoning, created_at, updated_at, title_generated, messages FROM assistant_sessions WHERE id = ?1",
        params![id.trim()], |row| {
            let mut value = summary(row)?;
            value["messages"] = serde_json::from_str(&row.get::<_, String>(7)?).unwrap_or_else(|_| json!([]));
            Ok(value)
        },
    ).optional().ok().flatten()).flatten()
}

pub fn save(payload: &Value) -> Result<Value, String> {
    let id = field(payload, "id");
    if id.is_empty() || id.chars().count() > MAX_ID_CHARS { return Err("缺少有效的会话 id".into()); }
    let messages = payload.get("messages").cloned().unwrap_or_else(|| json!([]));
    if !messages.is_array() { return Err("messages 必须是数组".into()); }
    let encoded = serde_json::to_string(&messages).map_err(|error| error.to_string())?;
    if encoded.chars().count() > MAX_MESSAGES_CHARS { return Err("这条会话的消息量过大，请开一条新会话".into()); }
    let title = truncate(field(payload, "title"), MAX_TITLE_CHARS);
    let model = (!field(payload, "model").is_empty()).then(|| field(payload, "model"));
    let reasoning = (!field(payload, "reasoning").is_empty()).then(|| field(payload, "reasoning"));
    let now = logging::now_ms();
    let db = database().ok_or("会话存储不可用")?;
    let written = db.with(|conn| conn.execute(
        "INSERT INTO assistant_sessions (id, title, messages, model, reasoning, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
         ON CONFLICT(id) DO UPDATE SET
           title = CASE WHEN assistant_sessions.title_generated = 1 THEN assistant_sessions.title ELSE excluded.title END,
           messages = excluded.messages, model = excluded.model, reasoning = excluded.reasoning, updated_at = excluded.updated_at",
        params![id, title, encoded, model, reasoning, now],
    ).is_ok());
    if written != Some(true) { return Err("会话写入失败".into()); }
    let mut value = get(id).ok_or("会话写入后无法读取")?;
    value.as_object_mut().expect("session object").remove("messages");
    Ok(value)
}

pub fn set_title(id: &str, expected_messages: &Value, title: &str) -> Result<Value, String> {
    let db = database().ok_or("会话存储不可用")?;
    let expected = serde_json::to_string(expected_messages).map_err(|error| error.to_string())?;
    db.with(|conn| conn.execute(
        "UPDATE assistant_sessions SET title = ?2, title_generated = 1 WHERE id = ?1 AND title_generated = 0 AND messages = ?3",
        params![id, truncate(title, MAX_TITLE_CHARS), expected],
    )).ok_or("会话数据库不可用")?.map_err(|_| "保存标题失败")?;
    let mut value = get(id).ok_or("会话不存在")?;
    value.as_object_mut().expect("session object").remove("messages");
    Ok(value)
}

pub fn fork(id: &str, new_id: &str, message_id: &str) -> Result<Value, String> {
    if new_id.is_empty() || new_id.chars().count() > MAX_ID_CHARS || id == new_id { return Err("分叉会话 id 无效".into()); }
    let db = database().ok_or("会话存储不可用")?;
    db.with_mut(|conn| -> Result<(), String> {
        let tx = conn.transaction().map_err(|_| "无法开始会话复制")?;
        let (title, raw, model, reasoning): (String, String, Option<String>, Option<String>) = tx.query_row(
            "SELECT title, messages, model, reasoning FROM assistant_sessions WHERE id = ?1", params![id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).map_err(|_| "原会话不存在")?;
        let mut messages: Value = serde_json::from_str(&raw).map_err(|_| "原会话消息无效")?;
        let items = messages.as_array_mut().ok_or("原会话消息无效")?;
        if !message_id.is_empty() {
            let index = items.iter().position(|message| field(message, "id") == message_id).ok_or("分叉消息不存在")?;
            if field(&items[index], "role") != "assistant" { return Err("只能从模型消息分叉".into()); }
            let calls = items[index].get("toolCalls").and_then(Value::as_array).cloned().unwrap_or_default();
            let mut end = index + 1;
            while end < items.len() && field(&items[end], "role") == "tool"
                && calls.iter().any(|call| field(call, "id") == field(&items[end], "toolCallId")) { end += 1; }
            items.truncate(end);
        }
        let mut replacements = HashMap::new();
        collect_image_ids(&messages, &mut replacements);
        let used: i64 = tx.query_row("SELECT COALESCE(SUM(length(bytes)), 0) FROM assistant_images", [], |row| row.get(0)).map_err(|_| "无法读取图片用量")?;
        let mut bytes_added = 0i64;
        let now = logging::now_ms();
        tx.execute("INSERT INTO assistant_sessions (id, title, messages, model, reasoning, created_at, updated_at, title_generated) VALUES (?1, ?2, '[]', ?3, ?4, ?5, ?5, 1)",
            params![new_id, truncate(&format!("{title} · 分叉"), MAX_TITLE_CHARS), model, reasoning, now]).map_err(|_| "新会话 id 已存在或无法创建")?;
        for (old, new) in &mut replacements {
            let (mime, bytes): (String, Vec<u8>) = tx.query_row("SELECT mime, bytes FROM assistant_images WHERE id = ?1 AND session_id = ?2", params![old, id],
                |row| Ok((row.get(0)?, row.get(1)?))).map_err(|_| "原会话的图片已不存在，无法完整复制")?;
            bytes_added = bytes_added.saturating_add(bytes.len() as i64);
            if used.saturating_add(bytes_added) > 512 * 1024 * 1024 { return Err("助手图片存储已满，无法复制会话".into()); }
            let mut random = [0u8; 32];
            getrandom::getrandom(&mut random).map_err(|_| "无法生成图片 id")?;
            *new = random.iter().map(|byte| format!("{byte:02x}")).collect();
            tx.execute("INSERT INTO assistant_images (id, session_id, mime, bytes, created_at) VALUES (?1, ?2, ?3, ?4, ?5)", params![new.as_str(), new_id, mime, bytes, now]).map_err(|_| "图片复制失败")?;
        }
        replace_image_ids(&mut messages, &replacements);
        tx.execute("UPDATE assistant_sessions SET messages = ?2 WHERE id = ?1", params![new_id, messages.to_string()]).map_err(|_| "保存复制消息失败")?;
        tx.commit().map_err(|_| "会话复制失败".to_string())
    }).ok_or("会话数据库不可用")??;
    get(new_id).ok_or_else(|| "会话复制后无法读取".into())
}

fn collect_image_ids(value: &Value, ids: &mut HashMap<String, String>) {
    match value {
        Value::Object(object) => {
            if let Some(id) = object.get("id").and_then(Value::as_str).filter(|id| id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())) {
                if object.get("mime").and_then(Value::as_str).is_some_and(|mime| mime.starts_with("image/")) { ids.insert(id.to_string(), String::new()); }
            }
            for child in object.values() { collect_image_ids(child, ids); }
        }
        Value::Array(items) => for child in items { collect_image_ids(child, ids); },
        _ => {}
    }
}

fn replace_image_ids(value: &mut Value, ids: &HashMap<String, String>) {
    match value {
        Value::String(text) => for (old, new) in ids { *text = text.replace(old, new); },
        Value::Object(object) => for child in object.values_mut() { replace_image_ids(child, ids); },
        Value::Array(items) => for child in items { replace_image_ids(child, ids); },
        _ => {}
    }
}

pub fn remove(id: &str) -> bool {
    database().and_then(|db| db.with(|conn| conn.execute("DELETE FROM assistant_sessions WHERE id = ?1", params![id.trim()]).map(|count| count > 0).unwrap_or(false))).unwrap_or(false)
}

pub fn clear() -> usize {
    database().and_then(|db| db.with(|conn| conn.execute("DELETE FROM assistant_sessions", []).unwrap_or(0))).unwrap_or(0)
}

/// 启动期后缀迁移：把历史会话上记录的模型名改成原始 ID。
/// 只改显示/续聊用的名字，不碰消息正文；返回被改动的行数。
pub fn migrate_model_ids(mapping: &HashMap<String, String>) -> usize {
    let Some(db) = database() else { return 0 };
    let mut changed = 0;
    for (old, new) in mapping {
        if old.eq_ignore_ascii_case(new) {
            continue;
        }
        let updated = db
            .with(|conn| {
                conn.execute(
                    "UPDATE assistant_sessions SET model = ?1 WHERE model = ?2 COLLATE NOCASE",
                    params![new, old],
                )
            })
            .and_then(Result::ok)
            .unwrap_or(0);
        changed += updated;
    }
    changed
}
