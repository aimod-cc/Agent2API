//! 智谱清言出站请求的薄封装：带签名的 JSON 请求与访客/续期换证。
//!
//! ── 为什么收在一处 ──────────────────────────────────────────
//! 本家**每个**请求都要带签名三件套 + `X-Device-Id`（见 `sign.rs` 的实测边界表：
//! 缺签名 40001/40012、缺设备号 40004、错密钥 40014、过期时间戳 40013），
//! 头集合一旦分叉，就会出现「某一条接口偶尔 40001」这种最难查的形态。
//! 因此头的拼装只有这一份。
//!
//! ── 错误翻译口径 ────────────────────────────────────────────
//!   - 连接 / 超时 → 502 / 504（提示语带「上游」字样，与各家同款）；
//!   - HTTP 401 / 403 → 401（登录态失效；调用方按 `supports_refresh` 决定
//!     是否先续期再试）；
//!   - 其余非 2xx → 原状态码 + 响应体前 240 字符（排障用）；
//!   - **业务码 `status != 0`**：本家把很多失败放在 HTTP 200 的 body 里
//!     （如 `-1001 剩余积分不足`、`40103 user needs login`），
//!     由调用方按语境判定成败（各接口的成功条件不同）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;

use crate::server::core::auth_http::{send_raw, ApiResponse};
use crate::server::core::egress;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::errors::GatewayError;

use super::{endpoints, sign};

/// JSON 请求的默认超时（管理接口 30 秒足够）
const REQUEST_TIMEOUT_MS: u64 = 30_000;

/// 拼一组业务请求头（签名三件套 + 设备号 + 客户端标识）。
///
/// `token` 非空时补 `Authorization: Bearer`。`extra` 用于逐接口追加
/// （如流式接口的 `Accept: text/event-stream`）。
///
/// **签名按 base_url 现算**（生产/测试域密钥不同），因此调用方不需要关心密钥。
pub fn request_headers(
    token: &str,
    device_id: &str,
    extra: &[(&str, &str)],
) -> Vec<(String, String)> {
    let base = endpoints::base_url();
    let signed = sign::sign_headers(&base);
    let mut headers: Vec<(String, String)> = vec![
        (
            "Content-Type".to_string(),
            "application/json;charset=utf-8".to_string(),
        ),
        ("App-Name".to_string(), endpoints::APP_NAME.to_string()),
        ("X-Device-Id".to_string(), device_id.to_string()),
        ("X-App-Platform".to_string(), endpoints::APP_PLATFORM.to_string()),
        ("X-App-Version".to_string(), endpoints::APP_VERSION.to_string()),
        ("X-Timestamp".to_string(), signed.timestamp),
        ("X-Nonce".to_string(), signed.nonce),
        ("X-Sign".to_string(), signed.sign),
        ("Origin".to_string(), endpoints::site_origin()),
        ("Referer".to_string(), format!("{}/", endpoints::site_origin())),
    ];
    if !token.trim().is_empty() {
        headers.push((
            "Authorization".to_string(),
            format!("Bearer {}", token.trim()),
        ));
    }
    for (key, value) in extra {
        headers.push(((*key).to_string(), (*value).to_string()));
    }
    headers
}

/// 把传输错误翻译成网关错误（超时 → 504，其余 → 502）
fn transport_error(what: &str, error: reqwest::Error) -> GatewayError {
    if error.is_timeout() {
        GatewayError::with_status(504, format!("{what}超时，请稍后重试"))
    } else {
        GatewayError::with_status(
            502,
            format!("{what}失败：{}", egress::describe_error_detail(&error)),
        )
    }
}

/// 把响应翻译成网关错误或返回 payload（HTTP 2xx 即成功，业务码由调用方判）
fn into_payload(response: ApiResponse, what: &str, url: &str) -> Result<Value, GatewayError> {
    if !response.ok {
        let detail = response
            .payload
            .as_ref()
            .map(|value| truncate(&value.to_string(), 240))
            .unwrap_or_default();
        let status = response.status;
        // URL 进文案：排障时「哪条接口 401」是最先要回答的问题
        let message = if status == 401 || status == 403 {
            format!("{what}登录态已失效（HTTP {status}，{url}），请重新登录或重新添加该账号")
        } else {
            format!("{what}失败（HTTP {status}，{url}）：{detail}")
        };
        return Err(GatewayError::with_status(
            if status == 401 || status == 403 {
                401
            } else {
                i32::from(status)
            },
            message,
        ));
    }
    Ok(response.payload.unwrap_or(Value::Null))
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    text.chars().take(limit).collect::<String>() + "…"
}

/// 一次带签名的 GET，返回已解析 JSON。
pub async fn get_json(
    path: &str,
    token: &str,
    device_id: &str,
    proxy: Option<&ResolvedProxy>,
    what: &str,
) -> Result<Value, GatewayError> {
    let url = endpoints::url(path);
    let headers = request_headers(token, device_id, &[]);
    let response = send_raw("GET", &url, None, &headers, proxy, Some(REQUEST_TIMEOUT_MS))
        .await
        .map_err(|error| transport_error(what, error))?;
    into_payload(response, what, &url)
}

