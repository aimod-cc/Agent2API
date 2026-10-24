//! 对外模型名到上游模型的解析。路由与发送侧共用本解析，避免选中一家后再换一套规则。
//!
//! ── 一个名字可以解析出**多个**目标（候选链）──────────────────
//! `target_candidates` 返回的是**有序列表**而不是单个答案：同一 alias 在同一家
//! 挂多条映射时，它们全部成为候选、按顺序尝试（撞限额 / 路由不存在就换下一个
//! —— 由 `upstream::provider_loop` 驱动）。这与「同一别名在多家各挂一条」
//! 的跨家主备是同一个算法，只是把候选粒度从「家」细化到「家 × 上游模型」。
//! 单个答案的入口（`wire_target_for_provider` / `builtin_target`）保留，
//! 取链上第一位 —— 于是**不消费候选链的调用方行为逐字不变**。

use serde_json::Value;

use crate::server::core::{custom_providers, model_rules};
use crate::server::core::models::model_id;
use crate::server::core::providers::{kind_from_id, kind_id, ProviderKind};

use super::{adapter_for, all_kinds, manifest_for};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WireTarget {
    pub model: String,
    pub reasoning: Option<String>,
}

pub(super) fn entry_id_in_manifest(manifest: &[Value], requested: &str) -> Option<String> {
    let requested = requested.trim();
    if requested.is_empty() {
        return None;
    }
    // 保留展示名兼容，但必须先匹配原始 ID；返回清单里的实际大小写。
    manifest.iter().find(|entry| model_id(entry).eq_ignore_ascii_case(requested))
        .or_else(|| manifest.iter().find(|entry| {
            entry.get("name")
                .map(crate::server::core::models::shape_value_text)
                .is_some_and(|name| name.eq_ignore_ascii_case(requested))
        }))
        .map(model_id)
}

pub(super) fn builtin_target(
    rules: &model_rules::ModelRules,
    kind: ProviderKind,
    manifest: &[Value],
    requested: &str,
) -> Option<WireTarget> {
    target_candidates(rules, kind, manifest, requested).into_iter().next()
}

