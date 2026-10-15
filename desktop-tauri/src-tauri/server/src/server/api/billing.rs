//! 积分 / 签到 / 运营活动路由（对照 server.mjs 871-911 行逐条实现）。
//!
//!   GET  /api/usage                     积分简报（queryCreditsSummary）
//!   GET  /api/checkin/status            签到活动状态
//!   POST /api/checkin                   领取每日签到（result.success ? 200 : 409）
//!   POST /api/checkin/claim-and-report  签到 + 回报最新积分
//!   GET  /api/activity/banner           运营 banner
//!   GET  /api/activity/ambassador       大使状态
//!   GET  /api/checkin-keepalive         国际版日活保活的模型链（读）
//!   POST /api/checkin-keepalive         国际版日活保活的模型链（存）
//!
//! ── 响应形态的两个坑（务必别统一）──────────────────────────
//!   ① 成功响应都是管理 API 信封 `{success:true, data}`；
//!   ② `POST /api/checkin` 的**状态码随结果变**：领取成功 200、已领取 409，
//!      且 body 的 `success` 跟着结果走（不是恒 true）。前端账号页据此把
//!      「今天已签到」显示成一条 warn 而不是错误。
//!   ③ 未登录/上游失败时，Node 版这几条都在最外层大 try 里 → 走 errorPayload
//!      的 OpenAI 风格 body（`{error:{message,type,upstream_code?}}`），
//!      **不是**管理 API 的 `{success:false,error}` 信封。所以这里用
//!      `BillingError::to_gateway_error().into_response()`。

use axum::body::Bytes;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

use crate::server::core::billing::{WorkbuddyActivity, BillingError};
use crate::server::core::providers::workbuddy::Region;
use crate::server::errors::GatewayError;
use crate::server::http::ok_json;
use crate::server::logging;
use crate::server::ServerState;

/// 计费错误 → 响应。
///
/// 走 OpenAI 风格 body（理由见模块头部第 ③ 条），状态码保留 BillingError 里的值
/// （504 超时 / 401 登录态过期 / 上游 HTTP 码 / 400 国际版无签到活动）。
fn billing_error(error: BillingError) -> Response {
    logging::log("[Billing]", &format!("❌ {}", error.message));
    error.to_gateway_error().into_response()
}

/// GET /api/usage —— 积分简报
///
/// Node 版是 `billing.queryCreditsSummary({ locale: opts.locale })`，
/// locale 来自 config.json 的 locale（默认 zh-CN）。
pub async fn get_usage(State(state): State<ServerState>) -> Response {
    match state.billing().query_credits_summary_default().await {
        Ok(data) => ok_json(data),
        Err(error) => billing_error(error),
    }
}

/// GET /api/checkin/status —— 签到活动状态（无活动/无数据时为 null）
pub async fn checkin_status(State(state): State<ServerState>) -> Response {
    match state.billing().get_checkin_status(None).await {
        Ok(data) => ok_json(data),
        Err(error) => billing_error(error),
    }
}

/// POST /api/checkin —— 领取每日签到积分
///
/// **状态码语义**（server.mjs 885-889 行）：
///   `sendJson(res, result.success ? 200 : 409, { success: result.success, data: result })`
/// 即「已领取」这次调用会返回 409 + `success:false`，body 里带着上游的
/// msg（如「今日已签到」）—— 前端按 warn 展示，不是错误。
pub async fn claim_checkin(State(state): State<ServerState>) -> Response {
    let result = match state.billing().claim_daily_checkin(None).await {
        Ok(result) => result,
        Err(error) => return billing_error(error),
    };
    let success = result
        .get("success")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !success {
        let detail = result.get("msg").and_then(serde_json::Value::as_str).unwrap_or("");
        logging::log("[Billing]", &format!("签到未领取（{detail}）"));
    }
    // 状态码与 body 的 success 都由结果决定，不能用 ok_json（它恒为 200/true）
    let status = if success {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::CONFLICT
    };
    (status, axum::Json(json!({ "success": success, "data": result }))).into_response()
}

/// POST /api/checkin/claim-and-report —— 签到 + 查余额
pub async fn claim_and_report(State(state): State<ServerState>) -> Response {
    match state.billing().checkin_and_report_default().await {
        Ok(data) => ok_json(data),
        Err(error) => billing_error(error),
    }
}

/// GET /api/activity/banner —— 运营 banner（拉不到时为 null）
pub async fn activity_banner(State(state): State<ServerState>) -> Response {
    let data = state.billing().get_activity_banner(None).await;
    ok_json(data)
}

/// GET /api/activity/ambassador —— 大使状态（拉不到时为 null）
pub async fn activity_ambassador(State(state): State<ServerState>) -> Response {
    let data = state.billing().get_ambassador_status(None).await;
    ok_json(data)
}

