//! Anthropic 顶层思考参数兼容；签名与历史内容不在本模块的修改范围内。

use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Value};

use crate::server::core::model_rules;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy)]
struct Claude<'a> {
    family: &'a str,
    major: u32,
    minor: u32,
}

impl Claude<'_> {
    fn adaptive(self) -> bool {
        self.major > 4 || self.major == 4 && self.minor >= 6
    }
    fn manual_removed(self) -> bool {
        self.major > 4 || self.major == 4 && self.minor >= 7
    }
    fn no_xhigh(self) -> bool {
        self.major == 4 && self.minor == 6
    }
    fn always_thinks(self) -> bool {
        self.major == 5
            && (["fable", "mythos"]
                .iter()
                .any(|family| self.family.eq_ignore_ascii_case(family))
                || self.family.eq_ignore_ascii_case("opus") && self.minor == 5)
    }
}

fn claude(model: &str) -> Option<Claude<'_>> {
    static PATTERN: OnceLock<Option<Regex>> = OnceLock::new();
    let pattern = PATTERN.get_or_init(|| Regex::new(
        r"(?i)(?:^|[^a-z0-9])(?:claude-)?(opus|sonnet|haiku|fable|mythos)[-.](\d{1,2})(?:[-.](\d{1,2}))?(?:[^0-9]|$)",
    ).ok()).as_ref()?;
    let found = pattern.captures(model)?;
    let family = found.get(1)?.as_str();
    let major = found.get(2)?.as_str().parse().ok()?;
    let minor = found
        .get(3)
        .and_then(|value| value.as_str().parse().ok())
        .unwrap_or(0);
    Some(Claude {
        family,
        major,
        minor,
    })
}

pub fn supports_adaptive(model: &str) -> bool {
    claude(model).is_some_and(Claude::adaptive)
        || model.to_ascii_lowercase().contains("claude-mythos-preview")
}

pub fn budget_for_effort(effort: &str) -> Option<i64> {
    Some(match model_rules::reasoning_rank(effort)? {
        0 | 1 => 1024,
        2 => 4096,
        3 => 10240,
        _ => 32768,
    })
}

fn effort_from_budget(budget: i64) -> &'static str {
    if budget <= 0 {
        "medium"
    } else if budget <= 1024 {
        "low"
    } else if budget <= 4096 {
        "medium"
    } else if budget <= 10240 {
        "high"
    } else if budget <= 32768 {
        "xhigh"
    } else {
        "max"
    }
}

pub fn effort(body: &Value) -> Option<String> {
    text(body.pointer("/output_config/effort"))
        .or_else(|| text(body.get("reasoning_effort")))
        .or_else(|| text(body.pointer("/reasoning/effort")))
        .or_else(|| {
            (body.pointer("/thinking/type").and_then(Value::as_str) == Some("enabled")).then(|| {
                effort_from_budget(
                    body.pointer("/thinking/budget_tokens")
                        .and_then(Value::as_i64)
                        .unwrap_or(0),
                )
                .to_string()
            })
        })
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn fitted_effort(model: &str, level: &str) -> String {
    let level = level.trim().to_ascii_lowercase();
    if level == "minimal" {
        "low".to_string()
    } else if level == "xhigh" && claude(model).is_some_and(Claude::no_xhigh) {
        "max".to_string()
    } else {
        level
    }
}

fn output_effort(body: &mut Value, level: &str) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    let output = object.entry("output_config").or_insert_with(|| json!({}));
    if let Some(output) = output.as_object_mut() {
        output.insert("effort".into(), level.into());
    }
}

fn thinking(body: &mut Value, kind: &str, budget: Option<i64>) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    let value = object.entry("thinking").or_insert_with(|| json!({}));
    if !value.is_object() {
        return;
    }
    if let Some(thinking) = value.as_object_mut() {
        thinking.insert("type".into(), kind.into());
        thinking.remove("budget_tokens");
        if let Some(budget) = budget {
            thinking.insert("budget_tokens".into(), budget.into());
        }
    }
}

