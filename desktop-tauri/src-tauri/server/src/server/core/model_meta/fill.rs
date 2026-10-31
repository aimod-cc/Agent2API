//! 把 models.dev 的元数据补进一条清单项（**清单成型处唯一的写入点**）。
//!
//! ── 优先级（本模块只有这一条规则）───────────────────────────
//! **用户覆盖 > 上游声明 > models.dev > 未声明**。因此这里只做一件事：
//! 清单项里**没有**某键（缺失或 null）时，用 models.dev 的值补上；上游已经
//! 声明过的键**一律不动**。用户覆盖层在调用方之后执行（`model_rules` 的
//! `apply_capability_overrides`），它天然排在最后。
//!
//! 为什么上游优先而不是 models.dev 优先：上游那份是**这条转发通道**的真实
//! 情况（小浣熊的服务端就下发 `supportThinking`、CatPaw 下发
//! `support_thinking`），而 models.dev 记的是模型在**实验室自己那套 API** 上的
//! 能力 —— 同一模型经不同中转通道能收的东西并不相同。用后者推翻前者，等于拿
//! 一份外部数据否认本通道的实测事实。
//!
//! ── 来源标记（`capSources`）─────────────────────────────────
//! 每个被填上的键都记一句「这个值是哪来的」（`upstream` = 清单里本来就有、
//! `modelsdev` = 本次补的、`user` = 用户覆盖层改的）。管理页据此把「真实值」
//! 与「我们猜的 / 数据库说的」分开显示 —— 没有这个标记，界面上的六个键看起来
//! 一样可信，而它们的可信度差着量级。
//!
//! ── 思考档位的归一（同一处完成）─────────────────────────────
//! 各家的档位列表、默认档和固定档在此归一，原字段保持不变。
//! 上游明确声明空表或不支持绑定时，不用 models.dev 的档位覆盖通道事实。
//!
//! 归一后的档位列表来源优先级：**这家自己声明 > models.dev 的 effort 列表**。
//! 都拿不到就**不出这个键** —— 前端据此回落通用档位表（「未声明」与「一档都
//! 没有」对用户是两件事，见 `ui-islands` 的 `model-capability.ts`）。

use serde_json::{json, Map, Value};

use super::snapshot::{self, Record};

/// 六键与 models.dev 字段的对应关系（顺序与 `capability::KEYS` 一致）。
///
/// 这份对应只写一次：出口的键名由 `capability` 决定，models.dev 的字段名由
/// 同步脚本决定，两者在这里汇合。
const BOOL_BRIDGE: [(&str, fn(&Record) -> Option<bool>); 4] = [
    ("supportsToolCall", |record| record.tool_call),
    ("supportsImages", |record| record.images),
    ("supportsVideo", |record| record.video),
    ("supportsReasoning", |record| record.reasoning),
];

const TOKEN_BRIDGE: [(&str, fn(&Record) -> Option<u64>); 2] = [
    ("maxInputTokens", |record| record.context),
    ("maxOutputTokens", |record| record.output),
];

/// 各家声明「本模型认识哪些档位」用的键名（按优先级：出口键名在前）。
///
/// 前一个是出口键本身（ZCode 直接用它），后两个是两家的内部键名 —— 归一了
/// 出口就统一了，但**不删原键**：那两家转发时仍按自己的键读（理由见模块头）。
const LEVEL_KEYS: [&str; 3] = ["reasoningLevels", "reasoningEfforts", "efforts"];

/// 把 models.dev 的值补进这条清单项（原地；上游已声明的键不动）。
///
/// 调用方只有两处，都是「一家全部出口的唯一源头」：内置家的
/// `providers::catalog::manifest_for`、自定义家的
/// `custom_providers::bindings::public_models`。收在两处就不可能出现「管理页
/// 改了、下游没变」那种分叉（与能力位覆盖层同一理由）。
pub fn apply(item: &mut Value) {
    let Some(id) = item.get("id").and_then(Value::as_str).map(str::to_string) else { return };
    let record = snapshot::lookup(&id);
    let mut sources: Map<String, Value> = Map::new();
    if let Some(record) = &record {
        if let Some(object) = item.as_object_mut() {
            for (key, pick) in BOOL_BRIDGE {
                match object.get(key) {
                    // 上游声明过：岿然不动，只记来源
                    Some(value) if !value.is_null() => {
                        sources.insert(key.to_string(), json!("upstream"));
                    }
                    _ => {
                        if let Some(flag) = pick(record) {
                            object.insert(key.to_string(), Value::Bool(flag));
                            sources.insert(key.to_string(), json!("modelsdev"));
                        }
                    }
                }
            }
            for (key, pick) in TOKEN_BRIDGE {
                match object.get(key) {
                    Some(value) if !value.is_null() => {
                        sources.insert(key.to_string(), json!("upstream"));
                    }
                    _ => {
                        if let Some(number) = pick(record) {
                            object.insert(key.to_string(), Value::from(number));
                            sources.insert(key.to_string(), json!("modelsdev"));
                        }
                    }
                }
            }
        }
    } else if let Some(object) = item.as_object() {
        // 没收录也要把上游声明记下来：管理页据此说明「这个值来自上游」，
        // 而不是显示成「没数据」。
        for key in BOOL_BRIDGE.iter().map(|(key, _)| *key).chain(TOKEN_BRIDGE.iter().map(|(key, _)| *key)) {
            if object.get(key).is_some_and(|value| !value.is_null()) {
                sources.insert(key.to_string(), json!("upstream"));
            }
        }
    }
    if !sources.is_empty() {
        if let Some(object) = item.as_object_mut() {
            object.insert("capSources".to_string(), Value::Object(sources));
        }
    }
    normalize_levels(item, record.as_ref());
}

