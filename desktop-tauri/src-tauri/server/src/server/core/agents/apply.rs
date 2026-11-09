use serde_json::{json, Value};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

#[cfg(test)]
#[path = "apply_tests.rs"]
mod tests;

use super::{
    backups, clients,
    files::{self, Change},
    preview, ClientDef, Profile,
};

fn mutation_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

pub fn apply(
    def: &ClientDef,
    path: &Path,
    profile: &Profile,
    operation: &str,
    token: &str,
    port: u16,
) -> Result<Value, String> {
    let _guard = mutation_lock()
        .lock()
        .map_err(|_| "客户端配置写入锁不可用")?;
    if token.is_empty() {
        return Err("应用前必须生成预览并确认".into());
    }
    let plan = clients::plan(def, path, profile, operation, port)?;
    preview::verify(&plan, def.id, token)?;
    for change in &plan.changes { change.check()?; }
    let changes: Vec<_> = plan
        .changes
        .iter()
        .filter(|change| !change.unchanged())
        .cloned()
        .collect();
    if changes.is_empty() {
        return Ok(json!({ "changed": false, "verified": true, "clientId": def.id }));
    }
    for change in &changes {
        change.check()?;
    }
    backups::ensure_no_pending(def.id, path)?;
    let mut backup = backups::prepare(def.id, path, &plan)?;
    if let Err(error) = commit(&changes) {
        if changes
            .iter()
            .all(|change| files::read(&change.target).is_ok_and(|bytes| bytes == change.before))
        {
            backup.phase = "rolled_back".into();
        }
        let metadata = backups::write(&backup);
        return Err(format!(
            "{error}{}",
            metadata
                .err()
                .map(|e| format!("；备份状态保存失败: {e}"))
                .unwrap_or_default()
        ));
    }
    if let Err(error) = verify_bytes(&changes) {
        let rollback = rollback(&changes);
        if rollback.is_ok() {
            backup.phase = "rolled_back".into();
            let _ = backups::write(&backup);
        }
        return Err(format!(
            "{error}{}",
            rollback
                .err()
                .map(|e| format!("；自动回滚失败: {e}"))
                .unwrap_or_default()
        ));
    }
    backup.phase = "committed".into();
    if let Err(error) = backups::write(&backup) {
        let recovery = rollback(&changes);
        return Err(format!(
            "保存应用记录失败: {error}{}",
            recovery
                .err()
                .map(|e| format!("；自动回滚失败: {e}"))
                .unwrap_or_default()
        ));
    }
    Ok(
        json!({ "changed": true, "verified": true, "clientId": def.id, "profileId": profile.id, "backupId": backup.id, "needsRestart": def.needs_restart }),
    )
}

pub fn apply_plan(def: &ClientDef, path: &Path, plan: &clients::Plan, token: &str) -> Result<(), String> {
    let _guard = mutation_lock().lock().map_err(|_| "客户端配置写入锁不可用")?;
    preview::verify(plan, def.id, token)?;
    write_plan(def, path, plan)
}

pub fn apply_plan_with<T>(def: &ClientDef, path: &Path, plan: &clients::Plan, token: &str,
    persist: impl FnOnce(&mut dyn FnMut() -> Result<(), String>) -> Result<T, String>,
) -> Result<T, String> {
    let _guard = mutation_lock().lock().map_err(|_| "客户端配置写入锁不可用")?;
    preview::verify(plan, def.id, token)?;
    persist(&mut || write_plan(def, path, plan))
}

pub fn apply_resource(def: &ClientDef, path: &Path, plan: &clients::Plan) -> Result<(), String> {
    let _guard = mutation_lock().lock().map_err(|_| "客户端配置写入锁不可用")?;
    write_plan(def, path, plan)
}

