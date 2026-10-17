//! 对外模型名到上游模型的解析。路由与发送侧共用本解析，避免选中一家后再换一套规则。

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
    let provider = kind_id(kind);
    if let Some(id) = entry_id_in_manifest(manifest, requested) {
        if rules.default_enabled(provider, &id) {
            return Some(WireTarget {
                reasoning: rules.binding(provider, &id, &id).and_then(|m| m.reasoning.clone()),
                model: id,
            });
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
    let selected = candidates.iter()
        .filter(|(mapping, _)| mapping.provider.is_some())
        .find(|(_, target)| prefix.is_some_and(|prefix| target.starts_with(prefix)))
        .or_else(|| candidates.iter().find(|(mapping, _)| mapping.provider.is_some()))
        .or_else(|| candidates.first());
    selected.map(|(mapping, target)| WireTarget {
        model: target.clone(),
        reasoning: rules.binding(provider, &mapping.alias, &mapping.target)
            .and_then(|effective| effective.reasoning.clone()),
    })
}

/// 原始能力查询不应用开关，供管理 API 展开旧版全局规则。
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
        models.iter().any(|entry| model_id(entry).eq_ignore_ascii_case(model))
            .then(|| provider.get("id").and_then(Value::as_str).unwrap_or("").to_string())
    }));
    result
}

/// 只返回有开启绑定的提供商，不把关闭的 ID 回落给默认提供商。
pub fn forwarding_providers(model: &str) -> Vec<String> {
    let rules = model_rules::current();
    let mut result: Vec<_> = all_kinds().into_iter().filter(|kind| {
        builtin_target(&rules, *kind, &manifest_for(*kind), model).is_some()
    }).map(|kind| kind_id(kind).to_string()).collect();
    result.extend(custom_providers::carriers_of_model(model));
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

/// 识图能力闸门的划分结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImageCapabilitySplit {
    /// 仍可承接本次请求的家（顺序照传入，实际尝试序仍由账号优先级决定）。
    pub kept: Vec<String>,
    /// 被剔掉的家，连同它们在清单上的模型 id（日志要点名到具体模型，
    /// 否则「为什么这家没被用」只能靠猜）。
    pub skipped: Vec<(String, String)>,
}

/// 这条清单条目是否**明确**声明不接受图片输入。
///
/// 只有 `supportsImages` **存在、不是 null、且为假值**才算「不支持」。
/// **键缺失与显式 null 一律放行**（两者都是"上游没说"）：上游目录给不给这个字段
/// 各家不一（静态表、远程清单形态都不同），把「不知道」当成「不支持」等于凭缺数据
/// 关掉一家 —— 症状是模型明明在列表里、请求却说「没有可用的账号」，比多撞一次
/// 502 难查得多。这条不是抽象考虑：qoder / autoclaw / accio / catpaw 的解析层
/// 此前都把"字段缺席"写成 `false`（`unwrap_or(false)`），闸门于是永远看不到
/// 「不知道」这一态；同一个错在 codearts 的福利源上已经量到后果（上游回
/// `InferHub.001001020.406 not multimodal` 才是真依据，而 zcode 的
/// `glm-5.3-flash` 那个假 false 上游已在 beta 里改回 true）。
///
/// 解析层因此统一成：读到 `Some(bool)` 才写这个键，读不到就写 `null`
/// （`json!` 会把 `Option<bool>` 的 `None` 序列化成 null）—— 出口那侧
/// `models/shape.rs` 仍按 `unwrap_or(false)` 给客户端一个布尔，对外形态不变。
///
/// 真值口径用 `js_truthy`（与 `/v1/models` 出口同一函数，两处判定不会分叉），
/// 因此字符串 `"false"` 是**真**（非空串）—— 这不是疏漏，是生态里的一贯口径；
/// 用户在模型管理页改的能力位覆盖写的就是布尔（见 `core::capability`）。
///
/// **社区库补的值不算声明**（`capSources.supportsImages == "modelsdev"` 直接放行）：
/// beta 的 `core::model_meta` 会把上游没声明的六个能力键由 models.dev 补齐，于是
/// 「键存在」不再等于「这条通道实测过」。拿一份外部数据去剔候选家，方向与上面
/// 那条「不知道就放行」相反 —— 社区库记的是模型在**厂商自己那套 API** 上的能力，
/// 不是本网关这条通道的（`fill` 的模块头为此把优先级定成「上游 > models.dev」，
/// 同一个理由用在路由上就是：它能填展示值，不能决定发给谁）。关掉该开关、或这个
/// 模型没被收录时 `capSources` 里没有这一项，本函数行为与没有补齐层时逐字相同。
///
/// 传进来的条目要先经 [`entry_with_capabilities_flattened`] 展平：自定义家的
/// 覆盖是嵌在 `capabilities` 里的，不展平就等于看不见用户改的东西。
pub fn entry_refuses_images(entry: &Value) -> bool {
    if entry
        .get("capSources")
        .and_then(|sources| sources.get("supportsImages"))
        .and_then(Value::as_str)
        == Some("modelsdev")
    {
        return false;
    }
    entry
        .get("supportsImages")
        .filter(|value| !value.is_null())
        .is_some_and(|value| !crate::server::core::models::js_truthy(value))
}