/// 一次带签名的 POST（空 body 用 `{}`），返回已解析 JSON。
pub async fn post_json(
    path: &str,
    token: &str,
    device_id: &str,
    body: &Value,
    proxy: Option<&ResolvedProxy>,
    what: &str,
) -> Result<Value, GatewayError> {
    let url = endpoints::url(path);
    let headers = request_headers(token, device_id, &[]);
    let response = send_raw(
        "POST",
        &url,
        Some(body),
        &headers,
        proxy,
        Some(REQUEST_TIMEOUT_MS),
    )
    .await
    .map_err(|error| transport_error(what, error))?;
    into_payload(response, what, &url)
}

/// 从上游响应体里判成败并取出 `result`。
///
/// 本家的业务信封是 `{ status, message, result }`，`status == 0`（或 200）
/// 表示成功。失败时把上游的 `message` 原样带上 —— 那是**客户端可见的最终文案**
/// （如「剩余积分不足，可购买积分充值包获得更多积分」），本家文案质量本就够用。
pub fn check_envelope(value: &Value, _what: &str) -> Result<Value, GatewayError> {
    let status = value.get("status").and_then(Value::as_i64).unwrap_or(0);
    if status == 0 || status == 200 {
        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
    }
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("上游返回失败");
    Err(GatewayError {
        message: format!("上游返回 {status}: {message}"),
        status_code: 502,
        upstream_code: Some(status),
        code: None,
    })
}

/// 访客换证：`POST /chatglm/user-api/guest/access`（空 body）。
///
/// 返回 `(access_token, refresh_token, uid)`。这是本家**零配置可用**的入口：
/// 不需要手机号、不需要扫码，换一个设备号就是一个新访客身份。
pub async fn guest_access(
    device_id: &str,
    proxy: Option<&ResolvedProxy>,
) -> Result<(String, String, String), GatewayError> {
    let payload = post_json(
        endpoints::PATH_GUEST_ACCESS,
        "",
        device_id,
        &serde_json::json!({}),
        proxy,
        "获取智谱清言访客凭证",
    )
    .await?;
    let result = check_envelope(&payload, "获取智谱清言访客凭证")?;
    let access = result
        .get("access_token")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let refresh = result
        .get("refresh_token")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let uid = result
        .get("user_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if access.is_empty() {
        return Err(GatewayError::with_status(
            502,
            "智谱清言访客换证未返回 access_token（上游响应形态可能已变）",
        ));
    }
    Ok((access, refresh, uid))
}

/// 续期：`POST /chatglm/user-api/user/refresh`（refresh_token 换 access_token）。
///
/// 返回 `(access_token, refresh_token, uid)`；上游**可能轮换 refresh_token**，
/// 因此返回的第二个值应当覆盖回账号记录（调用方负责落盘）。
pub async fn refresh_token(
    refresh_token: &str,
    device_id: &str,
    proxy: Option<&ResolvedProxy>,
) -> Result<(String, String, String), GatewayError> {
    if refresh_token.trim().is_empty() {
        return Err(GatewayError::with_status(
            401,
            "智谱清言账号没有 refresh_token，无法续期（请重新添加该账号）",
        ));
    }
    let payload = post_json(
        endpoints::PATH_REFRESH,
        refresh_token,
        device_id,
        &serde_json::json!({}),
        proxy,
        "刷新智谱清言凭证",
    )
    .await?;
    let result = check_envelope(&payload, "刷新智谱清言凭证")?;
    let access = result
        .get("access_token")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let rotated = result
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| refresh_token.to_string());
    let uid = result
        .get("user_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if access.is_empty() {
        return Err(GatewayError::with_status(
            401,
            "智谱清言续期未返回 access_token（refresh_token 可能已失效，请重新添加该账号）",
        ));
    }
    Ok((access, rotated, uid))
}

/// 用户信息：`GET /chatglm/user-api/user/info`（昵称 / 访客标记 / 有效期）
pub async fn user_info(
    token: &str,
    device_id: &str,
    proxy: Option<&ResolvedProxy>,
) -> Result<Value, GatewayError> {
    let payload = get_json(
        endpoints::PATH_USER_INFO,
        token,
        device_id,
        proxy,
        "获取智谱清言用户信息",
    )
    .await?;
    check_envelope(&payload, "获取智谱清言用户信息")
}