fn write_plan(def: &ClientDef, path: &Path, plan: &clients::Plan) -> Result<(), String> {
    for change in &plan.changes { change.check()?; }
    let changes = plan.changes.iter().filter(|change| !change.unchanged()).cloned().collect::<Vec<_>>();
    if changes.is_empty() { return Ok(()); }
    for change in &changes { change.check()?; }
    backups::ensure_no_pending(def.id, path)?;
    let mut backup = backups::prepare(def.id, path, plan)?;
    if let Err(error) = commit(&changes).and_then(|_| verify_bytes(&changes)) {
        if rollback(&changes).is_ok() { backup.phase = "rolled_back".into(); let _ = backups::write(&backup); }
        return Err(error);
    }
    backup.phase = "committed".into();
    if let Err(error) = backups::write(&backup) { let _ = rollback(&changes); return Err(error); }
    Ok(())
}

fn verify_bytes(changes: &[Change]) -> Result<(), String> {
    for change in changes {
        if files::target(&change.path)? != change.target
            || files::read(&change.target)? != change.after
        {
            return Err(format!("写入后校验 {} 失败", change.path.display()));
        }
    }
    Ok(())
}

fn commit(changes: &[Change]) -> Result<(), String> {
    let mut staged = Vec::new();
    for change in changes {
        if let Some(bytes) = &change.after {
            match files::stage(&change.target, bytes) {
                Ok(path) => staged.push(Some(path)),
                Err(error) => {
                    for path in staged.into_iter().flatten() {
                        let _ = std::fs::remove_file(path);
                    }
                    return Err(error);
                }
            }
        } else {
            staged.push(None);
        }
    }
    let result = (|| {
        for change in changes {
            change.check().map_err(|error| (error, Vec::new()))?;
        }
        let mut written = Vec::new();
        for (index, change) in changes.iter().enumerate() {
            if let Err(error) = change.check() {
                return Err((error, written));
            }
            let result = match &staged[index] {
                Some(temp) => files::replace(temp, &change.target),
                None => {
                    if change.before.is_some() {
                        std::fs::remove_file(&change.target)
                            .map_err(|e| format!("移除新建配置失败: {e}"))
                    } else {
                        Ok(())
                    }
                }
            };
            // rename 后的目录同步也可能失败，此时文件已经变了，同样需要回滚。
            if result.is_ok() || files::read(&change.target).ok().flatten() == change.after {
                written.push(change.clone());
            }
            if let Err(error) = result {
                return Err((error, written));
            }
        }
        Ok(())
    })();
    for temp in staged.into_iter().flatten() {
        let _ = std::fs::remove_file(temp);
    }
    match result {
        Ok(()) => Ok(()),
        Err((error, written)) => {
            let recovery = rollback(&written);
            Err(format!(
                "{error}{}",
                recovery
                    .err()
                    .map(|e| format!("；自动回滚失败: {e}，请保留事务备份并手动恢复"))
                    .unwrap_or_else(|| "；本次变更已回滚".into())
            ))
        }
    }
}

fn rollback(changes: &[Change]) -> Result<(), String> {
    let mut errors = Vec::new();
    for change in changes.iter().rev() {
        let current = files::read(&change.target)?;
        if current == change.before {
            continue;
        }
        if current != change.after {
            errors.push(format!(
                "{} 已被其他程序修改，未覆盖",
                change.path.display()
            ));
            continue;
        }
        let result = match &change.before {
            Some(bytes) => files::write_atomic(&change.target, bytes),
            None => std::fs::remove_file(&change.target)
                .map_err(|e| format!("移除 {} 失败: {e}", change.path.display())),
        };
        if let Err(error) = result {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("；"))
    }
}

