//! 手动代理的连通性测试设置，存于 kv 的 `proxySettings` 保留键。
//! `testUrl` 为空时沿用网关默认目标；旧内核端口配置不再消费。

use std::sync::OnceLock;

use serde_json::{json, Value};

use crate::server::db::Db;

pub const KV_KEY: &str = "proxySettings";
pub const DEFAULT_TEST_URL: &str = "https://www.google.com/";
const MAX_URL_LENGTH: usize = 2048;
static DB: OnceLock<Option<Db>> = OnceLock::new();

pub fn install(db: Option<Db>) {
    let _ = DB.set(db);
}

fn read_raw() -> Value {
    let Some(db) = DB.get().and_then(Option::as_ref) else {
        return json!({});
    };
    db.with(|conn| {
        conn.query_row("SELECT value FROM kv WHERE key = ?1", [KV_KEY], |row| {
            row.get::<_, String>(0)
        }).ok()
    })
    .flatten()
    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
    .filter(Value::is_object)
    .unwrap_or_else(|| json!({}))
}

fn write_raw(value: &Value) -> Result<(), String> {
    let Some(db) = DB.get().and_then(Option::as_ref) else {
        return Err("设置数据库不可用".to_string());
    };
    let text = serde_json::to_string(value).map_err(|error| format!("编码代理设置失败: {error}"))?;
    db.with(|conn| {
        conn.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![KV_KEY, text],
        )
        .map(|_| ())
        .map_err(|error| format!("保存代理设置失败: {error}"))
    })
    .ok_or_else(|| "设置数据库不可用".to_string())??;
    crate::server::core::cloud_sync::auto_sync::notify_change();
    Ok(())
}

pub fn test_url() -> Option<String> {
    let text = read_raw().get("testUrl").and_then(Value::as_str)
        .unwrap_or("").trim().to_string();
    (!text.is_empty()).then_some(text)
}

pub fn get() -> Value {
    json!({
        "testUrl": read_raw().get("testUrl").and_then(Value::as_str).unwrap_or(""),
        "defaults": { "testUrl": DEFAULT_TEST_URL },
    })
}

pub fn save(input: &Value) -> Result<Value, String> {
    if !input.is_object() {
        return Err("代理设置必须是对象".to_string());
    }
    let mut next = read_raw();
    if let Some(value) = input.get("testUrl") {
        let text = value.as_str().ok_or_else(|| "测试网址必须是字符串".to_string())?.trim();
        if !text.is_empty() {
            let url = url::Url::parse(text).map_err(|_| "测试网址无效".to_string())?;
            if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                return Err("测试网址要以 http:// 或 https:// 开头".to_string());
            }
            if text.chars().count() > MAX_URL_LENGTH {
                return Err("测试网址过长".to_string());
            }
        }
        next["testUrl"] = json!(text);
    }
    write_raw(&next)?;
    crate::server::logging::log("[Proxies]", "✅ 已更新连通性测试设置");
    Ok(get())
}
