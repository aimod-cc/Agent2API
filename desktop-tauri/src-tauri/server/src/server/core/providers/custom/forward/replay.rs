//! Host-owned opt-in evidence for ordinary custom-provider inference only.
use serde_json::Value;

use crate::server::core::upstream::request::UpstreamRequestError;
use crate::server::errors::GatewayError;

/// Called only at the send boundary, before an upstream response exists. Function
/// tools are returned to the client, not executed here. Unknown/server-side tools,
/// background jobs and server-managed conversations retain the original stop.
pub(super) fn transport_error(error: UpstreamRequestError, payload: &Value) -> GatewayError {
    let ordinary = ordinary_inference(payload);
    let mut gateway = error.to_gateway_error();
    gateway.unconfirmed_inference = !error.safe_to_retry && ordinary;
    gateway
}

pub(super) fn ordinary_inference(payload: &Value) -> bool {
    let Some(body) = payload.as_object() else { return false; };
    if body.get("background").is_some_and(|value| value != &Value::Bool(false))
        || ["conversation", "previous_response_id", "prompt"].iter().any(|key| body.get(*key).is_some_and(|value| !value.is_null())) {
        return false;
    }
    if body.get("modalities").is_some_and(|value| value.as_array()
        .is_none_or(|items| items.iter().any(|item| item.as_str() != Some("text")))) {
        return false;
    }
    match body.get("tools") {
        None | Some(Value::Null) => true,
        Some(Value::Array(tools)) => tools.iter().all(|tool| tool.is_object()
            && match tool.get("type") {
                // Anthropic client-defined tools omit type.
                None => tool.get("name").is_some_and(Value::is_string) && tool.get("input_schema").is_some_and(Value::is_object),
                Some(Value::String(kind)) => matches!(kind.as_str(), "function" | "custom"),
                _ => false,
            }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_ordinary_inference_gets_host_owned_evidence() {
        for body in [json!({}), json!({"tools": [{"type": "function", "function": {"name": "read_file"}}]}),
            json!({"tools": [{"name": "read_file", "input_schema": {"type": "object"}}]})] {
            assert!(ordinary_inference(&body));
        }
        for body in [json!({"background": true}), json!({"background": "false"}),
            json!({"previous_response_id": "resp_previous"}), json!({"conversation": "conv_previous"}),
            json!({"tools": [{"type": "web_search"}]}), json!({"tools": [{"type": "image_generation"}]}),
            json!({"tools": [{"type": "code_interpreter"}]}), json!({"tools": [{"type": "mcp"}]}),
            json!({"prompt": {"id": "hosted-prompt"}}), json!({"modalities": ["text", "image"]}),
            json!({"modalities": ["audio"]}), json!({"tools": [null]}), json!({"tools": {}})] {
            assert!(!ordinary_inference(&body), "{body}");
        }
    }

    #[test]
    fn a_business_error_code_alone_never_grants_replay() {
        assert!(!GatewayError::with_status(502, "failed").with_code("upstream_submission_unconfirmed").unconfirmed_inference);
        let error = UpstreamRequestError { message: "connection closed".into(), reason: "closed".into(), safe_to_retry: false, network_failure: false };
        assert!(transport_error(error.clone(), &json!({})).unconfirmed_inference);
        assert!(!transport_error(error, &json!({"tools": [{"type": "web_search"}]})).unconfirmed_inference);
    }
}
