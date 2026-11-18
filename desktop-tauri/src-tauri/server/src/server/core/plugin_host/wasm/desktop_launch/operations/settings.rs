use std::collections::BTreeMap;

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::server::db::Db;

const KEY: &str = "pluginDesktopClients";

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setting {
    pub path: String,
}

type Settings = BTreeMap<String, BTreeMap<String, Setting>>;

pub fn read(db: &Db, provider: &str, client: &str) -> Result<Setting, String> {
    db.with(|conn| {
        let text: Option<String> = conn.query_row("SELECT value FROM kv WHERE key=?1", [KEY], |row| row.get(0)).optional().map_err(|_| "本机客户端设置不可读")?;
        let settings: Settings = text.map(|text| serde_json::from_str(&text)).transpose().map_err(|_| "本机客户端设置格式无效")?.unwrap_or_default();
        Ok(settings.get(provider).and_then(|clients| clients.get(client)).cloned().unwrap_or_default())
    }).ok_or("本机客户端设置数据库不可用")?
}

pub fn save(db: &Db, provider: &str, client: &str, setting: Setting, expected_path: &str) -> Result<(), String> {
    db.with_mut(|conn| {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|_| "本机客户端设置无法保存")?;
        let text: Option<String> = tx.query_row("SELECT value FROM kv WHERE key=?1", [KEY], |row| row.get(0)).optional().map_err(|_| "本机客户端设置不可读")?;
        let mut settings: Settings = text.map(|text| serde_json::from_str(&text)).transpose().map_err(|_| "本机客户端设置格式无效")?.unwrap_or_default();
        let current = settings.get(provider).and_then(|clients| clients.get(client)).map(|value| value.path.as_str()).unwrap_or("");
        if current != expected_path { return Err("本机客户端路径设置已变化，请重新确认".into()); }
        settings.entry(provider.into()).or_default().insert(client.into(), setting);
        let value = serde_json::to_string(&settings).map_err(|_| "本机客户端设置无法编码")?;
        if value.len() > 256 * 1024 { return Err("本机客户端设置过多".into()); }
        tx.execute("INSERT INTO kv (key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [KEY, &value]).map_err(|_| "本机客户端设置保存失败")?;
        tx.commit().map_err(|_| "本机客户端设置保存结果未确认".to_string())
    }).ok_or("本机客户端设置数据库不可用")?
}

pub fn revision(package: &Value, path: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(serde_json::json!([package, path]).to_string().as_bytes()))
}