/// 把条目自己那层 `capabilities` 覆盖展平到顶层（**返回新值，不改入参**）。
///
/// 闸门读的两份清单形状**不一样**，必须先归一：
///   - 内置家：`manifest_for` 在源头就把全局覆盖（`modelRules.capabilities`）
///     展平过了，顶层直接是生效值；
///   - 自定义家：存储里是 `models[].capabilities` 那层**嵌套**对象（写侧就这么存），
///     只有 `/v1/models` 出口经过 `public_models` 才展平。
///
/// 不归一的后果是这条改动对自定义家**静默失效**：用户在管理页勾掉「支持图片」，
/// 闸门读顶层读不到，照旧把请求送上去 ——「改了没用」是最难发现的一类失败。
/// 展平复用 `capability::apply_overrides` + `normalize_object`（与出口同一对函数，
/// 连手改数据里的坏值归一都共用）。没有覆盖的对象时原样克隆，不多付一次改写。
pub fn entry_with_capabilities_flattened(entry: &Value) -> Value {
    let Some(overrides) = entry.get("capabilities").and_then(Value::as_object) else {
        return entry.clone();
    };
    let normalized = crate::server::core::capability::normalize_object(Some(&Value::Object(
        overrides.clone(),
    )));
    if normalized.is_empty() {
        return entry.clone();
    }
    let mut flat = entry.clone();
    crate::server::core::capability::apply_overrides(&mut flat, &normalized);
    // 来源标记要跟着改：覆盖层写进去的是「用户明确指定」，而 `capSources` 里
    // 可能还留着补齐层那条 `modelsdev` —— 不改成 `user`，闸门会按那份旧标记把
    // 用户亲手勾掉的「不支持图片」当成社区库的猜测放行。内置家不走这里：它们的
    // 覆盖在 `model_rules::apply_capability_overrides` 里应用，那边自己调 `mark_user`。
    crate::server::core::model_meta::mark_user(
        &mut flat,
        &normalized.keys().cloned().collect::<Vec<_>>(),
    );
    flat
}

/// 纯核：按「取这家承接 `model` 的那条清单条目」的回调做划分。
///
/// 抽出来是为了可测 —— `manifest_for` 读的是全局目录（含用户能力位覆盖），
/// 而本仓的目录测试会并发改写各家清单缓存（见 `codearts::models` 的测试），
/// 直接把全局读法写进这段判定会让用例在全量跑时不确定。取条目那一段单独测。
pub fn split_by_image_capability(
    providers: &[String],
    entry_of: impl Fn(&str) -> Option<Value>,
) -> ImageCapabilitySplit {
    let mut split = ImageCapabilitySplit::default();
    for id in providers {
        match entry_of(id) {
            // 有条目且明确不支持 → 剔掉
            Some(entry) if entry_refuses_images(&entry) => {
                split.skipped.push((id.clone(), model_id(&entry)));
            }
            // 条目缺失 / 键缺失 / 明确支持 → 放行（三种情形都是「不该由我拦」）
            _ => split.kept.push(id.clone()),
        }
    }
    split
}

