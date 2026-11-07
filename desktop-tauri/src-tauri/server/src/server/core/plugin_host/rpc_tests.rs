use super::*;

fn adapter() -> RpcAdapter {
    let info = serde_json::from_value(serde_json::json!({ "id": "demo", "label": "测试" }))
        .expect("最小 provider 描述有效");
    let host = Arc::new(Host::new(
        HostOptions::new("unused-test-plugin"),
        Arc::new(super::super::tests::TestCallbacks),
    ));
    RpcAdapter::new("demo".into(), ProviderKind::WorkBuddy, host, info)
}

#[test]
fn conversation_quota_keeps_class_reset_time_and_business_code() {
    let reset_at = 1_900_000_000_000;
    let error = conversation_error(
        403,
        UpstreamErrorClass::QuotaLimited {
            reset_at: Some(reset_at),
            status: 403,
            message: "daily pool exhausted".into(),
            upstream_code: Some(4291200),
        },
        &serde_json::json!({}),
    );
    assert_eq!(error.status_code, 403);
    assert_eq!(error.upstream_code, Some(4291200));
    assert_eq!(error.message, "daily pool exhausted");
    assert!(
        matches!(adapter().classify_conversation_error(&error), UpstreamErrorClass::QuotaLimited {
        reset_at: Some(at), status: 403, upstream_code: Some(4291200), ..
    } if at == reset_at)
    );
}

#[test]
fn an_explicit_fatal_429_is_not_reclassified_as_quota() {
    let error = conversation_error(
        429,
        UpstreamErrorClass::Fatal {
            status: 429,
            message: "temporary upstream failure".into(),
            upstream_code: Some(9),
        },
        &serde_json::json!({}),
    );
    assert!(matches!(
        adapter().classify_conversation_error(&error),
        UpstreamErrorClass::Fatal {
            status: 429,
            upstream_code: Some(9),
            ..
        }
    ));
}

#[test]
fn content_block_and_expired_token_survive_the_conversation_boundary() {
    let blocked = conversation_error(
        400,
        UpstreamErrorClass::ContentBlocked {
            status: 400,
            message: "content blocked".into(),
            upstream_code: Some(11128),
        },
        &serde_json::json!({}),
    );
    assert!(matches!(
        adapter().classify_conversation_error(&blocked),
        UpstreamErrorClass::ContentBlocked {
            status: 400,
            upstream_code: Some(11128),
            ..
        }
    ));
    let expired = conversation_error(
        401,
        UpstreamErrorClass::TokenExpired {
            message: "token expired".into(),
        },
        &serde_json::json!({}),
    );
    assert_eq!(expired.status_code, 401);
    assert!(matches!(
        adapter().classify_conversation_error(&expired),
        UpstreamErrorClass::TokenExpired { .. }
    ));
}

#[test]
fn empty_classification_message_falls_back_to_the_upstream_body() {
    let error = conversation_error(
        403,
        UpstreamErrorClass::QuotaLimited {
            reset_at: None,
            status: 403,
            message: String::new(),
            upstream_code: Some(42),
        },
        &serde_json::json!({ "message": "original explanation" }),
    );
    assert!(error.message.contains("original explanation"));
    assert_eq!(error.upstream_code, Some(42));
    assert!(matches!(
        adapter().classify_conversation_error(&error),
        UpstreamErrorClass::QuotaLimited { status: 403, .. }
    ));
}

#[test]
fn legacy_errors_without_classification_keep_their_fallback_behavior() {
    let adapter = adapter();
    let quota = GatewayError::with_status(429, "rate limited");
    assert!(matches!(
        adapter.classify_conversation_error(&quota),
        UpstreamErrorClass::QuotaLimited { .. }
    ));
    let permission = GatewayError::with_status(403, "permission denied");
    assert!(matches!(
        adapter.classify_conversation_error(&permission),
        UpstreamErrorClass::Fatal { status: 403, .. }
    ));
}
