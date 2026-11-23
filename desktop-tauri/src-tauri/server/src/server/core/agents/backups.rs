use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};

use super::{
    clients::Plan,
    files::{self, Change},
    store,
};
use crate::server::core::crypto::{self, SecretScope};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileBackup {
    pub path: PathBuf,
    pub target: PathBuf,
    pub before: Option<String>,
    pub before_hash: String,
    pub after_hash: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Backup {
    pub id: String,
    pub client_id: String,
    pub config_path: PathBuf,
    pub created_at: i64,
    pub phase: String,
    pub gateway: bool,
    pub baseline: Option<String>,
    pub files: Vec<FileBackup>,
    #[serde(default)]
    pub restored_ids: Vec<String>,
}

fn directory() -> PathBuf {
    crate::paths::config_dir().join("agent-backups")
}
fn backup_path(id: &str) -> Result<PathBuf, String> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("备份 ID 无效".into());
    }
    Ok(directory().join(format!("{id}.json")))
}

pub fn hash(bytes: Option<&[u8]>) -> String {
    bytes.map(store::digest).unwrap_or_else(|| "absent".into())
}

pub fn read(id: &str) -> Result<Backup, String> {
    let bytes =
        files::read_limited(&backup_path(id)?, 64 * 1024 * 1024)?.ok_or("找不到本次应用的备份")?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("备份文件损坏: {e}"))?;
    let envelope = value
        .get("encrypted")
        .and_then(serde_json::Value::as_str)
        .ok_or("不是完整事务备份；旧版单文件备份不能自动恢复")?;
    let data = crypto::open(envelope, SecretScope::Accounts)?;
    serde_json::from_slice(&data).map_err(|e| format!("备份内容损坏: {e}"))
}

pub fn write(backup: &Backup) -> Result<(), String> {
    let plain = serde_json::to_vec(backup).map_err(|e| format!("备份序列化失败: {e}"))?;
    let encrypted = crypto::seal(&plain, SecretScope::Accounts)?;
    files::write_atomic(
        &backup_path(&backup.id)?,
        &serde_json::to_vec(&json!({ "encrypted": encrypted }))
            .map_err(|e| format!("生成备份失败: {e}"))?,
    )
}

pub fn scan_directory() -> Result<Vec<String>, String> {
    let entries = match std::fs::read_dir(directory()) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("读取备份目录失败: {e}")),
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("读取备份失败: {e}"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        // 旧版备份是未加密原文件，必须与新的事务备份分开。
        if id.split('-').last().is_none_or(|part| part.len() != 16) {
            continue;
        }
        ids.push(id.to_string());
    }
    Ok(ids)
}

fn parse_backup_ts(client: &str, id: &str) -> Option<i64> {
    let rest = id.strip_prefix(client)?.strip_prefix('-')?;
    let (ts, suffix) = rest.rsplit_once('-')?;
    if suffix.len() != 16 {
        return None;
    }
    ts.parse::<i64>().ok()
}

