use std::sync::Arc;

use futures::FutureExt;
use super::Operation;
use crate::server::core::plugin_host::wasm::desktop_launch::{
    binding::Call, declaration::{self, CredentialPlan}, Secret, storage::{self, Edit, Journal},
    windows::{self, HotSwitchRespawn},
};

pub(super) async fn run(operation: Arc<Operation>) {
    let result = std::panic::AssertUnwindSafe(execute(operation.clone())).catch_unwind().await;
    match result {
        Ok(Ok(())) => {},
        Ok(Err(error)) => {
            let phase = operation.state.lock().map(|state| if state.started { if operation.hot_switch { "switched" } else { "started" } }
                else if state.committed { "committed" } else if operation.cancel.is_cancelled() { "canceled" } else { "failed" }).unwrap_or("recovery_required");
            operation.finish(phase, error, true);
        },
        Err(_) => operation.finish("recovery_required", "本机启动执行中断，请检查备份状态后恢复原凭证", true),
    }
}

async fn execute(operation: Arc<Operation>) -> Result<(), String> {
    operation.check()?;
    operation.binding.origin()?.check(operation.binding.origin()?.initial())?;
    operation.check_proxy()?;
    let call = Call::new(&operation.binding, operation.manager.clone(), Some(operation.executable.clone()), Some(operation.targets.clone()))
        .with_cancel(operation.cancel.clone());
    let result = Secret(operation.binding.invoke(call.clone(), declaration::PREPARE).await?);
    let observed = Secret(serde_json::to_value(call.observed()?).map_err(|_| "本机凭证读取快照不可用")?);
    let plan: CredentialPlan = serde_json::from_value(result.0.clone()).map_err(|_| "插件本机凭证准备格式无效")?;
    if plan.client != operation.binding.client.id { return Err("插件准备了另一客户端的凭证，未写入".into()); }
    operation.check()?;
    let accepted = operation.binding.origin()?.accept(&plan.expect)?;
    *operation.expected.lock().map_err(|_| "本机启动账号状态不可用")? = Some(Secret(accepted.0.clone()));
    let runner = operation.clone();
    // 真正写入的阻塞任务持有全部恢复材料；取消/HTTP超时不能提前释放客户端占位。
    tokio::task::spawn_blocking(move || commit(runner, plan, accepted, observed)).await.map_err(|_| "本机切换结果未确认，请检查备份状态")?
}

fn apply_edits(journal: &mut Journal) -> Result<(), String> {
    journal.phase = "writing".into();
    journal.save()?;
    for index in 0..journal.edits.len() {
        journal.attempted = index + 1;
        journal.save()?;
        let edit = &journal.edits[index];
        edit.target.write(&edit.before, &edit.after)?;
    }
    Ok(())
}

fn reassert_edits(edits: &[Edit]) -> Result<(), String> {
    for edit in edits {
        let current = Secret(edit.target.snapshot()?);
        if current.0 != edit.after {
            edit.target.write(&current.0, &edit.after)?;
        }
    }
    Ok(())
}

fn rollback_after_error(error: String, journal: &mut Journal, applied: bool) -> String {
    let rollback = if applied { storage::rollback(&journal.edits[..journal.attempted]) } else { Ok(()) };
    journal.phase = if rollback.is_ok() { if applied { "rolled_back" } else { "not_applied" } } else { "recovery_required" }.into();
    let saved = journal.save();
    format!("{error}{}{}", rollback.err().map(|err| format!("；恢复原凭证失败：{err}")).unwrap_or_default(),
        saved.err().map(|err| format!("；备份状态保存失败：{err}")).unwrap_or_default())
}

fn save_committed(journal: &mut Journal) -> Result<(), String> {
    journal.phase = "committed".into();
    if let Err(error) = journal.save() {
        let rollback = storage::rollback(&journal.edits[..journal.attempted]);
        journal.phase = if rollback.is_ok() { "rolled_back" } else { "recovery_required" }.into();
        let _ = journal.save();
        return Err(format!("本机切换记录保存失败：{error}{}", rollback.err().map(|err| format!("；恢复原凭证失败：{err}")).unwrap_or_default()));
    }
    Ok(())
}

