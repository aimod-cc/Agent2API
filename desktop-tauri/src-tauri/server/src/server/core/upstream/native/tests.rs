use super::*;
use crate::server::core::debug_traffic::TrafficCapture;
use crate::server::core::upstream::connections::Connections;

fn event(kind: &str, mut body: Value) -> String {
    body["type"] = kind.into();
    format!("event: {kind}\ndata: {body}\n\n")
}

fn anthropic() -> String {
    [
        event("message_start", json!({"message":{"id":"msg_1","type":"message","role":"assistant","model":"wire","content":[],
            "usage":{"input_tokens":10,"cache_read_input_tokens":20,"cache_creation_input_tokens":30,"output_tokens":1},"future":{"keep":true}}})),
        event("content_block_start", json!({"index":0,"content_block":{"type":"thinking","thinking":"","signature":""}})),
        event("content_block_delta", json!({"index":0,"delta":{"type":"thinking_delta","thinking":"思考🦜"}})),
        event("content_block_delta", json!({"index":0,"delta":{"type":"signature_delta","signature":"sig+/=="}})),
        event("content_block_stop", json!({"index":0})),
        event("content_block_start", json!({"index":1,"content_block":{"type":"redacted_thinking","data":"opaque+/=="}})),
        event("content_block_stop", json!({"index":1})),
        event("content_block_start", json!({"index":2,"content_block":{"type":"tool_use","id":"tool_1","name":"read","input":{}}})),
        event("content_block_delta", json!({"index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}})),
        event("content_block_delta", json!({"index":2,"delta":{"type":"input_json_delta","partial_json":"\"文档.txt\"}"}})),
        event("content_block_stop", json!({"index":2})),
        event("message_delta", json!({"delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":8}})),
        event("message_stop", json!({})),
    ].concat()
}

fn source(chunks: Vec<Bytes>) -> futures::stream::BoxStream<'static, Result<Bytes, io::Error>> {
    futures::stream::iter(chunks.into_iter().map(Ok)).boxed()
}

fn stream_with(
    source: futures::stream::BoxStream<'static, Result<Bytes, io::Error>>,
    protocol: WireProtocol,
    requested: &str,
    telemetry: Arc<RequestTelemetry>,
) -> NativeStream {
    NativeStream {
        source,
        protocol,
        requested: requested.into(),
        capture: telemetry.capture(),
        telemetry,
        observer: Observer::default(),
        buffer: Vec::new(),
        scan_from: 0,
        line_start: 0,
        pending: VecDeque::new(),
        ended: false,
        terminal: false,
        sequence: 0,
        saw_message: false,
        _slot: None,
        _connection: ConnectionGuard::new(Connections::new()),
    }
}

async fn drain(mut stream: NativeStream) -> Vec<u8> {
    let mut output = Vec::new();
    while let Some(chunk) = stream.next().await {
        output.extend_from_slice(&chunk.unwrap());
    }
    output
}

fn chunks(text: &str, size: usize) -> Vec<Bytes> {
    text.as_bytes()
        .chunks(size)
        .map(Bytes::copy_from_slice)
        .collect()
}

#[tokio::test]
async fn signed_anthropic_stream_is_byte_exact_at_every_split() {
    let text = anthropic();
    for split in 0..=text.len() {
        let telemetry = Arc::new(RequestTelemetry::new());
        let pieces = vec![
            Bytes::copy_from_slice(&text.as_bytes()[..split]),
            Bytes::copy_from_slice(&text.as_bytes()[split..]),
        ];
        let output = drain(stream_with(
            source(pieces),
            WireProtocol::Anthropic,
            "wire",
            telemetry.clone(),
        ))
        .await;
        assert_eq!(output, text.as_bytes(), "split={split}");
        let usage = telemetry.snapshot();
        assert_eq!(
            (
                usage.prompt_tokens,
                usage.completion_tokens,
                usage.total_tokens,
                usage.cache_read_tokens
            ),
            (60, 8, 68, 20)
        );
        assert!(usage.error.is_none(), "{:?}", usage.error);
    }
}

#[tokio::test]
async fn unicode_and_crlf_survive_one_byte_chunks_and_model_aliasing() {
    let text = anthropic().replace('\n', "\r\n");
    let telemetry = Arc::new(RequestTelemetry::new());
    let output = String::from_utf8(
        drain(stream_with(
            source(chunks(&text, 1)),
            WireProtocol::Anthropic,
            "alias",
            telemetry,
        ))
        .await,
    )
    .unwrap();
    assert!(output.contains("\"model\":\"alias\""));
    for line in text.lines().filter(|line| !line.contains("message_start")) {
        assert!(output.contains(line), "{line}");
    }
    assert!(output.contains("sig+/=="));
    assert!(output.contains("opaque+/=="));
    assert!(output.contains("思考🦜"));
}

#[tokio::test]
async fn anthropic_nonstream_aggregation_keeps_signatures_tools_and_usage() {
    let text = anthropic();
    let telemetry = RequestTelemetry::new();
    let body = collect_native(
        source(chunks(&text, 1)),
        WireProtocol::Anthropic,
        &telemetry,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        body["content"][0],
        json!({"type":"thinking","thinking":"思考🦜","signature":"sig+/=="})
    );
    assert_eq!(
        body["content"][1],
        json!({"type":"redacted_thinking","data":"opaque+/=="})
    );
    assert_eq!(body["content"][2]["input"], json!({"path":"文档.txt"}));
    assert_eq!(body["stop_reason"], "tool_use");
    assert_eq!(body["usage"]["output_tokens"], 8);
    assert_eq!(body["usage"]["cache_read_input_tokens"], 20);
    assert_eq!(body["future"], json!({"keep":true}));
    assert_eq!(telemetry.snapshot().total_tokens, 68);
}

#[tokio::test]
async fn responses_stream_keeps_encrypted_items_extensions_and_usage() {
    let text = [
        event("response.created", json!({"sequence_number":0,"response":{"id":"resp_1","model":"wire","status":"in_progress","output":[]}})),
        event("response.output_item.added", json!({"sequence_number":1,"item":{"type":"reasoning","id":"rs_1","encrypted_content":"opaque"}})),
        event("future.native.event", json!({"sequence_number":2,"future":{"signature":"keep"}})),
        event("response.completed", json!({"sequence_number":3,"response":{"id":"resp_1","model":"wire","status":"completed",
            "output":[{"type":"reasoning","id":"rs_1","encrypted_content":"opaque"}],
            "usage":{"input_tokens":42,"output_tokens":7,"total_tokens":49,"input_tokens_details":{"cached_tokens":30}},"future":true}})),
    ].concat();
    let telemetry = Arc::new(RequestTelemetry::new());
    assert_eq!(
        drain(stream_with(
            source(chunks(&text, 1)),
            WireProtocol::Responses,
            "wire",
            telemetry.clone()
        ))
        .await,
        text.as_bytes()
    );
    let usage = telemetry.snapshot();
    assert_eq!(
        (
            usage.prompt_tokens,
            usage.completion_tokens,
            usage.total_tokens,
            usage.cache_read_tokens
        ),
        (42, 7, 49, 30)
    );
    let body = collect_native(
        source(chunks(&text, 3)),
        WireProtocol::Responses,
        &telemetry,
        None,
    )
    .await
    .unwrap();
    assert_eq!(body["output"][0]["encrypted_content"], "opaque");
    assert_eq!(body["future"], true);
}

#[tokio::test]
async fn response_error_then_failed_event_are_both_forwarded() {
    let error = event(
        "error",
        json!({"sequence_number":8,"code":"quota","message":"quota exceeded"}),
    );
    let failed = event(
        "response.failed",
        json!({"sequence_number":9,"response":{"id":"resp_1","status":"failed","error":{"message":"quota exceeded"}}}),
    );
    let text = error + &failed;
    let telemetry = Arc::new(RequestTelemetry::new());
    assert_eq!(
        drain(stream_with(
            source(chunks(&text, 1)),
            WireProtocol::Responses,
            "wire",
            telemetry.clone()
        ))
        .await,
        text.as_bytes()
    );
    assert_eq!(
        telemetry.snapshot().error.as_deref(),
        Some("quota exceeded")
    );
}

#[tokio::test]
async fn truncated_stream_reports_error_without_forwarding_partial_event() {
    let first = event(
        "message_start",
        json!({"message":{"id":"msg_1","model":"wire"}}),
    );
    let text = first.clone() + "event: content_block_delta\ndata: {\"type\":";
    let telemetry = Arc::new(RequestTelemetry::new());
    let output = String::from_utf8(
        drain(stream_with(
            source(chunks(&text, 5)),
            WireProtocol::Anthropic,
            "wire",
            telemetry.clone(),
        ))
        .await,
    )
    .unwrap();
    assert!(output.starts_with(&first));
    assert!(!output.contains("event: content_block_delta"));
    assert_eq!(output.matches("event: error\n").count(), 1);
    assert!(telemetry.snapshot().error.is_some());
    assert!(collect_native(
        source(chunks(&text, 3)),
        WireProtocol::Anthropic,
        &RequestTelemetry::new(),
        None
    )
    .await
    .is_err());
}

#[tokio::test]
async fn cancellation_wakes_an_idle_stream_and_releases_connection() {
    let telemetry = Arc::new(RequestTelemetry::new());
    let token = Arc::new(cancellation::CancelToken::new());
    telemetry.set_cancel_token(token.clone());
    let source = cancellation::cancellable(futures::stream::pending().boxed(), Some(token.clone()));
    let connections = Connections::new();
    let mut guard = ConnectionGuard::new(connections.clone());
    guard.rebind(Some("account-1".into()));
    let mut stream = stream_with(source, WireProtocol::Anthropic, "wire", telemetry.clone());
    stream._connection = guard.handoff();
    assert_eq!(connections.snapshot(), vec![("account-1".into(), 1)]);
    token.cancel();
    let output = tokio::time::timeout(Duration::from_secs(1), drain(stream))
        .await
        .unwrap();
    assert!(String::from_utf8(output)
        .unwrap()
        .contains(cancellation::MANUAL_TERMINATED));
    assert_eq!(
        telemetry.snapshot().error.as_deref(),
        Some(cancellation::MANUAL_TERMINATED)
    );
    assert!(connections.snapshot().is_empty());
}

#[tokio::test]
async fn idle_timeout_emits_a_protocol_error() {
    let telemetry = Arc::new(RequestTelemetry::new());
    let source = stall::idle_guard(
        futures::stream::pending().boxed(),
        Duration::from_millis(15),
    );
    let output = tokio::time::timeout(
        Duration::from_secs(1),
        drain(stream_with(
            source,
            WireProtocol::Responses,
            "wire",
            telemetry.clone(),
        )),
    )
    .await
    .unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("event: error\n"));
    assert!(output.contains("\"sequence_number\":0"));
    assert!(telemetry.snapshot().error.is_some());
}

