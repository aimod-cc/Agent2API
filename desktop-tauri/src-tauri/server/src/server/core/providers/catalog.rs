//! 聚合模型目录：内置提供商的原始清单、对外绑定及发送名解析。
//!
//! 原始模型 ID 是默认绑定，别名是额外绑定。各绑定独立开关；关闭默认绑定
//! 不会禁用它指向的上游模型，也不会影响仍开启的别名。
//! 清单来源在本文件，绑定解析在 routing.rs，对外与管理视图在 view.rs。

mod routing;
mod view;

use serde_json::Value;

use crate::server::core::account_store::AccountStore;
use crate::server::core::model_meta;
use crate::server::core::model_rules;
use crate::server::core::models::model_id;
use crate::server::core::providers::adapter::adapter_for;
use crate::server::core::providers::{kind_from_id, kind_id, ProviderKind, PROVIDERS};

pub use routing::{
    default_model_catalog, default_model_usable, forwarding_providers, model_blocked_everywhere,
    providers_for_model, wire_target_for_provider, wire_targets_for_provider, WireTarget,
};
pub use view::{
    advertised_manifest_contains, advertised_model_ids, has_available_providers, manage_view,
    models_by_provider, models_response, session_models, suggest_advertised,
};

/// 原始能力清单不应用绑定开关；关闭原始 ID 后，别名仍需解析到这条上游记录。
///
/// **models.dev 的补齐**（`core::model_meta`）在**这里**统一应用，分两步：
///   ① 只填清单项里**上游没声明**的六个能力键（优先级见那个模块头：用户覆盖 >
///      上游 > models.dev）；
///   ② 把逐模型思考档位归一成出口那一对键（这家自己声明的 > models.dev 的）。
/// 位置在用户覆盖**之前** —— 覆盖层是「用户明确指定」，永远最高，它顺带把被改
/// 的键的来源标成 `user`（见 `model_rules::apply_capability_overrides`）。
///
/// 用户的能力位覆盖（`modelRules.capabilities`）同样在**这里**统一应用：本函数
/// 是内置家全部出口（`/v1/models`、Anthropic 列表、管理视图）的清单源头，收在
/// 一处就不会出现「管理页改了、下游没变」的分叉。覆盖只改元数据键，不参与
/// 路由 / 启停（见 `model_rules` 模块头的能力位覆盖一节）。
/// 这一家的清单（适配器的 `list_models` + 用户自建模型 + models.dev 能力补齐
/// + 逐模型的启用/能力覆盖）。
///
/// `pub(crate)`：`/api/health` 与 `/api/system_info` 要报「模型条数」——
/// 那个读数的旧口径是「进程里那一份 workbuddy 目录的条数」，现在目录没有了，
/// 报同一家的聚合清单长度才与旧值同义。
pub(crate) fn manifest_for(kind: ProviderKind) -> Vec<Value> {
    let mut models = adapter_for(kind).list_models();
    for item in model_rules::custom_models_for(kind_id(kind)) {
        let id = model_id(&item);
        if !id.is_empty()
            && !models.iter().any(|existing| model_id(existing).eq_ignore_ascii_case(&id))
        {
            models.push(item);
        }
    }
    for item in models.iter_mut() {
        model_meta::apply(item);
    }
    model_rules::apply_capability_overrides(kind_id(kind), &mut models);
    models
}

fn advertised_manifest_for(store: &AccountStore, kind: ProviderKind) -> Vec<Value> {
    adapter_for(kind).advertise_models(store, manifest_for(kind))
}

/// 这一家清单的「远程与否 + 拉取时刻」。
///
/// 两个消费方：本模块的对外视图（`meta.source` / 管理页的「来源」列）与
/// **适配器层的手动刷新结果**（`adapter::refresh_implemented_forced` 的
/// `refreshedAt`，界面「更新日期」列读它）—— 后者必须走这里而不是自己按
/// kind 拼一遍：十家的取值路径（workbuddy 按地区分格、按地区分格的三家、
/// Cline 的池）各不相同，抄一份就是一处会漂移的知识。
pub(crate) fn refresh_meta(kind: ProviderKind) -> (bool, i64) {
    // ── 全部都是一样的答案，这是事实而不是偷懒 ──────────────────
    // 每一家的目录拉取与它的缓存格都随实现搬到了提供商插件（`models.refresh`
    // 由插件执行、进度也由插件报）。网关这边**没有任何一家的缓存可读**，
    // 所以对任何 kind 都只能如实回「不是远程来源、从没刷过」。
    //
    // 保留 `kind` 参数（而不是改成零参）是为了让调用点继续按家提问 ——
    // 将来若真有一家的缓存要落回网关，改这里一处即可；也避免调用点误以为
    // 「目录来源与 provider 无关」。
    let _ = kind;
    (false, 0)
}

fn all_kinds() -> Vec<ProviderKind> {
    PROVIDERS.iter().filter_map(|meta| kind_from_id(meta.id)).collect()
}

pub fn provider_available(store: &AccountStore, kind: ProviderKind) -> bool {
    !store.accounts_for_provider(kind_id(kind)).is_empty()
        || adapter_for(kind).env_credentials_present()
}

/// 管理视图需要完整清单；对外视图在此基础上再应用每条绑定的开关。
pub fn active_manifests(store: &AccountStore) -> Vec<(ProviderKind, Vec<Value>)> {
    all_kinds()
        .into_iter()
        .filter(|kind| provider_available(store, *kind))
        .filter_map(|kind| {
            let manifest = advertised_manifest_for(store, kind);
            (!manifest.is_empty()).then_some((kind, manifest))
        })
        .collect()
}

fn aggregate_source(active: &[(ProviderKind, Vec<Value>)]) -> (&'static str, i64) {
    match active {
        [(kind, _)] => {
            let (remote, refreshed_at) = refresh_meta(*kind);
            (if remote { "remote" } else { "builtin" }, refreshed_at)
        }
        [] => ("none", 0),
        _ => ("aggregate", active.first().map(|(kind, _)| refresh_meta(*kind).1).unwrap_or(0)),
    }
}
