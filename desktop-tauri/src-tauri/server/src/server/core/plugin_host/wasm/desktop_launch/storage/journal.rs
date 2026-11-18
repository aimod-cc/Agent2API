use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;
use zeroize::Zeroizing;

use super::Edit;
use crate::server::core::{agents::files, crypto::{self, SecretScope}};

#[derive(Serialize, Deserialize)]
pub struct Journal {
    pub version: u32,
    pub id: String,
    pub provider: String,
    pub account: String,
    pub client: String,
    pub executable: PathBuf,
    pub package: serde_json::Value,
    pub phase: String,
    pub was_running: bool,
    pub edits: Vec<Edit>,
    #[serde(default)]
    pub attempted: usize,
}

fn directory() -> PathBuf { crate::paths::config_dir().join("desktop-client-backups") }

fn valid(id: &str) -> bool { id.len() == 48 && id.bytes().all(|byte| byte.is_ascii_hexdigit()) }

fn path(id: &str) -> Result<PathBuf, String> {
    if !valid(id) { return Err("本机启动备份标识无效".into()); }
    Ok(directory().join(format!("{id}.json")))
}

impl Journal {
    pub fn recoverable(&self) -> bool {
        self.edits[..self.attempted].iter().any(|edit| edit.before != edit.after)
    }

    pub fn save(&self) -> Result<(), String> {
        let bytes = Zeroizing::new(serde_json::to_vec(self).map_err(|_| "本机启动备份无法编码")?);
        let encrypted = crypto::seal(&bytes, SecretScope::DesktopLaunch)?;
        let path = path(&self.id)?;
        super::super::paths::checked(&path, true)?;
        files::write_atomic(&path, &serde_json::to_vec(&json!({ "encrypted": encrypted })).map_err(|_| "本机启动备份无法编码")?)
    }

    pub fn read(id: &str) -> Result<Self, String> {
        let path = path(id)?;
        crate::server::core::plugin_host::wasm::desktop_launch::paths::checked(&path, false)?;
        let bytes = files::read_limited(&path, 8 * 1024 * 1024)?.ok_or("本机启动备份不存在")?;
        let envelope: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| "本机启动备份损坏")?;
        let bytes = Zeroizing::new(crypto::open(envelope["encrypted"].as_str().ok_or("本机启动备份未加密，拒绝恢复")?, SecretScope::DesktopLaunch)?);
        let value: Self = serde_json::from_slice(&bytes).map_err(|_| "本机启动备份格式不支持")?;
        if value.version != 1 || value.id != id || value.edits.is_empty() || value.edits.len() > 8 || value.attempted > value.edits.len() {
            return Err("本机启动备份格式不支持".into());
        }
        Ok(value)
    }

    pub fn pending(executable: &Path, targets: &std::collections::BTreeMap<String, super::Target>) -> Result<Option<String>, String> {
        let entries = match std::fs::read_dir(directory()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("本机启动备份目录不可读".into()),
        };
        for entry in entries {
            let entry = entry.map_err(|_| "本机启动备份目录不可读")?;
            let name = entry.file_name();
            let Some(id) = name.to_str().and_then(|name| name.strip_suffix(".json")) else { continue; };
            if !valid(id) { continue; }
            let journal = Self::read(id)?;
            let shared = journal.edits.iter().any(|edit| targets.values().any(|target| edit.target.same_resource(target)));
            if (super::super::paths::same(&journal.executable, executable) || shared) && journal.recoverable()
                && matches!(journal.phase.as_str(), "prepared" | "writing" | "committed" | "restoring" | "recovery_required") { return Ok(Some(id.into())); }
        }
        Ok(None)
    }
}

pub fn pending_list() -> Result<Vec<Journal>, String> {
    let entries = match std::fs::read_dir(directory()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("本机启动备份目录不可读".into()),
    };
    let mut result = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| "本机启动备份目录不可读")?;
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(|name| name.strip_suffix(".json")).filter(|id| valid(id)) else { continue; };
        let journal = Journal::read(id)?;
        if journal.recoverable() && matches!(journal.phase.as_str(), "prepared" | "writing" | "committed" | "restoring" | "recovery_required") { result.push(journal); }
        if result.len() >= 64 { break; }
    }
    Ok(result)
}

impl Drop for Journal {
    fn drop(&mut self) {
        for edit in &mut self.edits {
            crate::server::core::plugin_host::wasm::desktop_launch::wipe(&mut edit.before);
            crate::server::core::plugin_host::wasm::desktop_launch::wipe(&mut edit.after);
        }
    }
}