#[tokio::test]
async fn debug_capture_contains_only_upstream_bytes_once() {
    let text = anthropic();
    let capture = Arc::new(TrafficCapture::begin("native-capture"));
    let telemetry = Arc::new(RequestTelemetry::new());
    telemetry.set_capture(capture.clone());
    let output = drain(stream_with(
        source(chunks(&text, 7)),
        WireProtocol::Anthropic,
        "alias",
        telemetry,
    ))
    .await;
    assert!(String::from_utf8(output)
        .unwrap()
        .contains("\"model\":\"alias\""));
    assert_eq!(capture.captured_body(), text);
}

#[tokio::test]
async fn plain_json_keeps_native_fields_and_rewrites_only_model() {
    let input = json!({"id":"msg_1","type":"message","model":"wire","content":[{"type":"thinking","thinking":"","signature":"signed"}],
        "usage":{"input_tokens":10,"cache_read_input_tokens":20,"output_tokens":5},"future":true});
    let response: reqwest::Response = axum::http::Response::builder()
        .status(201)
        .header("content-type", "application/json")
        .header("request-id", "request-1")
        .header("set-cookie", "upstream-secret")
        .body(input.to_string())
        .unwrap()
        .into();
    let telemetry = Arc::new(RequestTelemetry::new());
    let output = receive(
        response,
        WireProtocol::Anthropic,
        "alias",
        false,
        telemetry.clone(),
        &mut None,
        &mut ConnectionGuard::new(Connections::new()),
    )
    .await
    .unwrap();
    assert_eq!(output.status, 201);
    assert_eq!(output.headers["request-id"], "request-1");
    assert!(!output.headers.contains_key("set-cookie"));
    let NativeBody::Json(body) = output.body else {
        panic!("expected JSON")
    };
    assert_eq!(body["content"], input["content"]);
    assert_eq!(body["model"], "alias");
    assert_eq!(body["future"], true);
    assert_eq!(telemetry.snapshot().total_tokens, 35);
}