pub fn latest_active(
    client: &str,
    path: &Path,
    pre_scanned: Option<&[String]>,
) -> Result<Option<Backup>, String> {
    let owned_ids;
    let all_ids = match pre_scanned {
        Some(ids) => ids,
        None => {
            owned_ids = scan_directory()?;
            &owned_ids
        }
    };
    let prefix = format!("{client}-");
    let matching: Vec<&str> = all_ids
        .iter()
        .map(String::as_str)
        .filter(|id| id.starts_with(&prefix))
        .collect();
    if matching.is_empty() {
        return Ok(None);
    }
    let mut dated = Vec::with_capacity(matching.len());
    for id in &matching {
        let Some(ts) = parse_backup_ts(client, id) else {
            return Ok(list(client, path)?.into_iter().rev().find(|b| {
                matches!(
                    b.phase.as_str(),
                    "committed" | "prepared" | "restore_prepared"
                )
            }));
        };
        dated.push((ts, *id));
    }
    dated.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(a.1)));

    let mut restored = std::collections::HashSet::new();
    let mut idx = 0;
    while idx < dated.len() {
        let current_ts = dated[idx].0;
        let mut batch = Vec::new();
        while idx < dated.len() && dated[idx].0 == current_ts {
            let backup = read(dated[idx].1)?;
            if backup.config_path == path {
                if backup.phase == "restore_committed" {
                    restored.extend(backup.restored_ids.iter().cloned());
                } else {
                    batch.push(backup);
                }
            }
            idx += 1;
        }
        if let Some(found) = batch.into_iter().find(|backup| {
            !restored.contains(&backup.id)
                && matches!(
                    backup.phase.as_str(),
                    "committed" | "prepared" | "restore_prepared"
                )
        }) {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

pub fn list(client: &str, path: &Path) -> Result<Vec<Backup>, String> {
    let all_ids = scan_directory()?;
    let prefix = format!("{client}-");
    let mut backups = Vec::new();
    for id in all_ids.iter().filter(|id| id.starts_with(&prefix)) {
        let backup = read(id)?;
        if backup.config_path == path {
            backups.push(backup);
        }
    }
    backups.sort_by_key(|backup| backup.created_at);
    let restored: std::collections::HashSet<_> = backups
        .iter()
        .filter(|backup| backup.phase == "restore_committed")
        .flat_map(|backup| backup.restored_ids.iter().cloned())
        .collect();
    for backup in &mut backups {
        if restored.contains(&backup.id) {
            backup.phase = "restored".into();
        }
    }
    Ok(backups)
}

pub fn ensure_no_pending(client: &str, path: &Path) -> Result<(), String> {
    if list(client, path)?
        .iter()
        .any(|backup| matches!(backup.phase.as_str(), "prepared" | "restore_prepared"))
    {
        return Err("上次配置事务未完整结束，请先撤销上次应用后再写入".into());
    }
    Ok(())
}

pub fn prepare(client: &str, path: &Path, plan: &Plan) -> Result<Backup, String> {
    prepare_transaction(client, path, plan, Vec::new())
}

pub fn prepare_restore(
    client: &str,
    path: &Path,
    plan: &Plan,
    restored_ids: Vec<String>,
) -> Result<Backup, String> {
    prepare_transaction(client, path, plan, restored_ids)
}

fn prepare_transaction(
    client: &str,
    path: &Path,
    plan: &Plan,
    restored_ids: Vec<String>,
) -> Result<Backup, String> {
    let created_at = crate::server::logging::now_ms();
    let mut random = [0u8; 8];
    getrandom::getrandom(&mut random).map_err(|e| format!("无法生成备份 ID: {e}"))?;
    let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let id = format!("{client}-{created_at}-{suffix}");
    let previous = list(client, path)?
        .into_iter()
        .rev()
        .find(|b| b.phase == "committed");
    let gateway = plan.profile.kind == "gateway";
    let baseline = if gateway {
        match previous.filter(|backup| backup.gateway) {
            Some(backup) if restore_changes(client, path, true).is_ok() => backup.baseline,
            Some(_) => None,
            None => Some(id.clone()),
        }
    } else {
        None
    };
    let files = plan
        .changes
        .iter()
        .filter(|change| !change.unchanged())
        .map(|change| FileBackup {
            path: change.path.clone(),
            target: change.target.clone(),
            before: change.before.as_ref().map(|bytes| STANDARD.encode(bytes)),
            before_hash: hash(change.before.as_deref()),
            after_hash: hash(change.after.as_deref()),
        })
        .collect::<Vec<_>>();
    let backup = Backup {
        id,
        client_id: client.into(),
        config_path: path.into(),
        created_at,
        phase: if restored_ids.is_empty() {
            "prepared"
        } else {
            "restore_prepared"
        }
        .into(),
        gateway,
        baseline,
        files,
        restored_ids,
    };
    write(&backup)?;
    Ok(backup)
}

pub fn restore_changes(
    client: &str,
    path: &Path,
    disconnect: bool,
) -> Result<(Vec<Change>, Vec<String>), String> {
    let backups = list(client, path)?;
    if let Some(pending) = backups
        .iter()
        .rev()
        .find(|backup| backup.phase == "restore_prepared")
    {
        if disconnect {
            return Err("上次恢复事务未完整结束，请先撤销上次应用完成恢复".into());
        }
        let mut changes = Vec::new();
        for file in &pending.files {
            let mut change = Change::load(file.path.clone())?;
            let current = hash(change.before.as_deref());
            if change.target != file.target
                || (current != file.before_hash && current != file.after_hash)
            {
                return Err(format!(
                    "{} 在恢复中被其他程序修改，拒绝覆盖",
                    file.path.display()
                ));
            }
            change.after = file
                .before
                .as_ref()
                .map(|encoded| {
                    STANDARD
                        .decode(encoded)
                        .map_err(|error| format!("备份内容损坏: {error}"))
                })
                .transpose()?;
            changes.push(change);
        }
        return Ok((changes, vec![pending.id.clone()]));
    }
    let latest = backups
        .iter()
        .rev()
        .find(|b| b.phase == "committed" || b.phase == "prepared")
        .ok_or("没有可恢复的完整备份")?;
    let sequence: Vec<_> = if disconnect {
        if !latest.gateway {
            return Err("最近一次应用不是本网关配置，不能自动解除接入".into());
        }
        let baseline = latest
            .baseline
            .as_deref()
            .ok_or("接入期间已有外部修改，不能安全恢复到首次接入前；可撤销上次应用后手动核对")?;
        let start = backups
            .iter()
            .position(|b| b.id == baseline)
            .ok_or("首次接入备份已丢失，不能安全解除接入")?;
        backups[start..]
            .iter()
            .filter(|b| b.phase == "committed" || b.phase == "prepared")
            .collect()
    } else {
        vec![latest]
    };
    let mut targets = std::collections::BTreeMap::<PathBuf, (&FileBackup, &FileBackup)>::new();
    for backup in sequence.iter() {
        for file in &backup.files {
            targets
                .entry(file.target.clone())
                .and_modify(|slot| slot.1 = file)
                .or_insert((file, file));
        }
    }
    let mut changes = Vec::new();
    for (_, (first, last)) in targets {
        let mut change = Change::load(last.path.clone())?;
        let current_hash = hash(change.before.as_deref());
        if change.target != last.target
            || (current_hash != last.after_hash
                && !(latest.phase == "prepared" && current_hash == last.before_hash))
        {
            return Err(format!(
                "{} 在应用后已被其他程序修改，拒绝用备份覆盖；请手动核对",
                last.path.display()
            ));
        }
        change.after = first
            .before
            .as_ref()
            .map(|encoded| {
                STANDARD
                    .decode(encoded)
                    .map_err(|e| format!("备份内容损坏: {e}"))
            })
            .transpose()?;
        changes.push(change);
    }
    Ok((changes, sequence.iter().map(|b| b.id.clone()).collect()))
}
