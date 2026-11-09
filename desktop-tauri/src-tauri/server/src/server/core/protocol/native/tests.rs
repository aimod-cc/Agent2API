use super::*;

#[test]
fn native_anthropic_request_survives_projection_and_route_changes() {
    let original = json!({
        "model":"alias", "max_tokens":512, "thinking":{"type":"adaptive"},
        "output_config":{"effort":"max","format":{"type":"json_schema"}},
        "system":[{"type":"text","text":"system","cache_control":{"type":"ephemeral"}}],
        "tools":[{"type":"web_search_20250305","name":"web_search"}],
        "messages":[{"role":"assistant","content":[
            {"type":"thinking","thinking":"","signature":"signed+/=="},
            {"type":"redacted_thinking","data":"encrypted"},
            {"type":"text","text":"ok"}
        ]}],
        "future_parameter":{"unknown":true}, "_wb_route_group_id":"untrusted"
    });
    let (native, mut projection) =
        NativeRequest::prepare(WireProtocol::Anthropic, original.clone()).unwrap();
    projection["model"] = json!("resolved");
    projection["messages"] = json!([]);
    let routed = native.routed(&projection);
    assert_eq!(routed["model"], "resolved");
    assert_eq!(routed["messages"], original["messages"]);
    for key in [
        "system",
        "tools",
        "thinking",
        "output_config",
        "future_parameter",
    ] {
        assert_eq!(routed[key], original[key]);
    }
    assert!(routed.get("_wb_route_group_id").is_none());
    assert_eq!(*native.body, original);
}

#[test]
fn native_responses_preserves_stateful_and_unknown_fields() {
    for (field, value) in [
        ("previous_response_id", json!("resp_original")),
        ("conversation", json!({"id":"conv_1"})),
        ("prompt", json!({"id":"pmpt_1"})),
        ("background", json!(true)),
    ] {
        let mut body =
            json!({"model":"alias","input":"hi","store":true,"future_option":{"on":true}});
        body[field] = value.clone();
        let (native, projection) =
            NativeRequest::prepare(WireProtocol::Responses, body.clone()).unwrap();
        assert!(native.requires_native(), "{field}");
        assert_eq!(native.routed(&projection)[field], value);
        assert_eq!(*native.body, body);
    }
}

#[test]
fn stateless_responses_can_still_cross_protocols() {
    let body = json!({"model":"alias","input":"hi","store":false,"background":false});
    let (native, projection) = NativeRequest::prepare(WireProtocol::Responses, body).unwrap();
    assert!(!native.requires_native());
    assert!(projection["messages"].is_array());
}

#[test]
fn response_model_rewrite_does_not_touch_signed_or_tool_content() {
    for protocol in [WireProtocol::Anthropic, WireProtocol::Responses] {
        let key = if protocol == WireProtocol::Anthropic {
            "message"
        } else {
            "response"
        };
        let mut body = json!({"type":"event","sequence_number":11,"model":"wire","content":{"model":"user-value","signature":"signed"}});
        body[key] = json!({"model":"wire","content":[{"signature":"signed","type":"thinking","thinking":"hi"}]});
        let before = body.clone();
        assert!(rewrite_response_model(protocol, &mut body, "alias"));
        assert_eq!(body["model"], "alias");
        assert_eq!(body[key]["model"], "alias");
        assert_eq!(body["content"], before["content"]);
        assert_eq!(body[key]["content"], before[key]["content"]);
        assert!(!rewrite_response_model(protocol, &mut body, "alias"));
    }
}

#[test]
fn malformed_messages_are_rejected_before_any_forwarding() {
    for body in [json!({}), json!({"messages":"wrong"}), json!(null)] {
        assert!(NativeRequest::prepare(WireProtocol::Anthropic, body).is_err());
    }
}
