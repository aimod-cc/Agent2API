//! CodeArts 首次登记只接受本机壳的真实管理来源；准备不查账号，确认不重新接收材料。

use std::net::SocketAddr;

use axum::{body::Bytes, extract::{ConnectInfo, Query, State}, http::HeaderMap, response::Response};
use serde::Deserialize;
use serde_json::Value;
use zeroize::Zeroize;

use crate::server::{
    access::local_session::{self, Source},
    core::plugin_host::wasm::{codearts_enrollment as enrollment, managed_task::declaration},
    errors::management_error, http::ok_json, ServerState,
};
use enrollment::EnrollmentError;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Prepare { credential_json: String }

impl Drop for Prepare {
    fn drop(&mut self) { self.credential_json.zeroize(); }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirm { intent: String, confirmation: enrollment::Confirmation }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Intent { intent: String }

fn source(headers: &HeaderMap, peer: SocketAddr) -> Result<Source, EnrollmentError> {
    if !peer.ip().is_loopback() { return Err(EnrollmentError::Unavailable); }
    let mut values = headers.get_all("x-panel-token").iter();
    let token = values.next().and_then(|value| value.to_str().ok()).ok_or(EnrollmentError::Unavailable)?;
    if values.next().is_some() || token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(EnrollmentError::Unavailable);
    }
    local_session::capture(token).ok_or(EnrollmentError::Unavailable)
}

fn response(result: Result<Value, EnrollmentError>) -> Response {
    match result { Ok(value) => ok_json(value), Err(error) => management_error(error.status(), error.message()) }
}

fn parse<T: for<'de> Deserialize<'de>>(body: &Bytes, maximum: usize) -> Result<T, EnrollmentError> {
    if body.is_empty() { return Err(EnrollmentError::InvalidInput); }
    declaration::parse_object(body, maximum).map_err(|_| EnrollmentError::InvalidInput)
}

pub(crate) async fn prepare(
    State(state): State<ServerState>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, body: Bytes,
) -> Response {
    let manager = match source(&headers, peer) { Ok(manager) => manager, Err(error) => return response(Err(error)) };
    let input: Prepare = match parse(&body, 1024 * 1024) { Ok(input) => input, Err(error) => return response(Err(error)) };
    response(enrollment::prepare(manager, state.store().clone(), input.credential_json.as_bytes()))
}

pub(crate) async fn confirm(ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, body: Bytes) -> Response {
    let manager = match source(&headers, peer) { Ok(manager) => manager, Err(error) => return response(Err(error)) };
    let input: Confirm = match parse(&body, 4096) { Ok(input) => input, Err(error) => return response(Err(error)) };
    response(enrollment::confirm(manager, &input.intent, input.confirmation))
}

pub(crate) async fn cancel(ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, body: Bytes) -> Response {
    let manager = match source(&headers, peer) { Ok(manager) => manager, Err(error) => return response(Err(error)) };
    let input: Intent = match parse(&body, 4096) { Ok(input) => input, Err(error) => return response(Err(error)) };
    response(enrollment::cancel(manager, &input.intent))
}

pub(crate) async fn status(ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Query(input): Query<Intent>) -> Response {
    let manager = match source(&headers, peer) { Ok(manager) => manager, Err(error) => return response(Err(error)) };
    response(enrollment::status(manager, &input.intent))
}
