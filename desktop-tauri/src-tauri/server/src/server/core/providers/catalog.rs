//! 聚合模型目录：提供商的原始清单、对外绑定及发送名解析。
//!
//! 原始模型 ID 是默认绑定，别名是额外绑定。各绑定独立开关；关闭默认绑定
//! 不会禁用它指向的上游模型，也不会影响仍开启的别名。
//! 清单来源在本文件，绑定解析在 routing.rs，对外与管理视图在 view.rs。

mod routing;
mod snapshot;
mod view;

pub(crate) use snapshot::{config_changed, generation, invalidate, validation_snapshot, ValidationCache};

use serde_json::Value;

use crate::server::core::account_store::AccountStore;
use crate::server::core::model_meta;
use crate::server::core::model_rules;
use crate::server::core::models::model_id;
use crate::server::core::providers::adapter::adapter_by_id;
use crate::server::core::providers::{kind_from_id, kind_id, ProviderKind, PROVIDERS};

pub(crate) use routing::entry_id_in_manifest;
pub use routing::{
    default_model_catalog, default_model_usable, forwarding_providers, model_blocked_everywhere,
    providers_for_model, wire_target_for_provider, WireTarget,
};
pub use view::{
    advertised_manifest_contains, advertised_model_ids, has_available_providers, manage_view,
    models_by_provider, models_response, session_models, suggest_advertised,
};
/// 「这条映射在管理页上还有没有模型行可挂」——删除自映射时的放行判据（见函数说明）。
/// 只服务写侧一个调用点，不进对外的 `pub use`。
pub(crate) use view::host_row_exists;

pub(crate) fn upstream_manifests() -> Vec<(String, Vec<Value>)> {
    all_provider_ids().into_iter().map(|provider| {
        let models = adapter_by_id(&provider).map(|adapter| adapter.list_models()).unwrap_or_default();
        (provider, models)
    }).collect()
}

/// 原始能力清单不应用绑定开关；关闭原始 ID 后，别名仍需解析到这条上游记录。
/// 用户自建模型、models.dev 补齐与能力覆盖都在这一处应用，插件与内置身份共用。
pub(crate) fn manifest_for_provider(provider: &str) -> Vec<Value> {
    let Some(adapter) = adapter_by_id(provider) else {
        return Vec::new();
    };
    let mut models = adapter.list_models();
    for item in model_rules::custom_models_for(provider) {
        let id = model_id(&item);
        if !id.is_empty()
            && !models.iter().any(|existing| model_id(existing).eq_ignore_ascii_case(&id))
        {
            let mut item = item;
            item["_manualModel"] = Value::Bool(true);
            models.push(item);
        }
    }
    for item in models.iter_mut() {
        model_meta::apply(item);
    }
    model_rules::apply_capability_overrides(provider, &mut models);
    models
}

/// 编译期身份的调用方仍按 kind 查询，执行体优先取已安装的插件。
pub(crate) fn manifest_for(kind: ProviderKind) -> Vec<Value> {
    manifest_for_provider(kind_id(kind))
}

/// 旧内置目录已迁入插件，网关不再持有按 kind 划分的刷新元数据。
pub(crate) fn refresh_meta(_kind: ProviderKind) -> (bool, i64) {
    (false, 0)
}

pub(crate) fn refresh_meta_for_provider(provider: &str) -> (bool, i64) {
    if crate::server::core::plugin_host::serves_provider(provider) {
        let meta = crate::server::core::plugin_host::model_catalog_meta(provider);
        return (meta.remote, meta.refreshed_at);
    }
    kind_from_id(provider).map(refresh_meta).unwrap_or_default()
}

fn all_provider_ids() -> Vec<String> {
    let mut ids = super::executable_provider_ids();
    for provider in PROVIDERS {
        if !ids.iter().any(|id| id.eq_ignore_ascii_case(provider.id)) {
            ids.push(provider.id.to_string());
        }
    }
    ids
}

pub fn provider_available(store: &AccountStore, provider: &str) -> bool {
    store.routing_snapshot().available_providers.contains(provider)
}

/// 管理视图需要完整清单；对外视图在此基础上再应用每条绑定的开关。
pub(crate) fn managed_manifests(store: &AccountStore) -> Vec<(String, Vec<Value>)> {
    let accounts = store.routing_snapshot();
    all_provider_ids()
        .into_iter()
        .filter(|provider| accounts.available_providers.contains(provider))
        .map(|provider| {
            let manifest = manifest_for_provider(&provider);
            (provider, manifest)
        })
        .collect()
}

pub fn active_manifests(store: &AccountStore) -> Vec<(String, Vec<Value>)> {
    managed_manifests(store).into_iter().filter_map(|(provider, manifest)| {
        let manifest = adapter_by_id(&provider)
            .map(|adapter| adapter.advertise_models(store, manifest))
            .unwrap_or_default();
        (!manifest.is_empty()).then_some((provider, manifest))
    }).collect()
}

fn aggregate_source(active: &[(String, Vec<Value>)]) -> (&'static str, i64) {
    match active {
        [(provider, _)] => {
            let (remote, refreshed_at) = refresh_meta_for_provider(provider);
            (if remote { "remote" } else { "builtin" }, refreshed_at)
        }
        [] => ("none", 0),
        _ => ("aggregate", active.first().map(|(provider, _)| refresh_meta_for_provider(provider).1).unwrap_or(0)),
    }
}
