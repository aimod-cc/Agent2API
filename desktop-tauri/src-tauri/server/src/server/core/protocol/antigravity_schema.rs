//! Antigravity / Gemini v1internal 的**工具参数 JSON Schema 清洗**（规格坑 #13）。
//!
//! 从 `antigravity_outbound.rs` 拆出：`clean_schema` 与三遍递归（结构归并 /
//! 去不支持键 / 补 Gemini 要求的形态）是自成一体的纯函数库，混在请求信封
//! 转换里会把那个文件撑过本仓的单文件行数约定。
//!
//! ── 为什么必须清洗（不是洁癖）────────────────────────────────
//! Gemini 的 `Schema` proto **没有**这些 JSON Schema 关键字，出现一个就是
//! 上游 400「Unknown name …: Cannot find field」，整条请求被拒
//! （9router `UNSUPPORTED_SCHEMA_CONSTRAINTS` 的注释逐字如此，Manager
//! `clean_json_schema` 同款处理）。清洗口径取 9router 的实现（另一套独立
//! 实现 Manager 也做同一件事，键表略有出入 —— 取并集里 9router 的那份，
//! 它是 `openai-to-gemini` 路径上经过实测的表）。
//!
//! ── 三遍的顺序有意义（照 9router `cleanJSONSchemaForAntigravity`）──
//!   1. `merge_structure`：`type` 数组取第一个非 null（Gemini 只认单值）、
//!      **`type` 值转小写**（规格坑 #13 的硬要求；Manager 同样 to_lowercase）、
//!      `const`→`enum`、`enum` 值转字符串并补 `type`、`anyOf`/`oneOf` 选最优
//!      分支、`allOf` 合并 properties/required；
//!   2. `strip_keys`：删掉 [`UNSUPPORTED_SCHEMA_KEYS`] 与 `x-` 前缀扩展；
//!   3. `finalize_structure`：有 `properties` 缺 `type` → `object`、
//!      `array` 必须有 `items`、`required` 只留 properties 里存在的键、
//!      空对象 schema 补 `reason` 占位（Antigravity 要求 parameters 非空，
//!      9router 同款占位文案）。
//!
//! ── 与参考实现的一处刻意差异 ─────────────────────────────────
//! 递归只进 **子 schema**（`properties` 的每个值、`items`），不进
//! `properties` 这个**容器本身**：9router 的 JS 版在容器层也做键删除，
//! 于是工具参数里名叫 `format` / `title` / `default` 的属性会被删掉
//! （合法参数名撞上清洗表）。本仓按「容器不是 schema」处理，那条误伤不存在。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Map, Value};

/// 上游不认的 JSON Schema 键（9router `UNSUPPORTED_SCHEMA_CONSTRAINTS` 逐字；
/// `x-` 前缀的厂商扩展也一律去掉）。
const UNSUPPORTED_SCHEMA_KEYS: &[&str] = &[
    "minLength", "maxLength", "exclusiveMinimum", "exclusiveMaximum", "minItems", "maxItems",
    "format", "multipleOf", "uniqueItems", "contains", "unevaluatedProperties",
    "unevaluatedItems", "contentSchema", "prefixItems", "additionalItems", "default", "examples",
    "$schema", "$defs", "definitions", "const", "$ref", "$comment", "deprecated", "readOnly",
    "writeOnly", "additionalProperties", "propertyNames", "patternProperties", "enumDescriptions",
    "anyOf", "oneOf", "allOf", "not", "dependencies", "dependentSchemas", "dependentRequired",
    "title", "optional", "if", "then", "else", "contentMediaType", "contentEncoding",
    "cornerRadius", "fillColor", "fontFamily", "fontSize", "fontWeight", "gap", "padding",
    "strokeColor", "strokeThickness", "textColor",
];

