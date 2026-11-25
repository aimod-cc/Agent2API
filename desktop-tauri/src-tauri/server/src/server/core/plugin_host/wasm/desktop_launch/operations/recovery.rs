use super::super::lock::client_lock;
use std::sync::{Arc, Mutex, OnceLock};
use std::collections::HashMap;

use serde_json::Value;
use tokio::sync::{OwnedMutexGuard, Mutex as AsyncMutex};

use super::{find, Operation};
use crate::server::access::local_session::Source;
use crate::server::core::plugin_host::wasm::desktop_launch::{paths, storage::{self, Journal}, windows};

fn operation_lock(id: &str) -> Arc<AsyncMutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|error| error.into_inner());
    locks.retain(|_, lock| Arc::strong_count(lock) > 1);
    locks.entry(id.into()).or_default().clone()
}

async fn enter(operation: &Arc<Operation>, phase: &'static str) -> Result<OwnedMutexGuard<()>, String> {
    let lock = operation_lock(&operation.id).try_lock_owned().map_err(|_| "本次客户端操作正在执行")?;
    let host = operation.binding.host.clone();
    let _lifecycle = host.lifecycle.try_lock().map_err(|_| "插件运行状态正忙，请稍后重试")?;
    if host.activity.retired() || !host.ready() || crate::server::core::plugin_host::runtime::admission_closed() {
        return Err("插件内容或运行实例已变化，本次启动确认已失效".into());
    }
    let mut state = operation.state.lock().map_err(|_| "本机启动状态不可用")?;
    if state.worker { return Err("本次客户端操作正在执行".into()); }
    state.worker = true; state.phase = phase;
    Ok(lock)
}

pub async fn retry_start(manager: Source, intent: &str, restart_confirmed: bool) -> Result<Value, String> {
    let operation = find(&manager, intent)?;
    {
        let state = operation.state.lock().map_err(|_| "本机启动状态不可用")?;
        if state.worker { drop(state); return Ok(operation.public()); }
        if !state.committed || state.started || state.restored { return Err("本次操作没有待启动的客户端".into()); }
    }
    let lock = enter(&operation, "starting").await?;
    let runner = operation.clone();
    crate::spawn_task(async move {
        let result = tokio::task::spawn_blocking(move || {
            let _lock = lock; let _client = client_lock(&runner.executable, &runner.targets)?;
            if !runner.manager.is_current() { return Err("本机管理会话已变化，未启动客户端".into()); }
            runner.binding.current()?;
            if runner.cancel.is_cancelled() { return Err("本次启动已取消".into()); }
            paths::identity(&runner.binding.client, &runner.executable)?;
            let expected = runner.expected.lock().map_err(|_| "本机启动账号状态不可用")?;
            runner.binding.origin()?.check(expected.as_ref().ok_or("缺少本次启动账号快照")?)?;
            runner.check_proxy()?;
            let id = runner.state.lock().map_err(|_| "本机启动状态不可用")?.backup.clone().ok_or("缺少本次切换备份")?;
            let mut journal = Journal::read(&id)?;
            for edit in &journal.edits {
                let current = super::super::Secret(edit.target.snapshot()?);
                if current.0 != edit.after { return Err("客户端凭证已改变，未用旧账号启动".into()); }
            }
            let running = windows::running(&runner.executable)?;
            if running {
                if !restart_confirmed { return Err("客户端正在运行，请先保存未完成的内容并确认重启".into()); }
                runner.state.lock().map_err(|_| "本机启动状态不可用")?.phase = "closing";
                windows::close(&runner.executable)?;
                runner.state.lock().map_err(|_| "本机启动状态不可用")?.client_running = false;
                for edit in &journal.edits {
                    let current = super::super::Secret(edit.target.snapshot()?);
                    if current.0 != edit.after { edit.target.write(&current.0, &edit.after)?; }
                }
            }
            if !runner.manager.is_current() || runner.binding.current().is_err() { return Err("本机启动来源已变化，未启动客户端".into()); }
            runner.check_proxy()?;
            runner.state.lock().map_err(|_| "本机启动状态不可用")?.phase = "starting";
            windows::start(&runner.executable, runner.proxy.resolved.as_ref(), &runner.launch_args)?;
            {
                let mut state = runner.state.lock().map_err(|_| "本机启动状态不可用")?;
                state.started = true; state.client_running = true;
            }
            journal.phase = "started".into(); journal.save()?;
            Ok::<_, String>(())
        }).await;
        match result {
            Ok(Ok(())) => {
                let message = if operation.proxy.resolved.is_some() { "客户端已使用账号代理启动" } else { "客户端已启动" };
                operation.finish("started", message, false);
            }
            Ok(Err(error)) => {
                if let Ok(running) = windows::running(&operation.executable) {
                    if let Ok(mut state) = operation.state.lock() { state.client_running = running; }
                }
                let phase = if operation.state.lock().is_ok_and(|state| state.started) { "started" } else { "committed" };
                operation.finish(phase, error, true);
            }
            Err(_) => operation.finish("committed", "客户端启动结果未确认", true),
        }
    });
    Ok(find(&manager, intent)?.public())
}

