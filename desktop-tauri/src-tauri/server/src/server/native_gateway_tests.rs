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
            login: LoginService::new(store.clone()),
            upstream: UpstreamService::new(store.clone(), auth.clone()),
            auto_checkin: AutoCheckin::new(store.clone(), billing.clone()),
            update: UpdateManager::new(directory.path().join("updates")),
            store,
            auth,
            billing,
            request_stats: Arc::new(RequestStats::with_db(Some(db.clone()), Retention::default)),
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