/// 一家对某请求名的**全部**候选目标，按应当尝试的顺序排列。
///
/// ── 为什么需要「全部」而不是「一个」──────────────────────────
/// 同一 alias 在同一家挂多条映射（各自指向不同上游模型）时，旧实现只返回
/// **一个**：按配置顺序的第一个。于是「同名多模型」在一家之内只有先建的那条
/// 生效，其余全是影子条目 —— 而跨家（多家各挂一条同名映射）本来就一起进候选
/// 链主备切换。本函数把家内的多条映射也摊平成一条候选链，家内与家外因此走
/// **同一个算法**：按顺序试，撞限额/路由不存在就换下一个（消费方见
/// `upstream::provider_loop` 的 target 循环）。
///
/// ── 顺序的约定（第一位必须逐字等于旧的唯一答案）────────────────
/// 向后兼容的关键在这里：`target_candidates(...).first()` 等于旧
/// `builtin_target` 的返回值。否则存量配置的行为会在升级后悄悄改变 ——
/// 那是本项目最忌讳的一类回归。旧实现的挑选顺序是
/// 「原生 → Cline 池前缀项 → 第一条带 provider 的 → 第一条」，
/// 因此摊平成链时按同一优先级分三段：
///   ① **原生承载**（清单里有这个 id 且默认绑定启用）：旧实现在这一步就
///      直接返回，所以它必须**单独占满整条链** —— 后面的映射一条都不进
///      （「原生路由优先、映射是追加的兜底路」的既有语义）；
///   ② Cline 的**池前缀**匹配项（target 带本池的通道前缀，见
///      `cline::models::Pool::target_prefix`）；
///   ③ 其余带 `provider` 的映射项（按 `modelRules.mappings` 的配置顺序）；
///   ④ `provider` 缺失的旧版全局条目（同等条件下排最后）。
///
/// `enabled=false` 的映射一条都不进链（那条别名暂时不存在的既有语义）。
///
/// 链长 = 该 alias 在这家上的映射条数（由用户在管理页配置，天然有界）；
/// 每条目标都是**不同的上游模型**，因而也是不同的额度桶 —— 这正是
/// 「撞一个模型的限额就换另一个模型」成立的前提。
pub(super) fn target_candidates(
    rules: &model_rules::ModelRules,
    kind: ProviderKind,
    manifest: &[Value],
    requested: &str,
) -> Vec<WireTarget> {
    let provider = kind_id(kind);
    // ⓪ 原生承载：命中就到此为止（见函数头 ①）
    if let Some(id) = entry_id_in_manifest(manifest, requested) {
        if rules.default_enabled(provider, &id) {
            return vec![WireTarget {
                reasoning: rules.binding(provider, &id, &id).and_then(|m| m.reasoning.clone()),
                model: id,
            }];
        }
    }
    let candidates: Vec<_> = rules.mappings.iter()
        .filter(|mapping| mapping.alias.eq_ignore_ascii_case(requested))
        .filter(|mapping| !mapping.alias.eq_ignore_ascii_case(&mapping.target))
        .filter(|mapping| mapping.provider.as_deref().map_or(true, |id| id == provider))
        .filter(|mapping| rules.binding(provider, &mapping.alias, &mapping.target)
            .is_some_and(|effective| effective.enabled))
        .filter_map(|mapping| {
            entry_id_in_manifest(manifest, &mapping.target).map(|target| (mapping, target))
        })
        .collect();
    let prefix = super::super::cline::models::Pool::from_provider_id(provider)
        .map(super::super::cline::models::Pool::target_prefix);
    // 分组顺序即尝试顺序（见函数头 ①②③④）。组内保持 `rules.mappings` 的
    // 配置顺序 —— 稳定分区，不用 sort（那会按 target 字典序重排候选，
    // 与旧实现「先建先用」的口径分叉）。
    let mut out: Vec<WireTarget> = Vec::with_capacity(candidates.len());
    let mut rest: Vec<&(&model_rules::Mapping, String)> = Vec::with_capacity(candidates.len());
    let mut tail: Vec<&(&model_rules::Mapping, String)> = Vec::new();
    for entry in &candidates {
        match (entry.0.provider.is_some(), prefix.is_some_and(|p| entry.1.starts_with(p))) {
            // ② 池前缀优先级最高（Cline 两条池现在是两家 provider，
            //    带本池前缀的 target 才是这一家真正认的发送名）
            (true, true) => out.push(wire_of(rules, provider, entry)),
            // ③ 其余带 provider 的条目先进缓冲，② 全部收完再按原序接上
            (true, false) => rest.push(entry),
            // ④ 旧版全局条目垫底
            (false, _) => tail.push(entry),
        }
    }
    out.extend(rest.iter().map(|entry| wire_of(rules, provider, entry)));
    out.extend(tail.iter().map(|entry| wire_of(rules, provider, entry)));
    out
}

/// 一条候选 → 发送目标（思考等级跟着那条映射走，与旧实现同源）。
fn wire_of(
    rules: &model_rules::ModelRules,
    provider: &str,
    entry: &(&model_rules::Mapping, String),
) -> WireTarget {
    WireTarget {
        model: entry.1.clone(),
        reasoning: rules.binding(provider, &entry.0.alias, &entry.0.target)
            .and_then(|effective| effective.reasoning.clone()),
    }
}

