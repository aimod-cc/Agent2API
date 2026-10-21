//! Account-group failure boundaries: real upstream failures must not become 503.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{json, Value};

use super::ForwardRequest;
use crate::server::config::{
    self, RetryPatch, RetrySettings, DEFAULT_NO_RETRY_CODES, DEFAULT_RETRY_COUNT,
    DEFAULT_RETRY_INTERVAL_SECONDS, DEFAULT_RETRY_SWITCH_COUNT,
};
use crate::server::core::account_groups::AccountGroupScope;
use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::{catalog, officeace};
use crate::server::core::upstream::cancellation;
use crate::server::core::upstream::usage::RequestTelemetry;
use crate::server::core::usage_records::{self, UsageOutcome};

const MODEL: &str = "deepseek-v4.1-flash";
const GROUP_UNAVAILABLE_CODE: &str = "account_group_no_available_account";
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct RetryRestore(RetrySettings);

impl Drop for RetryRestore {
    fn drop(&mut self) {
        let restored = config::set_retry(RetryPatch {
            count: Some(self.0.count),
            account_switch_count: Some(self.0.account_switch_count),
            interval_seconds: Some(self.0.interval_seconds),
            no_retry_codes: Some(self.0.no_retry_codes.to_vec()),
        });
        if !restored {
            eprintln!("failed to restore account-group test retry settings");
        }
    }
}