fn commit(operation: Arc<Operation>, plan: CredentialPlan, accepted: Secret, observed: Secret) -> Result<(), String> {
    let _lock = super::super::lock::client_lock(&operation.executable, &operation.targets)?;
    operation.check()?;
    operation.binding.origin()?.check(&accepted)?;
    operation.check_proxy()?;
    if Journal::pending(&operation.executable, &operation.targets)?.is_some() { return Err("上次本机切换未完整结束，请先恢复原凭证".into()); }
    let running = windows::running(&operation.executable)?;
    if running && !operation.initial_running { return Err("客户端在准备后已启动，请重新确认".into()); }
    if running && !operation.hot_switch && !operation.restart { return Err("客户端在准备后已启动，请重新确认重启".into()); }
    let edits = storage::edits(&plan, &operation.targets)?;
    for edit in &edits {
        if observed.get(&edit.resource).is_some_and(|before| before != &edit.before) {
            return Err("客户端凭证在准备期间已变化，请重新发起切换".into());
        }
    }
    let mut journal = Journal { version: 1, id: operation.id.clone(), provider: operation.binding.provider.clone(),
        account: operation.binding.account.clone(), client: operation.binding.client.id.clone(), executable: operation.executable.clone(),
        package: operation.binding.package.clone(), phase: "prepared".into(), was_running: running, edits, attempted: 0 };
    journal.save()?;
    {
        let mut state = operation.state.lock().map_err(|_| "本机启动状态不可用")?;
        state.backup = Some(journal.id.clone());
        state.client_running = running;
        state.phase = if operation.hot_switch && running { "writing" } else { "closing" };
    }
    if operation.hot_switch && running {
        return commit_hot(operation, journal, accepted);
    }
    let prepared = (|| {
        operation.check()?;
        operation.binding.origin()?.check(&accepted)?;
        if running { windows::close(&operation.executable)?; }
        operation.state.lock().map_err(|_| "本机启动状态不可用")?.client_running = false;
        operation.check()?;
        operation.binding.origin()?.check(&accepted)?;
        if windows::running(&operation.executable)? { return Err("客户端尚未完全关闭，未写入凭证".into()); }
        // 客户端退出时可能回写凭证；与准备快照不符时停止，不能覆盖新状态。
        let current = storage::edits(&plan, &operation.targets)?;
        for (before, after) in journal.edits.iter().zip(&current) {
            if before.before != after.before { return Err("客户端在退出时更新了登录态，请重新发起切换；原凭证未被覆盖".into()); }
        }
        operation.state.lock().map_err(|_| "本机启动状态不可用")?.phase = "writing";
        apply_edits(&mut journal)?;
        Ok::<(), String>(())
    })();
    if let Err(error) = prepared {
        let applied = journal.phase == "writing";
        let message = rollback_after_error(error, &mut journal, applied);
        if matches!(journal.phase.as_str(), "not_applied" | "rolled_back") {
            if let Ok(mut state) = operation.state.lock() { state.backup = None; }
        }
        if journal.phase == "rolled_back" && running && operation.manager.is_current()
            && operation.binding.current().is_ok() && windows::running(&operation.executable) == Ok(false) {
            if windows::start(&operation.executable, None).is_ok() {
                if let Ok(mut state) = operation.state.lock() { state.client_running = true; }
            }
        }
        return Err(message);
    }
    save_committed(&mut journal)?;
    {
        let mut state = operation.state.lock().map_err(|_| "本机启动状态不可用")?;
        state.committed = true; state.phase = "starting";
    }
    if let Err(error) = operation.check().and_then(|_| operation.binding.origin()?.check(&accepted)) {
        operation.finish("committed", format!("凭证已切换；{error}"), false);
        return Ok(());
    }
    match operation.check_proxy().and_then(|_| windows::start(&operation.executable, operation.proxy.resolved.as_ref())) {
        Ok(()) => {
            {
                let mut state = operation.state.lock().map_err(|_| "本机启动状态不可用")?;
                state.started = true; state.client_running = true;
            }
            journal.phase = "started".into();
            let message = if operation.proxy.resolved.is_some() { "已切换凭证并使用账号代理启动客户端" } else { "已切换凭证并启动客户端" };
            match journal.save() {
                Ok(()) => operation.finish("started", message, false),
                Err(_) => operation.finish("started", "客户端已启动，但备份状态保存失败，备份已保留", true),
            }
        }
        Err(error) => operation.finish("committed", error, true),
    }
    Ok(())
}