/// 原始能力查询不应用开关，供管理 API 展开旧版全局规则。
///
/// 自定义家的那一段同样**不应用开关**：它回答「谁登记了这个名字」，而
/// `mappings` 里的别名也是登记过、可被请求的名字（与模型 ID 同列）。
/// 只看 `models[]` 会让「按映射名测这一家」被判成「清单里没有这个模型」——
/// 而按别名请求恰恰是自定义家最常见的用法（见 `custom_providers` 的模块头）。
pub fn providers_for_model(model: &str) -> Vec<String> {
    let model = model.trim();
    if model.is_empty() {
        return Vec::new();
    }
    let manifests: Vec<_> = all_kinds().into_iter().map(|kind| (kind, manifest_for(kind))).collect();
    let exact = manifests.iter().any(|(_, items)| {
        items.iter().any(|item| model_id(item).eq_ignore_ascii_case(model))
    });
    let mut result: Vec<_> = manifests.into_iter().filter(|(_, items)| {
        if exact {
            items.iter().any(|item| model_id(item).eq_ignore_ascii_case(model))
        } else {
            entry_id_in_manifest(items, model).is_some()
        }
    }).map(|(kind, _)| kind_id(kind).to_string()).collect();
    result.extend(custom_providers::list().into_iter().filter_map(|provider| {
        let models = provider.get("models")?.as_array()?;
        let declared = models.iter().any(|entry| model_id(entry).eq_ignore_ascii_case(model));
        // 映射别名也算「这一家认识这个名字」：别名是对外可请求的名字，
        // 判定口径与 `carriers_of_model` 的「能力」那一半一致。
        let mapped = provider
            .get("mappings")
            .and_then(Value::as_array)
            .is_some_and(|mappings| {
                mappings.iter().any(|mapping| {
                    mapping
                        .get("alias")
                        .and_then(Value::as_str)
                        .is_some_and(|alias| alias.eq_ignore_ascii_case(model))
                })
            });
        (declared || mapped)
            .then(|| provider.get("id").and_then(Value::as_str).unwrap_or("").to_string())
    }));
    // 插件家：清单由它们在握手 / 刷新时申报。不加这一段，「按映射名建到插件家」
    // 与「模型规则页里选插件家」都会被判成「这家不认识这个名字」。
    for id in crate::server::core::plugin_host::installed_ids() {
        let Some(adapter) = crate::server::core::plugin_host::lookup(&id) else {
            continue;
        };
        let declared = adapter
            .list_models()
            .iter()
            .any(|entry| model_id(entry).eq_ignore_ascii_case(model));
        let mapped = model_rules::current()
            .mappings
            .iter()
            .any(|mapping| {
                mapping.provider.as_deref() == Some(id.as_str())
                    && mapping.alias.eq_ignore_ascii_case(model)
            });
        if declared || mapped {
            result.push(id);
        }
    }
    result
}

/// 只返回有开启绑定的提供商，不把关闭的 ID 回落给默认提供商。
pub fn forwarding_providers(model: &str) -> Vec<String> {
    let rules = model_rules::current();
    let mut result: Vec<_> = all_kinds().into_iter().filter(|kind| {
        builtin_target(&rules, *kind, &manifest_for(*kind), model).is_some()
    }).map(|kind| kind_id(kind).to_string()).collect();
    result.extend(custom_providers::carriers_of_model(model));
    // 插件家：名字改写由插件自己做（它知道自己的目录），网关这边只要能回答
    // 「这个名字归这家」即可。判据与 `providers_for_model` 的插件段一致。
    for id in crate::server::core::plugin_host::installed_ids() {
        if !rules.default_enabled(&id, model) {
            continue;
        }
        let Some(adapter) = crate::server::core::plugin_host::lookup(&id) else {
            continue;
        };
        let declared = adapter
            .list_models()
            .iter()
            .any(|entry| model_id(entry).eq_ignore_ascii_case(model));
        let mapped = rules.mappings.iter().any(|mapping| {
            mapping.provider.as_deref() == Some(id.as_str())
                && mapping.alias.eq_ignore_ascii_case(model)
                && mapping.enabled
        });
        if declared || mapped {
            result.push(id);
        }
    }
    result
}

pub fn model_blocked_everywhere(model: &str) -> bool {
    if !forwarding_providers(model).is_empty() {
        return false;
    }
    !providers_for_model(model).is_empty()
        || model_rules::current().has_alias(model)
        || custom_providers::list().iter().any(|provider| {
            provider.get("mappings").and_then(Value::as_array).is_some_and(|mappings| {
                mappings.iter().any(|mapping| mapping.get("alias").and_then(Value::as_str)
                    .is_some_and(|alias| alias.eq_ignore_ascii_case(model)))
            })
        })
}