pub fn set_effort(body: &mut Value, model: &str, level: &str) {
    if model_rules::reasoning_is_off(level) {
        return;
    }
    let adaptive = supports_adaptive(model)
        || body.pointer("/thinking/type").and_then(Value::as_str) == Some("adaptive");
    if adaptive {
        thinking(body, "adaptive", None);
        output_effort(body, &fitted_effort(model, level));
    } else if let Some(budget) = budget_for_effort(level) {
        thinking(body, "enabled", Some(budget));
        if let Some(output) = body.get_mut("output_config").and_then(Value::as_object_mut) {
            output.remove("effort");
        }
        if body
            .get("max_tokens")
            .and_then(Value::as_i64)
            .is_some_and(|limit| limit <= budget)
        {
            body["max_tokens"] = budget.saturating_add(1024).into();
        }
    }
    if let Some(object) = body.as_object_mut() {
        object.remove("reasoning_effort");
        object.remove("reasoning");
    }
}

pub fn normalize(body: &mut Value, model: &str) -> Vec<&'static str> {
    let mut changes = Vec::new();
    let mode = body
        .pointer("/thinking/type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let profile = claude(model);
    let legacy_effort =
        text(body.get("reasoning_effort")).or_else(|| text(body.pointer("/reasoning/effort")));
    let explicit = text(body.pointer("/output_config/effort"));
    if let Some(level) = legacy_effort.as_deref() {
        if explicit.is_none()
            && !model_rules::reasoning_is_off(level)
            && !matches!(mode.as_str(), "disabled" | "between_tools")
        {
            if supports_adaptive(model) || mode == "adaptive" {
                output_effort(body, &fitted_effort(model, level));
                if mode.is_empty() {
                    thinking(body, "adaptive", None);
                }
            } else if mode.is_empty() {
                set_effort(body, model, level);
            }
        }
        if let Some(object) = body.as_object_mut() {
            object.remove("reasoning_effort");
            object.remove("reasoning");
        }
        changes.push("思考等级使用 Anthropic 参数");
    }
    if mode == "enabled" && profile.is_some_and(Claude::manual_removed) {
        let budget = body
            .pointer("/thinking/budget_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if text(body.pointer("/output_config/effort")).is_none() {
            output_effort(body, &fitted_effort(model, effort_from_budget(budget)));
        }
        thinking(body, "adaptive", None);
        if let Some(thinking) = body.get_mut("thinking").and_then(Value::as_object_mut) {
            thinking
                .entry("display")
                .or_insert_with(|| "summarized".into());
        }
        changes.push("旧思考模式已改为 adaptive，移除固定预算");
    } else if mode == "adaptive" {
        if let Some(thinking) = body.get_mut("thinking").and_then(Value::as_object_mut) {
            if thinking.remove("budget_tokens").is_some() {
                changes.push("adaptive 模式移除旧预算");
            }
        }
    } else if mode == "disabled" && profile.is_some_and(Claude::always_thinks) {
        thinking(body, "adaptive", None);
        if text(body.pointer("/output_config/effort")).is_none() {
            output_effort(body, "low");
        }
        changes.push("该模型不支持关闭思考，改用 adaptive");
    }
    if let Some(level) = text(body.pointer("/output_config/effort")) {
        let fitted = fitted_effort(model, &level);
        if fitted != level && supports_adaptive(model) {
            output_effort(body, &fitted);
            changes.push("思考等级按模型支持范围兼容");
        }
    }
    if profile.is_some_and(Claude::manual_removed) {
        if let Some(object) = body.as_object_mut() {
            let mut removed = false;
            for key in ["temperature", "top_p", "top_k"] {
                removed |= object.remove(key).is_some();
            }
            if removed {
                changes.push("移除该模型不支持的采样参数");
            }
        }
    }
    changes
}
