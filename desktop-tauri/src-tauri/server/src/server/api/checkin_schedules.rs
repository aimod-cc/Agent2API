//! 定时签到**条目**的路由（签到中心「定时签到」页的数据源）。
//!
//! ```text
//!   GET  /api/checkin-schedules                状态（总开关 + 条目数组 + 提供商清单）
//!   POST /api/checkin-schedules                保存一条（按 id upsert，{id?, label?, enabled?, time?, providers?}）
//!   POST /api/checkin-schedules/{id}/delete    删除一条（DELETE 方法同受理）
//!   POST /api/checkin-schedules/{id}/run       立即执行这一条
//! ```
//!
//! ── 为什么与 /api/auto-checkin 是两组接口 ─────────────────────
//! `autoCheckin.enabled` 是**总开关**，它的读写走老接口（`api::auto_checkin`，
//! Node 版历史契约，OpenAI 风格错误 body）；条目数组是纯新增的配置形状，
//! 走本模块。强并成一套会让老接口的 body 形状分叉 —— 分组是刻意的。
//!
//! ── 路由挂法与错误信封 ──────────────────────────────────────
//! 与 `/api/scheduled-tasks` 同构：三条入口都是 `any(...)`，方法判定收在本文件
//! 的 `dispatch`（拆独立路由会让「已注册路径 + 未注册方法」落到 405 兜底，
//! 这里统一给 404；POST 兼代 DELETE 是因为 CORS 允许方法里有 DELETE 但
//! 浏览器直连的预检对非简单方法敏感，与 tasks 的 PATCH/POST 同一取舍）。
//! 错误一律**管理信封** 400（`{success:false,error}`）：本接口没有需要对齐的
//! Node 契约，管理 API 的 400 保持同一形状（见 scheduled_tasks 的模块头）。

use axum::body::Bytes;
use axum::extract::State;
use axum::http::Method;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use crate::server::core::checkin_schedules;
use crate::server::errors::not_found_response;
use crate::server::http::{ok_json, parse_body, MAX_BODY_SIZE};
use crate::server::logging;
use crate::server::ServerState;

/// `/api/checkin-schedules*` 全部入口的公共处理（任意方法、任意子路径）。
pub async fn entry(State(state): State<ServerState>, request: axum::extract::Request) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let body = match axum::body::to_bytes(request.into_body(), MAX_BODY_SIZE).await {
        Ok(bytes) => bytes,
        Err(error) => return bad_request(format!("请求体读取失败或过大: {error}")),
    };
    dispatch(&state, method, &path, &body).await
}

/// 路径分发（手工切分，与 scheduled_tasks 同一取舍）。
async fn dispatch(state: &ServerState, method: Method, path: &str, body: &Bytes) -> Response {
    let rest = path
        .strip_prefix("/api/checkin-schedules")
        .unwrap_or_default()
        .trim_matches('/');

    if rest.is_empty() {
        if method == Method::GET {
            return ok_json(state.auto_checkin().state());
        }
        if method == Method::POST {
            return save(state, body).await;
        }
        return not_found(method.as_str(), path);
    }

    let mut segments = rest.splitn(2, '/');
    let id = segments.next().unwrap_or_default();
    let action = segments.next();

    match (method.clone(), action) {
        // DELETE 是规范写法；POST 是别名（CORS 预检考虑，同 tasks 的 PATCH/POST）
        (Method::DELETE, None) | (Method::POST, Some("delete")) => remove(state, id),
        (Method::POST, Some("run")) => run(state, id).await,
        _ => not_found(method.as_str(), path),
    }
}

/// 保存一条条目（新增或更新）。成功返回**整份状态**：条目数组里已是后端归一化
/// 之后的值（displayLabel / lastFiredToday / nextRunAt 都现成），界面以响应为准
/// 重绘，不必再跑一趟 GET（与 /api/models/refresh 带清单返回同一个理由）。
async fn save(state: &ServerState, body: &Bytes) -> Response {
    let payload = match parse_body(body) {
        Ok(value) => value,
        Err(error) => return bad_request(error.message),
    };
    if !payload.is_object() {
        return bad_request("请求内容不是有效对象");
    }
    match checkin_schedules::upsert(&payload) {
        Ok(_) => ok_json(state.auto_checkin().state()),
        Err(message) => bad_request(message),
    }
}

/// 删除一条条目。删除合成的默认条目 = 写入空数组（「删光了就是删光了」，
/// 见 checkin_schedules::remove 的说明）。
fn remove(_state: &ServerState, id: &str) -> Response {
    match checkin_schedules::remove(id) {
        Ok(()) => ok_json(json!({ "success": true })),
        Err(message) => bad_request(message),
    }
}

/// 立即执行一条条目（与到点触发同一条 fire 路径，自带全局防重与条目落账）。
///
/// 条目不存在与「正在执行中」分别给 400 —— `run_schedule_now` 对两者都返回
/// None，存在性在路由层先查。响应形状与 /api/auto-checkin/run 一致：
/// `{...summary, state}`（state 在后，界面就地重绘）。
async fn run(state: &ServerState, id: &str) -> Response {
    if !checkin_schedules::list().iter().any(|entry| entry.id == id) {
        return bad_request(format!("定时条目不存在：{id}"));
    }
    let Some(summary) = state.auto_checkin().run_schedule_now(id).await else {
        return bad_request("签到正在执行中，请稍候");
    };
    let merged = match summary {
        Value::Object(map) => {
            let mut merged = map;
            if let Value::Object(state_map) = state.auto_checkin().state() {
                for (key, value) in state_map {
                    merged.insert(key, value);
                }
            }
            Value::Object(merged)
        }
        other => other,
    };
    ok_json(merged)
}

/// 管理信封的 400（`{success:false,error}`），并打一行日志（scheduled_tasks 同款）。
fn bad_request(message: impl Into<String>) -> Response {
    let message = message.into();
    logging::log("[Checkin]", &format!("❌ {message}"));
    (
        axum::http::StatusCode::BAD_REQUEST,
        Json(json!({ "success": false, "error": message })),
    )
        .into_response()
}

/// 未命中任何一条：与全局兜底同文案（`Not found: <METHOD> <path>`）。
fn not_found(method: &str, path: &str) -> Response {
    not_found_response(method, path)
}
