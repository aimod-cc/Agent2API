//! 压缩换模型后的发送参数：不借用来源思考档，按实际承载模型的已知上限收口。

use std::borrow::Cow;
use serde_json::Value;

use crate::server::core::routing_groups;

pub fn clear_source_reasoning(body: &mut Value) {
    let Some(object) = body.as_object_mut() else { return; };
    for key in ["reasoning_effort", "reasoningEffort", "effort", "reasoning", "thinking", "enable_thinking", "thinking_budget"] {
        object.remove(key);
    }
    // output_config 可能还承载结构化输出格式，只移除来源模型的档位。
    if let Some(output) = object.get_mut("output_config").and_then(Value::as_object_mut) {
        output.remove("effort");
        if output.is_empty() { object.remove("output_config"); }
    }
}

pub fn cap_output(body: &mut Cow<'_, Value>, provider: &str, model: &str) {
    let Some(metadata) = routing_groups::catalog_model(provider, model) else { return; };
    let Some(limit) = metadata.get("maxOutputTokens").and_then(Value::as_u64).filter(|limit| *limit > 0) else { return; };
    for key in ["max_tokens", "max_completion_tokens"] {
        if body.get(key).and_then(Value::as_u64).is_some_and(|requested| requested > limit) {
            body.to_mut()[key] = limit.into();
        }
    }
}
