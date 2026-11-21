//! 本机续期只使用原账号绑定的固定文件和固定协议；写回前再次核对原始材料。

mod cline;
mod source;
mod protocols;

use std::sync::{Arc, Mutex, OnceLock};
use std::collections::HashMap;

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};
use tokio::sync::Mutex as AsyncMutex;

use super::{account_snapshot, invalid};
use super::super::{context::{Context, Shared}, native};
use crate::server::core::account_store::plugin_accounts;

pub(in crate::server::core::plugin_host::wasm) fn helper(provider: &str) -> Option<&'static str> {
    match provider {
        "kiro" => Some("desktop.refresh.kiro.v1"),
        "autoclaw" => Some("desktop.refresh.autoclaw.v1"),
        "autoclaw-intl" => Some("desktop.refresh.autoclaw-intl.v1"),
        "cline-free" => Some("desktop.refresh.cline-free.v1"),
        _ => None,
    }
}

pub(super) fn candidate(provider: &str, id: &str) -> Option<Value> {
    if helper(provider) != Some(id) { return None; }
    Some(json!({"pack":super::HOST,"version":"desktop-refresh-v1","provider":provider,
        "id":id,"operations":["status","refresh"],"digest":"desktop-refresh-v1"}))
}

fn locks() -> &'static Mutex<HashMap<String, Arc<AsyncMutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(in crate::server::core::plugin_host::wasm) struct Job {
    pub(in crate::server::core::plugin_host::wasm) shared: Arc<Shared>,
    account: String,
    reference: Value,
    provider: String,
    helper: String,
    pub(in crate::server::core::plugin_host::wasm) proxy: Value,
    pub(in crate::server::core::plugin_host::wasm) user_agent: String,
    pub(in crate::server::core::plugin_host::wasm) deadline: tokio::time::Instant,
}

impl Job {
    pub(in crate::server::core::plugin_host::wasm) fn current(&self) -> Result<(), proto::WireError> {
        if self.shared.cancel.is_cancelled() { return Err(invalid()); }
        native::authorized_builtin(&self.provider, &self.helper)?;
        let store = plugin_accounts::shared().ok_or_else(invalid)?;
        let record = store.plugin_account(&self.account).ok_or_else(invalid)?;
        let projection = super::account_projection(&record);
        if projection != self.reference { return Err(changed()); }
        Ok(())
    }
}

pub(super) fn changed() -> proto::WireError {
    proto::WireError::with_code("本机账号或登录文件已经变化，未覆盖新的登录态", "desktop_identity_changed")
}

pub(in crate::server::core::plugin_host::wasm) async fn invoke(context: &mut Context, params: &Value) -> Result<Value, proto::WireError> {
    let provider = context.shared.options.provider.clone();
    let id = helper(&provider).ok_or_else(invalid)?;
    if params["helper"] != id || !context.shared.options.manifest.helpers.iter().any(|item| item == id) {
        return Err(invalid());
    }
    let reference = account_snapshot(context)?.ok_or_else(invalid)?;
    let provider_copy = provider.clone();
    let reference_copy = reference.clone();
    let source = crate::server::core::blocking::run(move || source::Source::read(&provider_copy, &reference_copy)).await
        .map_err(|_| invalid())??;
    if account_snapshot(context)?.as_ref() != Some(&reference) { return Err(changed()); }
    if params["operation"] == "status" && params["payload"].as_object().is_some_and(|fields| fields.is_empty()) {
        return Ok(json!({"authorized":native::authorized_builtin(&provider,id).is_ok(),
            "refreshable":source.refreshable(),"sourceRevision":source.revision()}));
    }
    if params["operation"] != "refresh" || params["payload"].as_object().is_none_or(|fields|
        fields.len() != 1 || !fields.contains_key("sourceRevision"))
        || params["payload"]["sourceRevision"] != source.revision() { return Err(changed()); }
    native::authorized_builtin(&provider,id)?;
    let lock = {
        let mut all = locks().lock().map_err(|_| invalid())?;
        all.retain(|_, lock| Arc::strong_count(lock) > 1);
        all.entry(source.lock_key()).or_insert_with(|| Arc::new(AsyncMutex::new(()))).clone()
    };
    let guard = tokio::time::timeout_at(context.call_deadline, lock.lock_owned()).await.map_err(|_| invalid())?;
    let host = super::super::super::host_for_provider(&provider).ok_or_else(invalid)?;
    let lease = host.activity.acquire().ok_or_else(invalid)?;
    let job = Job { shared:context.shared.clone(), account:context.account.clone(), reference,
        provider, helper:id.into(), proxy:context.proxy.clone(), user_agent:context.user_agent.clone().unwrap_or_default(),
        deadline:context.call_deadline.min(tokio::time::Instant::now()+std::time::Duration::from_secs(30)) };
    crate::spawn_task(async move {
        let _guard = guard;
        let _lease = lease;
        job.current()?;
        let current = source::Source::read(&job.provider,&job.reference)?;
        if current.revision() != source.revision() { return Ok(json!({"updated":false,"sourceRevision":current.revision()})); }
        if !current.refreshable() { return Err(proto::WireError::with_code("本机登录态缺少续期材料，请重新登录官方客户端", "desktop_refresh_material_missing")); }
        if matches!(job.provider.as_str(), "autoclaw" | "autoclaw-intl") {
            // 本机文件锁已串行化换证和写回，历史失败不限制后续调度。
            let next = protocols::refresh(&job, &current.credentials).await?;
            job.current()?;
            let reference = job.reference.clone();
            let result = crate::server::core::blocking::run(move || current.persist(&next, &reference)).await
                .map_err(|_| invalid())??;
            if result["updated"] != true { return Err(changed()); }
            return Ok(result);
        }
        let store = plugin_accounts::shared().ok_or_else(invalid)?;
        let before = store.plugin_state_strict(&job.provider,&job.account,"host.desktop-refresh.v1").map_err(|_| invalid())?;
        let stamp = current.refresh_stamp();
        if before["value"]["stamp"] == stamp { return Err(proto::WireError::with_code("上次续期结果尚未确认，未重复使用一次性刷新凭据，请重新登录官方客户端", "desktop_refresh_unconfirmed")); }
        let revision = before["revision"].as_u64().ok_or_else(invalid)?;
        if store.plugin_state_compare_exchange_strict(&job.provider,&job.account,"host.desktop-refresh.v1",revision,1,
            json!({"stamp":stamp})).map_err(|_| invalid())?["updated"] != true { return Err(changed()); }
        let next = protocols::refresh(&job,&current.credentials).await?;
        job.current()?;
        let reference = job.reference.clone();
        let result = crate::server::core::blocking::run(move || current.persist(&next, &reference)).await.map_err(|_| invalid())??;
        if result["updated"] != true { return Err(changed()); }
        let reserved = store.plugin_state_strict(&job.provider,&job.account,"host.desktop-refresh.v1").map_err(|_| invalid())?;
        if reserved["value"]["stamp"] != stamp
            || store.plugin_state_compare_exchange_strict(&job.provider,&job.account,"host.desktop-refresh.v1",
                reserved["revision"].as_u64().ok_or_else(invalid)?,1,Value::Null).map_err(|_| invalid())?["updated"] != true {
            return Err(invalid());
        }
        Ok(result)
    }).await.map_err(|_| invalid())?
}
