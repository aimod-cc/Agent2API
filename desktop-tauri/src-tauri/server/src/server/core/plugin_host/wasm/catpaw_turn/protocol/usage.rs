//! 宿主只从原最后usage独立规范化；缺计数为未知，不借插件值、不补零或旧值。
//! 与既有CatPaw消费口径一致：prompt=max(prompt,total-completion)，整数及别名严格核对。
use agent2api_provider_protocol as proto;
use serde_json::{json, Value};
use super::super::settlement::invalid_evidence;

pub(super) fn normalize(value: Option<&Value>) -> Result<Option<Value>, proto::WireError> {
    let Some(value) = value else { return Ok(None); };
    if !value.is_object() { return Err(invalid_evidence()); }
    let prompt = count(value, "prompt_tokens", "promptTokens")?;
    let completion = count(value, "completion_tokens", "completionTokens")?;
    let total = count(value, "total_tokens", "totalTokens")?;
    let (Some(prompt), Some(completion), Some(total)) = (prompt, completion, total) else { return Ok(None); };
    let context_prompt = total.checked_sub(completion).ok_or_else(invalid_evidence)?;
    let prompt = prompt.max(context_prompt);
    let total = prompt.checked_add(completion).filter(|v| *v <= 9_007_199_254_740_991).ok_or_else(invalid_evidence)?;
    Ok(Some(json!({"prompt_tokens": prompt, "completion_tokens": completion, "total_tokens": total})))
}

fn count(value: &Value, snake: &str, camel: &str) -> Result<Option<u64>, proto::WireError> {
    let mut result = None;
    for field in [snake, camel] {
        if let Some(value) = value.get(field).filter(|v| !v.is_null()) {
            let count = value.as_u64().filter(|v| *v <= 9_007_199_254_740_991).ok_or_else(invalid_evidence)?;
            if result.is_some_and(|old| old != count) { return Err(invalid_evidence()); }
            result = Some(count);
        }
    }
    Ok(result)
}
