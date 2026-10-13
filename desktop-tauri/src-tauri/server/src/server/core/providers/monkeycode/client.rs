//! MonkeyCode 出站请求的薄封装（Cookie 认证，统一超时与错误翻译）。
//!
//! ── 为什么收在一处 ──────────────────────────────────────────
//! 三处消费者（`login` 校验 / `models` 目录刷新）共用同一套
//! 超时、错误翻译与「业务码 → 人话」口径；各写一份会让「哪个码算登录失效」
//! 这类判据分叉。
//!
//! ── 业务码口径（参考 `mvp/auth.py` / `mvp/models.py` + `docs/05-api`）──
//! 上游是 HTTP 状态码 + `{code, msg, data}` 双轨：
//!   - 成功：`code == 0`（`{"code":0,"msg":"success","data":{…}}`）；
//!   - 未授权：HTTP 401 / 403，或 body 里 `code` 为 401 / 403
//!     （`{"code":401,"message":"未授权 [trace_id:…]"}`）；
//!   - 其余：业务失败，`message` / `msg` 是上游文案。
//! 本模块只判「HTTP 层失败」与「登录态失效」，业务码的**语义**由调用方各自
//! 翻译（登录校验与目录刷新要的处置不同）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;

use crate::server::core::auth_http::send_raw;
use crate::server::errors::GatewayError;

use super::endpoints;
use super::region::Region;

/// 请求超时（目录 / 校验都是短请求，20 秒足够；上游挂住时不要拖住网关）
const REQUEST_TIMEOUT_MS: u64 = 20_000;

/// 成功业务码（整数 `0`）
pub const SUCCESS_CODE: i64 = 0;

/// 读 body 里的业务码（整数；缺失时返回 None，调用方按「缺失即未知」处置）
pub fn business_code(payload: &Value) -> Option<i64> {
    payload.get("code").and_then(Value::as_i64)
}

/// 上游业务文案（`message` → `msg` 兜底）
pub fn upstream_message(payload: &Value) -> String {
    payload
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| payload.get("msg").and_then(Value::as_str))
        .map(str::trim)
        .unwrap_or("")
        .to_string()
}

/// 这个 HTTP 状态 / 业务码是不是「登录态失效」
pub fn is_auth_failure(status: u16, payload: &Value) -> bool {
    if status == 401 || status == 403 {
        return true;
    }
    matches!(business_code(payload), Some(401) | Some(403))
}

/// 发一次带 cookie 的请求（`method` 目前只有 GET；任务创建的专用请求在 `task.rs`）。
///
/// 返回上游响应体。HTTP 层失败 / 超时翻译成网关错误；登录态失效统一翻成
/// `401`（文案指向「重新粘贴 session」）。业务码**不在这里判** —— 调用方各有
/// 各的处置。
pub async fn request(
    method: &str,
    region: Region,
    path: &str,
    session: &str,
    body: Option<&Value>,
    what: &str,
) -> Result<Value, GatewayError> {
    if session.trim().is_empty() {
        return Err(GatewayError::with_status(
            401,
            "MonkeyCode 账号缺少 session，请重新粘贴登录态",
        ));
    }
    let url = format!("{}{path}", region.base_url());
    let headers = endpoints::authed_headers(region, session);
    let response = send_raw(method, &url, body, &headers, None, Some(REQUEST_TIMEOUT_MS))
        .await
        .map_err(|error| {
            if error.is_timeout() {
                GatewayError::with_status(504, format!("{what}超时，请稍后重试"))
            } else {
                GatewayError::with_status(502, format!("{what}失败：{error}"))
            }
        })?;
    let payload = response.payload.unwrap_or(Value::Null);
    if is_auth_failure(response.status, &payload) {
        return Err(GatewayError::with_status(
            401,
            format!(
                "{what}：MonkeyCode 登录态已失效，请重新粘贴 session（HTTP {}）",
                response.status
            ),
        ));
    }
    if !response.ok {
        let detail = upstream_message(&payload);
        let detail = if detail.is_empty() {
            String::new()
        } else {
            format!("：{detail}")
        };
        return Err(GatewayError::with_status(
            response.status as i32,
            format!("{what}返回 HTTP {}{detail}", response.status),
        ));
    }
    Ok(payload)
}

/// GET 便捷入口（校验 / 目录 / 任务列表共用）
pub async fn get_json(
    region: Region,
    path: &str,
    session: &str,
    what: &str,
) -> Result<Value, GatewayError> {
    request("GET", region, path, session, None, what).await
}