pub fn wire_target_for_provider(model: &str, provider_id: &str, _account: Option<&Value>) -> WireTarget {
    let requested = model.trim();
    // 自定义提供商在自己的转发模块解析一次，避免把别名提前改写后丢失其等级绑定。
    let resolved = kind_from_id(provider_id).and_then(|kind| {
        builtin_target(&model_rules::current(), kind, &manifest_for(kind), requested)
    });
    resolved.unwrap_or_else(|| WireTarget { model: requested.to_string(), reasoning: None })
}

/// 一家对某请求名的**全部**候选目标（有序，见 [`target_candidates`] 的顺序约定）。
///
/// 消费方是转发层的 target 循环：按位逐个尝试，模型作用域的失败（限额 /
/// 路由不存在）换下一位。解析不出（请求名为空 / 该家既不原生承载、也没有
/// 映射指向它）时返回**空表** —— 与 [`wire_target_for_provider`] 的兜底口径
/// 不同：那边「发请求名」总能构造一次发送，这边没有可尝试的候选，调用方
/// 据此跳过循环、退回单目标路径（发送名原样 = 请求名，行为不变）。
///
/// `_account` 与 [`wire_target_for_provider`] 同名同义：候选链是按家算的，
/// 同家的账号共享同一份清单与规则（账号唯一影响发送时的池判定，见
/// `provider_loop` 的 send_cache 键），发送侧因此不按账号重算。
pub fn wire_targets_for_provider(model: &str, provider_id: &str, _account: Option<&Value>) -> Vec<WireTarget> {
    let requested = model.trim();
    if requested.is_empty() {
        return Vec::new();
    }
    kind_from_id(provider_id)
        .map(|kind| target_candidates(&model_rules::current(), kind, &manifest_for(kind), requested))
        .unwrap_or_default()
}

pub fn default_model_catalog() -> Vec<Value> {
    let rules = model_rules::current();
    let mut result = Vec::new();
    for kind in all_kinds().into_iter().filter(|kind| adapter_for(*kind).supports_default_model()) {
        for entry in manifest_for(kind) {
            let id = model_id(&entry);
            if !id.is_empty() && rules.default_enabled(kind_id(kind), &id)
                && !result.iter().any(|item| model_id(item).eq_ignore_ascii_case(&id))
            {
                result.push(entry);
            }
        }
    }
    result
}

