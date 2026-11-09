use std::collections::BTreeMap;

use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::{Zeroize, Zeroizing};

use crate::server::core::crypto::{self, SecretScope};

const SCOPES: [SecretScope; 5] = [
    SecretScope::Accounts,
    SecretScope::UpdateToken,
    SecretScope::CloudSync,
    SecretScope::CodingSessions,
    SecretScope::PanelCaptcha,
];

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct KeyArtifact {
    key: String,
    fingerprint: String,
}

impl Drop for KeyArtifact {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

pub(super) struct BackupKeys {
    values: BTreeMap<String, Zeroizing<[u8; 32]>>,
}

impl BackupKeys {
    pub fn collect() -> Result<Self, String> {
        let mut values = BTreeMap::new();
        for scope in SCOPES {
            if let Some(key) = crypto::backup_key(scope)? {
                values.insert(scope.key_file().to_string(), Zeroizing::new(key));
            }
        }
        Ok(Self { values })
    }

    pub fn artifacts(&self) -> BTreeMap<String, KeyArtifact> {
        self.values
            .iter()
            .map(|(name, key)| {
                (
                    name.clone(),
                    KeyArtifact {
                        key: STANDARD.encode(key.as_ref()),
                        fingerprint: crypto::fingerprint_of(key.as_ref()),
                    },
                )
            })
            .collect()
    }

    pub fn from_artifacts(artifacts: BTreeMap<String, KeyArtifact>) -> Result<Self, String> {
        if artifacts.len() > SCOPES.len() {
            return Err("备份包含不支持的密钥域".into());
        }
        let mut values = BTreeMap::new();
        for (name, artifact) in artifacts {
            if !SCOPES.iter().any(|scope| scope.key_file() == name) {
                return Err("备份包含不支持的密钥域".into());
            }
            let bytes = Zeroizing::new(
                STANDARD
                    .decode(&artifact.key)
                    .map_err(|_| "备份密钥编码损坏".to_string())?,
            );
            let key: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| "备份密钥长度异常".to_string())?;
            if crypto::fingerprint_of(&key) != artifact.fingerprint {
                return Err("备份密钥指纹不匹配，文件可能已损坏".into());
            }
            values.insert(name, Zeroizing::new(key));
        }
        Ok(Self { values })
    }

    pub fn names(&self) -> Vec<String> {
        self.values.keys().cloned().collect()
    }

    fn decrypt(
        &self,
        value: &str,
        scope: SecretScope,
        field: &str,
    ) -> Result<Zeroizing<Vec<u8>>, String> {
        let key = self
            .values
            .get(scope.key_file())
            .ok_or_else(|| format!("备份缺少 {}，无法恢复{field}", scope.key_file()))?;
        crypto::open_with_key(value, key)
            .map(Zeroizing::new)
            .map_err(|_| format!("{field}无法解密，数据与 {} 不匹配", scope.key_file()))
    }
}

struct SecretTransform<'a> {
    source: &'a BackupKeys,
    rewrite: bool,
    local: BTreeMap<String, Zeroizing<[u8; 32]>>,
    count: usize,
}

