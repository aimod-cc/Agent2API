//! `/api/accounts/usage` 的实现（**薄壳**：查询逻辑已下沉到
//! `core::usage_query`，见那里的模块头）。
//!
//! 本文件只做三件事：解析 `?id=`、调 core 拿结果、把 `TargetError` 转成管理
//! 信封的响应。目标集合解析、跨账号并发、单账号失败收敛、401 的刷新重试全在 core
//! ——「每账号自动查询」的心跳循环走同一份逻辑，而它不认识 axum。
//!
//! ── `?id=` 与批量是两条语义 ─────────────────────────────
//! 不带 `id` = 批量（目标集合是「全部**可用**账号」，供工具栏的「查询余额」）；
//! 带 `id` = 查这一个账号（账号页每一行的「余额」按钮走这条）。
//! **两条都不看启用状态**：禁用只表示「不参与转发」，与其余额能否查无关 ——
//! 按启用状态把批量挡掉，界面只会让那些行永远停在「未查询」，用户只能逐个手点
//! （那正是「自动查询看起来没生效」的来源）。凭证不完整的账号（available:false）
//! 仍在批量目标之外，`skipped` 记的就是它们的数量。
//! 判据落在 `core::usage_query::query_all` 的 `id` 参数上（那一段有完整说明）。
//!
//! ── 额外的一条：最近查询结果的快照 ─────────────────────────
//! `GET /api/accounts/usage/snapshot` 读记录表里各账号的最近结论（含失败行），
//! 供界面在**不点按钮**的情况下跟上自动查询的节奏（每账号到期的写法下，
//! 界面轮询一份快照就能拿到全部账号的最新读数）。

use axum::{body::Bytes, response::Response};
use serde_json::{json, Value};

use crate::server::core::usage_query;
use crate::server::errors::management_error;
use crate::server::http::{ok_json, query_param};
use super::accounts::parse_json_body;
use crate::server::ServerState;

/// GET /api/accounts/usage
///
/// 逐账号并发查询余额 / 积分汇总（`{ results: [{id,name,usage,error,code?}], skipped }`）。
/// 用户手动点「查询余额」走这条；`?id=` 则只查那一个账号（单行「余额」按钮）。
/// 查询行为见 `core::usage_query::query_all`。
pub async fn accounts_usage(state: &ServerState, query: &str) -> Response {
    // 空串与缺失同义（前端 `''` 时不带 id），所以这里 filter 掉空值再往下传
    let id = query_param(query, "id").filter(|value| !value.is_empty());
    match usage_query::query_all(state.store(), id.as_deref()).await {
        Ok(report) => ok_json(report),
        Err(error) => management_error(error.status_code as i32, error.message),
    }
}

/// POST /api/accounts/usage/start：返回任务编号，不等待上游请求结束。
pub fn start_usage(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_json_body(body, "请求内容不是有效 JSON") {
        Ok(value) => value,
        Err(response) => return response,
    };
    if !payload.is_object() || payload.get("id").is_some_and(|value| !value.is_null() && !value.is_string())
        || payload.get("replace").is_some_and(|value| !value.is_boolean()) {
        return management_error(400, "余额查询参数无效");
    }
    let id = payload.get("id").and_then(Value::as_str).filter(|value| !value.is_empty());
    let replace = payload.get("replace").and_then(Value::as_bool).unwrap_or(false);
    match usage_query::start_queries(state.store(), id, replace) {
        Ok(report) => ok_json(report),
        Err(error) => management_error(error.status_code as i32, error.message),
    }
}

pub fn cancel_usage(body: &Bytes) -> Response {
    let payload = match parse_json_body(body, "请求内容不是有效 JSON") {
        Ok(value) => value,
        Err(response) => return response,
    };
    let id = payload.get("id").and_then(Value::as_str).unwrap_or("");
    let request_id = payload.get("requestId").and_then(Value::as_str).unwrap_or("");
    if id.is_empty() || request_id.is_empty() { return management_error(400, "缺少账号或查询任务编号"); }
    ok_json(json!({ "cancelled": usage_query::jobs::cancel(id, request_id) }))
}

/// GET /api/accounts/usage/snapshot：历史读数与当前任务状态分别返回。
pub async fn accounts_usage_snapshot(state: &ServerState) -> Response {
    ok_json(usage_query::snapshot(state.store()))
}