pub fn default_model_usable(model: &str) -> bool {
    entry_id_in_manifest(&default_model_catalog(), model).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::core::model_rules::ModelRules;
    use serde_json::json;

    /// `from_raw` 收的是整份 config（它自己取 `modelRules` 键），所以包一层。
    fn rules_with(mappings: Value) -> ModelRules {
        let raw = json!({ "modelRules": { "mappings": mappings } });
        ModelRules::from_raw(raw.as_object().expect("对象字面量"))
    }

    /// 一家的最小清单：两个上游模型 + 一个展示名条目。
    fn manifest() -> Vec<Value> {
        vec![
            json!({ "id": "model-a", "name": "Model A" }),
            json!({ "id": "model-b", "name": "Model B" }),
        ]
    }

    fn targets(rules: &ModelRules, requested: &str) -> Vec<String> {
        target_candidates(rules, ProviderKind::WorkBuddy, &manifest(), requested)
            .into_iter()
            .map(|wire| wire.model)
            .collect()
    }

    /// 家内主备的核心契约：同一 alias 在同一家挂多条映射时，**全部**进链，
    /// 组内保持配置顺序（先建先用）。首位必须与改造前 `builtin_target` 的
    /// 唯一答案逐字相等 —— 那是存量配置行为不变的锚点。
    #[test]
    fn same_alias_in_one_family_becomes_an_ordered_chain() {
        let rules = rules_with(json!([
            { "alias": "free", "target": "model-a", "provider": "workbuddy", "enabled": true },
            { "alias": "free", "target": "model-b", "provider": "workbuddy", "enabled": true },
        ]));
        assert_eq!(targets(&rules, "free"), vec!["model-a", "model-b"]);
    }

    /// 原生承载独占整条链：请求名命中清单 id 时映射一条都不进 ——
    /// 「原生路由优先、映射是追加的兜底路」的既有语义不被摊平破坏。
    #[test]
    fn native_route_excludes_mappings() {
        let rules = rules_with(json!([
            { "alias": "model-a", "target": "model-b", "provider": "workbuddy", "enabled": true },
        ]));
        // model-a 是清单里的原生 id：链上只有它自己
        assert_eq!(targets(&rules, "model-a"), vec!["model-a"]);
        // model-b 走映射：链上只有映射的 target
        assert_eq!(targets(&rules, "model-b"), vec!["model-b"]);
    }

    /// 关闭的映射一条都不进链（「这条别名暂时不存在」的既有语义）。
    #[test]
    fn disabled_mapping_is_excluded() {
        let rules = rules_with(json!([
            { "alias": "free", "target": "model-a", "provider": "workbuddy", "enabled": false },
            { "alias": "free", "target": "model-b", "provider": "workbuddy", "enabled": true },
        ]));
        assert_eq!(targets(&rules, "free"), vec!["model-b"]);
    }

    /// 组内顺序是配置顺序，不是字典序 —— 稳定分区，不用 sort。
    #[test]
    fn mapping_order_is_config_order_not_lexicographic() {
        let rules = rules_with(json!([
            { "alias": "free", "target": "zeta", "provider": "workbuddy", "enabled": true },
            { "alias": "free", "target": "alpha", "provider": "workbuddy", "enabled": true },
        ]));
        let mut manifest = manifest();
        manifest.push(json!({ "id": "zeta", "name": "Zeta" }));
        manifest.push(json!({ "id": "alpha", "name": "Alpha" }));
        let got: Vec<String> = target_candidates(&rules, ProviderKind::WorkBuddy, &manifest, "free")
            .into_iter()
            .map(|wire| wire.model)
            .collect();
        assert_eq!(got, vec!["zeta", "alpha"]);
    }

    /// Cline 池前缀优先：带本池通道前缀的 target 排在链首（与旧实现同偏好），
    /// 其余带 provider 的条目按原序跟在后面。
    ///
    /// 展示名刻意不与请求名同形 —— `entry_id_in_manifest` 有「按展示名回落」
    /// 的兼容，同名会触发原生承载短路（那是另一个正确的分支，见
    /// `native_route_excludes_mappings`），测不到池前缀优先。
    #[test]
    fn cline_pool_prefix_is_preferred_then_rest_follow_in_order() {
        let rules = rules_with(json!([
            { "alias": "glm", "target": "z-ai/glm-5.3-flash", "provider": "cline-free", "enabled": true },
            { "alias": "glm", "target": "cline-free/glm-5.3-flash", "provider": "cline-free", "enabled": true },
        ]));
        let manifest = vec![json!({ "id": "cline-free/glm-5.3-flash", "name": "Cline Free GLM Flash" }),
                            json!({ "id": "z-ai/glm-5.3-flash", "name": "Z AI GLM Flash" })];
        let got: Vec<String> = target_candidates(&rules, ProviderKind::ClineFree, &manifest, "glm")
            .into_iter()
            .map(|wire| wire.model)
            .collect();
        // 带池前缀的 `cline-free/` 在前（旧实现的首位），裸 id 在后
        assert_eq!(got, vec!["cline-free/glm-5.3-flash", "z-ai/glm-5.3-flash"]);
    }

    /// 解析不出任何目标（未知名字、无映射）→ 空表：调用方退回单答案路径。
    #[test]
    fn unresolvable_name_yields_empty_chain() {
        let rules = rules_with(json!([]));
        assert!(targets(&rules, "unknown-model").is_empty());
        // 空请求名同样为空（wire_targets_for_provider 的入口短路）
        assert!(target_candidates(&rules, ProviderKind::WorkBuddy, &manifest(), "").is_empty());
    }

    /// 别家挂的同名映射不进这一家的链（provider 过滤仍在）。
    #[test]
    fn other_family_mapping_is_not_in_this_chain() {
        let rules = rules_with(json!([
            { "alias": "free", "target": "model-a", "provider": "raccoon", "enabled": true },
        ]));
        assert!(targets(&rules, "free").is_empty());
    }
}
