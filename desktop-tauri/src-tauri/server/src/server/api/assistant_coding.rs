use std::convert::Infallible;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, Request, State};
use axum::http::header;
use axum::middleware::{self, Next};
use axum::response::{sse::{Event, KeepAlive, Sse}, IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;

use crate::server::{access, errors, logging, ServerState};
use crate::server::core::assistant_coding::{client::{self, CodexClient}, config, fork, permissions, preferences, project_directories, projects, sessions, terminal, usage, CodingError, Engine};
use crate::server::http::ok_json;

pub fn routes(state: ServerState) -> Router<ServerState> {
    Router::new()
        .route("/api/assistant/coding/config", get(get_config).put(put_config))
        .route("/api/assistant/coding/status", get(status))
        .route("/api/assistant/coding/models", get(models))
        .route("/api/assistant/coding/usage", get(account_usage))
        .route("/api/assistant/coding/machines", get(machines))
        .route("/api/assistant/coding/machines/{id}/availability", get(availability))
        .route("/api/assistant/coding/projects", get(list_projects).post(create_project))
        .route("/api/assistant/coding/projects/directories", post(project_directories))
        .route("/api/assistant/coding/sessions", get(list).post(spawn))
        .route("/api/assistant/coding/sessions/{id}", get(detail))
        .route("/api/assistant/coding/sessions/{id}/settings", axum::routing::put(settings))
        .route("/api/assistant/coding/sessions/{id}/fork", post(fork_session))
        .route("/api/assistant/coding/sessions/{id}/messages", get(messages).post(send))
        .route("/api/assistant/coding/sessions/{id}/terminal", post(open_terminal))
        .route("/api/assistant/coding/sessions/{id}/actions/{action}", post(action))
        .route("/api/assistant/coding/sessions/{id}/permissions/{request_id}/{decision}", post(permission))
        .route("/api/assistant/coding/events", get(events))
        .layer(middleware::from_fn_with_state(state, require_session))
}

async fn require_session(State(state): State<ServerState>, mut request: Request, next: Next) -> Response {
    if !access::session_valid(request.headers()) { return errors::panel_login_required_response(request.headers()); }
    if request.method() != axum::http::Method::GET && request.headers().get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok()).is_none_or(|v| v.split(';').next().unwrap_or("").trim() != "application/json") {
        return fail(CodingError::new(415, "编程会话操作必须使用 JSON 请求"));
    }
    if !request.uri().path().ends_with("/config") {
        let current = match db(&state).and_then(config::load) { Ok(value) => value, Err(error) => return fail(error) };
        let revision = request.uri().query().and_then(|query| url::form_urlencoded::parse(query.as_bytes()).find(|(key, _)| key == "connection").map(|(_, value)| value.into_owned()));
        if revision.as_deref() != Some(current.revision().as_str()) { return fail(CodingError::new(409, "本机 Codex 设置已变更，请刷新后再操作")); }
        if !request.uri().path().starts_with("/api/assistant/coding/projects") {
            let client = match db(&state) { Ok(db) => match client::connect(db).await { Ok(value) => value, Err(error) => return fail(error) }, Err(error) => return fail(error) };
            if client.config.revision() != current.revision() || !access::session_valid(request.headers()) { return fail(CodingError::new(409, "本机 Codex 或管理员会话已变更，请刷新后再操作")); }
            request.extensions_mut().insert(client);
        }
    }
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let operation = if let Some((prefix, _)) = path.split_once("/permissions/") { format!("{prefix}/permissions/{}", path.rsplit('/').next().unwrap_or("decision")) } else { path };
    let is_action = method != axum::http::Method::GET && !operation.ends_with("/directories");
    if is_action { logging::log("[Codex]", &format!("管理员发起操作：{method} {operation}")); }
    let response = next.run(request).await;
    if is_action { logging::log("[Codex]", &format!("操作响应：{method} {operation} HTTP {}", response.status().as_u16())); }
    response
}

