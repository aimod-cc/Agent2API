//! Contract tests for account-group failover error preservation.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde_json::{json, Value};

use super::{ForwardOutcome, ForwardRequest};
use crate::server::core::account_groups::AccountGroupScope;
use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::catalog;
use crate::server::core::upstream::continuation::Continuation;
use crate::server::core::upstream::usage::RequestTelemetry;

const MODEL: &str = "deepseek-v4.1-flash";
const PROVIDER: &str = "workbuddy";
const GROUP_UNAVAILABLE_CODE: &str = "account_group_no_available_account";
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct TempAccounts<'a> {
    store: &'a AccountStore,
    ids: Vec<String>,
}

impl<'a> TempAccounts<'a> {
    fn new(store: &'a AccountStore) -> Self {
        Self {
            store,
            ids: Vec::new(),
        }
    }

    fn add(&mut self, case: &str, endpoint: &str) -> Value {
        let uid = unique_id(case);
        let record = self
            .store
            .add_account(
                &json!({
                    "auth": {
                        "accessToken": format!("{uid}-token"),
                        "refreshToken": format!("{uid}-refresh"),
                    },
                    "account": {"uid": uid},
                    "endpoint": endpoint,
                    "prefixPath": "/plugin",
                    "limiters": [],
                }),
                Some(&uid),
                Some(PROVIDER),
            )
            .expect("真实账号 fixture 应能写入");
        self.ids.push(
            record["id"]
                .as_str()
                .expect("真实账号 fixture 应有 id")
                .to_string(),
        );
        record
    }
}

impl Drop for TempAccounts<'_> {
    fn drop(&mut self) {
        for id in &self.ids {
            self.store.remove_account(id).expect("临时账号应能清理");
        }
    }
}

fn unique_id(case: &str) -> String {
    format!(
        "agent2api-account-groups-{}-{case}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    )
}

fn real_provider() -> &'static str {
    assert!(
        catalog::forwarding_providers(MODEL)
            .iter()
            .any(|provider| provider == PROVIDER),
        "测试模型应包含真实 WorkBuddy provider"
    );
    PROVIDER
}

fn member(id: &str, provider: &str) -> Value {
    json!({
        "id": id,
        "name": id,
        "provider": provider,
        "enabled": true,
        "hasCredentials": true,
        "limiters": [],
    })
}

fn stored_member(record: &Value) -> Value {
    member(
        record["id"].as_str().expect("账号记录应有 id"),
        record["provider"].as_str().expect("账号记录应有 provider"),
    )
}

fn request(scope: AccountGroupScope) -> ForwardRequest {
    ForwardRequest {
        body: json!({
            "model": MODEL,
            "messages": [{"role": "user", "content": "account-group failover"}],
        }),
        stream: false,
        dedupe_key: String::new(),
        client_headers: Default::default(),
        telemetry: Arc::new(RequestTelemetry::new()),
        allowed_providers: None,
        account_group_scope: Some(scope),
        pinned_account: None,
        ignore_model_gate: false,
        proxy_override: None,
    }
}

async fn serve(app: axum::Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("测试上游应能监听本地端口");
    let endpoint = format!(
        "http://{}",
        listener.local_addr().expect("应能读取测试端口")
    );
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("测试上游应能持续服务");
    });
    (endpoint, task)
}

fn sse_body(content: &str) -> String {
    format!(
        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{content}\"}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n"
    )
}

async fn error_upstream(status: StatusCode) -> (String, tokio::task::JoinHandle<()>) {
    let app = axum::Router::new().route(
        "/v2/chat/completions",
        axum::routing::post(move || async move {
            (
                status,
                [(header::CONTENT_TYPE, "application/json")],
                r#"{"message":"sentinel upstream failure","code":4242}"#,
            )
        }),
    );
    serve(app).await
}

async fn success_upstream(content: &'static str) -> (String, tokio::task::JoinHandle<()>) {
    let body = sse_body(content);
    let app = axum::Router::new().route(
        "/v2/chat/completions",
        axum::routing::post(move || {
            let body = body.clone();
            async move {
                (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "text/event-stream")],
                    body,
                )
            }
        }),
    );
    serve(app).await
}

async fn refresh_upstream() -> (String, tokio::task::JoinHandle<()>) {
    let body = sse_body("refresh-ok");
    let app = axum::Router::new()
        .route(
            "/v2/chat/completions",
            axum::routing::post(move |headers: HeaderMap| {
                let body = body.clone();
                async move {
                    let refreshed = headers
                        .get(header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        == Some("Bearer refreshed-token");
                    let (status, body) = if refreshed {
                        (StatusCode::OK, body)
                    } else {
                        (
                            StatusCode::UNAUTHORIZED,
                            r#"{"message":"expired","code":401}"#.to_string(),
                        )
                    };
                    (status, [(header::CONTENT_TYPE, "text/event-stream")], body)
                }
            }),
        )
        .route(
            "/v2/plugin/auth/token/refresh",
            axum::routing::post(|| async {
                (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "application/json")],
                    r#"{"code":0,"data":{"accessToken":"refreshed-token","refreshToken":"refreshed-refresh"}}"#,
                )
                    .into_response()
            }),
        );
    serve(app).await
}

fn expect_gateway_error(
    result: Result<ForwardOutcome, crate::server::errors::GatewayError>,
    message: &str,
) -> crate::server::errors::GatewayError {
    match result {
        Err(error) => error,
        Ok(_) => panic!("{message}"),
    }
}