/// 这家承接 `requested` 时实际用的那条清单条目（别名先解析成上游真名再取）。
///
/// 解析走 [`wire_target_for_provider`] —— 与发送侧同一个函数，因此不会出现
/// 「闸门按 A 条目判、发出去按 B 条目走」的分叉。取不到条目返回 `None`
/// （未知家 / 未知模型 / 自定义家没这条模型），调用方按「不知道」放行。
pub fn manifest_entry_for(provider_id: &str, requested: &str) -> Option<Value> {
    let wire = wire_target_for_provider(requested, provider_id, None).model;
    if let Some(kind) = kind_from_id(provider_id) {
        return manifest_for(kind)
            .iter()
            .find(|entry| model_id(entry).eq_ignore_ascii_case(&wire))
            .cloned()
            .map(|entry| entry_with_capabilities_flattened(&entry));
    }
    // 自定义家：`models_of` 给的是存储形态（能力位嵌在 `capabilities` 里），
    // 必须展平后再判，否则用户在管理页勾掉的能力对闸门不可见。
    custom_providers::models_of(provider_id)
        .into_iter()
        .find(|entry| model_id(entry).eq_ignore_ascii_case(&wire))
        .map(|entry| entry_with_capabilities_flattened(&entry))
}

/// 请求含图片时，把「这个模型明确不支持识图」的候选家剔掉。
pub fn without_image_refusing(providers: &[String], requested: &str) -> ImageCapabilitySplit {
    split_by_image_capability(providers, |id| manifest_entry_for(id, requested))
}

/// 这家承接 `requested` 时是否**明确**声明不接受图片输入。
///
/// 剥图层（`payload::send_body` 的第 ④ 层）读的就是这个读数，与闸门共用
/// [`manifest_entry_for`] 与 [`entry_refuses_images`] 两处既有实现 —— 别名解析、
/// 自定义家的能力位展平那些细节因此不会有两份。
pub fn provider_refuses_images(provider_id: &str, requested: &str) -> bool {
    manifest_entry_for(provider_id, requested)
        .is_some_and(|entry| entry_refuses_images(&entry))
}

/// 识图闸门读完划分之后的**唯一决定**。
///
/// 收成枚举而不是让调用点写一串 if/else，是因为这四个分支各有不同的用户后果，
/// 而它们的优先次序是这次改动真正的内容：
///   - 图全在历史轮 → 候选不动，交给剥图层（`core::image_strip`）；
///   · 当前这一问就依赖图、且还有能吃图的家 → 剔掉不吃图的；
///   · 当前这一问依赖图、而**所有**候选都不吃图 → 退回不过滤（目录可能说谎，
///     照原链转发比凭一份目录把请求判死好）。
/// `history_images_strippable` 由调用方给（判据在 `core::image_strip`，那一侧还要
/// 管环境变量开关），本函数只做三分支的编排 —— 于是这张表能脱离全局目录被测。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageGate {
    /// 请求没有图，或没有家被目录标成不吃图：候选原样
    Unchanged,
    /// 保留全部候选，发送前剥掉历史图片（`skipped` 是将被剥图的那几家）
    StripAndKeep(Vec<String>),
    /// 保留全部候选并告警目录可能不准（一支都不剩了）
    FallBackToAll(Vec<String>),
    /// 收窄到 `kept`（当前这一问依赖图片）
    Narrow(Vec<String>),
}

