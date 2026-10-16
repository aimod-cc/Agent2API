//! OrcaTerm 出站请求的薄封装：JSON 请求（CGI / assistant）与 SSE 流式请求。
//!
//! ── 为什么收在一处 ──────────────────────────────────────────
//! CGI 网关（`cgi/api`）与 assistant 网关（`lightai`）共用同一套鉴权头
//! （`Authorization: Bearer <JWT>`）与代理/超时/错误翻译口径；各写一份会让
//! 「哪个状态码算登录失效」这类判据分叉。
//!
//! ── 错误翻译口径 ────────────────────────────────────────────
//!   - 连接 / 超时 → 502 / 504；
//!   - HTTP 401 / 403 → 401（JWT 失效，调用方按 `supports_refresh = true`
//!     走「强制刷新后重试一次」的既有链路）；
//!   - 其余非 2xx → 原状态码 + 响应体前 240 字符。
//! CGI 的业务错误**不在这里判**：它 HTTP 恒 200，业务码在
//! `data.Response.Error.Code`，由调用方各自翻译。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;

use crate::server::core::egress;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::errors::GatewayError;

/// JSON 请求的默认超时（建会话 / 续期 / 余额这类管理接口，20 秒足够）
const REQUEST_TIMEOUT_MS: u64 = 20_000;

/// 把传输错误翻译成网关错误（超时 → 504，其余 → 502）。
fn transport_error(what: &str, error: reqwest::Error) -> GatewayError {
    if error.is_timeout() {
        GatewayError::with_status(504, format!("{what}超时，请稍后重试"))
    } else {
        GatewayError::with_status(502, format!("{what}失败：{}", egress::describe_error_detail(&error)))
    }
}

/// 非 2xx 响应的统一翻译。
fn translate_status(status: u16, url: &str, head: &str) -> GatewayError {
    if status == 401 || status == 403 {
        GatewayError::with_status(
            401,
            format!("OrcaTerm 登录态已失效，请重新导入或重新粘贴该账号的 accessToken（HTTP {status}，{url}）"),
        )
    } else {
        GatewayError::with_status(status as i32, format!("OrcaTerm 返回 HTTP {status}（{url}）：{head}"))
    }
}

/// 一次 JSON POST，返回已解析的 JSON（HTTP 非 2xx / 解析失败都翻译成网关错误）。
pub async fn post_json_value(
    url: &str,
    headers: &[(String, String)],
    body: &Value,
    proxy: Option<&ResolvedProxy>,
    timeout_ms: Option<u64>,
) -> Result<Value, GatewayError> {
    let client = egress::client_for(proxy);
    let mut builder = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json");
    for (name, value) in headers {
        builder = builder.header(name.as_str(), value);
    }
    builder = builder.json(body);
    builder = builder.timeout(std::time::Duration::from_millis(timeout_ms.unwrap_or(REQUEST_TIMEOUT_MS)));
    let response = builder
        .send()
        .await
        .map_err(|error| transport_error("OrcaTerm 请求", error))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let text = response.text().await.unwrap_or_default();
        let head: String = text.chars().take(240).collect();
        return Err(translate_status(status, url, &head));
    }
    let text = response.text().await.map_err(|error| {
        GatewayError::with_status(502, format!("OrcaTerm 响应读取失败：{error}"))
    })?;
    serde_json::from_str(&text).map_err(|error| {
        GatewayError::with_status(502, format!("OrcaTerm 返回非 JSON 响应（{url}）：{error}"))
    })
}

/// 发起一次 SSE 流式 POST（对话流；**不设总超时**，长推理不被截断）。
///
/// 返回原始 `reqwest::Response`（调用方遍历 `bytes_stream()`）。调用方负责
/// 读空闲兜底（编排层对出站流有统一处理，这里只做连接建立）。
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
        request = request.header(name.as_str(), value);
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
                format!("OrcaTerm 登录态已失效，请重新导入或重新粘贴该账号的 accessToken（HTTP {status}）"),
            ));
        }
        let head: String = text.chars().take(240).collect();
        return Err(GatewayError::with_status(
            status as i32,
            format!("OrcaTerm 返回 HTTP {status}（{what}）：{head}"),
        ));
    }
    Ok(response)
}
