//! 成长任务每日排程的管理端点（签到页 / 设置页用）。
//!
//! ```text
//!   GET  /api/growth-task-schedule        开关、时刻数组、今日已执行时刻、上一轮结果
//!   POST /api/growth-task-schedule        改配置 { enabled?, times? }
//!   POST /api/growth-task-schedule/run    立即巡检一轮（真打上游）
//! ```
//!
//! ── 为什么与猫猫旅行分一组接口 ───────────────────────────────
//! 成长任务与猫猫旅行服务的是**不同的玩法**（前者点亮任务领积分，后者推进
//! 猫猫状态机），参考项目也是独立开关与时刻。合成一组会让「关掉旅行」变成
//! 「关掉成长任务」的副作用。形状与 `/api/travel-schedule` 逐字相同（都走
//! 管理信封），前端能按同一个套路处理。
//!
//! ── 错误信封 ────────────────────────────────────────────────
//! 与 `/api/scheduled-tasks`、`/api/travel-schedule` 同一口径：参数校验失败走
//! 管理信封 400 `{success:false,error:"…"}`。

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use crate::server::core::growth_task_scheduler;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

/// `GET /api/growth-task-schedule` —— 当前配置与运行状态。
pub async fn get_state() -> Response {
    ok_json(growth_task_scheduler::state())
}

/// `POST /api/growth-task-schedule` —— 改开关与时刻（只改传进来的字段）。
pub async fn configure(body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return bad_request(error.message),
    };
    let has_field = payload
        .as_object()
        .map(|object| object.contains_key("enabled") || object.contains_key("times"))
        .unwrap_or(false);
    if !has_field {
        return bad_request("没有需要更新的字段（enabled / times）");
    }
    match growth_task_scheduler::configure(&payload) {
        Ok(state) => {
            let enabled = state.get("enabled").and_then(Value::as_bool).unwrap_or(false);
            let times = state
                .get("times")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" / ")
                })
                .unwrap_or_default();
            logging::log(
                "[GrowthTask]",
                &format!(
                    "成长任务排程已{}，时刻 {times}",
                    if enabled { "开启" } else { "关闭" }
                ),
            );
            ok_json(state)
        }
        Err(message) => bad_request(message),
    }
}

/// `POST /api/growth-task-schedule/run` —— 立即巡检一轮。
pub async fn run_now(State(state): State<ServerState>) -> Response {
    let store = state.store().clone();
    match growth_task_scheduler::run_now(&store).await {
        Some(summary) => ok_json(json!({
            "summary": summary,
            "schedule": growth_task_scheduler::state(),
        })),
        None => bad_request("成长任务巡检正在执行中，请稍候"),
    }
}

/// 管理信封的 400（`{success:false,error}`），并打一行日志。
fn bad_request(message: impl Into<String>) -> Response {
    let message = message.into();
    logging::log("[GrowthTask]", &format!("❌ {message}"));
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "success": false, "error": message })),
    )
        .into_response()
}