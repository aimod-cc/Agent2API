use axum::{body::Bytes, extract::State, response::Response};
use serde_json::{json, Value};
use crate::server::{errors::management_error, http::{ok_json, parse_body}, ServerState};
use crate::server::core::plugin_host::wasm::tasks;

pub async fn list() -> Response { ok_json(json!({ "tasks": tasks::snapshot() })) }

pub async fn configure(body: Bytes) -> Response {
    let value = parse_body(&body).unwrap_or(Value::Null);
    let provider = value["provider"].as_str().unwrap_or("");
    let id = value["task"].as_str().unwrap_or("");
    match tasks::configure(provider, id, &value["settings"]) {
        Ok(()) => list().await,
        Err(error) => management_error(400, error),
    }
}

pub async fn run(State(state): State<ServerState>, body: Bytes) -> Response {
    let value = parse_body(&body).unwrap_or(Value::Null);
    let provider = value["provider"].as_str().unwrap_or("");
    let id = value["task"].as_str().unwrap_or("");
    match tasks::run(state.store(), provider, id, true).await {
        Ok(value) => ok_json(value),
        Err(error) => management_error(400, error),
    }
}
