use super::*;
use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::adapter::{ChatRequestPlan, ModelRefreshOutcome};
use crate::server::core::providers::images::ImageResponseKind;
use crate::server::core::providers::policy::ErrorPolicy;
use crate::server::core::upstream::usage::RequestTelemetry;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Default)]
struct TestAdapter {
    retry: bool,
    stop: bool,
    content_blocked: bool,
    json_error: bool,
    suspend: bool,
}

impl ProviderAdapter for TestAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::WorkBuddy
    }
    fn provider_id(&self) -> &'static str {
        "retry-test"
    }
    fn list_models(&self) -> Vec<Value> {
        Vec::new()
    }
    fn build_chat_request(
        &self,
        _: &Value,
        _: &Value,
        _: &axum::http::HeaderMap,
    ) -> Result<ChatRequestPlan, GatewayError> {
        panic!("发送循环测试不应该读取真实账号或构造真实请求")
    }
    fn ensure_access_token<'a>(
        &'a self,
        _: &'a AccountStore,
        _: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async { panic!("发送循环测试不应该读取真实凭证") })
    }
    fn refresh_models<'a>(
        &'a self,
        _: &'a AccountStore,
        _: &'a str,
        _: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelRefreshOutcome> + Send + 'a>> {
        Box::pin(async { panic!("发送循环测试不应该拉取远程模型目录") })
    }
    fn classify_error(&self, status: u16, body: &Value) -> UpstreamErrorClass {
        let message = body["message"].as_str().unwrap_or("test error").to_string();
        let upstream_code = body["code"].as_i64();
        if self.content_blocked {
            return UpstreamErrorClass::ContentBlocked {
                status: 400,
                message,
                upstream_code,
            };
        }
        match status {
            401 => UpstreamErrorClass::TokenExpired { message },
            429 => UpstreamErrorClass::QuotaLimited {
                reset_at: None,
                status,
                message,
                upstream_code,
            },
            _ if self.json_error && status == 200 => UpstreamErrorClass::QuotaLimited {
                reset_at: None,
                status: 429,
                message,
                upstream_code,
            },
            _ => UpstreamErrorClass::Fatal {
                status,
                message,
                upstream_code,
            },
        }
    }
    fn is_error_response(&self, status: u16, headers: &axum::http::HeaderMap) -> bool {
        !(200..300).contains(&status)
            || self.json_error
                && headers
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| value.starts_with("application/json"))
    }
    fn retry_decision_async<'a>(
        &'a self,
        _: u16,
        _: &'a Value,
        attempt: usize,
        budget: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = RetryDecision> + Send + 'a>> {
        let decision = if self.stop {
            RetryDecision::Stop {
                reason: "明确终止".into(),
            }
        } else if self.retry && attempt < budget {
            RetryDecision::Retry {
                delay_ms: 0,
                reason: "测试重试".into(),
            }
        } else {
            RetryDecision::Default
        };
        Box::pin(async move { decision })
    }
    fn error_policy_async<'a>(
        &'a self,
        _: u16,
        _: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ErrorPolicy> + Send + 'a>> {
        let policy = ErrorPolicy {
            suspend_account: self.suspend,
            ..Default::default()
        };
        Box::pin(async move { policy })
    }
}

