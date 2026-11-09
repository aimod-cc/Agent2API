use super::*;

fn native_request(model: &str, stream: bool) -> Value {
    json!({"model":model,"stream":stream,"max_tokens":512,"thinking":{"type":"adaptive","display":"omitted"},
        "output_config":{"effort":"max","format":{"type":"json_schema","schema":{"type":"object"}}},
        "system":[{"type":"text","text":"You are Claude Code, Anthropic's official CLI for Claude","cache_control":{"type":"ephemeral"}}],
        "tools":[{"name":"read","input_schema":{"type":"object"},"cache_control":{"type":"ephemeral"}}],
        "messages":[
            {"role":"user","content":"hi"},
            {"role":"assistant","content":[{"type":"thinking","thinking":"","signature":"history-signed+/=="},
                {"type":"redacted_thinking","data":"history-opaque"},{"type":"text","text":"previous"}]},
            {"role":"user","content":[{"type":"text","text":"继续"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA=="}}]}
        ],"future_parameter":{"keep":true}})
}

fn message(model: &str) -> Value {
    json!({"type":"message","id":"msg_upstream","model":model,"role":"assistant","content":[
        {"type":"thinking","thinking":"","signature":"response-signed+/=="},
        {"type":"text","text":"ok"}],"stop_reason":"end_turn","stop_sequence":null,
        "usage":{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":30,"output_tokens":8},
        "future_response":{"keep":true}})
}

fn sse(kind: &str, mut value: Value) -> String {
    value["type"] = kind.into();
    format!("event: {kind}\ndata: {value}\n\n")
}

fn message_stream(model: &str) -> String {
    let mut start = message(model);
    start["content"] = json!([]);
    start["usage"]["output_tokens"] = 0.into();
    [sse("message_start", json!({"message":start})),
        sse("content_block_start", json!({"index":0,"content_block":{"type":"thinking","thinking":"","signature":""}})),
        sse("content_block_delta", json!({"index":0,"delta":{"type":"signature_delta","signature":"response-signed+/=="}})),
        sse("content_block_stop", json!({"index":0})),
        sse("content_block_start", json!({"index":1,"content_block":{"type":"text","text":""}})),
        sse("content_block_delta", json!({"index":1,"delta":{"type":"text_delta","text":"ok"}})),
        sse("content_block_stop", json!({"index":1})),
        sse("message_delta", json!({"delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":8}})),
        sse("message_stop", json!({}))].concat()
}

fn chat_stream(model: &str) -> String {
    format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        json!({"id":"chat_1","object":"chat.completion.chunk","model":model,"choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":null}]}),
        json!({"id":"chat_1","object":"chat.completion.chunk","model":model,"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":1,"total_tokens":4}})
    )
}

