//! 网关侧宿主 + `RpcAdapter` 的端到端验证。
//!
//! ── 这个测试验证什么 ──────────────────────────────────────────
//! 前面的 `scripts/verify-plugin-host.mjs` 验证的是**协议本身**（脚本扮演
//! 网关）。这里验证的是**网关侧的实现**：`Host` 能不能真把插件进程拉起来、
//! `RpcAdapter` 能不能把 trait 调用变成 RPC、回调能不能走通。
//!
//! ── 为什么它需要真二进制 ────────────────────────────────────
//! 这是整条链路上唯一无法用纯 Rust 单元测试覆盖的部分：spawn、管道、
//! 分帧、握手、进程退出。用假实现替代就等于没测。
//!
//! ── 运行条件 ────────────────────────────────────────────────
//! 这些联调用例默认标为 ignored，避免缺少插件时被误记为通过。
//! 显式运行时由 AGENT2API_TEST_PLUGIN_EXE 指定含 demo 的插件；路径或握手失败必须报错。

use std::sync::Arc;
use std::time::Duration;

#[path = "test_upstream.rs"]
mod test_upstream;

use super::*;
use crate::server::core::providers::adapter::{ReasoningPatch, UpstreamErrorClass};
use crate::server::core::providers::adapter::ProviderAdapter;
use crate::server::core::upstream::usage::RequestTelemetry;
use crate::server::core::upstream::ForwardOutcome;

fn plugin_exe() -> std::path::PathBuf {
    let path = std::path::PathBuf::from(std::env::var_os("AGENT2API_TEST_PLUGIN_EXE")
        .expect("显式运行插件联调时必须设置 AGENT2API_TEST_PLUGIN_EXE，指向包含 demo 的测试插件"));
    assert!(path.is_file(), "测试插件不存在：{}", path.display());
    path.canonicalize().expect("测试插件路径应可解析")
}

/// 一个最小回调实现：只为验证「插件 → 网关」这条路通。
pub(super) struct TestCallbacks;

impl GatewayCallbacks for TestCallbacks {
    fn account_provider(&self, account: &str) -> Option<String> {
        (!account.is_empty()).then(|| "demo".to_string())
    }

