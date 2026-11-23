use std::sync::Arc;
use std::path::PathBuf;

use serde_json::{json, Value};

use super::{binding::Binding, operations::{Operation, State}, paths, storage::{self, Journal}, windows};
use crate::server::{access::local_session::Source, db::Db};
use tokio::sync::Notify;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub async fn pending(manager: &Source) -> Result<Value, String> {
    if !manager.is_current() { return Err("本机管理会话不可用".into()); }
    let rows = crate::server::core::blocking::run(storage::journal::pending_list).await
        .map_err(|_| "本机启动备份读取中断")??;
    if !manager.is_current() { return Err("本机管理会话已变化".into()); }
    let rows = rows.into_iter().filter(|journal| !super::operations::backup_running(&journal.id)).collect::<Vec<_>>();
    Ok(json!({ "pending": rows.iter().map(|journal| json!({
        "backup": journal.id, "provider": journal.provider, "account": journal.account,
        "client": journal.client, "executable": journal.executable.display().to_string(),
        "label": super::super::super::host_for_provider(&journal.provider)
            .and_then(|host| host.options.wasm.as_ref().and_then(|options| options.manifest.desktop_launch.as_ref())
                .and_then(|declaration| declaration.clients.iter().find(|client| client.id == journal.client)).map(|client| client.label.clone()))
            .unwrap_or_else(|| journal.client.clone()),
    })).collect::<Vec<_>>() }))
}

pub async fn prepare(manager: Source, db: Db, id: String) -> Result<Value, String> {
    let journal = crate::server::core::blocking::run(move || Journal::read(&id)).await
        .map_err(|_| "本机启动备份读取中断")??;
    let id = journal.id.clone();
    if super::operations::backup_running(&id) { return Err("本次客户端操作正在执行".into()); }
    if !journal.recoverable() || !matches!(journal.phase.as_str(), "prepared" | "writing" | "committed" | "restoring" | "recovery_required") {
        return Err("该备份没有未完成的凭证切换".into());
    }
    let binding = Binding::recovery(&journal.provider, &journal.account, &journal.client)?;
    let executable = journal.executable.clone();
    let target_client = binding.client.clone();
    let target_executable = executable.clone();
    let targets = crate::server::core::blocking::run(move || {
        paths::identity(&target_client, &target_executable)?;
        storage::targets(&target_client, &target_executable)
    }).await.map_err(|_| "本机恢复客户端检查中断")??;
    for edit in &journal.edits[..journal.attempted] {
        let current = targets.get(&edit.resource).ok_or("当前插件不再声明原凭证资源，未恢复")?;
        if serde_json::to_value(current).map_err(|_| "凭证资源无法编码")? != serde_json::to_value(&edit.target).map_err(|_| "备份资源无法编码")? {
            return Err("当前插件的凭证范围与备份不同，未扩大恢复权限".into());
        }
    }
    let setting = super::operations::settings::read(&db, &binding.provider, &binding.client.id)?;
    if !manager.is_current() { return Err("本机管理会话已变化".into()); }
    binding.current()?;
    let running_executable = executable.clone();
    let running = crate::server::core::blocking::run(move || windows::running(&running_executable)).await
        .map_err(|_| "本机恢复进程检查中断")??;
    let operation = Arc::new(Operation {
        id: super::operations::random_id()?, manager, binding, db, executable: PathBuf::from(executable), targets: Arc::new(targets), setting,
        restart: running, hot_switch: false, initial_running: running, proxy: super::proxy::LaunchProxyState::disabled(), pending_backup: Some(id),
        cancel: CancellationToken::new(), deadline: Instant::now() + std::time::Duration::from_secs(180),
        state: std::sync::Mutex::new(State { phase: "awaiting_confirmation", message: String::new(), confirmed: false, worker: false,
            committed: false, started: false, restored: false, client_running: running, backup: None, error: false }), changed: Notify::new(),
        expected: std::sync::Mutex::new(None), recovery_only: true,
    });
    super::operations::insert(operation.clone())?;
    Ok(operation.public())
}