pub async fn restore(manager: Source, intent: &str, confirmed: bool) -> Result<Value, String> {
    if !confirmed { return Err("恢复原凭证前请确认会关闭客户端".into()); }
    let operation = find(&manager, intent)?;
    {
        let state = operation.state.lock().map_err(|_| "本机启动状态不可用")?;
        if state.restored { drop(state); return Ok(operation.public()); }
        if state.worker { drop(state); return Ok(operation.public()); }
    }
    let id = operation.state.lock().map_err(|_| "本机启动状态不可用")?.backup.clone()
        .or_else(|| operation.pending_backup.clone()).ok_or("没有可恢复的本机凭证备份")?;
    let lock = enter(&operation, "closing").await?;
    let runner = operation.clone();
    crate::spawn_task(async move {
        let result = tokio::task::spawn_blocking(move || {
            let _lock = lock; let _client = client_lock(&runner.executable, &runner.targets)?;
            if !runner.manager.is_current() { return Err("本机管理会话已变化，未恢复凭证".into()); }
            if runner.recovery_only && runner.cancel.is_cancelled() { return Err("本次恢复已取消".into()); }
            runner.binding.current()?;
            paths::identity(&runner.binding.client, &runner.executable)?;
            let mut journal = Journal::read(&id)?;
            if !paths::same(&journal.executable, &runner.executable) || journal.provider != runner.binding.provider || journal.client != runner.binding.client.id {
                return Err("备份不属于当前客户端，未恢复".into());
            }
            for edit in &journal.edits[..journal.attempted] {
                let declared = runner.targets.get(&edit.resource).ok_or("当前插件不再声明备份中的凭证资源，未恢复")?;
                if serde_json::to_value(declared).map_err(|_| "凭证资源无法编码")? != serde_json::to_value(&edit.target).map_err(|_| "备份资源无法编码")? {
                    return Err("当前凭证范围与备份不同，未扩大恢复权限".into());
                }
                let current = super::super::Secret(edit.target.snapshot()?);
                if current.0 != edit.before && current.0 != edit.after { return Err("客户端登录态已被其他程序更新，未覆盖新凭证".into()); }
            }
            runner.state.lock().map_err(|_| "本机启动状态不可用")?.phase = "closing";
            if windows::running(&runner.executable)? { windows::close(&runner.executable)?; }
            runner.state.lock().map_err(|_| "本机启动状态不可用")?.client_running = false;
            if !runner.manager.is_current() || runner.binding.current().is_err() { return Err("本机恢复来源已变化，未写入凭证".into()); }
            journal.phase = "restoring".into(); journal.save()?;
            runner.state.lock().map_err(|_| "本机启动状态不可用")?.phase = "writing";
            storage::rollback(&journal.edits[..journal.attempted])?;
            {
                let mut state = runner.state.lock().map_err(|_| "本机启动状态不可用")?;
                state.restored = true; state.committed = false; state.started = false;
            }
            journal.phase = "restored".into(); journal.save()?;
            Ok::<_, String>(())
        }).await;
        match result {
            Ok(Ok(())) => operation.finish("restored", "原凭证已恢复，客户端保持关闭", false),
            Ok(Err(error)) => {
                let phase = if operation.state.lock().is_ok_and(|state| state.restored) { "restored" } else { "recovery_required" };
                operation.finish(phase, error, true);
            }
            Err(_) => operation.finish("recovery_required", "恢复结果未确认，备份已保留", true),
        }
    });
    Ok(find(&manager, intent)?.public())
}
