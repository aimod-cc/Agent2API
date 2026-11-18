use agent2api_provider_protocol as proto;
use serde_json::{Map, Value};

use super::json_text;

pub(super) fn fields(message: &Value) -> Result<&Map<String, Value>, proto::WireError> {
    let fields = json_text::fields(message, &[
        "type", "messageId", "content", "finished", "conversationId",
        "createTime", "updateTime", "model", "modelDisplayName", "status",
    ], &["type", "content"])?;
    for (name, maximum) in [
        ("messageId", 256), ("conversationId", 256),
        ("model", 1024), ("modelDisplayName", 1024),
    ] {
        if fields.get(name).is_some_and(|value| {
            value.as_str().is_none_or(|text| !json_text::identifier(text, maximum))
        }) {
            return Err(super::error::protocol());
        }
    }
    for name in ["createTime", "updateTime"] {
        if fields.get(name).is_some_and(|value| {
            value.as_str().is_none_or(|text| text.len() > 64
                || chrono::DateTime::parse_from_rfc3339(text).is_err())
        }) {
            return Err(super::error::protocol());
        }
    }
    if fields.get("status").is_some_and(|value| {
        value.as_u64().is_none_or(|status| status > i32::MAX as u64)
    }) {
        return Err(super::error::protocol());
    }
    Ok(fields)
}
