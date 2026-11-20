pub(super) mod chatglm;
pub(crate) use chatglm::{prepare as desktop_import_prepare, confirm as desktop_import_confirm,
    cancel as desktop_import_cancel, status as desktop_import_status, ImportError};

use std::collections::HashSet;
use std::time::Duration;
use agent2api_provider_protocol as proto;
use futures::{stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use super::context::Context;
use crate::server::config;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Helper {
    pub id: String,
    pub operations: Vec<String>,
}

fn grants() -> Value {
    config::with_raw(|raw| raw.get("pluginNativeHelpers").cloned()).flatten().unwrap_or_else(|| json!({}))
}

pub(crate) async fn snapshot(pack_id: Option<&str>) -> Value {
    let registry = super::super::PackRegistry::load();
    let owners = super::super::ownership::owners(&registry);
    let all_grants = grants();
    let mut scoped_grants = serde_json::Map::new();
    let mut requested = Vec::new();
    for pack in registry.packs.iter().filter(|pack| pack_id.is_none_or(|id| pack.id == id)) {
        let options = super::super::host_for_pack(&pack.id).and_then(|host| host.options.wasm.clone())
            .or_else(|| {
                let dir = super::super::install::plugins_dir().join(&pack.id).join(&pack.version);
                let manifest = super::super::install::read_manifest(&dir).ok()?;
                super::manifest::read(&dir, &manifest).ok().flatten()
            });
        if let Some(options) = options {
            let provider = &options.provider;
            if owners.get(provider).map(String::as_str) == Some(pack.id.as_str()) {
                let available = super::super::host_for_provider(provider).is_some_and(|host| host.ready())
                    && super::super::serves_provider(provider);
                for id in &options.manifest.helpers {
                    if super::desktop_credentials::helper(provider) == Some(id.as_str()) { continue; }
                    requested.push(json!({ "pack": pack.id, "provider": provider, "id": id,
                        "available": available, "callerDigest": pack.sha256 }));
                }
            }
        }
        if let Some(providers) = all_grants.as_object() {
            for (provider, helpers) in providers {
                let Some(helpers) = helpers.as_object() else { continue };
                for (id, grant) in helpers {
                    if super::desktop_credentials::helper(provider) == Some(id.as_str()) { continue; }
                    if grant["callerPack"].as_str() != Some(pack.id.as_str()) { continue; }
                    scoped_grants.entry(provider.clone()).or_insert_with(|| json!({}))[id] = grant.clone();
                    if !requested.iter().any(|item| item["provider"] == *provider && item["id"] == *id) {
                        requested.push(json!({ "pack": pack.id, "provider": provider, "id": id,
                            "available": false, "callerDigest": pack.sha256 }));
                    }
                }
            }
        }
    }
    let wanted: HashSet<String> = requested.iter().filter(|item| item["available"] == true)
        .filter_map(|item| item["id"].as_str().map(str::to_string)).collect();
    let mut sources = Vec::new();
    if !wanted.is_empty() {
        for pack in super::super::list_packs().into_iter().filter(|pack| pack.enabled) {
            let Some(host) = super::super::host_for_pack(&pack.id).filter(|host| host.ready()) else { continue };
            if host.options.wasm.is_some() || host.options.script { continue; }
            let digest = registry.packs.iter().find(|item| item.id == pack.id).map(|item| item.sha256.clone()).unwrap_or_default();
            for provider in pack.providers.iter().filter(|provider| provider.loaded) {
                sources.push((pack.id.clone(), pack.version.clone(), digest.clone(), provider.id.clone(), host.clone()));
            }
        }
    }
    let mut candidates = Vec::new();
    for item in &requested {
        if let (Some(provider),Some(id)) = (item["provider"].as_str(),item["id"].as_str()) {
            if let Some(candidate) = super::desktop_credentials::candidate(provider,id) { candidates.push(candidate); }
        }
    }
    let mut results = stream::iter(sources).map(|(pack, version, digest, provider, host)| async move {
        let result = host.call_coded("native.helpers", json!({ "provider": provider }), Duration::from_secs(5)).await;
        (pack, version, digest, provider, result)
    }).buffer_unordered(4);
    while let Some((pack, version, digest, provider, result)) = results.next().await {
        let Ok(value) = result else { continue };
        let Ok(helpers) = parse(value) else { continue };
        for helper in helpers.into_iter().filter(|helper| wanted.contains(&helper.id)) {
            candidates.push(json!({ "pack": pack, "version": version, "provider": provider,
                "id": helper.id, "operations": helper.operations, "digest": digest }));
        }
    }
    candidates.sort_by_key(|item| format!("{}:{}:{}", item["pack"], item["provider"], item["id"]));
    json!({ "grants": scoped_grants, "requested": requested, "candidates": candidates })
}

fn parse(value: Value) -> Result<Vec<Helper>, proto::WireError> {
    let helpers: Vec<Helper> = serde_json::from_value(value).map_err(|_| super::errors::protocol())?;
    if helpers.len() > 32 || helpers.iter().any(|helper| helper.id.is_empty() || helper.id.len() > 80 || helper.operations.len() > 32
        || helper.operations.is_empty() || helper.operations.iter().any(|operation| operation.is_empty() || operation.len() > 80)) {
        return Err(super::errors::protocol());
    }
    Ok(helpers)
}

pub(crate) async fn grant(provider: &str, helper: &str, selected: &Value, pack_id: Option<&str>) -> Result<bool, String> {
    let _mutation = super::super::operations::mutation_guard().await;
    if provider.is_empty() || helper.is_empty() || provider.len() > 128 || helper.len() > 80 {
        return Err("插件或辅助能力标识无效".into());
    }
    let mut all = grants();
    if !all.is_object() { return Err("原生辅助授权记录无效".into()); }
    if selected.is_null() {
        let grant = &all[provider][helper];
        if grant.is_null() { return Ok(true); }
        if pack_id.is_some_and(|id| grant["callerPack"].as_str() != Some(id)) {
            return Err("不能修改其他插件的原生辅助授权".into());
        }
        all[provider].as_object_mut().ok_or("授权记录无效")?.remove(helper);
    } else {
        let caller = super::super::host_for_provider(provider).filter(|host| host.ready()).ok_or("Wasm 插件未加载")?;
        let options = caller.options.wasm.as_ref().ok_or("只有 Wasm 插件使用原生辅助授权")?;
        if !options.manifest.helpers.iter().any(|id| id == helper) { return Err("插件没有申报该辅助能力".into()); }
        let caller_pack = super::super::provider_pack(provider).ok_or("插件归属不可用")?;
        if pack_id.is_some_and(|id| id != caller_pack) { return Err("不能修改其他插件的原生辅助授权".into()); }
        let available = snapshot(Some(&caller_pack)).await;
        let candidate = available["candidates"].as_array().into_iter().flatten().find(|item|
            item["pack"] == selected["pack"] && item["provider"] == selected["provider"] && item["id"] == helper
                && item["version"] == selected["version"] && item["digest"] == selected["digest"]
                && item["operations"] == selected["operations"])
            .ok_or("原生辅助程序未安装、未启用或不支持此能力")?;
        if !all[provider].is_object() { all[provider] = json!({}); }
        all[provider][helper] = candidate.clone();
        all[provider][helper]["callerPack"] = json!(caller_pack);
        all[provider][helper]["callerDigest"] = json!(super::super::PackRegistry::load().packs.into_iter()
            .find(|pack| pack.id == caller_pack).map(|pack| pack.sha256));
    }
    Ok(config::update_raw_field("pluginNativeHelpers", all))
}

pub(super) fn authorized_builtin(provider: &str, helper: &str) -> Result<(), proto::WireError> {
    if helper == super::desktop_credentials::HELPER_CATPAW {
        let store = crate::server::core::account_store::plugin_accounts::shared().ok_or_else(super::desktop_credentials::invalid)?;
        if store.plugin_account("desktop-auth").is_some_and(|record| record["provider"] == provider && record["desktop"] == true) { return Ok(()); }
    }
    let grant = grants()[provider][helper].clone();
    let pack = super::super::provider_pack(provider).ok_or_else(|| super::error("插件归属不可用"))?;
    let digest = super::super::PackRegistry::load().packs.into_iter().find(|entry|entry.id==pack).map(|entry|entry.sha256);
    if super::desktop_credentials::candidate(provider,helper).is_none_or(|candidate|
        ["pack","version","provider","id","operations","digest"].iter().any(|key| grant[*key] != candidate[*key]))
        || grant["callerPack"] != pack || grant["callerDigest"].as_str()!=digest.as_deref() {
        let message = if helper == super::desktop_credentials::HELPER_RACCOON_REFRESH {
            "请在小浣熊插件设置里明确授权本机续期与安全写回"
        } else if super::desktop_credentials::refresh::helper(provider) == Some(helper) {
            "请在当前插件设置里明确授权本机续期与安全写回"
        } else if helper == super::qoder_umid::HELPER {
            "请在当前插件设置里明确授权官方设备验证组件"
        } else if helper.starts_with("desktop.device.") {
            "请在当前插件设置里明确授权读取这家客户端的设备标识"
        } else {
            "请在当前插件设置里明确授权主程序读取这家客户端的登录态"
        };
        return Err(proto::WireError::with_code(message, "native_helper_not_authorized"));
    }
    Ok(())
}

impl Context {
    pub(super) async fn native(&mut self, params: &Value) -> Result<Value, proto::WireError> {
        self.permission("native.helpers.v1")?;
        if self.shared.options.provider == "raccoon"
            && params["helper"] == super::desktop_credentials::HELPER_RACCOON_REFRESH {
            return super::desktop_credentials::raccoon_refresh(self, params).await;
        }
        if params["helper"] == super::qoder_umid::HELPER {
            return super::qoder_umid::invoke(self, params).await;
        }
        if super::client_device::helper(&self.shared.options.provider).is_some_and(|helper| params["helper"] == helper) {
            return super::client_device::invoke(self, params).await;
        }
        if params["helper"] == super::quwork_device::HELPER {
            return super::quwork_device::invoke(self, params).await;
        }
        if super::desktop_credentials::refresh::helper(&self.shared.options.provider).is_some_and(|helper| params["helper"] == helper) {
            return super::desktop_credentials::refresh::invoke(self, params).await;
        }
        if params["helper"].as_str() == super::desktop_credentials::helper(&self.shared.options.provider) {
            if params["operation"] == "read" && params["payload"].as_object().is_some_and(|fields| fields.is_empty()) {
                return super::desktop_credentials::invoke(self, &json!({})).await;
            }
            if matches!(self.shared.options.provider.as_str(), "autoclaw" | "autoclaw-intl") && params["operation"] == "decrypt" {
                super::desktop_credentials::authorize_decrypt(self, &params["payload"])?;
                let provider = self.shared.options.provider.clone();
                let payload = params["payload"].clone();
                let result = crate::server::core::blocking::run(move || super::desktop_credentials::decrypt(&provider, &payload)).await
                    .map_err(|_| super::desktop_credentials::invalid())?;
                super::desktop_credentials::authorize_decrypt(self, &params["payload"])?;
                return result;
            }
            return Err(proto::WireError::with_code("固定本机导入不支持该操作", "forbidden"));
        }
        let _mutation = super::super::operations::mutation_guard().await;
        let helper = params["helper"].as_str().unwrap_or("");
        let operation = params["operation"].as_str().unwrap_or("");
        if !self.shared.options.manifest.helpers.iter().any(|id| id == helper) || params.to_string().len() > 256 * 1024 {
            return Err(proto::WireError::with_code("原生辅助调用未申报或输入过大", "forbidden"));
        }
        let grant = grants()[&self.shared.options.provider][helper].clone();
        let caller_digest = super::super::provider_pack(&self.shared.options.provider).and_then(|id|
            super::super::PackRegistry::load().packs.into_iter().find(|pack| pack.id == id).map(|pack| pack.sha256));
        if grant["callerPack"].as_str() != super::super::provider_pack(&self.shared.options.provider).as_deref()
            || grant["callerDigest"].as_str() != caller_digest.as_deref() {
            return Err(proto::WireError::with_code("插件归属已变化，需要重新授权", "native_helper_not_authorized"));
        }
        let pack_id = grant["pack"].as_str().ok_or_else(|| proto::WireError::with_code("请先在当前插件的设置中授权原生辅助程序", "native_helper_not_authorized"))?;
        let provider = grant["provider"].as_str().ok_or_else(super::errors::protocol)?;
        let pack = super::super::list_packs().into_iter().find(|pack| pack.id == pack_id && pack.enabled
            && grant["version"].as_str() == Some(pack.version.as_str()))
            .ok_or_else(|| proto::WireError::with_code("原生辅助程序已更新或不可用，需要重新授权", "native_helper_not_authorized"))?;
        let registry = super::super::PackRegistry::load();
        if registry.packs.iter().find(|item| item.id == pack.id)
            .is_none_or(|item| grant["digest"].as_str() != Some(item.sha256.as_str())) {
            return Err(proto::WireError::with_code("辅助程序包内容已变化，需要重新授权", "native_helper_not_authorized"));
        }
        let host = super::super::host_for_pack(pack_id).ok_or_else(|| super::error("原生辅助执行体不可用"))?;
        let lease = host.activity.acquire().ok_or_else(|| super::error("原生辅助程序正在更新"))?;
        if !host.ready() { return Err(super::error("原生辅助程序尚未就绪")); }
        if host.options.wasm.is_some() || host.options.script
            || super::super::provider_pack(provider).as_deref() != Some(pack.id.as_str()) {
            return Err(proto::WireError::with_code("原生辅助授权目标已变化", "forbidden"));
        }
        let declared = parse(host.call_coded("native.helpers", json!({ "provider": provider }), Duration::from_secs(5)).await?)?;
        if !declared.iter().any(|item| item.id == helper && item.operations.iter().any(|item| item == operation))
            || !grant["operations"].as_array().into_iter().flatten().any(|item| item.as_str() == Some(operation)) {
            return Err(proto::WireError::with_code("原生辅助操作未获授权", "forbidden"));
        }
        let _lease = lease;
        drop(_mutation);
        let result = host.call_coded("native.call", json!({ "provider": provider, "caller": self.shared.options.provider,
            "helper": helper, "operation": operation, "payload": params["payload"], "flow": self.flow }), Duration::from_secs(60)).await?;
        if result.to_string().len() > 256 * 1024 { return Err(super::error("原生辅助结果过大")); }
        Ok(result)
    }
}