fn unique_id(label: &str) -> String {
    format!(
        "agent2api-account-groups-{label}-{}-{}-{}",
        std::process::id(),
        crate::server::logging::now_ms(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    )
}

struct AccountCleanup {
    store: AccountStore,
    ids: Vec<String>,
}

impl AccountCleanup {
    fn new(store: &AccountStore) -> Self {
        Self {
            store: store.clone(),
            ids: Vec::new(),
        }
    }

    fn add_account(&mut self, label: &str, endpoint: &str) -> Value {
        let id = unique_id(label);
        let mut record = self
            .store
            .add_account(
                &json!({
                    "auth": {"accessToken": format!("{id}-token")},
                    "account": {"uid": id},
                    "endpoint": endpoint,
                    "limiters": [],
                }),
                Some(&id),
                Some("workbuddy"),
            )
            .expect("真实账号 fixture 应能写入");
        record["limiters"] = json!([]);
        self.track(&record);
        record
    }

    fn track(&mut self, account: &Value) {
        if let Some(id) = account.get("id").and_then(Value::as_str) {
            self.ids.push(id.to_string());
        }
    }
}

impl Drop for AccountCleanup {
    fn drop(&mut self) {
        for id in self.ids.drain(..) {
            if let Err(error) = self.store.remove_account(&id) {
                eprintln!("failed to remove account-group test account {id}: {error}");
            }
        }
    }
}

fn request(scope: AccountGroupScope) -> ForwardRequest {
    ForwardRequest {
        body: json!({
            "model": MODEL,
            "messages": [{"role": "user", "content": "account-group failure"}],
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

fn expect_gateway_error(
    result: Result<super::ForwardOutcome, crate::server::errors::GatewayError>,
    message: &str,
) -> crate::server::errors::GatewayError {
    result.err().unwrap_or_else(|| panic!("{message}"))
}

fn account(id: &str, provider: &str) -> Value {
    json!({
        "id": id,
        "name": id,
        "provider": provider,
        "enabled": true,
        "hasCredentials": true,
        "limiters": [],
    })
}

fn real_provider() -> String {
    catalog::forwarding_providers(MODEL)
        .into_iter()
        .next()
        .expect("测试模型至少应有一个可转发 provider")
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

async fn json_upstream(
    path: &'static str,
    status: StatusCode,
    body: &'static str,
) -> (String, tokio::task::JoinHandle<()>) {
    let app = axum::Router::new().route(
        path,
        axum::routing::post(move || async move {
            (
                status,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                body,
            )
        }),
    );
    serve(app).await
}

#[tokio::test]
async fn unauthorized_without_session_keeps_real_error_before_group_503() {
    let (endpoint, task) = json_upstream(
        "/v2/chat/completions",
        StatusCode::UNAUTHORIZED,
        r#"{"message":"sentinel unauthorized","code":4101}"#,
    )
    .await;
    let env = super::super::account_groups::test_bootstrap::lock();
    let mut cleanup = AccountCleanup::new(env.state.store());
    let first = cleanup.add_account("unauthorized-first", &endpoint);
    let missing_id = unique_id("missing-after-401");
    let scope = AccountGroupScope::from_canonical(
        "unauthorized-group",
        vec![first, account(&missing_id, &real_provider())],
    );
    let error = expect_gateway_error(
        env.state.upstream().forward(request(scope)).await,
        "上游 401 后应返回真实错误",
    );
    task.abort();

    assert_eq!(error.status_code, 401);
    assert_eq!(error.upstream_code, Some(4101));
    assert_ne!(error.code.as_deref(), Some(GROUP_UNAVAILABLE_CODE));
    assert!(error.message.contains("sentinel unauthorized"));
}

#[tokio::test]
async fn pre_send_cancellation_does_not_become_group_503() {
    let env = super::super::account_groups::test_bootstrap::lock();
    let telemetry = Arc::new(RequestTelemetry::with_id());
    let id = telemetry.id();
    let token = cancellation::register(&id).expect("请求 id 非空时应能登记取消令牌");
    telemetry.set_cancel_token(token);
    assert!(cancellation::cancel(&id));
    let scope = AccountGroupScope::from_canonical(
        "cancel-group",
        vec![account(&unique_id("cancel-member"), &real_provider())],
    );
    let mut request = request(scope);
    request.telemetry = telemetry;
    let error = expect_gateway_error(
        env.state.upstream().forward(request).await,
        "预发送取消应返回手动终止错误",
    );
    cancellation::unregister(&id);

    assert_eq!(error.status_code, 408);
    assert_eq!(error.message, "请求已被手动终止");
    assert_ne!(error.code.as_deref(), Some(GROUP_UNAVAILABLE_CODE));
}

#[tokio::test]
async fn transport_failure_returns_real_502_without_group_code() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("应能分配不可达端口");
    let endpoint = format!("http://{}", listener.local_addr().expect("应能读取端口"));
    drop(listener);
    let env = super::super::account_groups::test_bootstrap::lock();
    let mut cleanup = AccountCleanup::new(env.state.store());
    let member = cleanup.add_account("transport-member", &endpoint);
    let scope = AccountGroupScope::from_canonical("transport-group", vec![member]);
    let error = expect_gateway_error(
        env.state.upstream().forward(request(scope)).await,
        "连接失败应返回真实上游错误",
    );

    assert_eq!(error.status_code, 502);
    assert!(error.message.contains("上游请求失败"));
    assert_ne!(error.code.as_deref(), Some(GROUP_UNAVAILABLE_CODE));
}

#[tokio::test]
async fn balance_limiter_exhaustion_returns_group_code_without_network_request() {
    let env = super::super::account_groups::test_bootstrap::lock();
    let account_id = unique_id("balance-limiter-member");
    let scope = AccountGroupScope::from_canonical(
        "balance-limiter-group",
        vec![{
            let mut member = account(&account_id, &real_provider());
            member["limiters"] = json!([{
                "type": "balance",
                "action": "skip",
                "threshold": 1,
                "enabled": true,
            }]);
            member
        }],
    );
    usage_records::write_result(
        &account_id,
        &UsageOutcome {
            usage: Some(json!({"available": 0.0})),
            error: None,
            code: None,
        },
    );

    let result = env.state.upstream().forward(request(scope)).await;
    usage_records::prune_orphans();
    usage_records::reload_facts();
    let error = expect_gateway_error(result, "余额不足时应由组池提前拒绝");
    assert_eq!(error.status_code, 503);
    assert_eq!(error.code.as_deref(), Some(GROUP_UNAVAILABLE_CODE));
}

#[tokio::test]
async fn refusal_uses_content_gate_and_is_not_masked_by_group_503() {
    let app = axum::Router::new()
        .route(
            "/v2/models",
            axum::routing::get(|| async {
                (
                    StatusCode::OK,
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    r#"{"data":[{"id":"c05f-refusal-model","object":"model"}]}"#,
                )
            }),
        )
        .route(
            "/v2/chat/completions",
            axum::routing::post(|| async {
                (
                    StatusCode::OK,
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    "data: {\"choices\":[{\"delta\":{\"content\":\"上游拒答\"},\"finish_reason\":\"content_filter\"}]}\n\ndata: [DONE]\n\n",
                )
            }),
        );
    let (endpoint, task) = serve(app).await;
    let env = super::super::account_groups::test_bootstrap::lock();
    let store = env.state.store();
    let mut cleanup = AccountCleanup::new(store);
    let account_name = unique_id("refusal-member");
    let model_app_key = unique_id("refusal-key");
    let model_app_secret = unique_id("refusal-secret");
    let record = store
        .add_officeace_account(
            &json!({
                "baseUrl": endpoint,
                "modelAppKey": model_app_key,
                "modelAppSecret": model_app_secret,
                "limiters": [],
            }),
            Some(&account_name),
            true,
        )
        .expect("OfficeAce fixture 应能写入");
    cleanup.track(&record);
    let account_id = record["id"].as_str().expect("OfficeAce 账号应有 id");
    let record = store
        .officeace_account_record(account_id)
        .expect("OfficeAce 账号应能读出");
    let credential =
        officeace::credentials::from_record(Some(&record)).expect("OfficeAce 凭证应完整");
    let outcome = officeace::models::refresh(&credential, true).await;
    assert!(outcome.refreshed, "模型目录刷新失败：{:?}", outcome.message);
    let scope = AccountGroupScope::from_canonical(
        "refusal-group",
        vec![account(
            account_id,
            record["provider"].as_str().unwrap_or("officeace"),
        )],
    );
    let mut request = request(scope);
    request.body = json!({
        "model": "c05f-refusal-model",
        "messages": [{"role": "user", "content": "refusal"}],
    });
    let error = expect_gateway_error(
        env.state.upstream().forward(request).await,
        "内容拦截应返回明确错误",
    );
    task.abort();
    assert_eq!(error.status_code, 502);
    assert!(error.message.contains("上游拒答"));
    assert!(error.message.contains("content_filter"));
    assert_ne!(error.code.as_deref(), Some(GROUP_UNAVAILABLE_CODE));
}

#[tokio::test]
async fn ordinary_error_switch_budget_is_restored_after_assertion_scope() {
    // Restore all four shared retry fields after changing the config snapshot.
    let env = super::super::account_groups::test_bootstrap::lock();
    let mut cleanup = AccountCleanup::new(env.state.store());
    let original = config::retry_settings();
    assert_eq!(
        original.switch_budget(),
        DEFAULT_RETRY_SWITCH_COUNT as usize
    );
    {
        let _restore = RetryRestore(original);
        assert!(config::set_retry(RetryPatch {
            count: Some(DEFAULT_RETRY_COUNT),
            account_switch_count: Some(0),
            interval_seconds: Some(DEFAULT_RETRY_INTERVAL_SECONDS),
            no_retry_codes: Some(DEFAULT_NO_RETRY_CODES.to_vec()),
        }));
        let (endpoint, task) = json_upstream(
            "/v2/chat/completions",
            StatusCode::PAYMENT_REQUIRED,
            r#"{"message":"sentinel ordinary error","code":4242}"#,
        )
        .await;
        let first = cleanup.add_account("budget-first", &endpoint);
        let second = cleanup.add_account("budget-second", &endpoint);
        let scope = AccountGroupScope::from_canonical("budget-group", vec![first, second]);

        let error = expect_gateway_error(
            env.state.upstream().forward(request(scope)).await,
            "普通错误应以真实上游错误收尾",
        );
        task.abort();
        assert_eq!(error.status_code, 402);
        assert_eq!(error.upstream_code, Some(4242));
        assert!(error.message.contains("sentinel ordinary error"));
    }
    assert_eq!(
        config::retry_settings().switch_budget(),
        DEFAULT_RETRY_SWITCH_COUNT as usize
    );
}
