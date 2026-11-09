use std::path::{Path, PathBuf};

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::server::db::Db;
use super::{projects, CodingError};

pub const KV_KEY: &str = "assistantCodexLocal";

#[derive(Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct CodingConfig {
    pub enabled: bool,
    pub executable: String,
    pub codex_home: String,
    pub workspace_roots: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigInput {
    pub enabled: bool,
    #[serde(default)]
    pub executable: String,
    #[serde(default)]
    pub codex_home: String,
}

impl CodingConfig {
    pub fn public(&self) -> Value {
        json!({ "enabled": self.enabled, "executable": self.executable, "codexHome": self.codex_home,
            "revision": self.revision(), "runtime": "local-codex" })
    }

    pub fn revision(&self) -> String {
        format!("{:x}", Sha256::digest(serde_json::to_vec(&json!({ "enabled": self.enabled, "executable": self.executable, "codexHome": self.codex_home })).unwrap_or_default()))
    }

    pub fn home(&self) -> PathBuf {
        if !self.codex_home.is_empty() { return PathBuf::from(&self.codex_home); }
        std::env::var_os("CODEX_HOME").filter(|value| !value.is_empty()).map(PathBuf::from)
            .unwrap_or_else(|| user_home().join(".codex"))
    }

    pub fn scope(&self) -> String {
        let home = self.home();
        let canonical = std::fs::canonicalize(&home).unwrap_or(home);
        let text = canonical.to_string_lossy();
        let text = if cfg!(windows) { text.to_lowercase() } else { text.into_owned() };
        format!("{:x}", Sha256::digest(text.as_bytes()))
    }
}

pub fn user_home() -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn absolute_path(raw: &str) -> Result<PathBuf, CodingError> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 4096 || raw.chars().any(char::is_control) { return Err(CodingError::new(400, "本机路径格式无效")); }
    let path = if raw == "~" { user_home() }
        else if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) { user_home().join(rest) }
        else { PathBuf::from(raw) };
    if !path.is_absolute() { return Err(CodingError::new(400, "请填写本机的绝对路径")); }
    Ok(path)
}

pub fn canonical_directory(raw: &str) -> Result<PathBuf, CodingError> {
    let path = absolute_path(raw)?;
    let path = std::fs::canonicalize(path).map_err(|_| CodingError::new(400, "目录不存在或当前运行账户无权访问"))?;
    if !path.is_dir() { return Err(CodingError::new(400, "请选择文件夹，而不是文件")); }
    Ok(path)
}

pub fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        if let Some(rest) = text.strip_prefix("\\\\?\\UNC\\") { return format!("\\\\{rest}"); }
        if let Some(rest) = text.strip_prefix("\\\\?\\") { return rest.to_string(); }
    }
    text.into_owned()
}

pub fn load(db: &Db) -> Result<CodingConfig, CodingError> {
    db.with_mut(|conn| {
        let tx = conn.transaction().map_err(|_| CodingError::new(500, "读取本机 Codex 设置失败"))?;
        let text: Option<String> = tx.query_row("SELECT value FROM kv WHERE key=?1", [KV_KEY], |row| row.get(0)).optional()
            .map_err(|_| CodingError::new(500, "读取本机 Codex 设置失败"))?;
        let mut config: CodingConfig = match text {
            Some(text) => serde_json::from_str(&text).map_err(|_| CodingError::new(500, "本机 Codex 设置已损坏，未覆盖原数据"))?,
            None => CodingConfig::default(),
        };
        if !config.workspace_roots.is_empty() {
            projects::migrate_legacy(&tx, &config)?;
            config.workspace_roots.clear();
            let text = serde_json::to_string(&config).map_err(|_| CodingError::new(500, "保存本机 Codex 设置失败"))?;
            tx.execute("UPDATE kv SET value=?2 WHERE key=?1", params![KV_KEY, text]).map_err(|_| CodingError::new(500, "保存本机 Codex 设置失败"))?;
        }
        tx.commit().map_err(|_| CodingError::new(500, "读取本机 Codex 设置失败"))?;
        Ok(config)
    }).ok_or_else(|| CodingError::new(500, "数据库不可用"))?
}

pub fn prepare(input: ConfigInput) -> Result<CodingConfig, CodingError> {
    let executable = if input.executable.trim().is_empty() { String::new() }
        else {
            let path = absolute_path(&input.executable)?;
            if !path.is_file() { return Err(CodingError::new(400, "Codex 程序不存在，请选择本机已安装的 Codex")); }
            display_path(&path)
        };
    let codex_home = if input.codex_home.trim().is_empty() { String::new() }
        else { display_path(&canonical_directory(&input.codex_home)?) };
    let config = CodingConfig { enabled: input.enabled, executable, codex_home, workspace_roots: Vec::new() };
    if config.enabled { super::executable::resolve(&config)?; }
    Ok(config)
}

pub fn save(db: &Db, config: &CodingConfig) -> Result<(), CodingError> {
    let text = serde_json::to_string(config).map_err(|_| CodingError::new(500, "保存本机 Codex 设置失败"))?;
    db.with(|conn| conn.execute("INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![KV_KEY, text]))
        .ok_or_else(|| CodingError::new(500, "数据库不可用"))?.map_err(|_| CodingError::new(500, "保存本机 Codex 设置失败"))?;
    Ok(())
}
