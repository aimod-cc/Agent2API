//! 模型探测结束后只同步插件清单，不再打一次上游目录接口。

use std::sync::Arc;
use std::time::Duration;

use agent2api_provider_protocol as proto;
use serde_json::Value;

use super::{rpc_cache, runtime, Host};
use crate::server::logging;

pub(super) fn notify(payload: &Value) {
    let Some(provider) = payload.get("provider").and_then(Value::as_str)
        .filter(|value| !value.is_empty()) else { return };
    if super::registry::is_switching(provider) { return; }
    let Some(host) = runtime::host_for_provider(provider) else { return };
    let Some(lease) = host.activity.acquire() else { return };
    let provider = provider.to_string();
    let account = payload.get("account").and_then(Value::as_str).unwrap_or("").to_string();
    crate::spawn_task(async move {
        let _lease = lease;
        if let Err(error) = sync(host, &provider, &account).await {
            logging::log("Plugin", &format!("{provider} 模型探测结果同步失败：{error}"));
        }
    });
}

async fn sync(host: Arc<Host>, provider: &str, account: &str) -> Result<(), String> {
    let value = host.call(
        proto::methods::gateway::MODELS_LIST,
        serde_json::json!({ "provider": provider, "account": account }),
        Duration::from_secs(15),
    ).await?;
    let Some(current) = runtime::host_for_provider(provider) else { return Ok(()) };
    if !Arc::ptr_eq(&host, &current) { return Ok(()) }
    let list = serde_json::from_value::<proto::ModelList>(value)
        .map_err(|error| format!("插件模型清单无法解析：{error}"))?;
    if !list.meta.remote { return Ok(()) }
    rpc_cache::set_remote(provider, list);
    crate::server::core::providers::adapter::seed_hidden_visibility_defaults();
    Ok(())
}
