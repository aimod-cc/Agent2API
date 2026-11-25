use std::net::SocketAddr;

use axum::{body::Bytes, extract::{ConnectInfo, Query, State}, http::HeaderMap, response::Response};
use serde::Deserialize;
use serde_json::Value;

use crate::server::{access::local_session::{self, Source}, core::plugin_host::wasm::desktop_launch::{self, operations::{PrepareSelection, Selection}},
    errors::management_error, http::ok_json, ServerState};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intent { intent: String }
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RetryStart { intent: String, #[serde(default)] restart_confirmed: bool }
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Confirm { intent: String, confirmed: bool, #[serde(default)] restart_confirmed: bool }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configure {
    selection: Selection,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    args: Option<String>,
    revision: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Recovery { backup: String }

fn source(peer: SocketAddr, headers: &HeaderMap) -> Result<Source, String> {
    if !peer.ip().is_loopback() || !crate::server::core::update::desktop_shell() { return Err("本机启动仅允许当前桌面应用操作，网页和手机不可用".into()); }
    let mut values = headers.get_all("x-panel-token").iter();
    let token = values.next().and_then(|value| value.to_str().ok()).ok_or("本机启动需要桌面管理会话")?;
    if values.next().is_some() || token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) { return Err("本机启动管理会话无效".into()); }
    local_session::capture(token).ok_or_else(|| "本机启动需要原桌面管理会话".into())
}

fn response(result: Result<Value, String>) -> Response {
    match result { Ok(value) => ok_json(value), Err(message) => management_error(409, message) }
}

fn parse<T: for<'de> Deserialize<'de>>(body: &Bytes) -> Result<T, String> {
    if body.is_empty() || body.len() > 8192 { return Err("本机启动请求过大或为空".into()); }
    crate::server::core::plugin_host::wasm::managed_task::declaration::parse_object(body, 8192)
        .map_err(|_| "本机启动请求字段无效或存在重复字段".into())
}

pub async fn prepare(State(state): State<ServerState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, body: Bytes) -> Response {
    let source = match source(peer, &headers) { Ok(source) => source, Err(message) => return management_error(403, message) };
    let selection: PrepareSelection = match parse(&body) { Ok(selection) => selection, Err(error) => return response(Err(error)) };
    let Some(db) = state.db().cloned() else { return response(Err("账号数据库不可用".into())); };
    response(desktop_launch::prepare(source, state.store().clone(), db, selection).await)
}

pub async fn confirm(ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, body: Bytes) -> Response {
    let source = match source(peer, &headers) { Ok(source) => source, Err(message) => return management_error(403, message) };
    let input: Confirm = match parse(&body) { Ok(input) => input, Err(error) => return response(Err(error)) };
    if !input.confirmed { return response(Err("请确认本次切换凭证并启动客户端".into())); }
    response(desktop_launch::confirm(source, &input.intent, input.restart_confirmed))
}

pub async fn status(ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Query(input): Query<Intent>) -> Response {
    let source = match source(peer, &headers) { Ok(source) => source, Err(message) => return management_error(403, message) };
    response(desktop_launch::status(source, &input.intent))
}

pub async fn cancel(ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, body: Bytes) -> Response {
    let source = match source(peer, &headers) { Ok(source) => source, Err(message) => return management_error(403, message) };
    let input: Intent = match parse(&body) { Ok(input) => input, Err(error) => return response(Err(error)) };
    response(desktop_launch::cancel(source, &input.intent))
}

pub async fn retry_start(ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, body: Bytes) -> Response {
    let source = match source(peer, &headers) { Ok(source) => source, Err(message) => return management_error(403, message) };
    let input: RetryStart = match parse(&body) { Ok(input) => input, Err(error) => return response(Err(error)) };
    response(desktop_launch::retry_start(source, &input.intent, input.restart_confirmed).await)
}

pub async fn restore(ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, body: Bytes) -> Response {
    let source = match source(peer, &headers) { Ok(source) => source, Err(message) => return management_error(403, message) };
    let input: Confirm = match parse(&body) { Ok(input) => input, Err(error) => return response(Err(error)) };
    response(desktop_launch::restore(source, &input.intent, input.confirmed).await)
}

pub async fn pending(ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap) -> Response {
    let source = match source(peer, &headers) { Ok(source) => source, Err(message) => return management_error(403, message) };
    response(desktop_launch::recovery::pending(&source).await)
}

pub async fn recovery_prepare(State(state): State<ServerState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, body: Bytes) -> Response {
    let source = match source(peer, &headers) { Ok(source) => source, Err(message) => return management_error(403, message) };
    let input: Recovery = match parse(&body) { Ok(input) => input, Err(error) => return response(Err(error)) };
    let Some(db) = state.db().cloned() else { return response(Err("账号数据库不可用".into())); };
    response(desktop_launch::recovery::prepare(source, db, input.backup).await)
}

pub async fn settings(State(state): State<ServerState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, body: Bytes) -> Response {
    let source = match source(peer, &headers) { Ok(source) => source, Err(message) => return management_error(403, message) };
    let selection = match parse(&body) { Ok(selection) => selection, Err(error) => return response(Err(error)) };
    let Some(db) = state.db().cloned() else { return response(Err("本机设置数据库不可用".into())); };
    response(desktop_launch::settings(source, state.store().clone(), db, selection).await)
}

pub async fn configure(State(state): State<ServerState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, body: Bytes) -> Response {
    let source = match source(peer, &headers) { Ok(source) => source, Err(message) => return management_error(403, message) };
    let input: Configure = match parse(&body) { Ok(input) => input, Err(error) => return response(Err(error)) };
    let Some(db) = state.db().cloned() else { return response(Err("本机设置数据库不可用".into())); };
    response(desktop_launch::configure(source, state.store().clone(), db, input.selection, input.path, input.args, &input.revision).await)
}
