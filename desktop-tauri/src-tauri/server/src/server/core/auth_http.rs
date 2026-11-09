//! 上游管理接口的传输层：**一次裸请求**与鉴权错误类型。
//!
//! ── 这一层现在只剩什么 ──────────────────────────────────────
//! `send_raw`：发一次请求、把响应读成 `{status, ok, payload}`，出口按调用方
//! 传入的代理走 `core::egress`（不传 = 直连）。
//!
//! 仍在网关里的几家（Raccoon / AutoClaw / CodeArts / 自定义提供商）用它打
//! 自己的管理接口（余额、目录、续期、登录）。WorkBuddy 那份的 `unwrap`、
//! 登录轮询码、`Context` 端点拼装随实现搬到了提供商插件。
//!
//! `WorkBuddyAuthError` 保留在这里：它是「管理接口失败」的最小公共形状，
//! 调用方（转发兜底、计费、账号路由）都按它取 `status_code` / `upstream_code`。

use std::time::Duration;

use serde_json::Value;

use crate::server::core::egress;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::core::user_agent::AccountRequest;
use crate::server::errors::GatewayError;

/// token 临期刷新窗口：过期前 5 分钟视为需要刷新
pub const REFRESH_WINDOW_MS: f64 = 5.0 * 60.0 * 1000.0;

/// 管理接口的默认请求超时（网络异常时不能把管理 API 卡死）
pub const REQUEST_TIMEOUT_MS: u64 = 30_000;

/// 管理接口的错误（对应 Node 版 WorkBuddyAuthError）。
///
/// `status_code` 为 None 时由路由层按 502 处理 —— 与 Node 版
/// `Number.isInteger(error.statusCode) ? error.statusCode : 502` 一致。
#[derive(Clone, Debug)]
pub struct WorkBuddyAuthError {
    pub message: String,
    pub status_code: Option<i32>,
    pub upstream_code: Option<i64>,
}

impl WorkBuddyAuthError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), status_code: None, upstream_code: None }
    }

    pub fn with_status(status: i32, message: impl Into<String>) -> Self {
        Self { message: message.into(), status_code: Some(status), upstream_code: None }
    }

    pub fn with_code(message: impl Into<String>, status: Option<i32>, code: Option<i64>) -> Self {
        Self { message: message.into(), status_code: status, upstream_code: code }
    }

    /// 归一后的 HTTP 状态码（无显式状态码 → 502，照抄 Node 版）
    pub fn http_status(&self) -> i32 {
        self.status_code.unwrap_or(502)
    }

    /// 转成统一的网关错误（路由层按管理 API 信封返回）
    pub fn to_gateway_error(&self) -> GatewayError {
        let mut error = GatewayError::with_status(self.http_status(), self.message.clone());
        if let Some(code) = self.upstream_code {
            error = error.upstream_code(code);
        }
        error
    }
}

impl std::fmt::Display for WorkBuddyAuthError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

/// 一次上游请求的原始结果：状态码 + 是否 2xx + 解析出的 JSON（解析失败为 None）。
#[derive(Clone, Debug, Default)]
pub struct ApiResponse {
    pub status: u16,
    pub ok: bool,
    pub payload: Option<Value>,
}

/// 发一次上游请求，返回原始状态码与解析出的 payload。
///
/// ── 出口 ──────────────────────────────────────────────────
/// `proxy` 为 `None` 时直连（含「账号没配代理」与「代理解析失败回退直连」两种
/// 情形）；否则交给 `core::egress::client_for` 取该出口的 Client（带连接池缓存）。
///
/// ── 超时 ──────────────────────────────────────────────────
/// `timeout_ms` 是**单请求总超时**，与客户端上的 `connect_timeout` /
/// `read_timeout` 是叠加关系（先到者生效）。传 None 表示不设总超时。
pub async fn send_raw(
    method: &str,
    url: &str,
    body: Option<&Value>,
    headers: &[(String, String)],
    proxy: Option<&ResolvedProxy>,
    timeout_ms: Option<u64>,
) -> Result<ApiResponse, reqwest::Error> {
    let client = egress::client_for(proxy);
    let mut builder = match method {
        "POST" => client.post(url),
        "PUT" => client.put(url),
        "DELETE" => client.delete(url),
        "PATCH" => client.patch(url),
        _ => client.get(url),
    };
    builder = builder.header("Accept", "application/json");
    for (key, value) in headers {
        builder = builder.header(key, value);
    }
    if let Some(payload) = body {
        builder = builder
            .header("Content-Type", "application/json")
            .body(payload.to_string());
    }
    if let Some(timeout) = timeout_ms {
        builder = builder.timeout(Duration::from_millis(timeout));
    }
    let response = builder.with_account_user_agent().send().await?;
    let status = response.status().as_u16();
    let ok = response.status().is_success();
    let text = response.text().await?;
    let payload = if text.trim().is_empty() {
        None
    } else {
        serde_json::from_str::<Value>(&text).ok()
    };
    Ok(ApiResponse { status, ok, payload })
}
