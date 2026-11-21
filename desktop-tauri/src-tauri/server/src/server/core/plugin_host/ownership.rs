use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::install::{self, InstalledPack, PackRegistry};

pub(super) fn unavailable_reason(pack: &InstalledPack) -> Option<String> {
    super::support::legacy_reason(pack).or_else(|| suspended_reason(pack))
}

pub(super) fn suspended_reason(pack: &InstalledPack) -> Option<String> {
    if super::support::installed_state(pack) != super::PackSupportState::Supported || pack.manifest_providers.is_empty()
        || pack.manifest_providers.iter().any(|provider|
            !crate::server::core::providers::availability::is_suspended(provider.id.as_str())) {
        return None;
    }
    if pack.manifest_providers.iter().all(|provider|
        crate::server::core::providers::availability::is_replaced_by_template(provider.id.as_str())) {
        return Some("Cline 订阅池已改为软件内置「Cline Pass」API 模板，无需再安装或运行此插件；请前往「添加账号 → API 模板」使用 Cline Pass。".into());
    }
    Some("插件文件已安装，但该提供商目前暂停开放，不会启动或参与转发；恢复开放后可重新启用。".into())
}

pub(super) fn reserved(id: &str) -> bool {
    crate::server::core::providers::availability::BUILTIN_SUBSCRIPTION_IDS.contains(&id)
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCategory {
    #[default]
    Agent,
    Web,
}

pub(super) fn category(dir: &Path) -> ProviderCategory {
    std::fs::read_to_string(dir.join("pack.json")).ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|manifest| manifest.get("provider_category").cloned())
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

pub(super) fn owners(registry: &PackRegistry) -> BTreeMap<String, String> {
    owners_with(registry, None)
}

pub(super) fn owners_with(registry: &PackRegistry, prepared: Option<&InstalledPack>) -> BTreeMap<String, String> {
    owners_using(registry, prepared, &super::script::package_for_provider)
}

pub(super) fn display_owners(registry: &PackRegistry, scripts: &std::collections::HashMap<String, String>) -> BTreeMap<String, String> {
    owners_using(registry, None, &|provider| scripts.get(provider).cloned())
}

fn owners_using(registry: &PackRegistry, prepared: Option<&InstalledPack>, script_owner: &impl Fn(&str) -> Option<String>) -> BTreeMap<String, String> {
    let mut owners = BTreeMap::new();
    // 已安装的 WASM 保留归属，停用或加载失败不切换到另一执行体。
    for pack in &registry.packs {
        let prepared = prepared.filter(|item| item.id == pack.id);
        let source = prepared.unwrap_or(pack);
        let supported = prepared.map(|pack| pack.entry.ends_with(".wasm"))
            .unwrap_or_else(|| super::support::installed_state(pack) == super::PackSupportState::Supported);
        if !supported { continue; }
        for provider in source.manifest_providers.iter().map(|item| item.id.as_str()) {
            if !reserved(provider) && !crate::server::core::providers::availability::is_suspended(provider)
                && script_owner(provider).is_none() {
                owners.insert(provider.to_string(), pack.id.clone());
            }
        }
    }
    owners
}

pub(super) fn overridden_by(owners: &BTreeMap<String, String>, pack: &str, provider: &str) -> Option<String> {
    super::script::package_for_provider(provider)
        .or_else(|| owners.get(provider).cloned())
        .filter(|owner| owner != pack)
}

pub(super) fn declared_ids(pack: &InstalledPack) -> Vec<String> {
    let dir = install::plugins_dir().join(&pack.id).join(&pack.version);
    install::read_manifest(&dir).map(|manifest| manifest.providers)
        .unwrap_or_else(|_| pack.manifest_providers.clone())
        .into_iter().map(|provider| provider.id.0).collect()
}