/// 工具参数 schema 清洗：结构归并 → 去不支持键 → 补 Gemini 要求的形态。
pub fn clean_schema(schema: &Value) -> Value {
    let mut out = if schema.is_object() {
        schema.clone()
    } else {
        json!({})
    };
    merge_structure(&mut out);
    strip_keys(&mut out);
    finalize_structure(&mut out);
    out
}

/// 递归进入一个 schema 的**子 schema**：`properties` 的每个值、`items`
/// （对象或数组两种形态）。
///
/// 刻意不遍历其它对象值：`properties` / `items` 之外的键要么是标量容器
/// （`required`、`enum`），要么已被 [`strip_keys`] 删掉（见模块头末条）。
fn for_each_child_schema(object: &mut Map<String, Value>, visit: &mut dyn FnMut(&mut Value)) {
    if let Some(properties) = object
        .get_mut("properties")
        .and_then(Value::as_object_mut)
    {
        for schema in properties.values_mut() {
            visit(schema);
        }
    }
    if let Some(items) = object.get_mut("items") {
        match items {
            Value::Object(_) => visit(items),
            Value::Array(list) => {
                for item in list {
                    visit(item);
                }
            }
            _ => {}
        }
    }
}

/// JS 的 `String(value)`（数字/布尔转字面量，null → 空串）——与
/// `protocol::string_value` 同口径；这里留一份局部实现，避免一个纯函数库
/// 反向依赖协议模块的其它部分。
fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// 结构归并（递归）：见模块头第 1 条。
fn merge_structure(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        if let Some(items) = value.as_array_mut() {
            for item in items {
                merge_structure(item);
            }
        }
        return;
    };
    // `type` 归一（规格坑 #13：v1internal 要求**小写**；Manager 同样 to_lowercase）：
    // 数组取第一个非 null 的成员（大小写不敏感），字符串原样转小写，
    // 只剩 `null` 时按上下文回退（有 properties → object、有 items → array、
    // 否则 string —— Manager 的 fallback 口径）
    let has_properties = object.contains_key("properties");
    let has_items = object.contains_key("items");
    let picked = match object.get("type") {
        Some(Value::Array(types)) => types
            .iter()
            .filter_map(Value::as_str)
            .map(|item| item.trim().to_ascii_lowercase())
            .find(|item| item != "null"),
        Some(Value::String(text)) => {
            let lowered = text.trim().to_ascii_lowercase();
            if lowered == "null" {
                None
            } else {
                Some(lowered)
            }
        }
        _ => None,
    };
    if let Some(picked) = picked {
        object.insert("type".to_string(), Value::String(picked));
    } else if object.contains_key("type") {
        let fallback = if has_properties {
            "object"
        } else if has_items {
            "array"
        } else {
            "string"
        };
        object.insert("type".to_string(), Value::String(fallback.to_string()));
    }
    if !object.contains_key("enum") {
        if let Some(constant) = object.remove("const") {
            object.insert("enum".to_string(), json!([constant]));
        }
    }
    if let Some(items) = object.get_mut("enum").and_then(Value::as_array_mut) {
        let values: Vec<Value> = items
            .iter()
            .map(|item| match item {
                Value::String(_) => item.clone(),
                other => Value::String(text_of(other)),
            })
            .collect();
        *items = values;
        object
            .entry("type".to_string())
            .or_insert_with(|| json!("string"));
    }
    for branch in ["anyOf", "oneOf"] {
        let selected = object
            .get(branch)
            .and_then(Value::as_array)
            .and_then(|items| best_branch(items))
            .cloned();
        if let Some(selected) = selected {
            object.remove(branch);
            if let Some(fields) = selected.as_object() {
                for (key, item) in fields {
                    object.insert(key.clone(), item.clone());
                }
            }
        }
    }
    if let Some(merged) = object.remove("allOf") {
        if let Some(parts) = merged.as_array() {
            for part in parts {
                let Some(fields) = part.as_object() else {
                    continue;
                };
                if let Some(properties) = fields.get("properties").and_then(Value::as_object) {
                    let entry = object
                        .entry("properties".to_string())
                        .or_insert_with(|| json!({}));
                    if let Some(target) = entry.as_object_mut() {
                        for (key, item) in properties {
                            target.insert(key.clone(), item.clone());
                        }
                    }
                }
                if let Some(required) = fields.get("required").and_then(Value::as_array) {
                    let entry = object.entry("required".to_string()).or_insert_with(|| json!([]));
                    if let Some(target) = entry.as_array_mut() {
                        for item in required {
                            if !target.contains(item) {
                                target.push(item.clone());
                            }
                        }
                    }
                }
            }
        }
    }
    for_each_child_schema(object, &mut merge_structure);
}

