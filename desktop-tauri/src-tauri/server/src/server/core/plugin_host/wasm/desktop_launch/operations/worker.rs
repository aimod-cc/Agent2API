use std::sync::Arc;

use futures::FutureExt;
use super::Operation;
use crate::server::core::plugin_host::wasm::desktop_launch::{binding::Call, declaration::{self, CredentialPlan}, Secret, storage::{self, Journal}, windows};

pub(super) async fn run(operation: Arc<Operation>) {
    let result = std::panic::AssertUnwindSafe(execute(operation.clone())).catch_unwind().await;
    match result {
        Ok(Ok(())) => {},
        Ok(Err(error)) => {
            let phase = operation.state.lock().map(|state| if state.started { "started" } else if state.committed { "committed" }
                else if operation.cancel.is_cancelled() { "canceled" } else { "failed" }).unwrap_or("recovery_required");
            operation.finish(phase, error, true);
        },
        Err(_) => operation.finish("recovery_required", "本机启动执行中断，请检查备份状态后恢复原凭证", true),
    }
}

async fn execute(operation: Arc<Operation>) -> Result<(), String> {
    operation.check()?;
    operation.binding.origin()?.check(operation.binding.origin()?.initial())?;
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

fn commit(operation: Arc<Operation>, plan: CredentialPlan, accepted: Secret, observed: Secret) -> Result<(), String> {
    let _lock = super::super::lock::client_lock(&operation.executable, &operation.targets)?;
    operation.check()?;
    operation.binding.origin()?.check(&accepted)?;
    if Journal::pending(&operation.executable, &operation.targets)?.is_some() { return Err("上次本机切换未完整结束，请先恢复原凭证".into()); }
    let running = windows::running(&operation.executable)?;
    if running && !operation.restart { return Err("客户端在准备后已启动，请重新确认重启".into()); }
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
        state.backup = Some(journal.id.clone()); state.phase = "closing";
    }
    let prepared = (|| {
        operation.check()?;
        operation.binding.origin()?.check(&accepted)?;
        if running { windows::close(&operation.executable)?; }
        operation.check()?;
        operation.binding.origin()?.check(&accepted)?;
        if windows::running(&operation.executable)? { return Err("客户端尚未完全关闭，未写入凭证".into()); }
        // 客户端退出时可能回写凭证；与准备快照不符时停止，不能覆盖新状态。
        let current = storage::edits(&plan, &operation.targets)?;
        for (before, after) in journal.edits.iter().zip(&current) {
            if before.before != after.before { return Err("客户端在退出时更新了登录态，请重新发起切换；原凭证未被覆盖".into()); }
        }
        journal.phase = "writing".into(); journal.save()?;
        operation.state.lock().map_err(|_| "本机启动状态不可用")?.phase = "writing";
        for index in 0..journal.edits.len() {
            journal.attempted = index + 1;
            journal.save()?;
            let edit = &journal.edits[index];
            edit.target.write(&edit.before, &edit.after)?;
        }
        Ok::<(), String>(())
    })();
    if let Err(error) = prepared {
        let applied = journal.phase == "writing";
        let rollback = if applied { storage::rollback(&journal.edits[..journal.attempted]) } else { Ok(()) };
        journal.phase = if rollback.is_ok() { if applied { "rolled_back" } else { "not_applied" } } else { "recovery_required" }.into();
        let saved = journal.save();
        if rollback.is_ok() && running && operation.manager.is_current()
            && operation.binding.current().is_ok() && windows::running(&operation.executable) == Ok(false) {
            let _ = windows::start(&operation.executable);
        }
        return Err(format!("{error}{}{}", rollback.err().map(|error| format!("；恢复原凭证失败：{error}")).unwrap_or_default(), saved.err().map(|error| format!("；备份状态保存失败：{error}")).unwrap_or_default()));
    }
    journal.phase = "committed".into();
    if let Err(error) = journal.save() {
        let rollback = storage::rollback(&journal.edits[..journal.attempted]);
        journal.phase = if rollback.is_ok() { "rolled_back" } else { "recovery_required" }.into();
        let _ = journal.save();
        return Err(format!("本机切换记录保存失败：{error}{}", rollback.err().map(|error| format!("；恢复原凭证失败：{error}")).unwrap_or_default()));
    }
    {
        let mut state = operation.state.lock().map_err(|_| "本机启动状态不可用")?;
        state.committed = true; state.phase = "starting";
    }
    if let Err(error) = operation.check().and_then(|_| operation.binding.origin()?.check(&accepted)) {
        operation.finish("committed", format!("凭证已切换；{error}"), false);
        return Ok(());
    }
    match windows::start(&operation.executable) {
        Ok(()) => {
            operation.state.lock().map_err(|_| "本机启动状态不可用")?.started = true;
            journal.phase = "started".into();
            match journal.save() {
                Ok(()) => operation.finish("started", "已切换凭证并启动客户端", false),
                Err(_) => operation.finish("started", "客户端已启动，但备份状态保存失败，备份已保留", true),
            }
        }
        Err(error) => operation.finish("committed", error, true),
    }
    Ok(())
}