/// 发一次对话请求并返回**原始响应文本**（SSE 全文）。
///
/// ── 谁用它（不是转发路径）────────────────────────────────────
/// 转发路径走编排层（`upstream::provider_loop`）自己建连接、把响应交给翻译层，
/// 不经过这里。本函数供**联调测试**使用：把真实的上游字节一次读全，交给
/// `protocol::chatglm_stream` 那台状态机验证差分 —— 那是唯一能证明
/// 「签名 + 信封 + 帧语义」与真实服务端对得上的地方（见 `live_tests.rs`）。
///
/// 超时给 120 秒（对话可能很长；管理接口那 30 秒不够）。
#[cfg(test)]
pub async fn post_text(
    path: &str,
    token: &str,
    device_id: &str,
    body: &Value,
    proxy: Option<&ResolvedProxy>,
    what: &str,
) -> Result<String, GatewayError> {
    let url = endpoints::url(path);
    // 流式接口要 Accept: text/event-stream（与 `build_chat_request` 同款）
    let headers = request_headers(token, device_id, &[("Accept", "text/event-stream")]);
    // 刻意不用 `auth_http::send_raw`：它把响应体当 JSON 解析，SSE 会被丢成
    // None（那一层服务的是管理接口）。这里要的就是**原始字节**。
    let client = crate::server::core::egress::client_for(proxy);
    let mut builder = client
        .post(&url)
        .timeout(std::time::Duration::from_millis(120_000))
        .header("Accept", "text/event-stream");
    for (key, value) in &headers {
        builder = builder.header(key, value);
    }
    let response = builder
        .body(body.to_string())
        .send()
        .await
        .map_err(|error| {
            GatewayError::with_status(
                502,
                format!("{what}失败：{}", egress::describe_error_detail(&error)),
            )
        })?;
    let status = response.status().as_u16();
    let text = response.text().await.map_err(|error| {
        GatewayError::with_status(
            502,
            format!("{what}读取响应失败：{}", egress::describe_error_detail(&error)),
        )
    })?;
    if !(200..300).contains(&status) {
        return Err(GatewayError::with_status(
            i32::from(status),
            format!("{what}失败（HTTP {status}）：{}", truncate(&text, 240)),
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 头集合必须包含签名三件套与设备号（缺任何一个都会被上游拒）
    #[test]
    fn headers_carry_signature_and_device() {
        let headers = request_headers("tok", "c9074ab1-208b-4255-8f71-09cc2ce32278", &[]);
        let names: Vec<&str> = headers.iter().map(|(key, _)| key.as_str()).collect();
        for required in [
            "Content-Type",
            "App-Name",
            "X-Device-Id",
            "X-App-Platform",
            "X-App-Version",
            "X-Timestamp",
            "X-Nonce",
            "X-Sign",
            "Authorization",
        ] {
            assert!(names.contains(&required), "缺少必需头 {required}");
        }
        // 设备号原样透传
        let device = headers
            .iter()
            .find(|(key, _)| key == "X-Device-Id")
            .map(|(_, value)| value.as_str());
        assert_eq!(device, Some("c9074ab1-208b-4255-8f71-09cc2ce32278"));
    }

    /// 空 token 时不带 Authorization（guest/access 与续期都不带）
    #[test]
    fn empty_token_omits_authorization() {
        let headers = request_headers("", "c9074ab1-208b-4255-8f71-09cc2ce32278", &[]);
        assert!(!headers.iter().any(|(key, _)| key == "Authorization"));
        // 续期用 refresh_token 当 Bearer
        let headers = request_headers("rrrr", "c9074ab1-208b-4255-8f71-09cc2ce32278", &[]);
        let auth = headers
            .iter()
            .find(|(key, _)| key == "Authorization")
            .map(|(_, value)| value.as_str());
        assert_eq!(auth, Some("Bearer rrrr"));
    }

    /// extra 逐接口追加（流式接口的 Accept）
    #[test]
    fn extra_headers_are_appended() {
        let headers = request_headers("tok", "d", &[("Accept", "text/event-stream")]);
        let accept = headers
            .iter()
            .find(|(key, _)| key == "Accept")
            .map(|(_, value)| value.as_str());
        assert_eq!(accept, Some("text/event-stream"));
    }

    /// 业务信封判定：status 0/200 成功，其余带上上游 message
    #[test]
    fn envelope_status_rules() {
        assert_eq!(check_envelope(&json!({"status": 0, "result": {"a": 1}}), "x").is_ok(), true);
        assert!(check_envelope(&json!({"status": 200, "result": 1}), "x").is_ok());
        // 缺 status 视为成功（部分接口直接回 result）
        assert!(check_envelope(&json!({"result": 1}), "x").is_ok());

        let error = check_envelope(
            &json!({"status": -1001, "message": "剩余积分不足", "result": null}),
            "x",
        )
        .expect_err("应当报错");
        assert_eq!(error.upstream_code, Some(-1001));
        assert!(error.message.contains("剩余积分不足"));

        let error = check_envelope(&json!({"status": 40103, "message": "user needs login"}), "x")
            .expect_err("应当报错");
        assert_eq!(error.upstream_code, Some(40103));
    }

    /// 截断工具不被多字节字符切坏
    #[test]
    fn truncate_is_char_safe() {
        let text = "中".repeat(300);
        let out = truncate(&text, 240);
        assert_eq!(out.chars().count(), 241, "240 个字符 + 省略号");
        assert!(out.ends_with('…'));
    }
}