fn commit_hot(operation: Arc<Operation>, mut journal: Journal, accepted: Secret) -> Result<(), String> {
    let Some(hot_switch_decl) = operation.binding.client.hot_switch.as_ref() else {
        return Err(rollback_after_error("插件没有声明在线切换配置".into(), &mut journal, false));
    };
    let hot_targets = match (|| {
        operation.check()?;
        operation.binding.origin()?.check(&accepted)?;
        windows::hot_switch_targets(&operation.executable, Some(hot_switch_decl))?
            .ok_or_else(|| "未检测到可在线重载的后台服务，请保存内容后重启客户端重试".to_string())
    })() {
        Ok(targets) => targets,
        Err(error) => return Err(rollback_after_error(error, &mut journal, false)),
    };
    if let Err(error) = apply_edits(&mut journal) {
        let applied = journal.phase == "writing";
        return Err(rollback_after_error(error, &mut journal, applied));
    }
    operation.state.lock().map_err(|_| "本机启动状态不可用")?.phase = "reloading";
    let reloaded = (|| {
        windows::stop_hot_services(&operation.executable, &hot_targets)?;
        reassert_edits(&journal.edits)?;
        Ok::<(), String>(())
    })();
    save_committed(&mut journal)?;
    {
        let mut state = operation.state.lock().map_err(|_| "本机启动状态不可用")?;
        state.committed = true;
        state.client_running = windows::running(&operation.executable).unwrap_or(true);
    }
    if let Err(error) = reloaded { return Err(error); }
    if let Err(error) = operation.check().and_then(|_| operation.binding.origin()?.check(&accepted)) {
        operation.finish("committed", format!("凭证已切换；{error}"), false);
        return Ok(());
    }
    match windows::wait_hot_services_respawn(&operation.executable, hot_switch_decl, &hot_targets, &operation.cancel) {
        Ok(HotSwitchRespawn::Ready) => {
            {
                let mut state = operation.state.lock().map_err(|_| "本机启动状态不可用")?;
                state.started = true; state.client_running = true;
            }
            journal.phase = "started".into();
            match journal.save() {
                Ok(()) => operation.finish("switched", "已在线切换凭证，客户端无需重启", false),
                Err(_) => operation.finish("switched", "已在线切换凭证，但备份状态保存失败，备份已保留", true),
            }
        }
        Ok(HotSwitchRespawn::ClientClosed) => {
            operation.state.lock().map_err(|_| "本机启动状态不可用")?.client_running = false;
            operation.finish("committed", "凭证已切换，客户端已被关闭", false);
        }
        Ok(HotSwitchRespawn::Canceled) => operation.finish("committed", "凭证已切换，已取消等待后台服务重载", true),
        Ok(HotSwitchRespawn::TimedOut) => operation.finish("committed", "凭证已切换，但后台服务未能自动重载；请保存未完成内容后重启客户端或恢复原凭证", true),
        Err(error) => {
            if let Ok(running) = windows::running(&operation.executable) {
                if let Ok(mut state) = operation.state.lock() { state.client_running = running; }
            }
            operation.finish("committed", error, true);
        }
    }
    Ok(())
}
