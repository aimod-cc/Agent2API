//! 模型的管理视图和对外视图。所有对外出口使用相同的开启绑定集合。

use std::collections::HashSet;

use serde_json::{json, Map, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::capability;
use crate::server::core::key_scope::{self, KeyScope};
use crate::server::core::model_meta;
use crate::server::core::model_rules;
use crate::server::core::models::{list_item, list_response_from, model_id, suggest_from};

use super::routing::{builtin_target, entry_id_in_manifest};
use super::{active_manifests, aggregate_source, managed_manifests, manifest_for_provider, refresh_meta_for_provider};

/// 一家会对外广告哪些名字：清单里原生的上游 id + **已启用**的映射别名。
///
/// ── `enabled` 这一道过滤不能少（曾经的漏洞）──────────────────
/// 漏掉它时，被关掉的别名仍会进候选名，然后在 [`builtin_target`] 里绕开
/// 「映射分支」的启停判定走另一条路：`entry_id_in_manifest` 按 id 找不到会
/// **回落到比对上一条目的展示名（`name`）**，命中后按 `default_enabled` 判定
/// —— 那与别名映射的开关无关。于是「关了映射、`/v1/models` 里却还在」。
/// 实测：`space-bunny-alpha`（与展示名逐字相同）会中，`mimo-v2.6-flash`
/// （展示名是 `Mimo V2.6 Flash`，比不中）不会中 —— 同一个开关两种结果。
/// 模块头那句「所有对外出口使用相同的开启绑定集合」说的就是这里必须过滤。
fn advertised_names(
    rules: &model_rules::ModelRules,
    provider: &str,
    manifest: &[Value],
) -> Vec<String> {
    let mut names: Vec<String> = manifest.iter().map(model_id).collect();
    names.extend(rules.mappings.iter()
        .filter(|mapping| mapping.enabled)
        .filter(|mapping| mapping.provider.as_deref().map_or(true, |owner| owner.eq_ignore_ascii_case(provider)))
        .map(|mapping| mapping.alias.clone()));
    names
}

/// 先在各提供商内判断绑定，再按对外名去重；不能先合并上游 ID 再拼别名，
/// 否则某家关闭的映射会借另一家的同名上游重新出现在列表中。
fn public_builtin_items(active: &[(String, Vec<Value>)]) -> Vec<(String, Value)> {
    let rules = model_rules::shared();
    let mut claimed = std::collections::HashMap::<String, usize>::new();
    let mut result: Vec<(String, Value)> = Vec::new();
    for (provider, manifest) in active {
        let names = advertised_names(&rules, provider, manifest);
        for name in names {
            let Some(wire) = builtin_target(&rules, provider, manifest, &name) else { continue };
            let Some(upstream) = manifest.iter().find(|item| model_id(item) == wire.model) else { continue };
            let mut item = list_item(upstream, provider);
            if let Some(index) = claimed.get(&name.to_lowercase()).copied() {
                let existing = &mut result[index].1;
                if item.get("supports_image_generation").and_then(Value::as_bool) == Some(true)
                    && existing.get("kind").and_then(Value::as_str) == Some("image")
                { existing["supports_image_generation"] = Value::Bool(true); }
                if item.get("supports_image_editing").and_then(Value::as_bool) == Some(true)
                    && existing.get("kind").and_then(Value::as_str) == Some("image")
                {
                    existing["supports_image_editing"] = Value::Bool(true);
                    existing["supports_images"] = Value::Bool(true);
                    existing["input_modalities"] = json!(["text", "image"]);
                }
                continue;
            }
            claimed.insert(name.to_lowercase(), result.len());
            if let Some(object) = item.as_object_mut() {
                object.insert("id".to_string(), Value::String(name.clone()));
                if !name.eq_ignore_ascii_case(&wire.model) {
                    object.insert("name".to_string(), Value::String(name));
                    object.insert("is_default".to_string(), Value::Bool(false));
                }
            }
            result.push((provider.to_string(), item));
        }
    }
    result
}

pub fn models_response(store: &AccountStore, scope: Option<&KeyScope>) -> Value {
    models_response_with_active(store, scope, active_manifests(store))
}

pub(super) fn models_response_with_active(
    store: &AccountStore,
    scope: Option<&KeyScope>,
    active: Vec<(String, Vec<Value>)>,
) -> Value {
    let active: Vec<_> = active.into_iter()
        .filter(|(provider, _)| key_scope::allows_provider(scope, provider))
        .collect();
    let (source, refreshed_at) = aggregate_source(&active);
    let mut data: Vec<_> = public_builtin_items(&active).into_iter()
        .map(|(_, item)| item)
        .filter(|item| key_scope::allows_model(scope, &model_id(item)))
        .collect();
    let builtin_count = data.len();
    super::super::custom::append_models_response(store, scope, &mut data);
    crate::server::core::routing_groups::append_public(store, scope, &mut data);
    let source = if data.len() > builtin_count { "aggregate" } else { source };
    list_response_from(data, source, refreshed_at)
}

/// 一家可用提供商都没有时（没加账号），入口校验要跳过「广告里有才放行」与
/// Key 白名单两条判定，把「没有可用账号」那句更可操作的错误留给转发层 ——
/// 与改造前的两条例外逐字同源（见 `pipeline::resolve_model` 的说明）。
///
/// 插件与本体 runtime ID 同样算可用，不能仅因它们没有旧枚举身份而跳过校验。
pub fn has_available_providers(store: &AccountStore) -> bool {
    super::validation_snapshot(store).available
}

pub fn advertised_model_ids(store: &AccountStore) -> Vec<String> {
    super::validation_snapshot(store).models.clone()
}

pub fn advertised_manifest_contains(store: &AccountStore, model: &str) -> bool {
    super::validation_snapshot(store).models.iter().any(|id| id.eq_ignore_ascii_case(model))
}

pub fn suggest_advertised(store: &AccountStore, model: &str, limit: usize) -> Vec<String> {
    suggest_from(advertised_model_ids(store), model, limit)
}

pub fn session_models(store: &AccountStore) -> Vec<Value> {
    models_response(store, None).get("data").and_then(Value::as_array)
        .cloned().unwrap_or_default().into_iter().map(|mut item| {
            let provider = item.get("owned_by").and_then(Value::as_str).unwrap_or("").to_string();
            let is_default = item.get("is_default").cloned().unwrap_or(Value::Bool(false));
            if let Some(object) = item.as_object_mut() {
                object.insert("providerLabel".to_string(), Value::String(super::super::label_of(&provider)));
                object.insert("provider".to_string(), Value::String(provider));
                object.insert("isDefault".to_string(), is_default);
            }
            item
        }).collect()
}

pub fn models_by_provider(store: &AccountStore) -> Value {
    let mut map = Map::new();
    for (provider, manifest) in active_manifests(store) {
        let mut names: Vec<_> = public_builtin_items(&[(provider.clone(), manifest)]).into_iter()
            .map(|(_, item)| model_id(&item)).collect();
        names.sort_by_key(|name| name.to_lowercase());
        map.insert(provider, json!(names));
    }
    for (provider, items) in super::super::custom::catalog_providers(store) {
        let mut names: Vec<_> = items.iter().map(model_id).collect();
        names.sort_by_key(|name| name.to_lowercase());
        map.insert(provider, json!(names));
    }
    for group in crate::server::config::routing_profiles().groups.values() {
        for member in group.members.iter().filter(|member| member.included) {
            if let Some(names) = map.get_mut(&member.provider).and_then(Value::as_array_mut) {
                if !names.iter().any(|name| name.as_str().is_some_and(|name| name.eq_ignore_ascii_case(&group.model))) {
                    names.push(group.model.clone().into());
                }
            }
        }
    }
    Value::Object(map)
}

/// 管理视图里的一行模型（`models` 数组的一项）+ 它对应的默认映射。
#[allow(clippy::too_many_arguments)]
fn push_managed_model(
    rules: &model_rules::ModelRules,
    models: &mut Vec<Value>,
    mappings: &mut Vec<Value>,
    attached: &mut HashSet<usize>,
    claimed: &mut HashSet<(String, String)>,
    provider: &str,
    source: &str,
    refreshed_at: i64,
    manifest: &[Value],
    item: Value,
) {
    let id = model_id(&item);
    if id.is_empty() {
        return;
    }
    if !claimed.insert((provider.to_string(), id.to_lowercase())) {
        return; // 同家同 id 的重复条目（见 `manage_view` 里 claimed 的说明）
    }
    let default = rules.binding(provider, &id, &id);
    let enabled = rules.default_enabled(provider, &id);
    mappings.push(json!({
        "alias": id, "target": id, "provider": provider, "enabled": enabled,
        "reasoning": default.and_then(|mapping| mapping.reasoning.clone()),
        "isDefault": true, "dangling": false, "carried": true,
    }));
    let mut aliases = Vec::new();
    for (index, mapping) in rules.mappings.iter().enumerate() {
        if !mapping.target.eq_ignore_ascii_case(&id)
            || !mapping.provider.as_deref().map_or(true, |owner| owner.eq_ignore_ascii_case(provider))
        { continue; }
        attached.insert(index);
        if mapping.alias.eq_ignore_ascii_case(&id)
            || aliases.iter().any(|alias: &String| alias.eq_ignore_ascii_case(&mapping.alias))
        { continue; }
        let Some(effective) = rules.binding(provider, &mapping.alias, &id) else { continue };
        aliases.push(mapping.alias.clone());
        mappings.push(json!({
            "alias": effective.alias, "target": id, "provider": provider,
            "enabled": rules.binding_enabled(provider, &effective.alias, &id, manifest), "reasoning": effective.reasoning,
            "isDefault": false, "dangling": false, "carried": true,
        }));
    }
    let source = if item.get("_manualModel").and_then(Value::as_bool) == Some(true) {
        "manual"
    } else { source };
    // 覆盖键列表从本次规则快照里查，不逐行重新获取配置。
    let cap_overrides: Vec<String> = rules
        .capabilities
        .iter()
        .find(|entry| entry.matches(provider, &id))
        .map(|entry| capability::overridden_keys(&entry.values))
        .unwrap_or_default();
    models.push(json!({
        "id": id, "name": item.get("name"), "credits": item.get("credits"),
        "isDefault": item.get("isDefault").and_then(Value::as_bool).unwrap_or(false),
        "provider": provider, "providerLabel": super::super::label_of(provider),
        "source": source, "enabled": enabled, "aliases": aliases,
        "capabilities": capability::effective(&item),
        "kind": if super::super::images::model_is_image(&item) { json!("image") }
            else { item.get("kind").cloned().unwrap_or(json!("chat")) },
        "output_modalities": item.get("output_modalities"),
        "supports_image_generation": super::super::images::provider_model_supports_generation(provider, &item),
        "supports_image_editing": super::super::images::provider_model_supports_editing(provider, &item),
        "capOverrides": cap_overrides,
        // 每个能力键的**来源**（`upstream` / `modelsdev` / `user`，见
        // `core::model_meta` 的模块头）。管理页据它把「上游实测」「社区
        // 库补的」「用户改的」三档分开显示 —— 没有它，六个键看起来一样
        // 可信，而它们的可信度差着量级。缺失（没有这个键）= 那一项没有
        // 任何来源（未声明）。
        "capSources": item.get("capSources"),
        // 模型**自己能配哪些思考档位**（出口键名，由 `core::model_meta`
        // 在清单成型处归一：这家自己声明的 > models.dev 的 effort 列表）。
        // 与上面 `mappings[].reasoning`（用户给某条映射**指定**的档位）
        // 是两件事：这里回答「这个模型支持哪些档」，那里回答「这条映射
        // 用哪一档」。缺失（null）= 该模型未声明，界面据此回落通用档位表。
        "reasoningLevels": item.get("reasoningLevels"),
        "reasoningDefaultLevel": item.get("reasoningDefaultLevel"),
        "reasoningFixedLevel": item.get("reasoningFixedLevel"),
        "reasoningBindingSupported": item.get("reasoningBindingSupported"),
        "reasoningSource": item.get("reasoningSource"),
        // 这一家的清单是什么时候拉到的（毫秒；0 = 从未成功拉过）。
        // 缓存恢复的清单与刚拉的清单**都是 `remote`**，时效只能靠这个
        // 时间戳说明（见 `providers::catalog_cache` 的模块头）。
        "refreshedAt": refreshed_at,
        // **可见性**：这个模型在官方客户端里是否会被列出。
        //
        // `"shown"` / `"hidden"` / `"undeclared"` 三态，由各家 `models.rs`
        // 在自己的解析层算出（判据各不相同：Antigravity 按世代、Qoder 读
        // 上游 `enable`、Raccoon 读 `visible`、CodeArts 读 `display_enabled`）。
        // 聚合层不重新判断 —— 它不认识各家的规则，只负责透传。
        //
        // 缺失（`null`）= 这家没有可见性判据（当前是除上面四家之外的所有
        // provider）。界面据此显示 `—`，**不**把它当成「可见」或「不可见」：
        // 「这家没这个信息」和「这家说它不可见」是两件事。
        "upstreamStatus": item.get("upstreamStatus"),
        "visibility": item.get("visibility").cloned().unwrap_or(Value::Null),
    }));
}

/// 每条原始 ID 都返回一个默认绑定；历史的同名映射合并进默认绑定，避免两个开关
/// 控制同一个请求名。默认绑定不可删除，关闭不影响这一行的其他别名。
///
/// ── 能力位（`capabilities` / `capOverrides`）─────────────────
/// 每行带两个能力字段（管理页的「上下文 / 输出」与「能力」两列读它们）：
///   · `capabilities`：**生效值**（清单原值 + 用户覆盖，六项齐全，
///     `null` = 未声明 —— 与「不支持 / 0」区分开，用户编辑时要靠它分辨
///     「不知道」与「明确不支持」）；
///   · `capOverrides`：被用户覆盖的键（管理页据此给「已改」标记；
///     空数组 = 全部继承清单原值）。
/// 清单原值已由 `manifest_for_provider` 应用过覆盖，所以这里读到的就是生效值。
/// 某个 `(provider, target)` 还能不能在管理页上落到一行模型行。
///
/// 判据与 [`manage_view`] 把映射标成 `dangling` 的那条路径**同源**：行的来源是
/// [`managed_manifests`]（有可用账号的家），带 `provider` 的映射只挂在自己那家、
/// 旧版全局条目（provider 缺失）挂到任何承载 target 的家。
///
/// ── 用途：自映射（默认绑定）能不能删 ─────────────────────────
/// `POST /api/models/mappings/remove` 用它判断一条 `alias == target` 的默认绑定
/// 能不能删。默认绑定跟着模型行走，正常只能关、不能删；但**模型行已经不在了**
/// （上游把模型下线、那家被移除或没有可用账号）时，它的开关没有落点，删除是唯一
/// 出路 —— 管理页的「未挂载的模型映射」正是给这批条目留的入口（见 [`manage_view`]
/// 末尾的 dangling 分支）。这里与视图共用同一份「有没有行」的判据，免得出现
/// 「界面显示为未挂载、点删除却被后端拒掉」的死锁。
pub(crate) fn host_row_exists(store: &AccountStore, provider: Option<&str>, target: &str) -> bool {
    managed_manifests(store)
        .into_iter()
        .any(|(owner, manifest)| {
            provider.map_or(true, |provider| owner.eq_ignore_ascii_case(provider))
                && manifest.iter().any(|item| model_id(item).eq_ignore_ascii_case(target))
        })
}

pub fn manage_view(store: &AccountStore) -> Value {
    let rules = model_rules::shared();
    let mut models = Vec::new();
    let mut mappings = Vec::new();
    let mut providers = Vec::new();
    let mut attached = HashSet::new();
    // `(provider, id 小写)` 的出口去重（修复 #177 的兜底一层）。
    //
    // 各家的目录解析**应**在源头去重（如 Trae 的 `parse_catalog` 按
    // `config_name`），但上游形态随时可能变化、别家未必有同样的去重 —— 管理页
    // 是「清单的可视检查」，同 id 画出两行会连带下面那条默认映射也重复一条。
    // 这一层独立于源头：不管哪家目录出了脏数据，这里只认第一次出现的那条。
    let mut claimed: HashSet<(String, String)> = HashSet::new();
    // 本视图里**真正出现了的家**（= 有可用账号的家）。悬空映射的归属家不在这份名单里
    // 时就不列它 —— 页面上根本没有这一家（左栏也不会有它），把它名下的映射列出来
    // 只会多出一批用户在这儿无从下手的条目（要管那家得先去账号页把它加回来）。
    let mut managed_providers = HashSet::new();
    for (provider, mut manifest) in managed_manifests(store) {
        managed_providers.insert(provider.to_lowercase());
        manifest.sort_by_key(|item| !rules.default_enabled(&provider, &model_id(item)));
        let (remote, refreshed_at) = refresh_meta_for_provider(&provider);
        let source = if remote { "remote" } else { "builtin" };
        let count = manifest.iter().map(model_id).filter(|id| !id.is_empty())
            .map(|id| id.to_lowercase()).collect::<HashSet<_>>().len();
        providers.push(json!({
            "provider": provider,
            "providerLabel": super::super::label_of(&provider),
            "count": count,
            "source": if count == 0 { "none" } else { source },
            "refreshedAt": refreshed_at,
        }));
        for item in manifest.iter().cloned() {
            push_managed_model(
                &rules, &mut models, &mut mappings, &mut attached, &mut claimed,
                &provider, source, refreshed_at, &manifest, item,
            );
        }
    }
    for (index, mapping) in rules.mappings.iter().enumerate() {
        if attached.contains(&index) { continue; }
        // 归属的家不在本视图里就跳过（见 `managed_providers` 的说明）。旧版全局
        // 条目（provider 缺失）不属于任何一家，不受这条约束 —— 它是「对所有家生效」
        // 的遗留形态，没有「哪一家没出现」这回事。
        if let Some(provider) = mapping.provider.as_deref() {
            if !managed_providers.contains(&provider.to_lowercase()) {
                continue;
            }
        }
        let carried = match mapping.provider.as_deref() {
            Some(provider) => entry_id_in_manifest(&manifest_for_provider(provider), &mapping.target).is_some(),
            None => !super::providers_for_model(&mapping.target).is_empty(),
        };
        let enabled = mapping.provider.as_deref().map_or(mapping.enabled, |provider| {
            rules.binding_enabled(provider, &mapping.alias, &mapping.target, &manifest_for_provider(provider))
        });
        mappings.push(json!({
            "alias": mapping.alias, "target": mapping.target, "provider": mapping.provider,
            "enabled": enabled, "reasoning": mapping.reasoning,
            "isDefault": mapping.alias.eq_ignore_ascii_case(&mapping.target),
            "dangling": true, "carried": carried,
        }));
    }
    json!({
        "models": models,
        "mappings": mappings,
        "providers": providers,
        "reasoningLevels": model_rules::REASONING_LEVELS,
        // models.dev 元数据的当前状态（`builtin` / `cache` / `network` + 生成
        // 时刻 + 条数）。它服务的是「这份档位/能力是什么时候的数据」这个问题 ——
        // 界面上那些档位来自哪一份、新不新，只能由这里回答。
        "modelsDev": model_meta::status(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `from_raw` 收的是整份 config（它自己取 `modelRules` 键），所以包一层
    fn rules_with(mappings: Value) -> model_rules::ModelRules {
        let raw = json!({ "modelRules": { "mappings": mappings } });
        model_rules::ModelRules::from_raw(raw.as_object().expect("对象字面量"))
    }

    /// 一条展示名与别名逐字相同的上游条目（`space-bunny-alpha` 是实测命中的那个）
    fn manifest() -> Vec<Value> {
        vec![json!({ "id": "stealth/space-bunny-alpha", "name": "space-bunny-alpha" })]
    }

    /// 被关掉的映射别名不能进广告候选名。漏了这道过滤时，别名会经由
    /// `builtin_target` 的「展示名回落」绕开启停判定 —— 于是「关了映射，
    /// `/v1/models` 里却还在」（2026-10-03 实测复现）。
    #[test]
    fn disabled_mapping_alias_is_not_advertised() {
        let rules = rules_with(json!([
            { "alias": "space-bunny-alpha", "target": "stealth/space-bunny-alpha",
              "provider": "cline-free", "enabled": false }
        ]));
        let names = advertised_names(&rules, "cline-free", &manifest());
        assert!(
            !names.iter().any(|name| name == "space-bunny-alpha"),
            "关掉的别名不该出现在候选名里: {names:?}"
        );
        // 原生 id 不受影响（开关只管别名）
        assert!(names.iter().any(|name| name == "stealth/space-bunny-alpha"));
    }

    /// 开着的映射别名照旧进候选 —— 不能让这次修复把功能一起修没
    #[test]
    fn enabled_mapping_alias_is_advertised() {
        let rules = rules_with(json!([
            { "alias": "space-bunny-alpha", "target": "stealth/space-bunny-alpha",
              "provider": "cline-free", "enabled": true }
        ]));
        let names = advertised_names(&rules, "cline-free", &manifest());
        assert!(names.iter().any(|name| name == "space-bunny-alpha"));
    }

    /// 缺 `enabled` 键 = 开关功能上线前的历史条目 = 默认启用（升级不改行为）
    #[test]
    fn mapping_without_enabled_key_is_advertised() {
        let rules = rules_with(json!([
            { "alias": "space-bunny-alpha", "target": "stealth/space-bunny-alpha",
              "provider": "cline-free" }
        ]));
        let names = advertised_names(&rules, "cline-free", &manifest());
        assert!(names.iter().any(|name| name == "space-bunny-alpha"));
    }

    /// 别家的映射别名不该混进这一家的候选（provider 过滤仍在）
    #[test]
    fn other_providers_alias_is_not_included() {
        let rules = rules_with(json!([
            { "alias": "x-alias", "target": "stealth/space-bunny-alpha",
              "provider": "workbuddy", "enabled": true }
        ]));
        let names = advertised_names(&rules, "cline-free", &manifest());
        assert!(!names.iter().any(|name| name == "x-alias"));
    }
}
