//! Coze 出站请求的薄封装：JSON 请求（PAT / Cookie 两种形态）与 SSE 流式请求。
//!
//! ── 为什么收在一处 ──────────────────────────────────────────
//! 全部接口（账户 / 签到 / 目录 / 推理）共用同一套代理与错误翻译；各写一份
//! 会让「哪个状态码算登录失效」这类判据分叉。PAT 与 Cookie 两种形态只差
//! 请求头（`credentials::pat_headers` / `cookie_headers`），传输层完全一致。
//!
//! ── 错误翻译口径 ────────────────────────────────────────────
//!   - 连接 / 超时 → 502 / 504（提示语带「上游」字样，与各家同款）；
//!   - HTTP 401 / 403 → 401（登录态失效；PAT 形态还可能是令牌临期，
//!     调用方按 `ensure_pat(force)` 重铸后重试，见 `chat.rs`）；
//!   - 其余非 2xx → 原状态码 + 响应体前 240 字符（排障用）。
//! 业务码（`code != 0`）**不在这里判**：各家接口的成功判定不同
//! （trade 系列看 `code`、passport 看 `message`），由调用方各自翻译。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;

use crate::server::core::auth_http::send_raw;
use crate::server::core::egress;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::errors::GatewayError;

/// JSON 请求的默认超时（账户 / 签到这类管理接口，30 秒足够）
const REQUEST_TIMEOUT_MS: u64 = 30_000;

/// 把传输错误翻译成网关错误（超时 → 504，其余 → 502）。
fn transport_error(what: &str, error: reqwest::Error) -> GatewayError {
    if error.is_timeout() {
        GatewayError::with_status(504, format!("{what}超时，请稍后重试"))
    } else {
        GatewayError::with_status(502, format!("{what}失败：{}", egress::describe_error_detail(&error)))
    }
}

/// 一次 GET，返回已解析的 JSON（HTTP 非 2xx / 解析失败都翻译成网关错误）。
pub async fn get_json_value(
    url: &str,
    headers: &[(String, String)],
    proxy: Option<&ResolvedProxy>,
    timeout_ms: Option<u64>,
) -> Result<Value, GatewayError> {
    let response = send_raw("GET", url, None, headers, proxy, Some(timeout_ms.unwrap_or(REQUEST_TIMEOUT_MS)))
        .await
        .map_err(|error| transport_error("Coze 请求", error))?;
    http_json(response, url)
}

/// 一次 JSON POST，返回已解析的 JSON。
pub async fn post_json_value(
    url: &str,
    headers: &[(String, String)],
    body: &Value,
    proxy: Option<&ResolvedProxy>,
    timeout_ms: Option<u64>,
) -> Result<Value, GatewayError> {
    let response = send_raw("POST", url, Some(body), headers, proxy, Some(timeout_ms.unwrap_or(REQUEST_TIMEOUT_MS)))
        .await
        .map_err(|error| transport_error("Coze 请求", error))?;
    http_json(response, url)
}

/// 把 `ApiResponse` 翻译成网关错误或返回 payload。
fn http_json(response: crate::server::core::auth_http::ApiResponse, url: &str) -> Result<Value, GatewayError> {
    if !response.ok {
        if response.status == 401 || response.status == 403 {
            return Err(GatewayError::with_status(
                401,
                format!("Coze 登录态已失效，请重新登录或重新添加该账号（HTTP {}，{url}）", response.status),
            ));
        }
        return Err(GatewayError::with_status(
            response.status as i32,
            format!("Coze 返回 HTTP {}（{url}）", response.status),
        ));
    }
    response.payload.ok_or_else(|| {
        GatewayError::with_status(502, format!("Coze 返回非 JSON 响应（{url}）"))
    })
}

/// 发起一次 SSE 流式 POST（推理流；**不设总超时**，长推理不被截断）。
///
/// 返回原始 `reqwest::Response`（调用方遍历 `bytes_stream()`）。调用方负责
/// 读空闲兜底（本项目编排层对出站流有统一处理，这里只做连接建立）。
pub async fn sse_post(
    url: &str,
    headers: &[(String, String)],
    body: &Value,
    proxy: Option<&ResolvedProxy>,
    what: &str,
) -> Result<reqwest::Response, GatewayError> {
    let client = egress::client_for(proxy);
    let mut request = client.post(url).header("Accept", "text/event-stream");
    for (name, value) in headers {
        request = request.header(name.as_str(), value.as_str());
    }
    let response = request
        .json(body)
        .send()
        .await
        .map_err(|error| transport_error(what, error))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let text = response.text().await.unwrap_or_default();
        if status == 401 || status == 403 {
            return Err(GatewayError::with_status(
                401,
                format!("Coze 登录态已失效，请重新登录或重新添加该账号（HTTP {status}，{what}）"),
            ));
        }
        let head: String = text.chars().take(240).collect();
        return Err(GatewayError::with_status(
            status as i32,
            format!("Coze 返回 HTTP {status}（{what}）：{head}"),
        ));
    }
    Ok(response)
}