#[tokio::test]
async fn missing_group_session_returns_group_unavailable_before_upstream() {
    let env = super::super::account_groups::test_bootstrap::lock();
    let scope = AccountGroupScope::from_canonical(
        unique_id("no-session-group"),
        vec![member("missing", real_provider())],
    );

    let error = expect_gateway_error(
        env.state.upstream().forward(request(scope)).await,
        "没有组内 session 时不应尝试默认登录或上游请求",
    );

    assert_eq!(error.status_code, 503);
    assert_eq!(error.code.as_deref(), Some(GROUP_UNAVAILABLE_CODE));
    assert_eq!(error.message, "组内无可用账号");
}

#[tokio::test]
async fn upstream_error_survives_failover_to_group_member_without_session() {
    let (endpoint, task) = error_upstream(StatusCode::PAYMENT_REQUIRED).await;
    let env = super::super::account_groups::test_bootstrap::lock();
    let mut accounts = TempAccounts::new(env.state.store());
    let first = accounts.add("last-error", &endpoint);
    let scope = AccountGroupScope::from_canonical(
        unique_id("last-error-group"),
        vec![
            stored_member(&first),
            member("missing-after-upstream", real_provider()),
        ],
    );

    let error = expect_gateway_error(
        env.state.upstream().forward(request(scope)).await,
        "上游失败后应返回最后一次真实上游错误",
    );
    task.abort();

    assert_eq!(error.status_code, 402);
    assert_eq!(error.upstream_code, Some(4242));
    assert_ne!(error.code.as_deref(), Some(GROUP_UNAVAILABLE_CODE));
    assert!(error.message.contains("sentinel upstream failure"));
}

#[tokio::test]
async fn quota_error_survives_failover_to_group_member_without_session() {
    let (endpoint, task) = error_upstream(StatusCode::TOO_MANY_REQUESTS).await;
    let env = super::super::account_groups::test_bootstrap::lock();
    let mut accounts = TempAccounts::new(env.state.store());
    let first = accounts.add("quota-error", &endpoint);
    let scope = AccountGroupScope::from_canonical(
        unique_id("quota-error-group"),
        vec![
            stored_member(&first),
            member("missing-after-quota", real_provider()),
        ],
    );

    let error = expect_gateway_error(
        env.state.upstream().forward(request(scope)).await,
        "429 后应返回最后一次真实上游错误",
    );
    task.abort();

    assert_eq!(error.status_code, 429);
    assert_eq!(error.upstream_code, Some(4242));
    assert_ne!(error.code.as_deref(), Some(GROUP_UNAVAILABLE_CODE));
    assert!(error.message.contains("sentinel upstream failure"));
}

#[tokio::test]
async fn single_group_member_quota_error_is_returned_as_is() {
    let (endpoint, task) = error_upstream(StatusCode::TOO_MANY_REQUESTS).await;
    let env = super::super::account_groups::test_bootstrap::lock();
    let mut accounts = TempAccounts::new(env.state.store());
    let record = accounts.add("single-quota", &endpoint);
    let scope = AccountGroupScope::from_canonical(
        unique_id("single-quota-group"),
        vec![stored_member(&record)],
    );

    let error = expect_gateway_error(
        env.state.upstream().forward(request(scope)).await,
        "单候选 429 应原样返回上游错误",
    );
    task.abort();

    assert_eq!(error.status_code, 429);
    assert_eq!(error.upstream_code, Some(4242));
    assert!(error.message.contains("sentinel upstream failure"));
}

#[tokio::test]
async fn expired_token_refreshes_and_retries_same_group_account() {
    let (endpoint, task) = refresh_upstream().await;
    let env = super::super::account_groups::test_bootstrap::lock();
    let mut accounts = TempAccounts::new(env.state.store());
    let record = accounts.add("refresh", &endpoint);
    let scope =
        AccountGroupScope::from_canonical(unique_id("refresh-group"), vec![stored_member(&record)]);

    let result = env.state.upstream().forward(request(scope)).await;
    task.abort();
    let outcome = result.expect("401 刷新后同账号应重试成功");
    let ForwardOutcome::Completion { body } = outcome else {
        panic!("非流式请求应返回 completion");
    };
    assert_eq!(body["choices"][0]["message"]["content"], "refresh-ok");
}

#[tokio::test]
async fn continuation_keeps_account_group_scope() {
    let (outside_endpoint, outside_task) = success_upstream("outside-result.").await;
    let (group_endpoint, group_task) = success_upstream("group-result.").await;
    let env = super::super::account_groups::test_bootstrap::lock();
    let mut accounts = TempAccounts::new(env.state.store());
    let outside = accounts.add("continuation-outside", &outside_endpoint);
    let group = accounts.add("continuation-group", &group_endpoint);
    let scope = AccountGroupScope::from_canonical(
        unique_id("continuation-scope"),
        vec![stored_member(&group)],
    );
    let continuation = Continuation {
        service: env.state.upstream().clone(),
        base_body: json!({
            "model": MODEL,
            "messages": [{"role": "user", "content": "continue"}],
        }),
        stream: false,
        client_headers: Default::default(),
        telemetry: Arc::new(RequestTelemetry::new()),
        allowed_providers: None,
        account_group_scope: Some(scope),
        max_rounds: 2,
    };

    let completed = continuation
        .complete(json!({
            "choices": [{
                "message": {"role": "assistant", "content": "half,"},
                "finish_reason": "length",
            }]
        }))
        .await;
    outside_task.abort();
    group_task.abort();

    assert_eq!(
        completed["choices"][0]["message"]["content"],
        "half,group-result."
    );
    assert_ne!(outside["id"], group["id"]);
}
