use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::response::Response;

use crate::server::core::billing::keepalive;
use crate::server::http::ok_json;
use crate::server::{errors, ServerState};

use super::date_range::date_bounds;

pub async fn get_trace(State(state): State<ServerState>, Query(params): Query<HashMap<String, String>>) -> Response {
    let text = |key: &str| params.get(key).map(|value| value.trim()).filter(|value| !value.is_empty()).map(str::to_string);
    let model = text("model");
    let search = text("search").unwrap_or_default();
    let status = text("status").unwrap_or_default();
    let offset = text("offset").and_then(|value| value.parse::<usize>().ok()).unwrap_or(0);
    let limit = text("limit").and_then(|value| value.parse::<usize>().ok()).unwrap_or(30).clamp(1, 100);
    let bounds = match date_bounds(&params) {
        Ok(bounds) => bounds,
        Err(error) => return errors::management_error(400, error),
    };
    let stats = state.request_stats();
    match tokio::task::spawn_blocking(move || stats.routing_trace(model.as_deref(), offset, limit, &status, &search, bounds)).await {
        Ok(mut value) => {
            keepalive::normalize_request_display(state.store(), &mut value);
            ok_json(value)
        }
        Err(error) => errors::management_error(500, format!("读取请求跟踪失败: {error}")),
    }
}

pub async fn get_trace_request(State(state): State<ServerState>, Query(params): Query<HashMap<String, String>>) -> Response {
    let id = params.get("id").map(|value| value.trim()).filter(|value| !value.is_empty()).unwrap_or("").to_string();
    let ts = params.get("ts").and_then(|value| value.parse::<i64>().ok());
    if id.is_empty() || id.len() > 256 { return errors::management_error(400, "请求 ID 不能为空且不能超过 256 字符") }
    let stats = state.request_stats();
    match tokio::task::spawn_blocking(move || stats.routing_trace_request(&id, ts)).await {
        Ok(Some(mut value)) => {
            keepalive::normalize_request_display(state.store(), &mut value);
            ok_json(value)
        }
        Ok(None) => errors::management_error(404, "请求记录已删除或超过留存范围"),
        Err(error) => errors::management_error(500, format!("读取请求详情失败: {error}")),
    }
}