pub(super) async fn run(h: &Harness) {
    config::set_retry(config::RetryPatch {
        count: Some(0),
        account_switch_count: Some(1),
        ..Default::default()
    });
    let anthropic = Mock::new().await;
    h.provider(
        "custom-native-anthropic",
        "anthropic",
        &anthropic,
        json!([
        {"id":"claude-opus-5-5","enabled":true}, {"id":"claude-opus-4-6","enabled":true},
        {"id":"claude-sonnet-4-5","enabled":true}]),
        json!([
        {"alias":"native-alias","target":"claude-opus-5-5","enabled":true},
        {"alias":"bound-alias","target":"claude-opus-5-5","enabled":true,"reasoning":"medium"}]),
    );
    h.key("key-anthropic-tests", &["custom-native-anthropic"], &[]);
    let key = "key-anthropic-tests";
    let request = native_request("native-alias", false);
    config::set_sanitize_fingerprints(true);
    anthropic.push(Reply::json(message("claude-opus-5-5")));
    let (status, headers, bytes) = h
        .call_headers(
            "/v1/messages",
            key,
            request.clone(),
            &[
                ("anthropic-version", "2023-06-01"),
                ("anthropic-beta", "test-feature-1,test-feature-2"),
                ("authorization", "Bearer must-not-leak"),
                ("cookie", "must-not-leak=1"),
            ],
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let received = anthropic.last();
    assert_eq!(received.path, "/anthropic/v1/messages");
    assert_eq!(received.body["model"], "claude-opus-5-5");
    for key in [
        "thinking",
        "output_config",
        "system",
        "tools",
        "messages",
        "future_parameter",
    ] {
        assert_eq!(received.body[key], request[key], "{key}");
    }
    assert_eq!(received.body["stream"], false);
    assert_eq!(
        received.headers["x-api-key"],
        "upstream-custom-native-anthropic"
    );
    assert_eq!(
        received.headers["anthropic-beta"],
        "test-feature-1,test-feature-2"
    );
    assert!(!received.headers.contains_key("authorization"));
    assert!(!received.headers.contains_key("cookie"));
    assert_eq!(headers["request-id"], "native-upstream-1");
    assert!(!headers.contains_key("set-cookie"));
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["model"], "native-alias");
    assert_eq!(body["content"], message("claude-opus-5-5")["content"]);
    assert_eq!(body["future_response"], json!({"keep":true}));
    let row = h
        .rows()
        .into_iter()
        .find(|row| row["model"] == "native-alias")
        .unwrap();
    assert_eq!(row["promptTokens"], 60, "{row}");
    assert_eq!(row["totalTokens"], 68);
    assert!(row["error"].is_null(), "{row}");
    assert!(h.state.upstream().connections().snapshot().is_empty());

    for stream in [true, false] {
        anthropic.push(Reply::sse(message_stream("claude-opus-5-5")));
        let (status, _, bytes) = h
            .call("/v1/messages", key, native_request("native-alias", stream))
            .await;
        assert_eq!(status, StatusCode::OK);
        if stream {
            let text = String::from_utf8(bytes).unwrap();
            assert!(text.contains("response-signed+/=="));
            assert!(text.contains("event: message_stop\n"));
            assert!(!text.contains("[DONE]"));
        } else {
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["content"], message("claude-opus-5-5")["content"]);
        }
        assert!(h.state.upstream().connections().snapshot().is_empty());
    }

    let mut legacy = native_request("claude-opus-5-5", false);
    legacy["thinking"] = json!({"type":"enabled","budget_tokens":1024});
    legacy.as_object_mut().unwrap().remove("output_config");
    legacy["reasoning_effort"] = "medium".into();
    legacy["max_tokens"] = 16.into();
    anthropic.push(Reply::json(message("claude-opus-5-5")));
    assert_eq!(
        h.call("/v1/messages", key, legacy.clone()).await.0,
        StatusCode::OK
    );
    let received = anthropic.last();
    assert_eq!(received.body["thinking"]["type"], "adaptive");
    assert!(received.body["thinking"].get("budget_tokens").is_none());
    assert!(received.body.get("reasoning_effort").is_none());
    assert_eq!(received.body["output_config"]["effort"], "medium");
    assert_eq!(received.body["messages"], legacy["messages"]);
    assert_eq!(received.body["max_tokens"], 16);

    anthropic.push(Reply::json(message("claude-opus-5-5")));
    assert_eq!(
        h.call("/v1/messages", key, native_request("bound-alias", false))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(anthropic.last().body["output_config"]["effort"], "medium");
    assert_eq!(anthropic.last().body["thinking"]["type"], "adaptive");

    for model in ["claude-opus-4-6", "claude-sonnet-4-5"] {
        let mut old = native_request(model, false);
        old["thinking"] = json!({"type":"enabled","budget_tokens":1024});
        old.as_object_mut().unwrap().remove("output_config");
        anthropic.push(Reply::json(message(model)));
        assert_eq!(
            h.call("/v1/messages", key, old.clone()).await.0,
            StatusCode::OK
        );
        assert_eq!(anthropic.last().body["thinking"], old["thinking"]);
    }

    let before = anthropic.count();
    assert_eq!(
        h.call(
            "/v1/messages",
            "invalid-key",
            native_request("native-alias", false)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    h.key(
        "key-restricted-tests",
        &["custom-native-anthropic"],
        &["different-model"],
    );
    assert_eq!(
        h.call(
            "/v1/messages",
            "key-restricted-tests",
            native_request("native-alias", false)
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(anthropic.count(), before);

    let raw_error = json!({"type":"error","error":{"type":"invalid_request_error","message":"original detail","parameter":"thinking"},"request_id":"upstream-error"});
    anthropic.push(Reply::failure(400, raw_error.clone()));
    let (status, headers, bytes) = h
        .call("/v1/messages", key, native_request("native-alias", false))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), raw_error);
    assert_eq!(headers["retry-after"], "2");

    config::set_sanitize_fingerprints(false);
    let chat = Mock::new().await;
    h.provider(
        "custom-native-chat",
        "chat_completions",
        &chat,
        json!([
        {"id":"test-chat","enabled":true},{"id":"fallback-model","enabled":true}]),
        json!([]),
    );
    h.key("key-chat-tests", &["custom-native-chat"], &[]);
    chat.push(Reply::sse(chat_stream("test-chat")));
    let (status, _, bytes) = h
        .call(
            "/v1/messages",
            "key-chat-tests",
            native_request("test-chat", true),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert_eq!(chat.last().path, "/v1/chat/completions");
    assert_eq!(chat.last().body["reasoning_effort"], "max");
    assert!(String::from_utf8(bytes)
        .unwrap()
        .contains("event: message_stop"));
    chat.push(Reply::sse(chat_stream("test-chat")));
    let (status, _, bytes) = h
        .call(
            "/v1/chat/completions",
            "key-chat-tests",
            json!({"model":"test-chat","stream":true,"messages":[{"role":"user","content":"hi"}]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8(bytes).unwrap().contains("[DONE]"));

    anthropic.push(Reply::sse(message_stream("claude-opus-5-5")));
    let (status,_,bytes) = h.call("/v1/chat/completions",key,json!({"model":"native-alias","stream":true,"reasoning_effort":"max","messages":[{"role":"user","content":"hi"}]})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(anthropic.last().body["thinking"]["type"], "adaptive");
    assert_eq!(anthropic.last().body["output_config"]["effort"], "max");
    assert!(String::from_utf8(bytes).unwrap().contains("[DONE]"));

    responses(h).await;
    failover(h).await;
    assert!(h.state.upstream().connections().snapshot().is_empty());
    assert!(h.rows().iter().all(|row| row["status"] != 0));
}

async fn responses(h: &Harness) {
    let mock = Mock::new().await;
    h.provider(
        "custom-native-responses",
        "responses",
        &mock,
        json!([{"id":"test-responses","enabled":true}]),
        json!([]),
    );
    h.key("key-responses-tests", &["custom-native-responses"], &[]);
    let request = json!({"model":"test-responses","stream":false,"input":[{"role":"user","content":"hi"}],
        "previous_response_id":"resp_previous","store":true,"reasoning":{"effort":"high","summary":"auto"},
        "tools":[{"type":"custom","name":"apply_patch","format":{"type":"text"}}],"future":{"keep":true}});
    let answer = json!({"id":"resp_next","object":"response","status":"completed","model":"test-responses","output":[{"type":"reasoning","encrypted_content":"opaque"}],
        "usage":{"input_tokens":25,"output_tokens":4,"total_tokens":29},"future":{"keep":true}});
    mock.push(Reply::json(answer.clone()));
    let (status, _, bytes) = h
        .call_headers(
            "/v1/responses",
            "key-responses-tests",
            request.clone(),
            &[("openai-beta", "responses-test")],
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert_eq!(mock.last().body, request);
    assert_eq!(
        mock.last().headers["authorization"],
        "Bearer upstream-custom-native-responses"
    );
    assert!(!mock.last().headers.contains_key("x-api-key"));
    assert_eq!(mock.last().headers["openai-beta"], "responses-test");
    assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), answer);
    let text = sse(
        "response.created",
        json!({"sequence_number":0,"response":{"id":"resp_next","model":"test-responses","status":"in_progress"}}),
    ) + &sse(
        "response.completed",
        json!({"sequence_number":1,"response":answer}),
    );
    mock.push(Reply::sse(text.clone()));
    let mut stream = request.clone();
    stream["stream"] = true.into();
    let (status, _, bytes) = h.call("/v1/responses", "key-responses-tests", stream).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, text.as_bytes());
    mock.push(Reply::sse(text));
    assert_eq!(
        h.call("/v1/responses", "key-responses-tests", request)
            .await
            .0,
        StatusCode::OK
    );
    let before = mock.count();
    let (status, _, _) = h
        .call(
            "/v1/responses",
            "key-chat-tests",
            json!({"model":"test-chat","input":"hi","previous_response_id":"resp_previous"}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(mock.count(), before);
}

async fn failover(h: &Harness) {
    let first = Mock::new().await;
    let second = Mock::new().await;
    let account = h.provider(
        "custom-native-fail-first",
        "anthropic",
        &first,
        json!([{ "id":"fallback-model","enabled":true }]),
        json!([]),
    );
    h.provider(
        "custom-native-fail-second",
        "chat_completions",
        &second,
        json!([{ "id":"fallback-model","enabled":true }]),
        json!([]),
    );
    h.key(
        "key-failover-tests",
        &["custom-native-fail-first", "custom-native-fail-second"],
        &[],
    );
    first.push(Reply::failure(
        429,
        json!({"type":"error","error":{"type":"rate_limit_error","message":"rate limited"}}),
    ));
    second.push(Reply::sse(chat_stream("fallback-model")));
    let (status, _, bytes) = h
        .call(
            "/v1/messages",
            "key-failover-tests",
            native_request("fallback-model", true),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    assert_eq!(first.count(), 1);
    assert_eq!(second.count(), 1);
    assert_eq!(first.last().body["thinking"]["type"], "adaptive");
    assert_eq!(second.last().body["reasoning_effort"], "max");
    let rows = h.rows();
    let row = rows
        .iter()
        .find(|row| row["model"] == "fallback-model")
        .unwrap();
    assert_eq!(row["attempts"], 2, "{row}");
    assert!(row["error"].is_null(), "{row}");
    let stored = h
        .state
        .store()
        .with_lock(|guard| h.state.store().load_locked(guard));
    let record = stored
        .accounts
        .iter()
        .find(|record| record.id() == account)
        .unwrap();
    let limits = record.get("rateLimits").cloned().unwrap_or(Value::Null);
    assert!(
        limits.get("fallback-model").is_some(),
        "stored rate limits: {limits}"
    );
    let accounts = h.state.store().list_accounts();
    let limited = accounts["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == account)
        .unwrap();
    assert_eq!(
        limited["rateLimits"], limits,
        "public account must include stored rate limits"
    );
    assert_eq!(
        h.state.store().route_account(&account).unwrap()["rateLimits"],
        limits
    );

    second.push(Reply::sse(chat_stream("fallback-model")));
    assert_eq!(
        h.call(
            "/v1/messages",
            "key-failover-tests",
            native_request("fallback-model", true)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        first.count(),
        1,
        "a cooling account must not receive another request"
    );
    assert_eq!(second.count(), 2);

    assert!(h.state.store().clear_rate_limit(&account, "fallback-model"));
    assert!(
        h.state.store().route_account(&account).unwrap()["rateLimits"]
            .get("fallback-model")
            .is_none()
    );
    first.push(Reply::sse(message_stream("fallback-model")));
    assert_eq!(
        h.call(
            "/v1/messages",
            "key-failover-tests",
            native_request("fallback-model", true)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        first.count(),
        2,
        "clearing the limit must make the account eligible again"
    );
    assert_eq!(second.count(), 2);
}