/// 归一逐模型档位（原键只读，见模块头）。
fn normalize_levels(item: &mut Value, record: Option<&Record>) {
    let declared = item.pointer("/reasoning/supportedEfforts").and_then(levels_of)
        .or_else(|| LEVEL_KEYS.iter().find_map(|key| item.get(*key).and_then(levels_of)));
    let binding_supported = item.get("reasoningBindingSupported").and_then(Value::as_bool);
    let fixed = if binding_supported == Some(false) { None } else {
        item.get("reasoningFixedLevel").and_then(level_text).or_else(|| {
            if declared.as_ref().is_some_and(|levels| !levels.is_empty()) { return None; }
            item.pointer("/reasoning/effort").and_then(level_text)
        })
    };
    let explicit = declared.is_some() || fixed.is_some() || binding_supported == Some(false);
    let levels = if binding_supported == Some(false) { Vec::new() }
    else if let Some(fixed) = &fixed {
        vec![fixed.clone()]
    } else if explicit {
        declared.unwrap_or_default()
    } else {
        record.map(|record| record.levels.clone()).unwrap_or_default()
    };
    let default = item.get("reasoningDefaultLevel").and_then(level_text)
        .or_else(|| item.pointer("/reasoning/defaultEffort").and_then(level_text))
        .or_else(|| item.pointer("/reasoning/effort").and_then(level_text))
        .and_then(|value| {
            if levels.is_empty() && item.get("supportsReasoning").and_then(Value::as_bool) != Some(false) { Some(value) }
            else { levels.iter().find(|level| level.eq_ignore_ascii_case(&value)).cloned() }
        }).or_else(|| fixed.clone());
    let source = item.get("reasoningSource").and_then(Value::as_str)
        .filter(|source| matches!(*source, "upstream" | "modelsdev" | "undeclared"))
        .map(str::to_string)
        .unwrap_or_else(|| {
            (if explicit { "upstream" } else if !levels.is_empty() { "modelsdev" } else { "undeclared" })
                .to_string()
        });
    let Some(object) = item.as_object_mut() else { return };
    if explicit || !levels.is_empty() {
        object.insert("reasoningLevels".into(), json!(levels));
    }
    object.remove("reasoningDefaultLevel");
    object.remove("reasoningFixedLevel");
    if let Some(default) = default { object.insert("reasoningDefaultLevel".into(), json!(default)); }
    if let Some(fixed) = fixed { object.insert("reasoningFixedLevel".into(), json!(fixed)); }
    object.insert("reasoningSource".into(), json!(source));
}

fn level_text(value: &Value) -> Option<String> {
    value.as_str().map(str::trim).filter(|value| !value.is_empty()).map(str::to_string)
}

/// 从清单项的一个档位键里读出档位表（非数组 / 空串 / 数字一律丢弃）。
fn levels_of(value: &Value) -> Option<Vec<String>> {
    let items = value.as_array()?;
    Some(
        items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|level| !level.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// 用户覆盖层改过的键，来源改记成 `user`（在覆盖层里调用）。
///
/// 覆盖值是**用户明确指定**的，既不是上游也不是 models.dev —— 管理页要把这条
/// 区分出来，否则用户会以为那是清单里的原值。
///
/// `sources` 的三个取值（`upstream` / `modelsdev` / `user`）是**前后端契约**：
/// 前端按同一批字面量渲染标记（`ui-islands` 的 `model-capability.ts`）。
pub fn mark_user(item: &mut Value, keys: &[String]) {
    if keys.is_empty() {
        return;
    }
    let Some(object) = item.as_object_mut() else { return };
    let mut sources = object
        .get("capSources")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for key in keys {
        sources.insert(key.clone(), json!("user"));
    }
    object.insert("capSources".to_string(), Value::Object(sources));
}
