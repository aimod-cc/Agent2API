use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::response::Response;

use crate::server::{errors, http::ok_json, request_stats::FlowQuery, ServerState};

pub async fn get_flow(State(state): State<ServerState>, Query(params): Query<HashMap<String, String>>) -> Response {
    let text = |key: &str| params.get(key).map(|value| value.trim().to_string()).unwrap_or_default();
    let minutes = params.get("minutes").and_then(|value| value.parse::<i64>().ok()).unwrap_or(10);
    if ![5, 10, 30, 60].contains(&minutes) { return errors::management_error(400, "时间范围必须为 5、10、30 或 60 分钟"); }
    let query = FlowQuery {
        model: text("model"), minutes, running_only: text("scope") == "running", search: text("search"), session_search: text("sessionSearch"),
        path: text("path"), session: text("session"),
        client: params.get("client").map(|value| if value == "__unknown" { String::new() } else { value.clone() }),
        provider: text("provider"), account: text("account"), activity: text("activity"),
        offset: params.get("offset").and_then(|value| value.parse().ok()).unwrap_or(0),
        limit: params.get("limit").and_then(|value| value.parse::<usize>().ok()).unwrap_or(30).clamp(1, 100),
    };
    if query.model.len() > 1024 || query.search.len() > 1024 || query.session_search.len() > 1024
        || query.path.len() > 4096 || query.session.len() > 128 || query.client.as_ref().is_some_and(|value| value.len() > 256)
        || query.provider.len() > 256 || query.account.len() > 256 {
        return errors::management_error(400, "流向查询条件过长");
    }
    if !["", "window", "running"].contains(&text("scope").as_str()) || !["", "all", "running", "idle"].contains(&query.activity.as_str()) {
        return errors::management_error(400, "未知的请求活动范围");
    }
    let detail = text("detail");
    if !["", "requests", "sessions", "models"].contains(&detail.as_str()) { return errors::management_error(400, "未知的流向详情类型"); }
    let stats = state.request_stats();
    match tokio::task::spawn_blocking(move || stats.routing_flow(&query, (!detail.is_empty()).then_some(detail.as_str()))).await {
        Ok(value) => ok_json(value),
        Err(error) => errors::management_error(500, format!("读取请求流向失败: {error}")),
    }
}