#[tokio::test]
async fn malformed_json_body_is_not_a_success() {
    let response: reqwest::Response = axum::http::Response::builder()
        .status(200)
        .header("content-type", "application/json")
        .body("not json")
        .unwrap()
        .into();
    let result = receive(
        response,
        WireProtocol::Anthropic,
        "alias",
        false,
        Arc::new(RequestTelemetry::new()),
        &mut None,
        &mut ConnectionGuard::new(Connections::new()),
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn bare_done_without_a_native_message_is_not_success() {
    for protocol in [WireProtocol::Anthropic, WireProtocol::Responses] {
        let telemetry = Arc::new(RequestTelemetry::new());
        let _ = drain(stream_with(
            source(chunks("data: [DONE]\n\n", 1)),
            protocol,
            "wire",
            telemetry.clone(),
        ))
        .await;
        assert!(
            telemetry.snapshot().error.is_some(),
            "a Chat marker is not a native completed response"
        );
    }
}

#[tokio::test]
async fn malformed_content_followed_by_stop_is_not_success() {
    let text = event(
        "message_start",
        json!({"message":{"id":"msg_1","model":"wire"}}),
    ) + "event: content_block_delta\ndata: {broken}\n\n"
        + &event("message_stop", json!({}));
    let telemetry = Arc::new(RequestTelemetry::new());
    let _ = drain(stream_with(
        source(chunks(&text, 4)),
        WireProtocol::Anthropic,
        "wire",
        telemetry.clone(),
    ))
    .await;
    assert!(telemetry.snapshot().error.is_some());
    assert!(collect_native(
        source(chunks(&text, 4)),
        WireProtocol::Anthropic,
        &RequestTelemetry::new(),
        None
    )
    .await
    .is_err());
}

#[tokio::test]
async fn unclosed_anthropic_block_is_not_silently_aggregated() {
    let text = event(
        "message_start",
        json!({"message":{"id":"msg_1","model":"wire","content":[]}}),
    ) + &event(
        "content_block_start",
        json!({"index":0,"content_block":{"type":"thinking","thinking":"partial","signature":""}}),
    ) + &event("message_stop", json!({}));
    assert!(collect_native(
        source(chunks(&text, 4)),
        WireProtocol::Anthropic,
        &RequestTelemetry::new(),
        None
    )
    .await
    .is_err());
}

#[tokio::test]
async fn oversized_complete_event_obeys_the_same_limit_as_a_fragment() {
    let text = event(
        "future.native.event",
        json!({"data":"x".repeat(8 * 1024 * 1024)}),
    ) + &event("message_stop", json!({}));
    let telemetry = Arc::new(RequestTelemetry::new());
    let _ = drain(stream_with(
        source(vec![Bytes::from(text)]),
        WireProtocol::Anthropic,
        "wire",
        telemetry.clone(),
    ))
    .await;
    assert!(telemetry.snapshot().error.is_some());
}
