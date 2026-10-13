use super::*;
use crate::server::core::providers::zcode::adapter::{ZCODE_ADAPTER, ZCODE_INTL_ADAPTER};
use crate::server::core::upstream::usage::RequestTelemetry;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const QUOTA_BODY: &str = r#"{"code":1005,"msg":"exceed quota limit","logid":"test-request"}"#;

async fn mock_upstream(
    status: u16,
    content_type: &'static str,
    body: &'static str,
) -> (
    TransportRequest,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let hits = Arc::new(AtomicUsize::new(0));
    let count = hits.clone();
    let app = axum::Router::new().route(
        "/messages",
        axum::routing::post(move || {
            count.fetch_add(1, Ordering::SeqCst);
            async move {
                (
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    [(axum::http::header::CONTENT_TYPE, content_type)],
                    body,
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/messages", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (
        TransportRequest {
            url,
            headers: vec![("Content-Type".into(), "application/json".into())],
            payload: r#"{"stream":true}"#.into(),
            proxy: None,
            // 测试替身按默认能力位（false = 直连），与生产里不带这一步的家同款
            system_proxy_when_unset: false,
        },
        hits,
        task,
    )
}

#[tokio::test]
async fn zcode_http_200_quota_json_is_classified_before_streaming_without_resend() {
    for adapter in [&ZCODE_ADAPTER, &ZCODE_INTL_ADAPTER] {
        let (transport, hits, task) =
            mock_upstream(200, "application/json; charset=utf-8", QUOTA_BODY).await;
        let mut budget = RetryBudget::new(3);
        let capture = crate::server::core::debug_traffic::TrafficCapture::begin("quota-test");
        let result = send_with_retry(
            adapter,
            &transport,
            &mut budget,
            Some(&capture),
            &RequestTelemetry::new(),
            false,
        )
        .await;
        task.abort();
        let failure = match result {
            Err(failure) => failure,
            Ok(_) => panic!("HTTP 200 quota envelope must not enter the success stream"),
        };
        assert!(matches!(
            failure.class,
            UpstreamErrorClass::QuotaLimited {
                status: 429,
                upstream_code: Some(1005),
                reset_at: None,
                ..
            }
        ));
        assert_eq!(failure.error.status_code, 429);
        assert_eq!(failure.error.upstream_code, Some(1005));
        assert!(failure.error.message.contains("exceed quota limit"));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_eq!(budget.remaining, 3);
        assert_eq!(capture.captured_body(), QUOTA_BODY);
    }
}

#[test]
fn zcode_header_detection_handles_mime_parameters_without_requiring_sse_headers() {
    use axum::http::{header::CONTENT_TYPE, HeaderMap, HeaderValue};
    for (mime, expected) in [
        (None, false),
        (Some("application/json"), true),
        (Some("Application/JSON; charset=utf-8"), true),
        (Some("text/event-stream; charset=utf-8"), false),
        (Some("application/octet-stream"), false),
    ] {
        let mut headers = HeaderMap::new();
        if let Some(mime) = mime {
            headers.insert(CONTENT_TYPE, HeaderValue::from_static(mime));
        }
        assert_eq!(ZCODE_ADAPTER.is_error_response(200, &headers), expected);
        assert!(ZCODE_ADAPTER.is_error_response(401, &headers));
        assert!(ZCODE_ADAPTER.is_error_response(429, &headers));
    }
}

#[tokio::test]
async fn zcode_http_auth_and_quota_errors_keep_their_status() {
    for (status, body) in [(401, ""), (429, r#"{"msg":"rate limited"}"#)] {
        let (transport, _, task) = mock_upstream(status, "application/json", body).await;
        let result = send_with_retry(
            &ZCODE_ADAPTER,
            &transport,
            &mut RetryBudget::new(0),
            None,
            &RequestTelemetry::new(),
            false,
        )
        .await;
        task.abort();
        let failure = match result {
            Err(failure) => failure,
            Ok(_) => panic!("HTTP error must not be accepted"),
        };
        assert_eq!(failure.error.status_code, i32::from(status));
        if status == 401 {
            assert!(matches!(
                failure.class,
                UpstreamErrorClass::TokenExpired { .. }
            ));
        } else {
            assert!(matches!(
                failure.class,
                UpstreamErrorClass::QuotaLimited { .. }
            ));
        }
    }
}

#[tokio::test]
async fn zcode_unknown_or_malformed_success_json_returns_an_error() {
    for body in [r#"{"code":9999,"msg":"unexpected rejection"}"#, "not json"] {
        let (transport, _, task) = mock_upstream(200, "application/json", body).await;
        let result = send_with_retry(
            &ZCODE_ADAPTER,
            &transport,
            &mut RetryBudget::new(0),
            None,
            &RequestTelemetry::new(),
            false,
        )
        .await;
        task.abort();
        let failure = match result {
            Err(failure) => failure,
            Ok(_) => panic!("non-SSE ZCode response must not be accepted"),
        };
        assert_eq!(failure.error.status_code, 502);
        assert!(failure.error.message.contains(if body == "not json" {
            "not json"
        } else {
            "unexpected rejection"
        }));
    }
}

#[tokio::test]
async fn zcode_success_sse_and_other_providers_json_remain_unconsumed() {
    let sse = "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_test\"}}\n\n";
    let cases: [(&dyn ProviderAdapter, &str, &str); 2] = [
        (&ZCODE_ADAPTER, "text/event-stream", sse),
        (
            adapter_for(ProviderKind::WorkBuddy),
            "application/json",
            QUOTA_BODY,
        ),
    ];
    for (adapter, content_type, body) in cases {
        let (transport, _, task) = mock_upstream(200, content_type, body).await;
        let result = send_with_retry(
            adapter,
            &transport,
            &mut RetryBudget::new(0),
            None,
            &RequestTelemetry::new(),
            false,
        )
        .await;
        task.abort();
        let response = match result {
            Ok(response) => response,
            Err(failure) => panic!("valid response rejected: {}", failure.error.message),
        };
        assert_eq!(response.text().await.unwrap(), body);
    }
}
