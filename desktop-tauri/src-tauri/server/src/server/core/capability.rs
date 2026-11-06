//! 模型能力位的共享知识：六个键名、值的归一，以及把覆盖写进目录条目。
//!
//! ── 这六个键是什么 ──────────────────────────────────────────
//! 它们是 `list_item`（`/v1/models` 的条目）向**下游客户端**声明的那组字段：
//!
//!   maxInputTokens     上下文窗口（客户端按它估算截断）
//!   maxOutputTokens    单次回复的输出上限
//!   supportsToolCall   支持工具调用
//!   supportsImages     支持图片输入
//!   supportsVideo      支持视频输入
//!   supportsReasoning  支持思考
//!
//! 值来自各家的目录清单（代码里的静态表 / 上游远程目录）。**它们可以是错的**
//! —— 上游目录撒谎、网关的静态表跟不上上游调整时，下游会按错的能力构造请求
//! （给不支持的模型发图片、按虚高的窗口堆历史）。此前纠正的唯一手段是改代码；
//! 现在用户在模型管理页里改（覆盖层），这里就是那层覆盖的**共享形状**。
//!
//! ── 为什么键名用驼峰（与目录条目同形），而不是出口的下划线 ────
//! 覆盖直接以「目录条目风格」写回条目（`apply_overrides`），于是 `list_item`
//! 一行不用改就能透出；出口的下划线命名（`max_input_tokens` 等）仍由
//! `list_item` 一处决定。若覆盖层另用一套名字，出口处就多一次映射，而那份
//! 映射是「改了一个字段、另一处忘了改」的经典来源。
//!
//! ── 两份存储共用本模块 ──────────────────────────────────────
//! 内置家的覆盖存 `modelRules.capabilities`（见 `model_rules`），自定义家存
//! 提供商记录的 `models[].capabilities` 里。两边的读侧容错、写侧校验都调
//! 这里，保证同一份覆盖在两处存储的判定口径不会分叉。

use serde_json::{Map, Value};

/// 六个能力键。顺序即管理页弹窗字段与能力徽章的展示顺序（前端按同一顺序
/// 渲染，`overridden_keys` 也按它排）。
pub const KEYS: [&str; 6] = [
    "maxInputTokens",
    "maxOutputTokens",
    "supportsToolCall",
    "supportsImages",
    "supportsVideo",
    "supportsReasoning",
];

/// token 数值键的上限（1 亿）：再大就是手滑多敲了几个零，而虚高的上下文窗口
/// 会让下游按错误的量级堆积历史 —— 那正是这层覆盖要修的毛病之一，不该由它
/// 引入一个新的。下限是 1（0 / 负数对下游没有意义）。
pub const MAX_TOKEN_VALUE: f64 = 100_000_000.0;

/// 该键是不是 token 数值键（`maxInputTokens` / `maxOutputTokens`）。
pub fn is_token_key(key: &str) -> bool {
    key == "maxInputTokens" || key == "maxOutputTokens"
}

/// 该键是不是布尔能力键。
pub fn is_bool_key(key: &str) -> bool {
    key == "supportsToolCall"
        || key == "supportsImages"
        || key == "supportsVideo"
        || key == "supportsReasoning"
}

/// 单值归一：token 键收正整数（浮点写法取整，如 `200000.0`），布尔键收 bool；
/// 其余键名 / 非法值一律 `None`（调用方按「丢弃」处理）。
pub fn normalize_value(key: &str, value: &Value) -> Option<Value> {
    if is_token_key(key) {
        let number = value.as_f64()?;
        if !number.is_finite() || number < 1.0 || number > MAX_TOKEN_VALUE {
            return None;
        }
        return Some(Value::from(number.trunc() as u64));
    }
    if is_bool_key(key) {
        return value.as_bool().map(Value::Bool);
    }
    None
}

/// 从任意 JSON 对象里挑出合法能力键，返回**稀疏表**（没给的键不出现；非法
/// 键名 / 非法值丢弃；null 值丢弃 —— 存储层不存在「显式 null」这一档，
/// 清除覆盖靠删键表达）。
pub fn normalize_object(value: Option<&Value>) -> Map<String, Value> {
    let mut result = Map::new();
    let Some(object) = value.and_then(Value::as_object) else {
        return result;
    };
    for key in KEYS {
        if let Some(found) = object.get(key).and_then(|item| normalize_value(key, item)) {
            result.insert(key.to_string(), found);
        }
    }
    result
}

/// 把稀疏覆盖写进目录条目（原地）。覆盖表里不会出现 null（见 `normalize_object`），
/// 所以这里只做「有则改写」。
pub fn apply_overrides(item: &mut Value, overrides: &Map<String, Value>) {
    if overrides.is_empty() {
        return;
    }
    let Some(object) = item.as_object_mut() else {
        return;
    };
    for (key, value) in overrides {
        object.insert(key.clone(), value.clone());
    }
}

pub fn images_for_channel(provider: &str, item: &Value) -> Option<bool> {
    let user = item.pointer("/capSources/supportsImages").and_then(Value::as_str) == Some("user")
        || item.pointer("/capabilities/supportsImages").and_then(Value::as_bool).is_some();
    // CodeArts 的福利目录固定填 false，缺字段的 agent 目录也会补 false；它们不是拒图证据。
    if provider == "codearts" && !user && item.get("supportsImages").and_then(Value::as_bool) == Some(false) { return None; }
    item.get("supportsImages").and_then(Value::as_bool)
}

/// 条目的生效能力：六个键齐全，未声明给 null，管理页不能把未知当成不支持。
pub fn effective(item: &Value) -> Value {
    let mut result = Map::new();
    for key in KEYS {
        let value = item.get(key).filter(|value| !value.is_null()).cloned();
        result.insert(key.to_string(), value.unwrap_or(Value::Null));
    }
    Value::Object(result)
}

/// 生效值里被覆盖的键（按 [`KEYS`] 的顺序）。
pub fn overridden_keys(overrides: &Map<String, Value>) -> Vec<String> {
    KEYS.iter()
        .filter(|key| overrides.contains_key(**key))
        .map(|key| (*key).to_string())
        .collect()
}