pub fn decide_image_gate(split: &ImageCapabilitySplit, strippable: bool) -> ImageGate {
    if split.skipped.is_empty() {
        return ImageGate::Unchanged;
    }
    let refused: Vec<String> = split.skipped.iter().map(|(id, _)| id.clone()).collect();
    if strippable {
        return ImageGate::StripAndKeep(refused);
    }
    if split.kept.is_empty() {
        return ImageGate::FallBackToAll(refused);
    }
    ImageGate::Narrow(split.kept.clone())
}

#[cfg(test)]
mod vision_gate {
    //! 识图能力闸门的三条判据。
    //!
    //! ── 为什么这一组值得单钉 ────────────────────────────────
    //! 闸门判错的后果**不对称**：把「不支持」误判成「支持」只多撞一次 502
    //! （改动前就是这么做的，不算变坏）；把「不知道」误判成「不支持」则会
    //! **凭空关掉一家** —— 症状是模型明明在 `/v1/models` 里、请求却说没有可用
    //! 账号，而排障的人会去查账号而不是查目录。所以缺键那一条是这里最要紧的
    //! 断言，不是凑数的边界。
    use serde_json::json;

    use super::{
        decide_image_gate, entry_refuses_images, entry_with_capabilities_flattened,
        manifest_entry_for, split_by_image_capability, ImageCapabilitySplit, ImageGate,
    };