impl SecretTransform<'_> {
    fn value(
        &mut self,
        value: &mut Value,
        scope: SecretScope,
        field: &str,
        json: bool,
    ) -> Result<(), String> {
        let Some(text) = value.as_str() else {
            return Err(format!("{field}格式异常"));
        };
        if text.is_empty() {
            return Ok(());
        }
        let encrypted = crypto::is_envelope(text);
        if !encrypted && !json {
            return Ok(());
        }
        let plain = if encrypted {
            self.source.decrypt(text, scope, field)?
        } else {
            Zeroizing::new(text.as_bytes().to_vec())
        };
        if json {
            let parsed: Value =
                serde_json::from_slice(&plain).map_err(|_| format!("{field}内容不是合法 JSON"))?;
            if !parsed.is_object() {
                return Err(format!("{field}内容不是对象"));
            }
        }
        if !self.rewrite {
            return Ok(());
        }
        let name = scope.key_file().to_string();
        if !self.local.contains_key(&name) {
            self.local
                .insert(name.clone(), Zeroizing::new(crypto::secret_key(scope)?));
        }
        let key = self.local.get(&name).ok_or("目标机器密钥不可用")?;
        *value = Value::String(crypto::seal_with_key(&plain, key)?);
        self.count += 1;
        Ok(())
    }

    fn field(
        &mut self,
        object: &mut Value,
        name: &str,
        scope: SecretScope,
        label: &str,
        json: bool,
    ) -> Result<(), String> {
        if let Some(value) = object.get_mut(name) {
            self.value(value, scope, label, json)?;
        }
        Ok(())
    }

    fn config(&mut self, name: &str, value: &mut Value) -> Result<(), String> {
        match name {
            "apiKey" => self.value(value, SecretScope::Accounts, "网关 Key", false)?,
            "githubToken" => self.value(value, SecretScope::UpdateToken, "GitHub 令牌", false)?,
            "apiKeys" => {
                let items = value.as_array_mut().ok_or("网关 Key 列表格式异常")?;
                for item in items {
                    self.field(item, "key", SecretScope::Accounts, "网关 Key", false)?;
                }
            }
            "agentProfiles" => {
                let clients = value.as_object_mut().ok_or("Agent 方案格式异常")?;
                for profiles in clients.values_mut() {
                    let profiles = profiles.as_array_mut().ok_or("Agent 方案列表格式异常")?;
                    for profile in profiles {
                        self.field(
                            profile,
                            "apiKey",
                            SecretScope::Accounts,
                            "Agent 方案 Key",
                            false,
                        )?;
                        if profile.get("native").is_some_and(Value::is_string) {
                            self.field(
                                profile,
                                "native",
                                SecretScope::Accounts,
                                "Agent 原生方案",
                                true,
                            )?;
                        }
                    }
                }
            }
            "panelCaptcha" => {
                self.field(value, "secretKey", SecretScope::PanelCaptcha, "Turnstile 服务端密钥", false)?;
            }
            "assistantCoding" => {
                self.field(value, "accessToken", SecretScope::CodingSessions, "编程会话连接密钥", false)?;
            }
            "cloudSync" => {
                if !value.is_object() {
                    return Err("云同步设置格式异常".into());
                }
                self.field(
                    value,
                    "syncPassword",
                    SecretScope::CloudSync,
                    "云同步密码",
                    false,
                )?;
                if let Some(webdav) = value.get_mut("webdav") {
                    self.field(
                        webdav,
                        "password",
                        SecretScope::CloudSync,
                        "WebDAV 密码",
                        false,
                    )?;
                }
                if let Some(s3) = value.get_mut("s3") {
                    self.field(
                        s3,
                        "secretAccessKey",
                        SecretScope::CloudSync,
                        "S3 凭据",
                        false,
                    )?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

pub(super) fn validate(conn: &Connection, source: &BackupKeys) -> Result<(), String> {
    transform(conn, source, false).map(|_| ())
}

pub(super) fn reencrypt(conn: &mut Connection, source: &BackupKeys) -> Result<usize, String> {
    conn.set_db_config(
        rusqlite::config::DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER,
        false,
    )
    .map_err(|error| format!("禁用暂存库触发器失败: {error}"))?;
    let transaction = conn
        .transaction()
        .map_err(|error| format!("开启凭证迁移事务失败: {error}"))?;
    let count = transform(&transaction, source, true)?;
    transaction
        .commit()
        .map_err(|error| format!("提交凭证迁移失败: {error}"))?;
    Ok(count)
}

fn transform(conn: &Connection, source: &BackupKeys, rewrite: bool) -> Result<usize, String> {
    let mut transform = SecretTransform {
        source,
        rewrite,
        local: BTreeMap::new(),
        count: 0,
    };
    let mut last_rowid = None;
    loop {
        let row: Option<(i64, String)> = conn.query_row(
            "SELECT rowid, data FROM accounts WHERE ?1 IS NULL OR rowid > ?1 ORDER BY rowid LIMIT 1",
            [last_rowid],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional().map_err(|error| format!("读取账号凭证失败: {error}"))?;
        let Some((id, text)) = row else { break };
        last_rowid = Some(id);
        let mut value = Value::String(text);
        transform.value(&mut value, SecretScope::Accounts, "账号凭证", true)?;
        if rewrite {
            conn.execute(
                "UPDATE accounts SET data=?1 WHERE rowid=?2",
                params![value.as_str(), id],
            )
            .map_err(|error| format!("迁移账号凭证失败: {error}"))?;
        }
    }
    for name in [
        "apiKey",
        "apiKeys",
        "githubToken",
        "agentProfiles",
        "assistantCoding",
        "panelCaptcha",
        "cloudSync",
    ] {
        let text: Option<String> = conn
            .query_row("SELECT value FROM kv WHERE key=?1", [name], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|error| format!("读取加密配置失败: {error}"))?;
        let Some(text) = text else { continue };
        let mut value: Value =
            serde_json::from_str(&text).map_err(|_| format!("配置项 {name} 格式异常"))?;
        transform.config(name, &mut value)?;
        if rewrite {
            let serialized = serde_json::to_string(&value).map_err(|_| "序列化加密配置失败")?;
            conn.execute(
                "UPDATE kv SET value=?1 WHERE key=?2",
                params![serialized, name],
            )
            .map_err(|error| format!("迁移加密配置失败: {error}"))?;
        }
    }
    Ok(transform.count)
}
