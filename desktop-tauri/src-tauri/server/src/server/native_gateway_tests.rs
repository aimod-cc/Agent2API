use super::*;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::Response;
use futures::StreamExt;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::Mutex;
use tower::ServiceExt;

mod cases;

#[derive(Clone)]
struct Reply {
    status: u16,
    content_type: &'static str,
    body: String,
    fragmented: bool,
}

impl Reply {
    fn json(body: Value) -> Self {
        Self {
            status: 200,
            content_type: "application/json",
            body: body.to_string(),
            fragmented: false,
        }
    }
    fn sse(body: String) -> Self {
        Self {
            status: 200,
            content_type: "text/event-stream",
            body,
            fragmented: true,
        }
    }
    fn failure(status: u16, body: Value) -> Self {
        Self {
            status,
            ..Self::json(body)
        }
    }
}

#[derive(Clone)]
struct Captured {
    path: String,
    headers: HeaderMap,
    body: Value,
}

struct Mock {
    url: String,
    replies: Arc<Mutex<VecDeque<Reply>>>,
    captured: Arc<Mutex<Vec<Captured>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Mock {
    async fn new() -> Self {
        let replies = Arc::new(Mutex::new(VecDeque::<Reply>::new()));
        let captured = Arc::new(Mutex::new(Vec::<Captured>::new()));
        let queue = replies.clone();
        let received = captured.clone();
        let app = axum::Router::new().fallback(
            move |uri: axum::http::Uri, headers: HeaderMap, body: Bytes| {
                let queue = queue.clone();
                let received = received.clone();
                async move {
                    received.lock().unwrap().push(Captured {
                        path: uri.path().to_string(),
                        headers,
                        body: serde_json::from_slice(&body).unwrap(),
                    });
                    let reply = queue.lock().unwrap().pop_front().unwrap_or_else(|| {
                        Reply::failure(500, json!({"error":{"message":"unexpected request"}}))
                    });
                    let body = if reply.fragmented {
                        let chunks: Vec<_> = reply
                            .body
                            .as_bytes()
                            .chunks(7)
                            .map(Bytes::copy_from_slice)
                            .collect();
                        Body::from_stream(futures::stream::iter(chunks).then(|chunk| async move {
                            tokio::task::yield_now().await;
                            Ok::<_, std::io::Error>(chunk)
                        }))
                    } else {
                        Body::from(reply.body)
                    };
                    Response::builder()
                        .status(reply.status)
                        .header("content-type", reply.content_type)
                        .header("request-id", "native-upstream-1")
                        .header("retry-after", "2")
                        .header("set-cookie", "must-not-leak=yes")
                        .body(body)
                        .unwrap()
                }
            },
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            url,
            replies,
            captured,
            task,
        }
    }

    fn push(&self, reply: Reply) {
        self.replies.lock().unwrap().push_back(reply);
    }
    fn count(&self) -> usize {
        self.captured.lock().unwrap().len()
    }
    fn last(&self) -> Captured {
        self.captured.lock().unwrap().last().unwrap().clone()
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Harness {
    state: ServerState,
    router: axum::Router,
    directory: tempfile::TempDir,
    config_before: serde_json::Map<String, Value>,
}

impl Harness {
    fn new() -> Self {
        logging::process_started_at();
        let home = crate::paths::config_dir();
        let directory = tempfile::tempdir().unwrap();
        let db = Db::open(&directory.path().join("agent2api.db")).unwrap();
        config::init(Some(db.clone()));
        let store = AccountStore::with_db(Some(db.clone()));
        let auth = AuthService::new(store.clone());
        let billing = BillingService::new(auth.clone());
        let config_before = config::current().raw().clone();
        config::set_sanitize_fingerprints(false);
        config::set_auto_continue(false);
        config::set_routing(config::RoutingPatch {
            strategy: Some(config::RouteStrategy::Order),
            sticky: Some(config::StickyMode::Off),
            failover_backoff: Some(false),
            sink: Some(false),
            ..Default::default()
        });
        let state = ServerState {
            port: 0,
            panel_port: None,
            host: "127.0.0.1".parse().unwrap(),
            local_panel: false,
            config_dir: home,
            ui_dir: None,
            ui_assets: None,
            login: LoginService::new(store.clone()),
            upstream: UpstreamService::new(store.clone(), auth.clone()),
            auto_checkin: AutoCheckin::new(store.clone(), billing.clone()),
            update: UpdateManager::new(directory.path().join("updates")),
            store,
            auth,
            billing,
            request_stats: Arc::new(RequestStats::with_db(Some(db.clone()), Retention::default)),
            advertisements: crate::server::core::advertisements::AdvertisementCache::new(Some(db.clone())),
            panel_events_stop: tokio_util::sync::CancellationToken::new(),
            db: Some(db),
            upgrade_pending: Arc::new(AtomicBool::new(false)),
            startup_maintenance: Arc::new(AtomicBool::new(false)),
        };
        let router = http::gateway_router(state.clone());
        Self {
            state,
            router,
            directory,
            config_before,
        }
    }

    fn provider(
        &self,
        id: &str,
        protocol: &str,
        mock: &Mock,
        models: Value,
        mappings: Value,
    ) -> String {
        let base = if protocol == "anthropic" {
            format!("{}/anthropic", mock.url)
        } else {
            format!("{}/v1", mock.url)
        };
        let mut providers = core::custom_providers::list();
        providers.push(
            json!({"id":id,"name":id,"protocol":protocol,"baseUrl":base,"enabled":true,
            "models":models,"mappings":mappings}),
        );
        config::update_raw_field(
            core::custom_providers::KEY_CUSTOM_PROVIDERS,
            json!(providers),
        );
        self.state
            .store()
            .add_custom_account(
                id,
                &json!({"apiKey":format!("upstream-{id}"),"proxy":{"source":"direct"}}),
                Some(id),
            )
            .unwrap()
            .account["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn key(&self, key: &str, providers: &[&str], models: &[&str]) {
        let mut keys = config::current()
            .raw()
            .get("apiKeys")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        keys.push(json!({"id":key,"name":key,"key":key,"enabled":true,"allowedProviders":providers,"allowedModels":models}));
        config::replace_api_keys(json!(keys));
    }

    async fn call(&self, path: &str, key: &str, body: Value) -> (StatusCode, HeaderMap, Vec<u8>) {
        self.call_headers(path, key, body, &[]).await
    }

    async fn call_headers(
        &self,
        path: &str,
        key: &str,
        body: Value,
        headers: &[(&str, &str)],
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut request = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .header("x-api-key", key)
            .header("user-agent", "native-regression-client");
        for (key, value) in headers {
            request = request.header(*key, *value);
        }
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.router
                .clone()
                .oneshot(request.body(Body::from(body.to_string())).unwrap()),
        )
        .await
        .expect("gateway request timed out")
        .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024),
        )
        .await
        .expect("gateway body timed out")
        .unwrap()
        .to_vec();
        (status, headers, bytes)
    }

    fn rows(&self) -> Vec<Value> {
        self.state
            .request_stats()
            .query_requests(&request_stats::RequestQuery {
                limit: Some(200),
                ..Default::default()
            })["entries"]
            .as_array()
            .unwrap()
            .clone()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        config::update_raw_fields(|raw| *raw = self.config_before.clone());
        config::set_sanitize_fingerprints(true);
        config::set_auto_continue(false);
        let _ = self.directory.path();
    }
}

#[test]
fn native_gateway_regression_matrix() {
    crate::test_support::isolated(concat!(module_path!(), "::native_gateway_regression_matrix"), || {
        crate::test_support::runtime().block_on(async {
            let harness = Harness::new();
            cases::run(&harness).await;
        });
    });
}

async fn request_json(
    router: &axum::Router,
    method: &str,
    uri: &str,
    api_key: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-api-key", api_key)
        .header("user-agent", "smoke-test-client");
    let payload = match body {
        Some(val) => {
            builder = builder.header("content-type", "application/json");
            Body::from(val.to_string())
        }
        None => Body::empty(),
    };
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        router.clone().oneshot(builder.body(payload).unwrap()),
    )
    .await
    .unwrap_or_else(|_| panic!("接口请求超时（疑似死锁或阻塞）: {method} {uri}"))
    .unwrap();
    let status = response.status();
    let bytes = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024),
    )
    .await
    .unwrap_or_else(|_| panic!("读取响应体超时: {method} {uri}"))
    .unwrap();
    let parsed = serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|_| {
        json!({ "_raw": String::from_utf8_lossy(&bytes).to_string() })
    });
    (status, parsed)
}

