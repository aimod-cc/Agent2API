use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::install::{self, InstalledPack, PackRegistry};

pub(super) const WEB_PACK_ID: &str = "agent2api-web-providers";
const LEGACY_PACK_ID: &str = "agent2api-providers";

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
    let mut owners = BTreeMap::new();
    for pack in registry.packs.iter().filter(|pack| pack.enabled) {
        let ids = match prepared.filter(|prepared| prepared.id == pack.id) {
            Some(prepared) => prepared.manifest_providers.iter().map(|provider| provider.id.0.clone()).collect(),
            None => declared_ids(pack),
        };
        for provider in ids.into_iter().filter(|id| {
            !reserved(id) && !crate::server::core::providers::availability::is_suspended(id)
        }) {
            owners.entry(provider).or_insert_with(|| pack.id.clone());
        }
    }
    // 旧包仍包含 chatglm 时，新包接管后不得因停用或加载失败回退到旧执行体。
    if registry.packs.iter().any(|pack| pack.id == WEB_PACK_ID) {
        owners.insert("chatglm".to_string(), WEB_PACK_ID.to_string());
    }
    owners
}

pub(super) fn declared_ids(pack: &InstalledPack) -> Vec<String> {
    let dir = install::plugins_dir().join(&pack.id).join(&pack.version);
    install::read_manifest(&dir).map(|manifest| manifest.providers)
        .unwrap_or_else(|_| pack.manifest_providers.clone())
        .into_iter().map(|provider| provider.id.0).collect()
}

pub(super) fn migrated_enabled(registry: &PackRegistry, pack: &str, provider: &str) -> Option<bool> {
    if pack != WEB_PACK_ID || provider != "chatglm" { return None; }
    registry.packs.iter().find(|pack| pack.id == LEGACY_PACK_ID)
        .and_then(|pack| pack.providers.get(provider).copied())
}