pub fn restore_preview(def: &ClientDef, path: &Path, disconnect: bool) -> Result<Value, String> {
    let (changes, _) = backups::restore_changes(def.id, path, disconnect)?;
    let pending = backups::list(def.id, path)?
        .iter()
        .any(|backup| backup.phase == "restore_prepared");
    let mut warnings =
        vec!["仅当目标文件仍与上次应用结果一致时恢复；不会覆盖其他程序的新修改。".into()];
    if pending {
        warnings.push("上次恢复未完整结束；本次先撤回未完成的恢复，原应用备份仍保留。完成后可再次撤销上次应用。".into());
    }
    let plan = clients::Plan {
        changes,
        profile: Profile {
            id: "restore".into(),
            ..Default::default()
        },
        operation: if disconnect { "disconnect" } else { "restore" }.into(),
        warnings,
    };
    preview::create_restore(def.id, &plan)
}

pub fn restore(
    def: &ClientDef,
    path: &Path,
    disconnect: bool,
    token: &str,
) -> Result<Value, String> {
    let _guard = mutation_lock()
        .lock()
        .map_err(|_| "客户端配置写入锁不可用")?;
    let (changes, ids) = backups::restore_changes(def.id, path, disconnect)?;
    let plan = clients::Plan {
        changes: changes.clone(),
        profile: Profile {
            id: "restore".into(),
            ..Default::default()
        },
        operation: if disconnect { "disconnect" } else { "restore" }.into(),
        warnings: Vec::new(),
    };
    preview::verify(&plan, def.id, token)?;
    let pending = backups::list(def.id, path)?
        .into_iter()
        .rev()
        .find(|backup| backup.phase == "restore_prepared");
    if let Some(mut pending) = pending {
        commit(&changes)?;
        if let Err(error) = verify_bytes(&changes) {
            let _ = rollback(&changes);
            return Err(error);
        }
        pending.phase = "rolled_back".into();
        if let Err(error) = backups::write(&pending) {
            let _ = rollback(&changes);
            return Err(error);
        }
        return Ok(json!({ "clientId": def.id, "verified": true, "recovered": true }));
    }
    let mut journal = backups::prepare_restore(def.id, path, &plan, ids)?;
    if let Err(error) = commit(&changes) {
        if changes
            .iter()
            .all(|change| files::read(&change.target).is_ok_and(|bytes| bytes == change.before))
        {
            journal.phase = "rolled_back".into();
            let _ = backups::write(&journal);
        }
        return Err(error);
    }
    if let Err(error) = verify_bytes(&changes) {
        let recovery = rollback(&changes);
        if recovery.is_ok() {
            journal.phase = "rolled_back".into();
            let _ = backups::write(&journal);
        }
        return Err(format!(
            "{error}{}",
            recovery
                .err()
                .map(|error| format!("；自动回滚失败: {error}"))
                .unwrap_or_default()
        ));
    }
    journal.phase = "restore_committed".into();
    if let Err(error) = backups::write(&journal) {
        let recovery = rollback(&changes);
        if recovery.is_ok() {
            journal.phase = "rolled_back".into();
            let _ = backups::write(&journal);
        }
        return Err(format!(
            "恢复记录保存失败: {error}{}",
            recovery
                .err()
                .map(|error| format!("；自动回滚失败: {error}"))
                .unwrap_or_default()
        ));
    }
    Ok(json!({ "clientId": def.id, "verified": true, "disconnected": disconnect }))
}

pub fn applied_status(client: &str, path: &Path) -> Result<Value, String> {
    let backups = backups::list(client, path)?;
    let Some(last) = backups.iter().rev().find(|b| {
        matches!(
            b.phase.as_str(),
            "committed" | "prepared" | "restore_prepared"
        )
    }) else {
        return Ok(json!({ "canUndo": false, "canDisconnect": false }));
    };
    let unchanged = last.files.iter().all(|file| {
        files::read(&file.target)
            .is_ok_and(|bytes| backups::hash(bytes.as_deref()) == file.after_hash)
    });
    Ok(
        json!({ "canUndo": true, "canDisconnect": last.gateway && last.baseline.is_some(), "modifiedExternally": !unchanged, "pendingRecovery": matches!(last.phase.as_str(), "prepared" | "restore_prepared"), "backupId": last.id, "appliedAt": last.created_at }),
    )
}