    fn providers(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    /// 只有**明确为假**才算不支持识图；**缺键与 `null` 都是"上游没说"= 放行**。
    ///
    /// `null` 这一格是 2026-09-30 改的：各家解析层此前把"字段缺席"写成 `false`，
    /// 闸门于是永远看不到未声明态；解析层改成 `Option<bool>` 之后，未声明在条目里
    /// 就是 `null`，必须与 `false` 分开（否则三态白做）。
    /// 真值口径仍是 `js_truthy`（与 `/v1/models` 出口同源），所以 `"false"` 这个
    /// 非空字符串是**真**，而 `0` / `""` 是明确假值 —— 用例把这几样都点一遍，
    /// 免得日后有人「顺手加个字符串解析」把两处口径拆开。
    #[test]
    fn only_an_explicit_false_counts_as_refusing_images() {
        assert!(entry_refuses_images(&json!({"id": "m", "supportsImages": false})));
        assert!(!entry_refuses_images(&json!({"id": "m", "supportsImages": null})),
                "null = 上游没给这个字段（未声明），不许当成「不支持」用来关一家");
        assert!(entry_refuses_images(&json!({"id": "m", "supportsImages": 0})));
        assert!(entry_refuses_images(&json!({"id": "m", "supportsImages": ""})));

        assert!(!entry_refuses_images(&json!({"id": "m", "supportsImages": true})));
        assert!(!entry_refuses_images(&json!({"id": "m", "supportsImages": "false"})),
                "非空串按 js_truthy 是真（与出口同一口径）");
        // 关键的一条：目录没给这个键 = 不知道，不是不支持
        assert!(!entry_refuses_images(&json!({"id": "m"})), "缺键必须放行");
        assert!(!entry_refuses_images(&json!({})));
    }

    /// 来源为 `modelsdev` 的假值**不算声明**（beta 的 `core::model_meta` 补齐层）。
    ///
    /// 补齐之后「键存在」不再等于「这条通道实测过」：社区库记的是模型在厂商自己
    /// API 上的能力。让它去剔候选家，等于凭一份外部数据把健康的账号移出链路，
    /// 方向与本文件「不知道就放行」那条主轴相反。反过来，`upstream` / `user` 两个
    /// 来源仍按原样判 —— 缺 `capSources`（开关关掉 / 未收录）时行为逐字不变。
    #[test]
    fn a_modelsdev_value_is_not_a_declaration_but_upstream_and_user_are() {
        assert!(
            !entry_refuses_images(&json!({
                "id": "m", "supportsImages": false,
                "capSources": {"supportsImages": "modelsdev"}
            })),
            "社区库补的 false 不该把这家从候选链里剔掉"
        );
        for source in ["upstream", "user"] {
            assert!(
                entry_refuses_images(&json!({
                    "id": "m", "supportsImages": false,
                    "capSources": {"supportsImages": source}
                })),
                "{source} 声明的 false 仍要生效"
            );
        }
        // 别的键有来源记录、这个键没有 → 视作清单原值，照判
        assert!(entry_refuses_images(&json!({
            "id": "m", "supportsImages": false,
            "capSources": {"supportsToolCall": "modelsdev"}
        })));
    }

    /// 自定义家的覆盖展平时必须把来源改成 `user`，否则用户的勾会被旧标记吃掉。
    ///
    /// `capability::apply_overrides` 只写值不记来源（记来源的是 `model_rules` 那条
    /// 路径，自定义家不经过它），所以这里不补一次 `mark_user`，一条「补齐层填过
    /// false、用户在管理页勾成 true」的条目会两个方向都错：闸门放行了本该剔的家，
    /// 而反向（用户勾成 false）时又因 modelsdev 标记放行 —— 后者是静默失效。
    #[test]
    fn flattening_a_custom_override_rewrites_the_capability_source() {
        let entry = json!({
            "id": "m",
            "supportsImages": true,
            "capSources": {"supportsImages": "modelsdev"},
            "capabilities": {"supportsImages": false}
        });
        let flat = entry_with_capabilities_flattened(&entry);
        assert_eq!(flat.get("supportsImages"), Some(&json!(false)), "覆盖要落到顶层");
        assert!(
            flat.pointer("/capSources/supportsImages")
                .and_then(|value| value.as_str())
                == Some("user"),
            "用户勾过的键不能再挂着补齐层的来源标记"
        );
        assert!(entry_refuses_images(&flat), "展平后的条目按用户声明生效");
        // 反向：用户勾成 true、补齐层说 false —— 也不能被旧标记放行掉……本来放行就是
        // 不剔家，这里钉的是「值以用户为准」这一条
        let reverse = entry_with_capabilities_flattened(&json!({
            "id": "m",
            "supportsImages": false,
            "capSources": {"supportsImages": "modelsdev"},
            "capabilities": {"supportsImages": true}
        }));
        assert!(!entry_refuses_images(&reverse));
    }

    /// 闸门读完划分之后的**四个分支次序**：图可剥就一个家都不剔。
    ///
    /// 划分（谁不吃图）与决定（要不要因此绕开它）是两件事，这张表只测后者；
    /// `strippable` 由调用方从 `core::image_strip` 取，因此这里能脱离全局目录跑。
    #[test]
    fn the_gate_prefers_stripping_over_dropping_a_home() {
        // 划分本身不管"图在哪个轮次"，四个分支的次序才是这次改动的内容
        let refused = |homes: &[&str]| ImageCapabilitySplit {
            kept: vec!["trae".to_string()],
            skipped: homes
                .iter()
                .map(|id| (id.to_string(), "deepseek-v4.1-flash".to_string()))
                .collect(),
        };
        // ① 图可剥 → 候选一个不剔，被剥的那几家点名出来
        assert_eq!(
            ImageGate::StripAndKeep(vec!["codearts".to_string()]),
            decide_image_gate(&refused(&["codearts"]), true)
        );
        // ② 图不可剥（当前这一问依赖图）→ 照旧收窄
        assert_eq!(
            ImageGate::Narrow(vec!["trae".to_string()]),
            decide_image_gate(&refused(&["codearts"]), false)
        );
        // ③ 不可剥、且候选全被剔光 → 退回不过滤（目录可能说谎）
        let all_refused = ImageCapabilitySplit {
            kept: Vec::new(),
            skipped: vec![("codearts".to_string(), "m".to_string())],
        };
        assert_eq!(
            ImageGate::FallBackToAll(vec!["codearts".to_string()]),
            decide_image_gate(&all_refused, false)
        );
        // ④ 没有家被标成不吃图 → 一律不动（连日志都不该写）
        let none = ImageCapabilitySplit {
            kept: vec!["codearts".to_string()],
            skipped: Vec::new(),
        };
        assert_eq!(ImageGate::Unchanged, decide_image_gate(&none, true));
        assert_eq!(ImageGate::Unchanged, decide_image_gate(&none, false));
    }

    /// 划分保留传入顺序，并把被剔的家**连同模型 id** 一起交出去（日志要点名）。
    #[test]
    fn split_drops_only_the_flagged_home_and_keeps_order() {
        let table = std::collections::HashMap::from([
            ("vision", json!({"id": "glm-v", "supportsImages": true})),
            ("plain", json!({"id": "deepseek-v4.1-flash", "supportsImages": false})),
            // 这家条目里压根没有能力键 —— 放行
            ("silent", json!({"id": "some-model"})),
        ]);
        let split = split_by_image_capability(
            &providers(&["plain", "vision", "silent", "ghost"]),
            |id| table.get(id).cloned(),
        );
        assert_eq!(vec!["vision".to_string(), "silent".to_string(), "ghost".to_string()], split.kept,
                   "顺序照传入，被剔的不动其他家");
        assert_eq!(
            vec![("plain".to_string(), "deepseek-v4.1-flash".to_string())],
            split.skipped,
            "只有明确标假的那家被剔，且带上真名供日志点名"
        );
    }

    /// 取不到条目时返回 `None`（闸门因此放行）—— 未知模型名与未知家都是这条路径。
    ///
    /// 这里刻意用**一定不存在**的名字：`manifest_for` 读全局目录，而本仓的目录
    /// 测试会并发改写各家清单缓存，全量跑时按真实模型名断言会不确定。未知名字
    /// 在任何一份目录里都取不到，断言因此与并发无关。
    #[test]
    fn an_unresolvable_name_or_home_yields_no_entry() {
        assert!(manifest_entry_for("codearts", "definitely-not-a-model-xyz").is_none());
        assert!(manifest_entry_for("custom-does-not-exist", "also-not-a-model-xyz").is_none());
    }

    /// 自定义家的能力位**嵌在 `capabilities` 里**，展平这层是「用户在管理页改了
    /// 到底生不生效」的那道接线 —— 不展平就是静默失效（读顶层读不到，闸门照放行）。
    #[test]
    fn nested_capability_overrides_are_what_the_gate_actually_reads() {
        // 顶层给了 true，覆盖层改成 false：生效值必须是 false（覆盖赢）
        let stored = json!({
            "id": "glm-v",
            "supportsImages": true,
            "capabilities": { "supportsImages": false }
        });
        let flat = entry_with_capabilities_flattened(&stored);
        assert_eq!(Some(&json!(false)), flat.get("supportsImages"), "覆盖层必须赢过顶层");
        assert!(entry_refuses_images(&flat), "展平后闸门才看得见用户改的这一步");
        assert!(!entry_refuses_images(&stored), "没展平就是这道接线会断的样子");

        // 没有覆盖层：原样返回，且顶层缺失仍然算「不知道」
        let bare = json!({"id": "glm-v"});
        assert_eq!(bare, entry_with_capabilities_flattened(&bare), "无覆盖不多改一次数据");
        // 空覆盖对象同样不动（存储里 `capabilities: {}` 是合法形态）
        let empty = json!({"id": "glm-v", "supportsImages": true, "capabilities": {}});
        assert!(!entry_refuses_images(&entry_with_capabilities_flattened(&empty)));
        // 坏值（键名不在六键表里 / 类型不对）由 `normalize_object` 挡掉，
        // 不会把一个支持的条目判成不支持
        let junk = json!({
            "id": "glm-v", "supportsImages": true,
            "capabilities": { "supportsImages": "yes", "nonsense": 1 }
        });
        assert!(!entry_refuses_images(&entry_with_capabilities_flattened(&junk)),
                "\"yes\" 是非空串 = 真值，与 /v1/models 出口同判据");
    }
}