/// GET /api/checkin-keepalive —— WorkBuddy 国际版日活保活的模型链（读）
pub async fn get_keepalive() -> Response {
    ok_json(crate::server::core::billing::keepalive::state())
}

/// POST /api/checkin-activity —— WorkBuddy 国际版日活任务的**手动粒度入口**。
///
/// body：`{id, mode}`，`mode ∈ full | claim | keepalive`（缺省 full）。
/// 与单账号签到（/api/accounts/checkin）分开端点的理由：那条入口对国际版恒走
/// 完整组合（定时与批量也走它），粒度细分只属于手动按钮 —— 三颗按钮各打各的
/// 粒度，结果行与签到同形（`{id, name, claim, activity}`），前端 toast 分流共用。
pub async fn run_activity(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match serde_json::from_slice::<Value>(&body) {
        Ok(value) => value,
        Err(error) => {
            return GatewayError::new(format!("请求体不是合法 JSON：{error}")).into_response()
        }
    };
    let id = payload
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if id.is_empty() {
        return GatewayError::with_status(400, "缺少账号 id").into_response();
    }
    let mode = WorkbuddyActivity::parse(payload.get("mode").and_then(Value::as_str));

    // 账号必须存在且属于 WorkBuddy 国际版 —— 别的家的会话打进国际版的活跃
    // 接口只会稳定报错（与 checkin_for 按提供商分派是同一条理由）
    let account = state
        .store()
        .list_accounts()
        .get("accounts")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|account| account.get("id").and_then(Value::as_str) == Some(id.as_str()))
        })
        .cloned();
    let Some(account) = account else {
        return GatewayError::with_status(404, "账号不存在").into_response();
    };
    if account.get("provider").and_then(Value::as_str) != Some(Region::Intl.provider_id()) {
        return GatewayError::with_status(400, "该账号不是 WorkBuddy 国际版账号").into_response();
    }
    // 展示名快照：统一展示名优先（与账号页 / 请求日志同口径，见
    // `store_view::display_name_of`），缺键（旧快照）按旧口径回落 name
    let name = account
        .get("displayName")
        .filter(|value| value.as_str().is_some_and(|text| !text.is_empty()))
        .cloned()
        .or_else(|| account.get("name").cloned())
        .unwrap_or(Value::Null);
    let Some(entry) = state.store().get_session_by_id(&id) else {
        return GatewayError::with_status(400, "没有可用凭证").into_response();
    };
    let activity = state
        .billing()
        .workbuddy_daily_activity(&entry.session, mode)
        .await;
    let row = json!({
        "id": id,
        "name": name,
        "mode": mode.as_str(),
        "claim": activity.get("claim").cloned().unwrap_or(Value::Null),
        "activity": activity.get("activity").cloned().unwrap_or(Value::Null),
    });
    // 签到记录台账：这条路径不走 run_checkin（没有唯一记账点可借），单独补
    // 一条（source=activity）—— 判定在 checkin_records::record_activity
    // （keepalive 只看保活读数，见那里的说明）。写失败在台账侧只打日志，
    // 不影响本响应。
    crate::server::core::checkin_records::record_activity(
        &row,
        account.get("provider").and_then(Value::as_str).unwrap_or(""),
        mode.as_str(),
    );
    ok_json(row)
}

/// POST /api/checkin-keepalive —— 保存保活模型链。
///
/// body：`{ models: string[] }`（前端把输入框按分隔符拆好再传；也收逗号 /
/// 顿号 / 空白分隔的单个字符串，后端统一拆）。空清单 = 恢复缺省链
/// （语义见 `billing::keepalive::set_models`）。返回保存后的完整状态
/// （`{models, defaultModels}`），前端拿它直接覆盖快照里的 keepalive 段。
pub async fn save_keepalive(body: Bytes) -> Response {
    let payload = match serde_json::from_slice::<serde_json::Value>(&body) {
        Ok(value) => value,
        Err(error) => {
            return GatewayError::new(format!("请求体不是合法 JSON：{error}")).into_response()
        }
    };
    // 两种入参都收：数组（规范形态）与字符串（输入框原样）——
    // 字符串按逗号 / 顿号 / 空白拆，省得前端再写一遍分隔逻辑。
    let raw: Vec<String> = match payload.get("models") {
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        Some(serde_json::Value::String(text)) => text
            .split([',', '，', '、', ' ', '\t'])
            .map(str::to_string)
            .collect(),
        _ => return GatewayError::new("models 必须是字符串数组或分隔符字符串").into_response(),
    };
    match crate::server::core::billing::keepalive::set_models(&raw) {
        Ok(_) => ok_json(crate::server::core::billing::keepalive::state()),
        Err(message) => GatewayError::new(message).into_response(),
    }
}