fn fail(error: CodingError) -> Response {
    (axum::http::StatusCode::from_u16(error.status).unwrap_or(axum::http::StatusCode::BAD_GATEWAY), axum::Json(json!({
        "success": false, "error": { "code": error.code, "type": error.code, "message": error.message }
    }))).into_response()
}
fn respond(result: Result<Value, CodingError>) -> Response { match result { Ok(value) => ok_json(value), Err(error) => fail(error) } }
fn db(state: &ServerState) -> Result<&crate::server::db::Db, CodingError> { state.db().ok_or_else(|| CodingError::new(500, "数据库不可用")) }
fn parse<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, CodingError> {
    if body.len() > 320 * 1024 { return Err(CodingError::new(413, "请求内容过大")); }
    serde_json::from_slice(body).map_err(|_| CodingError::new(400, "请求参数格式不正确"))
}
async fn durable(task: impl std::future::Future<Output = Result<Value, CodingError>> + Send + 'static) -> Response {
    respond(tokio::spawn(task).await.unwrap_or_else(|_| Err(CodingError::uncertain("后台操作中断，请核对会话状态，不要重复提交"))))
}

async fn get_config(State(state): State<ServerState>) -> Response { respond(db(&state).and_then(config::load).map(|c| json!({ "config": c.public() }))) }
async fn put_config(State(state): State<ServerState>, body: Bytes) -> Response {
    let input = match parse(&body) { Ok(input) => input, Err(error) => return fail(error) };
    let db = match db(&state) { Ok(db) => db.clone(), Err(error) => return fail(error) };
    durable(async move { client::save_config(&db, input).await.map(|c| json!({ "config": c.public() })) }).await
}
async fn status(Extension(client): Extension<Arc<CodexClient>>) -> Response { respond(sessions::status(&client).await) }
async fn models(Extension(client): Extension<Arc<CodexClient>>) -> Response { respond(preferences::models(&client).await) }
async fn account_usage(Extension(client): Extension<Arc<CodexClient>>) -> Response { respond(usage::account(&client).await) }
async fn settings(Extension(client): Extension<Arc<CodexClient>>, Path(id): Path<String>, body: Bytes) -> Response {
    let input = match parse(&body) { Ok(input) => input, Err(error) => return fail(error) };
    durable(async move { preferences::update(&client, &id, input).await }).await
}
async fn fork_session(Extension(client): Extension<Arc<CodexClient>>, Path(id): Path<String>, body: Bytes) -> Response {
    let input = match parse(&body) { Ok(input) => input, Err(error) => return fail(error) };
    durable(async move { fork::create(&client, &id, input).await }).await
}
async fn machines(Extension(client): Extension<Arc<CodexClient>>) -> Response { ok_json(json!({ "machines": [sessions::machine(&client)] })) }
async fn availability(Extension(_client): Extension<Arc<CodexClient>>, Path(id): Path<String>) -> Response {
    if id != "local" { return fail(CodingError::new(400, "只能使用当前 Agent2API 所在电脑")); }
    ok_json(json!({ "agents": [{ "agent": "codex", "available": true }] }))
}
async fn list_projects(State(state): State<ServerState>) -> Response {
    respond((|| { let db = db(&state)?; let config = config::load(db)?;
        Ok(json!({ "projects": projects::public(&projects::load(db, &config.scope())?) })) })())
}
async fn create_project(State(state): State<ServerState>, body: Bytes) -> Response {
    let input = match parse::<projects::ProjectInput>(&body) { Ok(input) => input, Err(error) => return fail(error) };
    let db = match db(&state) { Ok(db) => db.clone(), Err(error) => return fail(error) };
    durable(async move {
        tokio::task::spawn_blocking(move || { let config = config::load(&db)?; projects::create(&db, &config, input) }).await
            .map_err(|_| CodingError::new(500, "创建项目的后台操作中断"))?
    }).await
}
async fn project_directories(body: Bytes) -> Response {
    let input = match parse::<project_directories::DirectoryInput>(&body) { Ok(input) => input, Err(error) => return fail(error) };
    respond(tokio::task::spawn_blocking(move || project_directories::browse(input)).await
        .unwrap_or_else(|_| Err(CodingError::new(500, "读取文件夹的后台操作中断"))))
}
#[derive(Deserialize)]
struct ListQuery { engine: Engine }
async fn list(Extension(client): Extension<Arc<CodexClient>>, Query(query): Query<ListQuery>) -> Response {
    let _ = query.engine;
    respond(sessions::list(&client).await)
}
async fn spawn(Extension(client): Extension<Arc<CodexClient>>, body: Bytes) -> Response {
    let input = match parse(&body) { Ok(input) => input, Err(error) => return fail(error) };
    durable(async move { sessions::spawn(&client, input).await }).await
}
async fn detail(Extension(client): Extension<Arc<CodexClient>>, Path(id): Path<String>) -> Response { respond(sessions::detail(&client, &id).await.map(|s| json!({ "session": s }))) }
async fn messages(Extension(client): Extension<Arc<CodexClient>>, Path(id): Path<String>) -> Response { respond(sessions::messages(&client, &id).await) }
async fn send(Extension(client): Extension<Arc<CodexClient>>, Path(id): Path<String>, body: Bytes) -> Response {
    let input = match parse(&body) { Ok(input) => input, Err(error) => return fail(error) };
    durable(async move { sessions::send(&client, &id, input).await }).await
}
async fn open_terminal(Extension(client): Extension<Arc<CodexClient>>, Path(id): Path<String>, body: Bytes) -> Response {
    if let Err(error) = parse::<Value>(&body) { return fail(error); }
    durable(async move { terminal::open(client, &id).await }).await
}
async fn action(Extension(client): Extension<Arc<CodexClient>>, Path((id, action)): Path<(String, String)>, body: Bytes) -> Response {
    if let Err(error) = parse::<Value>(&body) { return fail(error); }
    durable(async move { sessions::action(&client, &id, &action).await }).await
}
async fn permission(Extension(client): Extension<Arc<CodexClient>>, Path((id, request_id, decision)): Path<(String, String, String)>, body: Bytes) -> Response {
    if !["approve", "deny"].contains(&decision.as_str()) { return fail(CodingError::new(400, "审批决定无效")); }
    let body: Value = match parse(&body) { Ok(body) => body, Err(error) => return fail(error) };
    durable(async move { sessions::detail(&client, &id).await?; permissions::answer(&client, &id, &request_id, decision == "approve", &body).await }).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventsQuery { session_id: String }
async fn events(Extension(client): Extension<Arc<CodexClient>>, headers: axum::http::HeaderMap, Query(query): Query<EventsQuery>) -> Response {
    if let Err(error) = sessions::detail(&client, &query.session_id).await { return fail(error); }
    let mut source = client.state.events.subscribe();
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(16);
    tokio::spawn(async move {
        let event = Event::default().data(json!({ "type": "connection-changed", "data": { "status": "connected", "resume": "gap" } }).to_string());
        if tx.send(Ok(event)).await.is_err() { return; }
        let mut check = tokio::time::interval(Duration::from_secs(10));
        loop {
            let value = tokio::select! {
                _ = tx.closed() => break,
                _ = check.tick() => {
                    if !client.running.load(Ordering::Acquire) || !access::session_valid(&headers) || !config::load(&client.db).is_ok_and(|c| c.enabled && c == client.config) { break; }
                    json!({ "type": "heartbeat" })
                }
                event = source.recv() => match event {
                    Ok(event) if event.get("sessionId").and_then(Value::as_str) == Some(query.session_id.as_str()) => event,
                    Ok(_) => continue,
                    Err(broadcast::error::RecvError::Lagged(_)) => json!({ "type": "messages-invalidated", "sessionId": query.session_id }),
                    Err(_) => break,
                }
            };
            if tx.send(Ok(Event::default().data(value.to_string()))).await.is_err() { break; }
        }
    });
    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::new().interval(Duration::from_secs(20))).into_response()
}