/// anyOf/oneOf 里挑「最像对象」的分支（9router `selectBest` 的评分口径：
/// object 3 分、array 2 分、其它 1 分；`type:"null"` 的分支跳过）。
fn best_branch(items: &[Value]) -> Option<&Value> {
    let mut best: Option<(&Value, i32)> = None;
    for item in items {
        if !item.is_object() || item.get("type").and_then(Value::as_str) == Some("null") {
            continue;
        }
        let score = if item.get("type").and_then(Value::as_str) == Some("object")
            || item.get("properties").is_some()
        {
            3
        } else if item.get("type").and_then(Value::as_str) == Some("array")
            || item.get("items").is_some()
        {
            2
        } else {
            1
        };
        if best.map(|(_, best_score)| score > best_score).unwrap_or(true) {
            best = Some((item, score));
        }
    }
    best.map(|(item, _)| item)
}

/// 去掉上游不认的键（递归进子 schema；`properties` 里的**名字**不删）
fn strip_keys(value: &mut Value) {
    match value {
        Value::Object(object) => {
            let keys: Vec<String> = object
                .keys()
                .filter(|key| {
                    UNSUPPORTED_SCHEMA_KEYS.contains(&key.as_str()) || key.starts_with("x-")
                })
                .cloned()
                .collect();
            for key in keys {
                object.remove(&key);
            }
            for_each_child_schema(object, &mut strip_keys);
        }
        Value::Array(items) => {
            for item in items {
                strip_keys(item);
            }
        }
        _ => {}
    }
}

/// 补 Gemini 要求的形态（递归）：见模块头第 3 条。
fn finalize_structure(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if object.get("properties").is_some() && object.get("type").is_none() {
        object.insert("type".to_string(), json!("object"));
    }
    if object.get("type").and_then(Value::as_str) == Some("array") && object.get("items").is_none() {
        object.insert("items".to_string(), json!({ "type": "string" }));
    }
    if let Some(required) = object.get("required").and_then(Value::as_array) {
        let properties = object.get("properties").and_then(Value::as_object);
        let kept: Vec<Value> = match properties {
            Some(properties) => required
                .iter()
                .filter(|item| {
                    item.as_str()
                        .map(|name| properties.contains_key(name))
                        .unwrap_or(false)
                })
                .cloned()
                .collect(),
            None => Vec::new(),
        };
        if kept.is_empty() {
            object.remove("required");
        } else {
            object.insert("required".to_string(), Value::Array(kept));
        }
    }
    let needs_placeholder = if object.is_empty() {
        true
    } else if object.get("type").and_then(Value::as_str) == Some("object") {
        object
            .get("properties")
            .and_then(Value::as_object)
            .map(|properties| properties.is_empty())
            .unwrap_or(true)
    } else {
        false
    };
    if needs_placeholder {
        // Antigravity/VALIDATED 模式要求 parameters 非空（9router 的占位文案逐字）
        object.insert("type".to_string(), json!("object"));
        object.insert(
            "properties".to_string(),
            json!({
                "reason": {
                    "type": "string",
                    "description": "Brief explanation of why you are calling this tool"
                }
            }),
        );
        object.insert("required".to_string(), json!(["reason"]));
    }
    for_each_child_schema(object, &mut finalize_structure);
}
