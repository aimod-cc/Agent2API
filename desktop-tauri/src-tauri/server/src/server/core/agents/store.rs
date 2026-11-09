use rusqlite::OptionalExtension;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::server::core::crypto::{self, SecretScope};
use crate::server::{access, db::Db};

use super::{types::NativeConfig, Profile};

const KEY: &str = "agentProfiles";

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn client_revision(value: Option<&Value>) -> String {
    digest(
        value
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()))
            .to_string()
            .as_bytes(),
    )
}

pub fn load_client(client: &str) -> Result<Vec<Profile>, String> {
    load_with_revision(client).map(|(profiles, _)| profiles)
}

pub fn load_with_revision(client: &str) -> Result<(Vec<Profile>, String), String> {
    let text = access::kv_get(KEY);
    let value: Value = text
        .map(|text| serde_json::from_str(&text).map_err(|e| format!("已保存方案无法读取: {e}")))
        .transpose()?
        .unwrap_or_else(|| serde_json::json!({}));
    if !value.is_object() {
        return Err("已保存方案存储不是对象，拒绝覆盖".into());
    }
    let revision = client_revision(value.get(client));
    let Some(items) = value.get(client) else {
        return Ok((Vec::new(), revision));
    };
    let items = items
        .as_array()
        .ok_or("已保存方案不是数组，已停止写入以免丢失数据")?;
    Ok((
        items.iter().map(decode).collect::<Result<_, _>>()?,
        revision,
    ))
}

fn decode(value: &Value) -> Result<Profile, String> {
    let mut value = value.clone();
    if let Some(object) = value.as_object_mut() {
        if let Some(secret) = object
            .get("apiKey")
            .and_then(Value::as_str)
            .filter(|key| crypto::is_envelope(key))
        {
            let plain = crypto::open(secret, SecretScope::Accounts)?;
            let key = String::from_utf8(plain).map_err(|_| "方案密钥无法解码")?;
            object.insert("apiKey".into(), Value::String(key));
        }
        if let Some(envelope) = object.get("native").and_then(Value::as_str) {
            let bytes = crypto::open(envelope, SecretScope::Accounts)?;
            let native = serde_json::from_slice::<Value>(&bytes)
                .map_err(|e| format!("原生方案无法解码: {e}"))?;
            object.insert("native".into(), native);
        }
    }
    let mut profile: Profile =
        serde_json::from_value(value).map_err(|e| format!("已保存方案字段无法读取: {e}"))?;
    if profile.auth_mode.is_empty() {
        profile.auth_mode = if profile.kind == "gateway" {
            "gateway"
        } else {
            "preserve"
        }
        .into();
    }
    Ok(profile)
}

fn encode(profile: &Profile) -> Result<Value, String> {
    let mut value = serde_json::to_value(profile).map_err(|e| format!("方案序列化失败: {e}"))?;
    let object = value.as_object_mut().ok_or("方案序列化不是对象")?;
    if !profile.api_key.is_empty() {
        object.insert(
            "apiKey".into(),
            Value::String(crypto::seal(
                profile.api_key.as_bytes(),
                SecretScope::Accounts,
            )?),
        );
    }
    let native =
        serde_json::to_vec(&profile.native).map_err(|e| format!("原生方案序列化失败: {e}"))?;
    object.insert(
        "native".into(),
        Value::String(crypto::seal(&native, SecretScope::Accounts)?),
    );
    Ok(value)
}

pub fn save_client(
    db: Option<&Db>,
    client: &str,
    profiles: &[Profile],
    expected: &str,
) -> Result<(), String> {
    save_client_with(db, client, profiles, expected, || Ok(()))
}

pub fn save_client_with<T>(
    db: Option<&Db>,
    client: &str,
    profiles: &[Profile],
    expected: &str,
    apply: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let db = db.ok_or("数据库不可用，无法保存方案")?;
    let encoded: Vec<Value> = profiles.iter().map(encode).collect::<Result<_, _>>()?;
    let result = db.with_mut(|connection| -> Result<T, String> {
        let transaction = connection.transaction().map_err(|e| format!("开启方案事务失败: {e}"))?;
        let text: Option<String> = transaction.query_row("SELECT value FROM kv WHERE key = ?1", [KEY], |row| row.get(0))
            .optional().map_err(|e| format!("读取方案失败: {e}"))?;
        let mut all: Map<String, Value> = match text {
            Some(text) => serde_json::from_str(&text).map_err(|e| format!("方案存储已损坏，拒绝覆盖: {e}"))?,
            None => Map::new(),
        };
        if expected.is_empty() || client_revision(all.get(client)) != expected {
            return Err("方案列表已被其他窗口修改，请重新检测后再保存".into());
        }
        all.insert(client.into(), Value::Array(encoded));
        let text = serde_json::to_string(&all).map_err(|e| format!("生成方案失败: {e}"))?;
        transaction.execute("INSERT INTO kv(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", rusqlite::params![KEY, text])
            .map_err(|e| format!("保存方案失败: {e}"))?;
        let result = apply()?;
        transaction.commit().map_err(|e| format!("配置列表保存失败: {e}；如已写入客户端，请重新检测，原文件备份仍保留"))?;
        Ok(result)
    }).ok_or("数据库锁不可用")??;
    crate::server::core::cloud_sync::auto_sync::notify_change();
    Ok(result)
}

pub fn from_request(value: &Value, old: Option<&Profile>) -> Result<Profile, String> {
    let mut profile: Profile =
        serde_json::from_value(value.clone()).map_err(|e| format!("方案字段无效: {e}"))?;
    profile.id = profile.id.trim().into();
    profile.name = profile.name.trim().into();
    profile.base_url = profile.base_url.trim().trim_end_matches('/').into();
    profile.model = profile.model.trim().into();
    profile.env_key = profile.env_key.trim().into();
    for model in &mut profile.models {
        model.id = model.id.trim().into();
        model.name = model.name.trim().into();
    }
    // 原生结构与凭证只能由已保存记录或客户端导入提供，不能信任请求里的隐藏字段。
    profile.native = old
        .map(|p| p.native.clone())
        .unwrap_or_else(|| NativeConfig {
            schema: 2,
            ..Default::default()
        });
    if profile.api_key.is_empty()
        && !value
            .get("clearApiKey")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        profile.api_key = old.map(|p| p.api_key.clone()).unwrap_or_default();
    }
    if profile.auth_mode.is_empty() {
        profile.auth_mode = "preserve".into();
    }
    Ok(profile)
}
