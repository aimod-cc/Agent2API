use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{json, Value};

use crate::server::core::plugin_host::{startup, PackRegistry};
use crate::server::core::providers;

impl super::AccountStore {
    pub(crate) fn plugin_account_summary(&self) -> Value {
        let snapshot = self.routing_snapshot();
        let booting = startup::booting();
        let mut counts = BTreeMap::new();
        let mut hidden = BTreeMap::new();
        for account in &snapshot.public_accounts {
            let provider = crate::server::core::routing::provider_of(account);
            *counts.entry(provider.to_string()).or_insert(0usize) += 1;
            if !super::store_view::account_list_visible(provider, booting) {
                *hidden.entry(provider.to_string()).or_insert(0usize) += 1;
            }
        }
        json!({ "counts": counts, "hidden": summary(hidden) })
    }
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum HiddenReason {
    NotInstalled,
    PackDisabled,
    ProviderDisabled,
    Loading,
    LoadFailed,
    LoadTimeout,
    NotLoaded,
    LegacyUnsupported,
    ReplacedByTemplate,
}

fn reason_of(provider: &str, registry: &PackRegistry) -> HiddenReason {
    if providers::availability::is_replaced_by_template(provider) {
        return HiddenReason::ReplacedByTemplate;
    }
    if let Some(state) = startup::provider(provider) {
        if !state.pack_enabled { return HiddenReason::PackDisabled; }
        if !state.enabled { return HiddenReason::ProviderDisabled; }
        return match state.startup.phase {
            startup::Phase::Failed => HiddenReason::LoadFailed,
            startup::Phase::Timeout => HiddenReason::LoadTimeout,
            phase if phase.pending() || phase == startup::Phase::Ready => HiddenReason::Loading,
            _ => HiddenReason::NotLoaded,
        };
    }
    if let Some(state) = crate::server::core::plugin_host::script::provider_state(provider) {
        return match state {
            "pack_disabled" => HiddenReason::PackDisabled,
            "provider_disabled" => HiddenReason::ProviderDisabled,
            _ => HiddenReason::NotLoaded,
        };
    }
    if registry.packs.iter().any(|pack| {
        pack.manifest_providers.iter().any(|item| item.id.as_str() == provider)
            && crate::server::core::plugin_host::support_state(pack)
                == crate::server::core::plugin_host::PackSupportState::LegacyUnsupported
    }) && !registry.packs.iter().any(|pack| {
        pack.manifest_providers.iter().any(|item| item.id.as_str() == provider)
            && crate::server::core::plugin_host::support_state(pack)
                == crate::server::core::plugin_host::PackSupportState::Supported
    }) { return HiddenReason::LegacyUnsupported; }
    let mut reason = HiddenReason::NotInstalled;
    for pack in &registry.packs {
        if crate::server::core::plugin_host::support_state(pack)
            != crate::server::core::plugin_host::PackSupportState::Supported {
            continue;
        }
        if !pack
            .manifest_providers
            .iter()
            .any(|item| item.id.as_str() == provider)
        {
            continue;
        }
        if pack.enabled {
            if pack.providers.get(provider).copied().unwrap_or(true) {
                return HiddenReason::NotLoaded;
            }
            reason = HiddenReason::ProviderDisabled;
        } else if matches!(reason, HiddenReason::NotInstalled) {
            reason = HiddenReason::PackDisabled;
        }
    }
    reason
}

/// 只返回数量和提供商元数据，不返回隐藏账号的记录或凭证。
pub(super) fn summary(counts: BTreeMap<String, usize>) -> Value {
    let total = counts.values().sum::<usize>();
    let mut details = Vec::with_capacity(counts.len());
    if !counts.is_empty() {
        let registry = PackRegistry::load();
        for (provider, count) in &counts {
            let state = startup::provider(provider);
            let mut row = json!({
                "id": provider,
                "label": providers::label_of(provider),
                "count": count,
                "reason": reason_of(provider, &registry),
                "packId": state.as_ref().map(|state| &state.pack_id),
                "startup": state.as_ref().map(|state| &state.startup),
            });
            if let Some(template_key) = providers::availability::replaced_template_key(provider) {
                row["templateKey"] = json!(template_key);
            }
            details.push(row);
        }
    }
    json!({
        "total": total,
        "byProvider": counts,
        "providers": details,
    })
}
