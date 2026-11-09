use super::*;

fn request(model: &str) -> Value {
    json!({
        "model": model, "max_tokens": 8192,
        "system": [{"type":"text","text":"保持不变","cache_control":{"type":"ephemeral"}}],
        "tools": [{"name":"read","input_schema":{"type":"object"}}],
        "messages": [
            {"role":"user","content":"hi"},
            {"role":"assistant","content":[
                {"type":"thinking","thinking":"历史思考","signature":"signed+/=="},
                {"type":"redacted_thinking","data":"opaque+/=="},
                {"type":"text","text":"你好"}
            ]},
            {"role":"user","content":"继续"}
        ]
    })
}

#[test]
fn adaptive_request_keeps_all_native_controls_and_history() {
    let mut body = request("claude-opus-5-5");
    body["thinking"] = json!({"type":"adaptive","display":"omitted","block_binding":{"prefix_mismatch_behavior":"error"}});
    body["output_config"] =
        json!({"effort":"max","format":{"type":"json_schema","schema":{"type":"object"}}});
    let original = body.clone();
    assert!(normalize(&mut body, "claude-opus-5-5").is_empty());
    assert_eq!(body, original);
}

#[test]
fn legacy_opus_55_request_is_upgraded_without_changing_history_or_limit() {
    let mut body = request("claude-opus-5-5");
    body["thinking"] = json!({"type":"enabled","budget_tokens":1024});
    body["reasoning_effort"] = json!("medium");
    body["max_tokens"] = json!(16);
    let original = body.clone();
    assert!(!normalize(&mut body, "claude-opus-5-5").is_empty());
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert!(body["thinking"].get("budget_tokens").is_none());
    assert_eq!(body["output_config"]["effort"], "medium");
    assert!(body.get("reasoning_effort").is_none());
    assert_eq!(body["max_tokens"], 16);
    for key in ["messages", "system", "tools"] {
        assert_eq!(body[key], original[key]);
    }
}

#[test]
fn explicit_native_effort_wins_over_a_legacy_alias() {
    let mut body = request("claude-opus-5-5");
    body["thinking"] = json!({"type":"enabled","budget_tokens":1024,"display":"omitted"});
    body["reasoning_effort"] = json!("low");
    body["output_config"] = json!({"effort":"max","format":{"type":"json_schema"}});
    normalize(&mut body, "claude-opus-5-5");
    assert_eq!(body["output_config"]["effort"], "max");
    assert_eq!(
        body["output_config"]["format"],
        json!({"type":"json_schema"})
    );
    assert_eq!(body["thinking"]["display"], "omitted");
}

#[test]
fn older_and_unknown_models_keep_their_manual_mode() {
    for model in [
        "claude-3-7-sonnet",
        "claude-sonnet-4-5",
        "claude-opus-4-6",
        "glm-5.3",
        "my-deployment",
    ] {
        let mut body = request(model);
        body["thinking"] = json!({"type":"enabled","budget_tokens":2048});
        let original = body.clone();
        assert!(normalize(&mut body, model).is_empty(), "{model}");
        assert_eq!(body, original, "{model}");
    }
}

#[test]
fn model_spellings_and_case_are_supported() {
    for model in [
        "claude-opus-5-5",
        "claude-opus-5.5",
        "anthropic.claude-opus-5-5-v1:0",
        "anthropic/claude-opus-5-5",
        "CLAUDE-OPUS-5-5",
        "opus-5.5",
        "claude-sonnet-5-5@20261001",
    ] {
        let mut body = request(model);
        body["thinking"] = json!({"type":"enabled","budget_tokens":4096});
        normalize(&mut body, model);
        assert_eq!(body["thinking"]["type"], "adaptive", "{model}");
        assert_eq!(body["output_config"]["effort"], "medium", "{model}");
    }
}

#[test]
fn effort_binding_uses_the_target_model_protocol_and_keeps_other_settings() {
    let mut body = request("claude-opus-5-5");
    body["thinking"] = json!({"type":"enabled","budget_tokens":1024,"display":"omitted"});
    body["output_config"] = json!({"effort":"low","format":{"type":"json_schema"}});
    body["reasoning_effort"] = json!("low");
    let history = body["messages"].clone();
    set_effort(&mut body, "claude-opus-5-5", "max");
    normalize(&mut body, "claude-opus-5-5");
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["thinking"]["display"], "omitted");
    assert!(body["thinking"].get("budget_tokens").is_none());
    assert_eq!(body["output_config"]["effort"], "max");
    assert!(body["output_config"].get("format").is_some());
    assert_eq!(body["messages"], history);
    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn disabled_binding_never_removes_an_explicit_client_setting() {
    for level in ["none", "off", "OFF"] {
        let mut body = request("claude-opus-5-5");
        body["thinking"] = json!({"type":"adaptive"});
        body["output_config"] = json!({"effort":"high"});
        let original = body.clone();
        set_effort(&mut body, "claude-opus-5-5", level);
        assert_eq!(body, original);
    }
}

#[test]
fn only_known_incompatible_effort_levels_are_adjusted() {
    for (model, level, expected) in [
        ("claude-opus-4-6", "xhigh", "max"),
        ("claude-opus-5-5", "xhigh", "xhigh"),
        ("claude-opus-5-5", "max", "max"),
        ("claude-sonnet-4-6", "minimal", "low"),
        ("my-deployment", "xhigh", "xhigh"),
    ] {
        let mut body = request(model);
        body["thinking"] = json!({"type":"adaptive"});
        body["output_config"] = json!({"effort":level});
        normalize(&mut body, model);
        assert_eq!(body["output_config"]["effort"], expected);
    }
}

#[test]
fn normalization_is_idempotent_for_each_thinking_mode() {
    for mode in ["adaptive", "enabled", "disabled"] {
        for model in [
            "claude-opus-5-5",
            "claude-opus-4-6",
            "claude-sonnet-4-5",
            "custom",
        ] {
            let mut body = request(model);
            body["thinking"] = json!({"type":mode,"budget_tokens":2048});
            body["reasoning_effort"] = json!("medium");
            normalize(&mut body, model);
            let once = body.clone();
            normalize(&mut body, model);
            assert_eq!(body, once, "{model} {mode}");
        }
    }
}

#[test]
fn chat_conversion_selects_adaptive_only_for_compatible_models() {
    let chat = json!({"model":"alias","messages":[{"role":"user","content":"hi"}],"max_tokens":16,"reasoning_effort":"max"});
    let modern = crate::server::core::protocol::anthropic_outbound::anthropic_request_from_chat(
        &chat,
        "claude-opus-5-5",
    )
    .unwrap();
    assert_eq!(modern["thinking"]["type"], "adaptive");
    assert_eq!(modern["output_config"]["effort"], "max");
    assert_eq!(modern["max_tokens"], 16);
    let old = crate::server::core::protocol::anthropic_outbound::anthropic_request_from_chat(
        &chat,
        "claude-sonnet-4-5",
    )
    .unwrap();
    assert_eq!(old["thinking"]["type"], "enabled");
    assert!(
        old["max_tokens"].as_i64().unwrap() > old["thinking"]["budget_tokens"].as_i64().unwrap()
    );
}

#[test]
fn valid_adaptive_mode_on_an_unknown_deployment_is_never_downgraded() {
    let mut body = request("deployment-123");
    body["thinking"] = json!({"type":"adaptive","display":"summarized"});
    body["reasoning_effort"] = json!("high");
    normalize(&mut body, "deployment-123");
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["output_config"]["effort"], "high");
    assert!(body.get("reasoning_effort").is_none());
}
