use serde_json::{json, Value};

use super::{active, mutation_guard};
use crate::server::core::account_store::plugin_accounts;
use crate::server::core::plugin_host::{self, install::PackRegistry, switching::ChangeScope};
use crate::server::errors::GatewayError;

pub enum PackChange {
    Uninstall,
    Enabled(bool),
    Provider { id: String, enabled: bool },
}

pub async fn change(id: String, change: PackChange) -> Result<Value, GatewayError> {
    // HTTP 连接取消不能把已经开始的宿主切换留在半途。
    crate::spawn_task(async move { apply(&id, change).await }).await
        .map_err(|error| GatewayError::with_status(500, format!("插件变更任务中断：{error}")))?
}

async fn apply(id: &str, change: PackChange) -> Result<Value, GatewayError> {
    let _mutation = mutation_guard().await;
    if active(id) { return Err(GatewayError::with_status(409, "这个插件正在下载或安装")); }
    let store = plugin_accounts::shared()
        .ok_or_else(|| GatewayError::with_status(503, "账号存储尚未就绪，未变更插件"))?;
    let before = PackRegistry::load();
    let mut after = before.clone();
    let pack = after.packs.iter_mut().find(|pack| pack.id == id)
        .ok_or_else(|| GatewayError::with_status(404, "没有安装这个插件包"))?;
    match &change {
        PackChange::Uninstall => after.packs.retain(|pack| pack.id != id),
        PackChange::Enabled(enabled) => pack.enabled = *enabled,
        PackChange::Provider { id, enabled } => {
            if !pack.manifest_providers.iter().any(|provider| provider.id.as_str() == id) {
                return Err(GatewayError::bad_request("这个插件包里没有指定的提供商"));
            }
            pack.providers.insert(id.clone(), *enabled);
        }
    }
    let scope = ChangeScope::between(&before, &after, id, None);
    let switch = scope.begin().ok_or_else(|| GatewayError::with_status(
        409, "插件仍有在途请求、登录或后台任务，未执行变更，请稍后重试",
    ))?;
    if let Err(error) = scope.stop().await {
        let _ = scope.start(&store).await;
        return Err(GatewayError::with_status(503, error));
    }
    switch.clear_adapters();
    let outcome = match change {
        PackChange::Uninstall => {
            let id = id.to_string();
            tokio::task::spawn_blocking(move || plugin_host::uninstall(&id)).await
                .map_err(|error| format!("卸载任务中断：{error}")).and_then(|result| result)
        }
        _ => after.save(),
    };
    let loaded = scope.start(&store).await;
    drop(switch);
    scope.refresh_catalogs();
    outcome.map_err(GatewayError::bad_request)?;
    let mut result = plugin_host::management_snapshot();
    result["ok"] = json!(true);
    result["hostError"] = loaded.err().map(Value::String).unwrap_or(Value::Null);
    Ok(result)
}