struct MockUpstream {
    transport: TransportRequest,
    hits: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockUpstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn upstream(responses: Vec<(u16, &'static str, &'static str)>) -> MockUpstream {
    assert!(!responses.is_empty());
    let hits = Arc::new(AtomicUsize::new(0));
    let count = hits.clone();
    let app = axum::Router::new().route(
        "/messages",
        axum::routing::post(move || {
            let index = count
                .fetch_add(1, Ordering::SeqCst)
                .min(responses.len() - 1);
            let (status, content_type, body) = responses[index];
            async move {
                (
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    [("content-type", content_type)],
                    body,
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/messages", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    MockUpstream {
        transport: TransportRequest {
            url,
            headers: vec![("Content-Type".into(), "application/json".into())],
            payload: "{}".into(),
            proxy: None,
            system_proxy_when_unset: false,
        },
        hits,
        task,
    }
}

async fn send(
    adapter: &TestAdapter,
    upstream: &MockUpstream,
    budget: &mut RetryBudget,
    telemetry: &Arc<RequestTelemetry>,
    degraded: bool,
    image: Option<ImageResponseKind>,
) -> Result<super::super::images::ImageResponse, OutboundFailure> {
    tokio::time::timeout(
        Duration::from_secs(5),
        send_with_retry(
            adapter,
            &upstream.transport,
            budget,
            None,
            telemetry,
            degraded,
            image,
            None,
        ),
    )
    .await
    .expect("本机模拟请求不能无限重试或等待")
}

fn failure(
    result: Result<super::super::images::ImageResponse, OutboundFailure>,
) -> OutboundFailure {
    match result {
        Ok(_) => panic!("应当返回上游错误"),
        Err(error) => error,
    }
}

fn telemetry() -> Arc<RequestTelemetry> {
    let telemetry = Arc::new(RequestTelemetry::new());
    telemetry.note_attempt_started("retry-test", "test-account");
    telemetry
}

const ERROR: (u16, &str, &str) = (
    409,
    "application/json",
    r#"{"code":123,"message":"retry later"}"#,
);
const SUCCESS: (u16, &str, &str) = (200, "text/event-stream", "data: [DONE]\n\n");

#[tokio::test]
async fn successful_response_does_not_spend_retry_budget() {
    let upstream = upstream(vec![SUCCESS]).await;
    let mut budget = RetryBudget::new(3);
    let telemetry = telemetry();
    assert!(send(
        &TestAdapter::default(),
        &upstream,
        &mut budget,
        &telemetry,
        false,
        None
    )
    .await
    .is_ok());
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 1);
    assert_eq!(budget.remaining, 3);
    assert!(telemetry.snapshot().attempts_detail[0].retries.is_empty());
}

#[tokio::test]
async fn a_transient_failure_can_retry_and_then_succeed() {
    let upstream = upstream(vec![ERROR, SUCCESS]).await;
    let mut budget = RetryBudget::new(3);
    let telemetry = telemetry();
    let adapter = TestAdapter {
        retry: true,
        ..Default::default()
    };
    assert!(
        send(&adapter, &upstream, &mut budget, &telemetry, false, None)
            .await
            .is_ok()
    );
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 2);
    assert_eq!(budget.remaining, 2);
    let snapshot = telemetry.snapshot();
    assert_eq!(snapshot.attempts_detail[0].retries.len(), 1);
    assert_eq!(snapshot.attempts_detail[0].retries[0].status, Some(409));
}

#[tokio::test]
async fn retry_limit_is_initial_send_plus_budget_and_is_shared_across_calls() {
    let upstream = upstream(vec![ERROR]).await;
    let mut budget = RetryBudget::new(2);
    let adapter = TestAdapter {
        retry: true,
        ..Default::default()
    };
    let telemetry = telemetry();
    let error = failure(send(&adapter, &upstream, &mut budget, &telemetry, false, None).await);
    assert_eq!(error.error.status_code, 409);
    assert_eq!(error.error.upstream_code, Some(123));
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 3);
    assert_eq!(budget.remaining, 0);
    assert_eq!(telemetry.snapshot().attempts_detail[0].retries.len(), 2);
    let _ = failure(send(&adapter, &upstream, &mut budget, &telemetry, false, None).await);
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn zero_retry_budget_still_allows_one_initial_request() {
    let upstream = upstream(vec![ERROR]).await;
    let mut budget = RetryBudget::new(0);
    let _ = failure(
        send(
            &TestAdapter {
                retry: true,
                ..Default::default()
            },
            &upstream,
            &mut budget,
            &telemetry(),
            false,
            None,
        )
        .await,
    );
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 1);
    assert_eq!(budget.remaining, 0);
}

#[tokio::test]
async fn deterministic_and_account_errors_are_not_resent_in_place() {
    for status in [401, 404, 413, 429] {
        let upstream = upstream(vec![(status, ERROR.1, ERROR.2)]).await;
        let mut budget = RetryBudget::new(3);
        let error = failure(
            send(
                &TestAdapter::default(),
                &upstream,
                &mut budget,
                &telemetry(),
                false,
                None,
            )
            .await,
        );
        assert_eq!(error.error.status_code, i32::from(status));
        assert_eq!(
            upstream.hits.load(Ordering::SeqCst),
            1,
            "HTTP {status} 不应原样重发"
        );
        assert_eq!(budget.remaining, 3);
    }
}

#[tokio::test]
async fn provider_stop_and_suspension_override_retry_advice() {
    for adapter in [
        TestAdapter {
            retry: true,
            stop: true,
            ..Default::default()
        },
        TestAdapter {
            retry: true,
            suspend: true,
            ..Default::default()
        },
    ] {
        let upstream = upstream(vec![ERROR]).await;
        let mut budget = RetryBudget::new(3);
        let error =
            failure(send(&adapter, &upstream, &mut budget, &telemetry(), false, None).await);
        assert!(error.stop || error.policy.suspend_account);
        assert_eq!(upstream.hits.load(Ordering::SeqCst), 1);
        assert_eq!(budget.remaining, 3);
    }
}

#[tokio::test]
async fn content_blocking_only_retries_after_the_degraded_attempt() {
    let adapter = TestAdapter {
        retry: true,
        content_blocked: true,
        ..Default::default()
    };
    for (degraded, expected) in [(false, 1), (true, 3)] {
        let upstream = upstream(vec![(400, ERROR.1, ERROR.2)]).await;
        let mut budget = RetryBudget::new(2);
        let error = failure(
            send(
                &adapter,
                &upstream,
                &mut budget,
                &telemetry(),
                degraded,
                None,
            )
            .await,
        );
        assert!(matches!(
            error.class,
            UpstreamErrorClass::ContentBlocked { .. }
        ));
        assert_eq!(upstream.hits.load(Ordering::SeqCst), expected);
    }
}

#[tokio::test]
async fn http_200_business_error_is_classified_instead_of_returned_as_success() {
    let upstream = upstream(vec![(200, ERROR.1, ERROR.2)]).await;
    let mut budget = RetryBudget::new(3);
    let adapter = TestAdapter {
        json_error: true,
        ..Default::default()
    };
    let error = failure(send(&adapter, &upstream, &mut budget, &telemetry(), false, None).await);
    assert!(matches!(
        error.class,
        UpstreamErrorClass::QuotaLimited { status: 429, .. }
    ));
    assert_eq!(error.error.status_code, 429);
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 1);
    assert_eq!(budget.remaining, 3);
}

#[tokio::test]
async fn uncertain_image_failure_is_not_resent_even_with_retry_advice() {
    let upstream = upstream(vec![(503, ERROR.1, ERROR.2)]).await;
    let mut budget = RetryBudget::new(3);
    let adapter = TestAdapter {
        retry: true,
        ..Default::default()
    };
    let error = failure(
        send(
            &adapter,
            &upstream,
            &mut budget,
            &telemetry(),
            false,
            Some(ImageResponseKind::Openai),
        )
        .await,
    );
    assert!(error.stop);
    assert!(error.error.message.contains("未自动重试"));
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 1);
    assert_eq!(budget.remaining, 3);
}

