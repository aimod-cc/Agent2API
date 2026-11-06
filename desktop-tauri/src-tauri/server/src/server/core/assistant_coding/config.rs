use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::{Mutex, OnceLock};
use url::Url;
use zeroize::Zeroizing;

use crate::server::core::crypto::{self, SecretScope};
use crate::server::db::Db;
use super::CodingError;

pub const KV_KEY: &str = "assistantCoding";

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CodingConfig {
    pub enabled: bool,
    pub hub_url: String,
    pub access_token: String,
}

impl Default for CodingConfig {
    fn default() -> Self {
        Self { enabled: false, hub_url: "http://127.0.0.1:3006".into(), access_token: String::new() }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigInput {
    pub enabled: bool,
    pub hub_url: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub clear_token: bool,
}

impl CodingConfig {
    pub fn public(&self) -> Value {
        json!({ "enabled": self.enabled, "hubUrl": self.hub_url, "hasToken": !self.access_token.is_empty(), "revision": self.revision() })
    }

    pub fn revision(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(self.hub_url.as_bytes());
        hash.update([0]);
        hash.update(self.access_token.as_bytes());
        format!("{:x}", hash.finalize())
    }

    pub fn token(&self) -> Result<Zeroizing<String>, CodingError> {
        let bytes = Zeroizing::new(crypto::open(&self.access_token, SecretScope::CodingSessions)
            .map_err(|_| CodingError::new(400, "编程会话连接密钥无法解密，请重新保存"))?);
        String::from_utf8(bytes.to_vec()).map(Zeroizing::new)
            .map_err(|_| CodingError::new(400, "编程会话连接密钥格式异常，请重新保存"))
    }
}

pub fn load(db: &Db) -> Result<CodingConfig, CodingError> {
    let text: Option<String> = db.with(|conn| conn.query_row(
        "SELECT value FROM kv WHERE key=?1", [KV_KEY], |row| row.get(0),
    ).optional()).ok_or_else(|| CodingError::new(500, "数据库不可用"))?
        .map_err(|_| CodingError::new(500, "读取编程会话设置失败"))?;
    match text {
        Some(text) => serde_json::from_str(&text).map_err(|_| CodingError::new(500, "编程会话设置已损坏，未覆盖原数据")),
        None => Ok(CodingConfig::default()),
    }
}

pub fn save(db: &Db, input: ConfigInput) -> Result<CodingConfig, CodingError> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner());
    let mut next = load(db)?;
    let url = validate_url(&input.hub_url)?;
    let token = Zeroizing::new(input.access_token.trim().to_string());
    if token.len() > 4096 || token.chars().any(char::is_control) {
        return Err(CodingError::new(400, "连接密钥格式不正确"));
    }
    // 地址变化不能把上一台后台的凭证自动发送给新地址。
    if url != next.hub_url || input.clear_token { next.access_token.clear(); }
    if !token.is_empty() { next.access_token = crypto::seal(token.as_bytes(), SecretScope::CodingSessions)?; }
    next.hub_url = url;
    next.enabled = input.enabled;
    if next.enabled && next.access_token.is_empty() {
        return Err(CodingError::new(400, "启用编程会话前，请填写 HAPI 连接密钥"));
    }
    let text = serde_json::to_string(&next).map_err(|_| CodingError::new(500, "保存编程会话设置失败"))?;
    db.with(|conn| conn.execute("INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![KV_KEY, text])).ok_or_else(|| CodingError::new(500, "数据库不可用"))?
        .map_err(|_| CodingError::new(500, "保存编程会话设置失败"))?;
    Ok(next)
}

pub fn validate_url(input: &str) -> Result<String, CodingError> {
    let mut url = Url::parse(input.trim()).map_err(|_| CodingError::new(400, "请输入完整的 HAPI 后台地址"))?;
    let loopback = match url.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(CodingError::new(400, "远程 HAPI 地址必须使用 HTTPS；只有本机回环地址可使用 HTTP"));
    }
    if !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some()
        || url.host_str().is_none() || url.as_str().len() > 2048 {
        return Err(CodingError::new(400, "后台地址不能包含账号、密码、查询参数或片段"));
    }
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(&path);
    Ok(url.to_string().trim_end_matches('/').to_string())
}
