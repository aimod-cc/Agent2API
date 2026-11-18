use std::time::Duration;

use serde_json::Value;

use super::policy::InstallPolicy;
use super::{mutation_guard, set_stage};
use crate::server::core::account_store::plugin_accounts;
use crate::server::core::plugin_host::{self, install::PackRegistry, packages, switching::ChangeScope, update};

pub(super) async fn install(
    prepared: packages::PreparedPack,
    policy: Option<(InstallPolicy, update::PackAsset)>,
) -> Result<Value, String> {
    let id = prepared.id().to_string();
    let store = plugin_accounts::shared().ok_or_else(|| "账号存储尚未就绪，未开始切换插件".to_string())?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        let mutation = tokio::time::timeout_at(deadline, mutation_guard()).await
            .map_err(|_| "另一个插件正在安装或切换，等待超时；未替换原插件，请稍后重试".to_string())?;
        if let Some((policy, asset)) = &policy { policy.validate(&id, asset)?; }
        let before = PackRegistry::load();
        let record = prepared.record(&before);
        let mut after = before.clone();
        match after.packs.iter_mut().find(|pack| pack.id == id) {
            Some(pack) => *pack = record.clone(),
            None => after.packs.push(record.clone()),
        }
        let scope = ChangeScope::between(&before, &after, &id, Some(&record));
        let Some(switch) = scope.begin() else {
            set_stage(&id, "waiting");
            drop(mutation);
            if tokio::time::Instant::now() >= deadline {
                return Err("插件仍有在途请求、登录或后台任务，等待切换超时；未替换原插件，请稍后重试".to_string());
            }
            tokio::time::sleep_until((tokio::time::Instant::now() + Duration::from_millis(500)).min(deadline)).await;
            continue;
        };
        let change = packages::provenance::begin_scope_change(&scope.packs, &id);
        set_stage(&id, "installing");
        if let Err(error) = scope.stop().await {
            let recovery = scope.start(&store).await;
            drop(switch);
            return Err(with_recovery(error, recovery));
        }
        switch.clear_adapters();
        let installed = tokio::task::spawn_blocking(move || prepared.install_pending_changed(change)).await
            .map_err(|error| format!("插件安装任务中断：{error}"))
            .and_then(|result| result);
        let mut transaction = match installed {
            Ok(transaction) => transaction,
            Err(error) => {
                let recovery = scope.start(&store).await;
                drop(switch);
                scope.refresh_catalogs();
                return Err(with_recovery(error, recovery));
            }
        };
        set_stage(&id, "loading");
        let source = transaction.take_start_source();
        let started = scope.start_install(&store, source).await;
        if let Err(error) = &started {
            set_stage(&id, "restoring");
            if let Err(stop) = scope.stop().await {
                switch.clear_adapters();
                return Err(format!("新插件启动失败：{error}；{stop}；旧版本备份已保留"));
            }
            switch.clear_adapters();
            tokio::task::spawn_blocking(move || transaction.rollback()).await
                .map_err(|error| format!("恢复插件任务中断：{error}"))??;
            let recovery = scope.start(&store).await;
            drop(switch);
            scope.refresh_catalogs();
            return Err(with_recovery(format!("新插件启动失败：{error}"), recovery));
        }
        if let Ok(Some(receipt)) = started {
            // 不同事务或重复回执只取消可信绑定，不把普通已加载插件冒充安装失败。
            let _ = transaction.accept_start(receipt);
        }
        tokio::task::spawn_blocking(move || transaction.commit()).await
            .map_err(|error| format!("插件已加载，但提交安装事务失败：{error}"))?;
        drop(switch);
        scope.refresh_catalogs();
        plugin_host::catalog::wake();
        let mut result = plugin_host::management_snapshot();
        result["pack"] = plugin_host::list_packs().into_iter().find(|pack| pack.id == id)
            .map(serde_json::to_value).transpose().map_err(|error| error.to_string())?
            .unwrap_or(Value::Null);
        result["installNotice"] = result["pack"]["unavailableReason"].clone();
        result["hostError"] = Value::Null;
        return Ok(result);
    }
}

fn with_recovery(error: String, recovery: Result<(), String>) -> String {
    match recovery {
        Ok(()) => format!("{error}；已恢复原插件状态"),
        Err(recovery) => format!("{error}；恢复原插件运行失败：{recovery}"),
    }
}
