mod component;
mod discovery;
mod runner;

use std::sync::atomic::{AtomicBool, Ordering};

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{context::Context, native};

pub(super) const HELPER: &str = "desktop.qoder-umid.v1";
static INSTALLING: AtomicBool = AtomicBool::new(false);

pub(super) fn candidate(provider: &str, id: &str) -> Option<Value> {
    if provider != "qoder-intl" || id != HELPER { return None; }
    Some(json!({ "pack": "agent2api-host", "version": "qoder-umid-v1", "provider": provider,
        "id": id, "operations": ["status", "install", "identity"], "digest": "qoder-umid-v1" }))
}

pub(super) fn invalid() -> proto::WireError {
    proto::WireError::with_code("Qoder 设备验证组件缺失、来源校验失败或执行未完成，请检查组件授权和官方安装",
        "qoder_umid_unavailable")
}

pub(super) fn status() -> Result<Value, proto::WireError> {
    let found = component::discover()?;
    let authorized = native::authorized_builtin("qoder-intl", HELPER).is_ok();
    Ok(json!({ "available": found.is_some() && authorized, "componentAvailable": found.is_some(),
        "source": found.as_ref().map(|item| item.source), "installSupported": component::supported(),
        "installing": INSTALLING.load(Ordering::Acquire), "authorized": authorized,
        "message": if authorized { "" } else { "Qoder 国际版插件未就绪或未声明设备验证组件" },
        "dailyCampaignsReady": found.is_some() && authorized }))
}

pub(super) async fn invoke(context: &mut Context, params: &Value) -> Result<Value, proto::WireError> {
    context.permission("native.helpers.v1")?;
    if context.shared.options.provider != "qoder-intl" || params["helper"] != HELPER
        || !context.shared.options.manifest.helpers.iter().any(|id| id == HELPER) {
        return Err(invalid());
    }
    match params["operation"].as_str() {
        Some("status") if params["payload"].as_object().is_some_and(|fields| fields.is_empty()) => status(),
        Some("install") if params["payload"].as_object().is_some_and(|fields| fields.is_empty()) => {
            native::authorized_builtin("qoder-intl", HELPER)?;
            if INSTALLING.swap(true, Ordering::AcqRel) { return Err(invalid()); }
            struct Installing;
            impl Drop for Installing { fn drop(&mut self) { INSTALLING.store(false, Ordering::Release); } }
            let _installing = Installing;
            component::install(context.shared.cancel.clone()).await
        }
        Some("identity") if params["payload"].as_object().is_some_and(|fields| fields.len() == 1 && fields.contains_key("uid")) => {
            identity(context, &params["payload"]).await
        }
        _ => Err(invalid()),
    }
}

async fn identity(context: &mut Context, params: &Value) -> Result<Value, proto::WireError> {
    native::authorized_builtin("qoder-intl", HELPER)?;
    let account = context.account()?.to_string();
    let before = context.shared.callbacks.store_get("qoder-intl".into(), account.clone()).await?;
    let access = ["accessToken", "access", "token", "access_token"].iter()
        .find_map(|key| before["secret"][*key].as_str().filter(|value| !value.is_empty())).ok_or_else(invalid)?;
    let expected_uid = params["uid"].as_str().filter(|uid| !uid.is_empty() && uid.len() <= 256
        && !uid.chars().any(char::is_control)).ok_or_else(invalid)?;
    // 活动uid属于上游活动响应，不从插件传入值或登录userId构造账号归属。
    let url = "https://openapi.qoder.sh/sash/api/v1/me/campaigns?forceRefresh=true";
    context.shared.options.check_url(url, false)?;
    let selected = crate::server::core::proxies::ResolvedProxy::from_json(&context.proxy).map_err(|_| invalid())?;
    let client = crate::server::core::egress::managed::FrozenHttpClient::from_selected(selected).await.map_err(|_| invalid())?;
    let work = async {
        let mut response = client.client().map_err(|_| invalid())?.get(url)
            .header("Authorization", format!("Bearer {access}"))
            .header("Cosy-ClientType", "10").header("Accept", "application/json").send().await.map_err(|_| invalid())?;
        if !response.status().is_success() || response.url().as_str() != url { return Err(invalid()); }
        let mut bytes = zeroize::Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| invalid())? {
            if bytes.len().saturating_add(chunk.len()) > 4 * 1024 * 1024 { return Err(invalid()); }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = super::managed_task::declaration::parse_object(&bytes, 4 * 1024 * 1024).map_err(|_| invalid())?;
        let uid = value["data"]["uid"].as_str().or_else(|| value["uid"].as_str()).ok_or_else(invalid)?;
        if uid != expected_uid { return Err(invalid()); }
        Ok(uid.to_string())
    };
    let uid = tokio::time::timeout_at(context.call_deadline, work).await.map_err(|_| invalid())??;
    let component = component::discover()?.ok_or_else(invalid)?;
    let digest = format!("{:x}", Sha256::digest(before["secret"].to_string()));
    let result = runner::identity(&component, &account, &uid, &digest,
        context.shared.cancel.clone(), context.call_deadline).await?;
    let after = context.shared.callbacks.store_get("qoder-intl".into(), account).await?;
    if ["secret", "identity", "desktop"].iter().any(|key| after[*key] != before[*key]) { return Err(invalid()); }
    native::authorized_builtin("qoder-intl", HELPER)?;
    let platform = match std::env::consts::OS { "windows" => "win32", "macos" => "darwin", other => other };
    let architecture = match std::env::consts::ARCH { "aarch64" => "arm64", other => other };
    let mut headers = json!({ "Cosy-MachineToken": result["machineToken"], "Cosy-MachineCode": result["machineCode"],
        "Cosy-MachineType": result["machineType"], "Cosy-MachineOS": format!("{architecture}_{platform}"),
        "Cosy-Version": component.version });
    let hostname = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).ok();
    if let Some(hostname) = hostname.filter(|value| !value.is_empty() && value.len() <= 96 && value.is_ascii()
        && !value.chars().any(char::is_control)) { headers["Cosy-MachineHostname"] = json!(hostname); }
    if let Some(machine) = before["secret"]["machineId"].as_str().filter(|value| !value.is_empty() && value.len() <= 256) {
        headers["Cosy-MachineId"] = json!(machine);
    }
    Ok(json!({ "headers": headers, "uid": uid }))
}