/// 全链路真启动 + 存量数据后台回填整理 + 管理面板首屏核心接口 + 对话记账 + 数据库防重入死锁综合冒烟测试。
#[test]
fn full_lifecycle_bootstrap_maintenance_and_panel_smoke() {
    crate::test_support::isolated(
        concat!(
            module_path!(),
            "::full_lifecycle_bootstrap_maintenance_and_panel_smoke"
        ),
        || {
            crate::test_support::runtime().block_on(async {
                let config_dir = config::config_dir();
                let db_path = config_dir.join(db::FILE_NAME);
                let now = logging::now_ms();
                let yesterday = now - 86_400_000;

                // 1a. 验证数据库锁底层防重入保护：
                //     测试严格模式下，同一线程在 db.with 内部再次调用 db.with 必须立刻 panic 暴露问题；
                //     生产降级模式下，同一线程重入必须立即返回 None 绝不死锁。
                {
                    let (reentrant_db, _guard) = db::test_temp::TempDb::open("reentrancy-fallback");
                    let fallback_res = db::with_reentrancy_fallback_mode(|| {
                        reentrant_db.with(|_| reentrant_db.with(|_| 42))
                    });
                    assert_eq!(
                        fallback_res,
                        Some(None),
                        "生产降级模式下同一线程重入 Db 锁必须安全返回 None 而不死锁"
                    );
                    let panic_caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        reentrant_db.with(|_| {
                            let _ = reentrant_db.with(|_| ());
                        });
                    }));
                    assert!(
                        panic_caught.is_err(),
                        "测试模式下同一线程嵌套重入 Db 锁必须立即触发 panic 报警"
                    );
                }

                // 1b. 预置模拟老用户升级前的存量数据库（含未完成僵尸请求、无费用历史明细、旧版日报）
                {
                    let seed_db = Db::open(&db_path).expect("预置测试数据库应当成功");

                    seed_db
                        .with_mut(|conn| -> rusqlite::Result<()> {
                            // 预置一条上次崩溃遗留的进行中请求（status = 0）
                            conn.execute(
                                "INSERT INTO requests (id, ts, model, account_id, account_name, status, duration_ms, \
                                 attempts, prompt_tokens, completion_tokens, total_tokens, cache_read_tokens, \
                                 provider, client_model, upstream_model, attempt_details, sensitive_hits, \
                                 client_reasoning, upstream_reasoning, phase, is_test, client, api_key_id, \
                                 api_key_name, api_key_source, client_evidence, cache_creation_tokens, \
                                 cache_creation_1h_tokens, cost_usd) \
                                 VALUES ('req-stale-running', ?1, 'claude-opus-4-6', 'acc-1', '测试账号', 0, 0, \
                                 1, 0, 0, 0, 0, 'workbuddy', 'claude-opus-4-6', 'claude-opus-4-6', '[]', '[]', \
                                 '', '', 'waiting', 0, 'claude-code', 'k-smoke', 'SmokeKey', 'key', '{}', 0, 0, NULL)",
                                rusqlite::params![yesterday - 7_200_000],
                            )?;
                            // 预置两条尚未计算费用的历史完成请求（cost_usd IS NULL），触发启动后台费用回填与日报重算
                            conn.execute(
                                "INSERT INTO requests (id, ts, model, account_id, account_name, status, duration_ms, \
                                 attempts, prompt_tokens, completion_tokens, total_tokens, cache_read_tokens, \
                                 provider, client_model, upstream_model, attempt_details, sensitive_hits, \
                                 client_reasoning, upstream_reasoning, phase, is_test, client, api_key_id, \
                                 api_key_name, api_key_source, client_evidence, cache_creation_tokens, \
                                 cache_creation_1h_tokens, cost_usd) \
                                 VALUES ('req-hist-1', ?1, 'claude-opus-4-6', 'acc-1', '测试账号', 200, 850, \
                                 1, 100000, 20000, 120000, 10000, 'workbuddy', 'claude-opus-4-6', 'claude-opus-4-6', '[]', '[]', \
                                 '', '', '', 0, 'claude-code', 'k-smoke', 'SmokeKey', 'key', '{}', 0, 0, NULL)",
                                rusqlite::params![yesterday],
                            )?;
                            conn.execute(
                                "INSERT INTO requests (id, ts, model, account_id, account_name, status, duration_ms, \
                                 attempts, prompt_tokens, completion_tokens, total_tokens, cache_read_tokens, \
                                 provider, client_model, upstream_model, attempt_details, sensitive_hits, \
                                 client_reasoning, upstream_reasoning, phase, is_test, client, api_key_id, \
                                 api_key_name, api_key_source, client_evidence, cache_creation_tokens, \
                                 cache_creation_1h_tokens, cost_usd) \
                                 VALUES ('req-hist-2', ?1, 'gpt-4o', 'acc-1', '测试账号', 200, 420, \
                                 1, 50000, 10000, 60000, 0, 'workbuddy', 'gpt-4o', 'gpt-4o', '[]', '[]', \
                                 '', '', '', 0, 'cursor', 'k-smoke', 'SmokeKey', 'key', '{}', 0, 0, NULL)",
                                rusqlite::params![now - 60_000],
                            )?;
                            // 预置旧版日报行（source_stats_version = 1），触发启动期的模型维度与来源维度回填
                            for ts_val in [yesterday, now - 60_000] {
                                let date_str = chrono::DateTime::from_timestamp_millis(ts_val)
                                    .unwrap()
                                    .with_timezone(&chrono::Local)
                                    .format("%Y-%m-%d")
                                    .to_string();
                                conn.execute(
                                    "INSERT OR REPLACE INTO request_daily (date, requests, successful, tokens, \
                                     cache_hit_tokens, cache_input_tokens, model_tokens, provider_stats, \
                                     account_stats, client_stats, api_key_stats, source_stats_version, \
                                     cost_usd, unpriced_requests) \
                                     VALUES (?1, 1, 1, 60000, 0, 50000, '[]', '[]', '[]', '[]', '[]', 1, 0.0, 0)",
                                    rusqlite::params![date_str],
                                )?;
                            }
                            Ok(())
                        })
                        .expect("预置存量数据失败")
                        .expect("写入存量数据事务失败");
                }

                // 2. 走真实的 ServerState::bootstrap + start_background_maintenance 启动全流程
                let state = ServerState::bootstrap(0, "127.0.0.1".parse().unwrap())
                    .expect("ServerState::bootstrap 真实启动不应失败");
                access::attach_db(state.db().cloned());

                // 配置一把管理与转发共用的网关 Key 以及一个模拟上游提供商
                let mock_upstream = Mock::new().await;
                let base_url = format!("{}/v1", mock_upstream.url);
                config::update_raw_field(
                    core::custom_providers::KEY_CUSTOM_PROVIDERS,
                    json!([{
                        "id": "custom-smoke",
                        "name": "SmokeProvider",
                        "protocol": "chat_completions",
                        "baseUrl": base_url,
                        "enabled": true,
                        "models": [{"id": "gpt-4o", "enabled": true}],
                        "mappings": []
                    }]),
                );
                state
                    .store()
                    .add_custom_account(
                        "custom-smoke",
                        &json!({"apiKey": "sk-smoke-upstream", "proxy": {"source": "direct"}}),
                        Some("SmokeAccount"),
                    )
                    .expect("添加测试账号应当成功");
                let smoke_key = "sk-smoke-admin-key";
                config::replace_api_keys(json!([{
                    "id": "k-smoke",
                    "name": "SmokeKey",
                    "key": smoke_key,
                    "enabled": true,
                    "allowedProviders": [],
                    "allowedModels": []
                }]));

                let router = http::router(state.clone());

                // 启动后台历史统计整理（remap_model_dimension_once + backfill_source_dimensions + backfill_pricing_once + prune）
                state.start_background_maintenance();

                // 3. 在后台维护运行的同时，立即并发请求管理面板首屏核心接口，验证绝无锁阻塞或超时
                let (
                    health_res,
                    panel_status_res,
                    session_res,
                    config_res,
                    accounts_res,
                    models_res,
                    pricing_res,
                    meta_res,
                    logs_res,
                    storage_res,
                ) = tokio::join!(
                    request_json(&router, "GET", "/health", smoke_key, None),
                    request_json(&router, "GET", "/api/panel/status", smoke_key, None),
                    request_json(&router, "GET", "/api/session", smoke_key, None),
                    request_json(&router, "GET", "/api/config", smoke_key, None),
                    request_json(&router, "GET", "/api/accounts", smoke_key, None),
                    request_json(&router, "GET", "/api/models/manage", smoke_key, None),
                    request_json(&router, "GET", "/api/model-pricing", smoke_key, None),
                    request_json(&router, "GET", "/api/models/metadata", smoke_key, None),
                    request_json(&router, "GET", "/api/logs", smoke_key, None),
                    request_json(&router, "GET", "/api/storage", smoke_key, None),
                );
                assert_eq!(health_res.0, StatusCode::OK, "GET /health 失败: {:?}", health_res.1);
                assert_eq!(panel_status_res.0, StatusCode::OK, "GET /api/panel/status 失败: {:?}", panel_status_res.1);
                assert_eq!(session_res.0, StatusCode::OK, "GET /api/session 失败: {:?}", session_res.1);
                assert_eq!(config_res.0, StatusCode::OK, "GET /api/config 失败: {:?}", config_res.1);
                assert_eq!(accounts_res.0, StatusCode::OK, "GET /api/accounts 失败: {:?}", accounts_res.1);
                assert_eq!(models_res.0, StatusCode::OK, "GET /api/models/manage 失败: {:?}", models_res.1);
                assert_eq!(pricing_res.0, StatusCode::OK, "GET /api/model-pricing 失败: {:?}", pricing_res.1);
                assert_eq!(meta_res.0, StatusCode::OK, "GET /api/models/metadata 失败: {:?}", meta_res.1);
                assert_eq!(logs_res.0, StatusCode::OK, "GET /api/logs 失败: {:?}", logs_res.1);
                assert_eq!(storage_res.0, StatusCode::OK, "GET /api/storage 失败: {:?}", storage_res.1);

                // 4. 等待后台历史统计整理完成（5 秒上限，防止任何潜在卡死）
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                while state.startup_maintenance_running() {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "start_background_maintenance 在 5 秒内未完成，疑似发生阻塞或死锁"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }

                // 5. 校验后台整理结果：
                //    - 僵尸进行中请求已被收尾为 408
                //    - 历史请求已被自动回填美元费用（costUsd > 0）
                //    - 统计报表汇总的 costUsd > 0
                let (req_status, req_page) =
                    request_json(&router, "GET", "/api/stats/requests?limit=20", smoke_key, None).await;
                assert_eq!(req_status, StatusCode::OK);
                let entries = req_page["data"]["entries"]
                    .as_array()
                    .expect("请求列表 entries 应为数组");
                let stale_row = entries
                    .iter()
                    .find(|r| r["id"] == "req-stale-running")
                    .expect("应当能查到被收尾的遗留请求");
                assert_eq!(stale_row["status"], 408, "遗留进行中请求启动后应自动收尾为 408");

                let hist_row = entries
                    .iter()
                    .find(|r| r["id"] == "req-hist-1")
                    .expect("应当能查到预置历史请求 req-hist-1");
                let hist_cost = hist_row["costUsd"].as_f64().unwrap_or(0.0);
                assert!(
                    hist_cost > 0.0,
                    "启动后台回填后历史请求应当算出非零费用，实际: {hist_row}"
                );

                let (sum_status, sum_body) =
                    request_json(&router, "GET", "/api/stats/summary?range=30", smoke_key, None).await;
                assert_eq!(sum_status, StatusCode::OK);
                let total_cost = sum_body["data"]["overview"]["costUsd"]
                    .as_f64()
                    .unwrap_or(0.0);
                assert!(
                    total_cost > 0.0,
                    "报表概览应当汇总出回填后的历史总费用，实际: {sum_body}"
                );

                // 6. 发起一次真实的端到端 /v1/chat/completions 对话转发并验证实时记账与计价
                mock_upstream.push(Reply::sse(format!(
                    "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                    json!({"id":"chat_smoke","object":"chat.completion.chunk","model":"gpt-4o","choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}]}),
                    json!({"id":"chat_smoke","object":"chat.completion.chunk","model":"gpt-4o","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1000,"completion_tokens":500,"total_tokens":1500}})
                )));
                let (chat_status, chat_resp) = request_json(
                    &router,
                    "POST",
                    "/v1/chat/completions",
                    smoke_key,
                    Some(json!({
                        "model": "gpt-4o",
                        "stream": false,
                        "messages": [{"role": "user", "content": "smoke test"}]
                    })),
                )
                .await;
                assert_eq!(chat_status, StatusCode::OK, "POST /v1/chat/completions 失败: {chat_resp}");
                assert_eq!(chat_resp["choices"][0]["message"]["content"], "hello");

                // 7. 正常停机收尾刷盘
                state.request_stats().flush();
            });
        },
    );
}
