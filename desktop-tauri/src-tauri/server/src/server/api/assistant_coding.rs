use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Extension, Path, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use futures::StreamExt;
use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::server::{access, errors, logging, ServerState};
use crate::server::core::assistant_coding::{client, config, sessions, CodingError, Engine};
use crate::server::http::ok_json;

pub fn routes(state: ServerState) -> Router<ServerState> {
    Router::new()
        .route("/api/assistant/coding/config", get(get_config).put(put_config))
        .route("/api/assistant/coding/status", get(status))
        .route("/api/assistant/coding/machines", get(machines))
        .route("/api/assistant/coding/machines/{id}/availability", get(availability))
        .route("/api/assistant/coding/machines/{id}/directories", post(directories))
        .route("/api/assistant/coding/sessions", get(list).post(spawn))
        .route("/api/assistant/coding/sessions/{id}", get(detail))
        .route("/api/assistant/coding/sessions/{id}/messages", get(messages).post(send))
        .route("/api/assistant/coding/sessions/{id}/actions/{action}", post(action))
        .route("/api/assistant/coding/sessions/{id}/permissions/{request_id}/{decision}", post(permission))
        .route("/api/assistant/coding/events", get(events))
        .layer(middleware::from_fn_with_state(state, require_session))
}

async fn require_session(State(state): State<ServerState>, mut request: Request, next: Next) -> Response {
    if !access::session_valid(request.headers()) {
        return errors::panel_login_required_response(request.headers());
    }
    if request.method() != Method::GET && request.headers().get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok()).is_none_or(|v| v.split(';').next().unwrap_or("").trim() != "application/json") {
        return fail(CodingError::new(415, "编程会话操作必须使用 JSON 请求"));
    }
    if !request.uri().path().ends_with("/config") {
        let current = match db(&state).and_then(config::load) { Ok(value) => value, Err(error) => return fail(error) };
        let revision = request.uri().query().and_then(|query| url::form_urlencoded::parse(query.as_bytes()).find(|(key, _)| key == "connection").map(|(_, value)| value.into_owned()));
        if revision.as_deref() != Some(current.revision().as_str()) {
            return fail(CodingError::new(409, "编程会话连接设置已变更，请刷新后再操作"));
        }
        let client = match hub(&state).await { Ok(value) => value, Err(error) => return fail(error) };
        if client.config.revision() != current.revision() || !access::session_valid(request.headers()) {
            return fail(CodingError::new(409, "编程会话连接或管理员会话已变更，请刷新后再操作"));
        }
        request.extensions_mut().insert(client);
    }
    let method = request.method().clone();
    let operation = request.uri().path().to_string();
    let operation = if let Some((prefix, _)) = operation.split_once("/permissions/") {
        format!("{prefix}/permissions/{}", operation.rsplit('/').next().unwrap_or("decision"))
    } else { operation };
    let is_action = method != Method::GET && !operation.ends_with("/directories");
    if is_action { logging::log("[Coding]", &format!("管理员发起编程会话操作：{method} {operation}")); }
    let response = next.run(request).await;
    if is_action { logging::log("[Coding]", &format!("编程会话操作响应：{method} {operation} HTTP {}", response.status().as_u16())); }
    response
}

fn fail(error: CodingError) -> Response {
    (StatusCode::from_u16(error.status).unwrap_or(StatusCode::BAD_GATEWAY), axum::Json(json!({
        "success": false, "error": { "code": error.code, "message": error.message }
    }))).into_response()
}
fn respond(result: Result<Value, CodingError>) -> Response { match result { Ok(value) => ok_json(value), Err(error) => fail(error) } }
fn db(state: &ServerState) -> Result<&crate::server::db::Db, CodingError> {
    state.db().ok_or_else(|| CodingError::new(500, "数据库不可用"))
}
async fn hub(state: &ServerState) -> Result<Arc<client::HubClient>, CodingError> { client::connect(db(state)?).await }
fn parse<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, CodingError> {
    if body.len() > 320 * 1024 { return Err(CodingError::new(413, "请求内容过大")); }
    serde_json::from_slice(body).map_err(|_| CodingError::new(400, "请求参数格式不正确"))
}