    fn provider_allowed(&self, provider: &str) -> bool {
        provider == "demo"
    }
    fn store_get(
        &self,
        _provider: String,
        account: String,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, proto::WireError>> + Send + '_>,
    > {
        Box::pin(async move {
            if account == "missing" {
                return Err(proto::WireError::with_code("账号不存在", "not_found"));
            }
            Ok(serde_json::json!({
                "id": account,
                "provider": "demo",
                "secret": { "token": "demo-token", "expires_at": i64::MAX },
                "public": { "name": "测试账号" },
            }))
        })
    }

    fn store_list(
        &self,
        _provider: String,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, proto::WireError>> + Send + '_>,
    > {
        Box::pin(async { Ok(serde_json::json!([])) })
    }

    fn store_update(
        &self,
        _account: String,
        _expect: serde_json::Value,
        _secret: serde_json::Value,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, proto::WireError>> + Send + '_>,
    > {
        Box::pin(async { Ok(serde_json::json!({ "updated": true })) })
    }

    fn store_mark_limited(
        &self,
        _account: String,
        _model: String,
        _reset_at: Option<i64>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), proto::WireError>> + Send + '_>>
    {
        Box::pin(async { Ok(()) })
    }

    fn ledger_get(
        &self,
        _account: String,
        _kind: String,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, proto::WireError>> + Send + '_>,
    > {
        Box::pin(async { Ok(serde_json::Value::Null) })
    }

    fn ledger_put(
        &self,
        _account: String,
        _kind: String,
        _value: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), proto::WireError>> + Send + '_>>
    {
        Box::pin(async { Ok(()) })
    }

    fn settings_get(
        &self,
        _provider: String,
        _key: String,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, proto::WireError>> + Send + '_>,
    > {
        Box::pin(async { Ok(serde_json::Value::Null) })
    }

    fn settings_set(
        &self,
        _provider: String,
        _key: String,
        _value: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), proto::WireError>> + Send + '_>>
    {
        Box::pin(async { Ok(()) })
    }

    fn log(&self, _level: String, _message: String) {}

    fn notify(&self, _kind: String, _payload: serde_json::Value) {}
}

/// 拉起宿主并握手。
async fn boot() -> (tempfile::TempDir, Arc<Host>, Vec<proto::ProviderInfo>) {
    let dir = tempfile::tempdir().expect("插件测试应使用独立目录");
    let options = HostOptions {
        init_timeout: Duration::from_secs(20),
        data_dir: dir.path().to_string_lossy().to_string(),
        ..HostOptions::new(plugin_exe())
    };
    let host = Arc::new(Host::new(options, Arc::new(TestCallbacks)));
    let providers = host.ensure().await.expect("插件存在时握手失败不能算测试通过");
    (dir, host, providers)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要 AGENT2API_TEST_PLUGIN_EXE 指向含 demo 的插件二进制"]
async fn host_starts_and_lists_providers() {
    let (_dir, host, providers) = boot().await;
    assert!(!providers.is_empty(), "握手应当返回至少一家 provider");
    let demo = providers
        .iter()
        .find(|p| p.id.0 == "demo")
        .expect("示例 provider 应当在列表里");
    assert!(demo.capabilities.chat, "示例 provider 声明了 chat");
    assert!(demo.capabilities.stateful, "示例 provider 声明了 stateful");
    // 握手带回来的能力位要能被适配器读到
    let adapter = RpcAdapter::new(
        "demo".into(),
        crate::server::core::providers::ProviderKind::WorkBuddy, // 占位 kind，测试不关心
        host.clone(),
        demo.clone(),
    );
    assert!(adapter.supports_chat());
    assert!(adapter.is_stateful());
    assert!(adapter.supports_usage());
    host.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要 AGENT2API_TEST_PLUGIN_EXE 指向含 demo 的插件二进制"]
async fn build_chat_request_goes_through_rpc() {
    let (_dir, host, providers) = boot().await;
    let demo = providers.iter().find(|p| p.id.0 == "demo").expect("demo 存在");
    let adapter = RpcAdapter::new(
        "demo".into(),
        crate::server::core::providers::ProviderKind::WorkBuddy,
        host.clone(),
        demo.clone(),
    );

    let account = serde_json::json!({ "id": "acc-1" });
    let body = serde_json::json!({
        "messages": [{ "role": "user", "content": "你好" }],
        "model": "demo-fast",
    });
    let headers = axum::http::HeaderMap::new();

    let plan = adapter
        .build_chat_request_async(&account, &body, &headers)
        .await
        .expect("插件应当返回构造计划");
    assert!(plan.url.ends_with("/v1/chat/completions"), "URL: {}", plan.url);
    assert_eq!(plan.body["model"], "demo-fast");
    assert!(
        plan.headers.iter().any(|(k, _)| k == "authorization"),
        "应当带上鉴权头：{:?}",
        plan.headers
    );

    // 同步版必须报错（说明调用链还在用旧签名）
    assert!(
        adapter.build_chat_request(&account, &body, &headers).is_err(),
        "插件实现的同步版应当拒绝，而不是返回另一份结果"
    );

    host.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要 AGENT2API_TEST_PLUGIN_EXE 指向含 demo 的插件二进制"]
async fn classify_error_goes_through_rpc() {
    let (_dir, host, providers) = boot().await;
    let demo = providers.iter().find(|p| p.id.0 == "demo").expect("demo 存在");
    let adapter = RpcAdapter::new(
        "demo".into(),
        crate::server::core::providers::ProviderKind::WorkBuddy,
        host.clone(),
        demo.clone(),
    );

    let quota = adapter
        .classify_error_async(429, &serde_json::json!({ "error": { "message": "限额" } }))
        .await;
    assert!(
        matches!(quota, UpstreamErrorClass::QuotaLimited { .. }),
        "429 应当归为 QuotaLimited，得到 {quota:?}"
    );

    let expired = adapter
        .classify_error_async(401, &serde_json::json!({ "error": { "message": "过期" } }))
        .await;
    assert!(
        matches!(expired, UpstreamErrorClass::TokenExpired { .. }),
        "401 应当归为 TokenExpired，得到 {expired:?}"
    );

    host.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要 AGENT2API_TEST_PLUGIN_EXE 指向含 demo 的插件二进制"]
async fn reasoning_patch_goes_through_rpc() {
    let (_dir, host, providers) = boot().await;
    let demo = providers.iter().find(|p| p.id.0 == "demo").expect("demo 存在");
    let adapter = RpcAdapter::new(
        "demo".into(),
        crate::server::core::providers::ProviderKind::WorkBuddy,
        host.clone(),
        demo.clone(),
    );

    let patch = adapter
        .reasoning_patch_async("high", "demo-fast", &serde_json::json!({}))
        .await;
    match patch {
        ReasoningPatch::Set { field, value } => {
            assert_eq!(field, "reasoning_effort");
            assert_eq!(value, serde_json::json!("high"));
        }
        other => panic!("期望 Set，得到 {other:?}"),
    }

    // 表外档位：插件应当拒绝并给出原因（不是静默注入）
    let skip = adapter
        .reasoning_patch_async("ultra", "demo-fast", &serde_json::json!({}))
        .await;
    match skip {
        ReasoningPatch::Skip { reason } => {
            assert!(reason.contains("ultra"), "原因里应当提到收到的值：{reason}");
        }
        other => panic!("期望 Skip，得到 {other:?}"),
    }

    host.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要 AGENT2API_TEST_PLUGIN_EXE 指向含 demo 的插件二进制"]
async fn refusal_markers_come_from_handshake() {
    let (_dir, host, providers) = boot().await;
    let demo = providers.iter().find(|p| p.id.0 == "demo").expect("demo 存在");
    // 握手里的标记（协议层）
    assert_eq!(demo.refusal_finishes, vec!["content_policy_violation".to_string()]);
    assert_eq!(demo.refusal_markers, vec!["insufficient_quota".to_string()]);

    // 适配器侧应当把它们作为 &'static 切片暴露出来（trait 的签名要求）
    let adapter = RpcAdapter::new(
        "demo".into(),
        crate::server::core::providers::ProviderKind::WorkBuddy,
        host.clone(),
        demo.clone(),
    );
    assert_eq!(adapter.stream_refusal_finishes(), &["content_policy_violation"]);
    assert_eq!(adapter.stream_refusal_markers(), &["insufficient_quota"]);

    host.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要 AGENT2API_TEST_PLUGIN_EXE 指向含 demo 的插件二进制"]
async fn ensure_token_reads_the_account_through_callbacks() {
    let (_dir, host, providers) = boot().await;
    let demo = providers.iter().find(|p| p.id.0 == "demo").expect("demo 存在");
    let adapter = RpcAdapter::new(
        "demo".into(),
        crate::server::core::providers::ProviderKind::WorkBuddy,
        host.clone(),
        demo.clone(),
    );

    // 这里要一个 AccountStore 才能调 ensure_access_token —— 它是 trait 的
    // 签名要求（内置家从 store 取凭证）。插件实现忽略它（凭证在插件那边）。
    // 用临时库避免碰真实数据。
    let (db, _guard) = crate::server::db::test_temp::TempDb::open("plugin-host-token");
    let store = crate::server::core::account_store::AccountStore::with_db(Some(db));

    let token = adapter
        .ensure_access_token(&store, "acc-1")
        .await
        .expect("插件应当从回调读到账号并给出 token");
    assert_eq!(token, "demo-token");

    host.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要 AGENT2API_TEST_PLUGIN_EXE 指向含 demo 的插件二进制"]
async fn missing_account_reports_not_found() {
    let (_dir, host, providers) = boot().await;
    let demo = providers.iter().find(|p| p.id.0 == "demo").expect("demo 存在");
    let adapter = RpcAdapter::new(
        "demo".into(),
        crate::server::core::providers::ProviderKind::WorkBuddy,
        host.clone(),
        demo.clone(),
    );
    let (db, _guard) = crate::server::db::test_temp::TempDb::open("plugin-host-missing");
    let store = crate::server::core::account_store::AccountStore::with_db(Some(db));

    let result = adapter.ensure_access_token(&store, "missing").await;
    assert!(result.is_err(), "账号不存在时应当报错而不是返回空 token");

    host.shutdown().await;
}

// ── 流式帧（head / chunk / credit / abort）──────────────────────

/// 一个只接受本机请求的假上游。
///
/// ── 为什么必须在测试里起一个真 HTTP 服务 ──────────────────────
/// `demo` 的 `conversation` 是真的发请求（那正是要验的路径：插件自己出网、
/// 自己推帧）。把它换成假实现就等于只测了网关自己的循环。
struct FakeUpstream(tokio::task::JoinHandle<()>);

impl Drop for FakeUpstream {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn fake_upstream(payload: &'static str) -> (String, FakeUpstream) {
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move |_body: axum::body::Bytes| async move {
            ([("content-type", "text/event-stream")], payload)
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, FakeUpstream(task))
}

/// 建一个 demo 适配器。
///
/// ── 假上游地址怎么传进去 ────────────────────────────────────
/// 走 `x-demo-upstream` 请求头，不走环境变量：插件的 env 在 spawn 时固定，
/// 而一个测试进程里要起好几个假上游（各占一个随机端口），环境变量只能有
/// 一个值。
async fn demo_adapter() -> (tempfile::TempDir, Arc<Host>, RpcAdapter, Arc<RequestTelemetry>) {
    let (dir, host, providers) = boot().await;
    let demo = providers.iter().find(|p| p.id.0 == "demo").expect("测试插件必须提供 demo").clone();
    let adapter = RpcAdapter::new(
        "demo".into(),
        crate::server::core::providers::ProviderKind::WorkBuddy,
        host.clone(),
        demo,
    );
    let telemetry = Arc::new(RequestTelemetry::new());
    (dir, host, adapter, telemetry)
}

/// 带 `x-demo-upstream` 的入站头。
fn upstream_headers(upstream: &str) -> axum::http::HeaderMap {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "x-demo-upstream",
        axum::http::HeaderValue::from_str(upstream).expect("假上游地址应当是合法头值"),
    );
    headers
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要 AGENT2API_TEST_PLUGIN_EXE 指向含 demo 的插件二进制"]
async fn conversation_streams_head_and_chunks() {
    let (upstream, _server) = fake_upstream(
        "data: {\"choices\":[{\"delta\":{\"content\":\"你\"}}]}\n\ndata: [DONE]\n\n",
    )
    .await;
    let (_dir, host, adapter, telemetry) = demo_adapter().await;
    let (db, _guard) = crate::server::db::test_temp::TempDb::open("plugin-host-conv-stream");
    let store = crate::server::core::account_store::AccountStore::with_db(Some(db));
    let body = serde_json::json!({ "model": "demo-fast", "messages": [] });

    let outcome = adapter
        .forward_conversation(
            &store,
            "acc-1",
            &body,
            &upstream_headers(&upstream),
            None,
            true,
            &telemetry,
        )
        .await
        .expect("流式转发应当成功");
    let ForwardOutcome::Stream { status, stream } = outcome else {
        panic!("stream:true 应当返回 ForwardOutcome::Stream");
    };
    assert_eq!(status, 200, "head 里的状态码要透传给客户端");

    use futures::StreamExt;
    let mut stream = stream;
    let mut text = String::new();
    // 上限 8 帧：假上游只推两帧，多出来的就是网关合成的收尾（也正常），
    // 但真挂住的话这个循环会等到超时而不是永远转。
    for _ in 0..8 {
        match tokio::time::timeout(Duration::from_secs(10), stream.next()).await {
            Ok(Some(Ok(bytes))) => {
                text.push_str(&String::from_utf8_lossy(&bytes));
                if text.contains("[DONE]") {
                    break;
                }
            }
            Ok(Some(Err(error))) => panic!("流不应报错: {error}"),
            Ok(None) => break,
            Err(_) => panic!("等下一帧超时（流卡住了）"),
        }
    }
    assert!(text.contains("\"content\":\"你\""), "应当收到插件推来的正文帧：{text}");
    assert!(text.contains("[DONE]"), "流末尾要有终止帧：{text}");

    host.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要 AGENT2API_TEST_PLUGIN_EXE 指向含 demo 的插件二进制"]
async fn conversation_non_stream_aggregates_on_the_gateway() {
    let (upstream, _server) = fake_upstream(
        "data: {\"choices\":[{\"delta\":{\"content\":\"你\"},\"index\":0}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"好\"},\"index\":0}]}\n\ndata: [DONE]\n\n",
    )
    .await;
    let (_dir, host, adapter, telemetry) = demo_adapter().await;
    let (db, _guard) = crate::server::db::test_temp::TempDb::open("plugin-host-conv-aggregate");
    let store = crate::server::core::account_store::AccountStore::with_db(Some(db));
    let body = serde_json::json!({ "model": "demo-fast", "messages": [] });

    let outcome = adapter
        .forward_conversation(
            &store,
            "acc-1",
            &body,
            &upstream_headers(&upstream),
            None,
            false,
            &telemetry,
        )
        .await
        .expect("非流式转发应当成功");
    let ForwardOutcome::Completion { body } = outcome else {
        panic!("stream:false 应当返回 ForwardOutcome::Completion");
    };
    let content = body
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    assert_eq!(content, "你好", "网关要把插件推来的帧聚合成完整正文：{body}");

    host.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要 AGENT2API_TEST_PLUGIN_EXE 指向含 demo 的插件二进制"]
async fn conversation_http_200_body_is_forwarded_without_hanging() {
    let (upstream, _server) = fake_upstream(
        "{\"error\":{\"message\":\"rate limited\",\"code\":429}}",
    )
    .await;
    let (_dir, host, adapter, telemetry) = demo_adapter().await;
    let (db, _guard) = crate::server::db::test_temp::TempDb::open("plugin-host-conv-error");
    let store = crate::server::core::account_store::AccountStore::with_db(Some(db));
    let body = serde_json::json!({ "model": "demo-fast", "messages": [] });

    // 假上游固定回 200，所以这条走的是「正常状态 + 正文里是错误」——
    // demo 的 conversation 不解析正文，于是流里就是那段 JSON。
    // 真正要钉的是「不 panic、不挂住、能收尾」。
    let outcome = adapter
        .forward_conversation(
            &store,
            "acc-1",
            &body,
            &upstream_headers(&upstream),
            None,
            true,
            &telemetry,
        )
        .await
        .expect("200 状态不该在这一层报错");
    let ForwardOutcome::Stream { stream, .. } = outcome else {
        panic!("stream:true 应当返回流");
    };
    use futures::StreamExt;
    let mut stream = stream;
    let collected = tokio::time::timeout(Duration::from_secs(10), async {
        let mut text = String::new();
        while let Some(Ok(bytes)) = stream.next().await {
            text.push_str(&String::from_utf8_lossy(&bytes));
            if text.contains("[DONE]") {
                break;
            }
        }
        text
    })
    .await
    .expect("收流不应超时");
    assert!(collected.contains("rate limited"), "正文应当透传：{collected}");

    host.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "需要 AGENT2API_TEST_PLUGIN_EXE 指向含 demo 的插件二进制"]
async fn dropping_the_stream_aborts_the_plugin_call() {
    use futures::StreamExt;

    let mut upstream = test_upstream::PendingUpstream::start().await;
    let (_dir, host, adapter, telemetry) = demo_adapter().await;
    let (db, _guard) = crate::server::db::test_temp::TempDb::open("plugin-host-conv-drop");
    let store = crate::server::core::account_store::AccountStore::with_db(Some(db));
    let body = serde_json::json!({ "model": "demo-fast", "messages": [] });

    let outcome = adapter
        .forward_conversation(
            &store,
            "acc-1",
            &body,
            &upstream_headers(&upstream.url),
            None,
            true,
            &telemetry,
        )
        .await
        .expect("流式转发应当成功");
    let ForwardOutcome::Stream { mut stream, .. } = outcome else {
        panic!("应当返回未结束的流");
    };
    let first = tokio::time::timeout(Duration::from_secs(10), async {
        let mut text = String::new();
        while !text.contains("started") {
            let chunk = stream.next().await.expect("取消之前上游不能结束").unwrap();
            text.push_str(&String::from_utf8_lossy(&chunk));
        }
        text
    }).await.expect("应当收到首帧");
    assert!(!first.contains("[DONE]"));
    assert!(matches!(upstream.disconnected.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
    drop(stream);
    tokio::time::timeout(Duration::from_secs(10), &mut upstream.disconnected)
        .await.expect("取消后插件必须关闭仍在等待的上游连接")
        .expect("模拟上游应当报告连接释放");
    let still_alive = adapter
        .ensure_access_token(&store, "acc-1")
        .await
        .is_ok();
    assert!(still_alive, "丢弃流之后宿主应当仍然可用");

    host.shutdown().await;
}