#[tokio::test]
async fn an_already_cancelled_request_never_reaches_the_upstream() {
    let upstream = upstream(vec![SUCCESS]).await;
    let mut budget = RetryBudget::new(3);
    let telemetry = telemetry();
    let id = super::super::request::new_request_id();
    let token = cancellation::register(&id).unwrap();
    cancellation::unregister(&id);
    token.cancel();
    telemetry.set_cancel_token(token);
    let error = failure(
        send(
            &TestAdapter::default(),
            &upstream,
            &mut budget,
            &telemetry,
            false,
            None,
        )
        .await,
    );
    assert!(error.stop);
    assert_eq!(error.error.message, cancellation::MANUAL_TERMINATED);
    assert_eq!(upstream.hits.load(Ordering::SeqCst), 0);
    assert_eq!(budget.remaining, 3);
}

#[test]
fn every_retry_advice_stops_when_the_budget_is_exhausted() {
    for status in [400, 408, 409, 500, 502, 503, 504] {
        assert!(transient_retry_advice(status, 0).is_none());
        let class = UpstreamErrorClass::Fatal {
            status,
            message: "failed".into(),
            upstream_code: None,
        };
        assert!(fallback_retry_advice(&class, 0, status).is_none());
    }
    let error = super::super::request::UpstreamRequestError {
        message: "connection refused".into(),
        reason: "connection refused".into(),
        safe_to_retry: true,
        network_failure: true,
    };
    assert!(transport_retry_advice(&error, 0).is_none());
}