async fn get_config(State(state): State<ServerState>) -> Response {
    respond(db(&state).and_then(config::load).map(|c| json!({ "config": c.public() })))
}
async fn put_config(State(state): State<ServerState>, body: Bytes) -> Response {
    respond((|| { let input = parse(&body)?; let config = config::save(db(&state)?, input)?; Ok(json!({ "config": config.public() })) })())
}
async fn status(Extension(client): Extension<Arc<client::HubClient>>) -> Response {
    respond(async {
        let health = client.health().await?;
        let data = sessions::machines(&client).await?;
        Ok(json!({ "connected": true, "protocolVersion": health["protocolVersion"], "machines": data["machines"] }))
    }.await)
}
async fn machines(Extension(client): Extension<Arc<client::HubClient>>) -> Response {
    respond(sessions::machines(&client).await)
}
async fn availability(Extension(client): Extension<Arc<client::HubClient>>, Path(id): Path<String>) -> Response {
    respond(async {
        sessions::validate_id(&id)?;
        client.request(Method::GET, &["api", "machines", &id, "agent-availability"], &[], None).await
    }.await)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectoryInput { path: String }
async fn directories(Extension(client): Extension<Arc<client::HubClient>>, Path(id): Path<String>, body: Bytes) -> Response {
    respond(async {
        sessions::validate_id(&id)?;
        let input: DirectoryInput = parse(&body)?;
        if input.path.len() > 4096 || input.path.chars().any(char::is_control) { return Err(CodingError::new(400, "目录路径无效")); }
        client.request(Method::POST, &["api", "machines", &id, "list-directory"], &[], Some(&json!({ "path": input.path }))).await
    }.await)
}
#[derive(Deserialize)]
struct ListQuery { engine: Engine }
async fn list(Extension(client): Extension<Arc<client::HubClient>>, Query(query): Query<ListQuery>) -> Response {
    respond(sessions::list(&client, query.engine).await)
}
async fn spawn(Extension(client): Extension<Arc<client::HubClient>>, body: Bytes) -> Response {
    respond(async { sessions::spawn(&client, parse(&body)?).await }.await)
}
async fn detail(Extension(client): Extension<Arc<client::HubClient>>, Path(id): Path<String>) -> Response {
    respond(async { Ok(json!({ "session": sessions::detail(&client, &id).await? })) }.await)
}
async fn messages(Extension(client): Extension<Arc<client::HubClient>>, Path(id): Path<String>, Query(query): Query<BTreeMap<String, String>>) -> Response {
    respond(async {
        sessions::detail(&client, &id).await?;
        let allowed = ["limit", "beforeSeq", "beforeAt", "afterSeq", "afterAt", "untilSeq", "untilAt", "epoch"];
        let mut fields = Vec::new();
        for (key, value) in query {
            if key == "connection" { continue; }
            if !allowed.contains(&key.as_str()) || value.parse::<u64>().is_err() { return Err(CodingError::new(400, "消息分页参数无效")); }
            if key == "limit" && !value.parse::<u64>().is_ok_and(|v| (1..=100).contains(&v)) { return Err(CodingError::new(400, "每次最多读取 100 条消息")); }
            fields.push((key, value));
        }
        client.request(Method::GET, &["api", "sessions", &id, "messages"], &fields, None).await
    }.await)
}
async fn send(Extension(client): Extension<Arc<client::HubClient>>, Path(id): Path<String>, body: Bytes) -> Response {
    respond(async { sessions::send(&client, &id, parse(&body)?).await }.await)
}
async fn action(Extension(client): Extension<Arc<client::HubClient>>, Path((id, action)): Path<(String, String)>, body: Bytes) -> Response {
    respond(async { let _: Value = parse(&body)?; sessions::action(&client, &id, &action).await }.await)
}
async fn permission(Extension(client): Extension<Arc<client::HubClient>>, Path((id, request_id, decision)): Path<(String, String, String)>, body: Bytes) -> Response {
    respond(async {
        if !["approve", "deny"].contains(&decision.as_str()) { return Err(CodingError::new(400, "审批决定无效")); }
        sessions::permission(&client, &id, &request_id, decision == "approve", parse(&body)?).await
    }.await)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EventsQuery { session_id: String, last_event_id: Option<String>, connection: String }
async fn events(State(state): State<ServerState>, Extension(client): Extension<Arc<client::HubClient>>, headers: HeaderMap, Query(query): Query<EventsQuery>) -> Response {
    let result: Result<Response, CodingError> = async {
        if query.connection != client.config.revision() { return Err(CodingError::new(409, "编程会话连接设置已变更，请刷新后再操作")); }
        sessions::detail(&client, &query.session_id).await?;
        let mut fields = vec![("sessionId".into(), query.session_id), ("visibility".into(), "hidden".into())];
        if let Some(id) = query.last_event_id {
            if id.len() > 200 || id.chars().any(char::is_control) { return Err(CodingError::new(400, "事件游标无效")); }
            fields.push(("lastEventId".into(), id));
        }
        let response = client.events(&fields).await?;
        if !response.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("text/event-stream")) {
            return Err(CodingError::new(502, "HAPI 未返回实时事件流"));
        }
        let (tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(16);
        tokio::spawn(async move {
            let mut source = response.bytes_stream();
            let mut check = tokio::time::interval(Duration::from_secs(15));
            loop {
                tokio::select! {
                    _ = tx.closed() => break,
                    _ = check.tick() => {
                        let current = db(&state).and_then(config::load);
                        if !access::session_valid(&headers) || !current.is_ok_and(|c| c.enabled && c.hub_url == client.config.hub_url && c.access_token == client.config.access_token) { break; }
                    }
                    chunk = source.next() => match chunk {
                        Some(Ok(chunk)) => if tx.send(Ok(chunk)).await.is_err() { break; },
                        Some(Err(_)) | None => break,
                    }
                }
            }
        });
        let mut response = Response::new(Body::from_stream(ReceiverStream::new(rx)));
        response.headers_mut().insert(header::CONTENT_TYPE, "text/event-stream; charset=utf-8".parse().unwrap());
        response.headers_mut().insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
        response.headers_mut().insert("x-accel-buffering", "no".parse().unwrap());
        Ok(response)
    }.await;
    result.unwrap_or_else(fail)
}
