//! ModelScope 两种出站请求的薄封装：网页站（cookie 鉴权）与推理域（Bearer）。
//!
//! ── 为什么收在一处 ──────────────────────────────────────────
//! 四处消费者（`checkin` / `balance` / `models` / 账号添加时的探活）共用同一套
//! 超时、错误翻译与「响应信封 → 人话」口径；各写一份会让「哪个码算令牌失效」
//! 这类判据分叉。
//!
//! ── 上游的**两种信封**（实测 2026-10-16，都出现在同一家的不同接口上）──
//! 这是本文件最需要注意的一处：魔搭自己的接口没统一大小写。
//!
//! ```text
//!   /api/v1/users/login/info        {"Code":200,"Success":true,"Data":{…},"Message":"success"}
//!   /api/v1/inference/rate-limit    {"Code":200,"Success":true,"Data":{…}}
//!   /openapi/v1/magicubes/balance   {"success":true,"data":{…}}            ← 全小写
//! ```
//!
//! 因此取值一律**两种写法都认**（[`succeeded`] / [`data_of`] / [`message_of`]），
//! 不要只按一种写死 —— 那会让某一条链路把成功读成失败（或反之）。
//!
//! ── 令牌失效的判据 ──────────────────────────────────────────
//! 网页站对无效令牌给的是 **HTTP 401 + `{"Code":10019901001,"Message":"access
//! forbidden"}`**（实测：拿国内站的 key 打国际站网页接口就是这个响应）。
//! 也有接口用 HTTP 200 + `Success:false` 表达失败 —— 两条路都要认：
//! HTTP 401/403 → [`GatewayError`] 401，业务位为假 → 502 并把上游文案带出来。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::Value;

use crate::server::core::auth_http::send_raw;
use crate::server::errors::GatewayError;

use super::region::Region;

/// 网页站接口的超时（余额 / 签到都是小请求，20 秒足够；别用 egress 默认的 600 秒
/// SSE 超时，那会让一个挂住的查询把前端转圈转到天荒地老）
const REQUEST_TIMEOUT_MS: u64 = 20_000;

/// 响应信封的**成功位**（大写 `Success` 与小写 `success` 都认，见模块头）。
///
/// 两种都没有时看 `Code`（缺失则视为成功 —— HTTP 已经 2xx 了，没有反证就照实报成功）。
pub fn succeeded(payload: &Value) -> bool {
    if let Some(flag) = payload.get("Success").and_then(Value::as_bool) {
        return flag;
    }
    if let Some(flag) = payload.get("success").and_then(Value::as_bool) {
        return flag;
    }
    match payload.get("Code").and_then(Value::as_i64) {
        Some(code) => code == 200,
        None => true,
    }
}

/// 响应信封里的**业务数据**（`Data` / `data` 两层都认）。
pub fn data_of(payload: &Value) -> Value {
    payload
        .get("Data")
        .or_else(|| payload.get("data"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// 响应信封里的**上游文案**（`Message` / `message` / `msg` 三种都认）。
pub fn message_of(payload: &Value) -> String {
    payload
        .get("Message")
        .or_else(|| payload.get("message"))
        .or_else(|| payload.get("msg"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string()
}

/// 响应信封里的业务码（`Code` / `code`；都没有时给 0）。
pub fn code_of(payload: &Value) -> i64 {
    payload
        .get("Code")
        .or_else(|| payload.get("code"))
        .and_then(Value::as_i64)
        .unwrap_or(0)
}

/// 拼一个网页站 URL（`region` 决定站点，见 `region.rs`）。
pub fn site_url(region: Region, path: &str) -> String {
    format!("{}{path}", region.site_base_url())
}

/// 向**网页站**发一次带会话 cookie 的 GET。
///
/// 鉴权是 `Cookie: m_session_id=<令牌>`（实测口径，与插件 `msclient.go` 的
/// `req.Header.Set("Cookie", "m_session_id="+sessionID)` 一致）。
///
/// 返回上游响应体；HTTP 层失败翻译成网关错误。**业务位不在本函数里判** ——
/// 调用方各自知道「这次调用的成功长什么样」（签到只看 HTTP，余额看业务位）。
pub async fn site_get(
    region: Region,
    path: &str,
    token: &str,
    what: &str,
) -> Result<Value, GatewayError> {
    if token.trim().is_empty() {
        return Err(GatewayError::with_status(
            401,
            "ModelScope 账号缺少令牌，无法查询",
        ));
    }
    let url = site_url(region, path);
    let headers = vec![("Cookie".to_string(), format!("m_session_id={token}"))];
    let response = send_raw("GET", &url, None, &headers, None, Some(REQUEST_TIMEOUT_MS))
        .await
        .map_err(|error| {
            if error.is_timeout() {
                GatewayError::with_status(504, format!("{what}超时，请稍后重试"))
            } else {
                GatewayError::with_status(502, format!("{what}失败：{error}"))
            }
        })?;
    if !response.ok {
        if response.status == 401 || response.status == 403 {
            return Err(GatewayError::with_status(
                401,
                format!(
                    "{what}：ModelScope 令牌已失效或不属于该站点（{}），请重新粘贴（HTTP {}）",
                    region.site_base_url(),
                    response.status
                ),
            ));
        }
        return Err(GatewayError::with_status(
            response.status as i32,
            format!("{what}返回 HTTP {}", response.status),
        ));
    }
    Ok(response.payload.unwrap_or(Value::Null))
}

/// 向**推理域**发一次带 Bearer 的 GET（目前只有模型目录用）。
///
/// 与 [`site_get`] 是**两个域**：网页站挂了会连签到一起失败，而推理域挂了只影响
/// 模型目录 —— 分开成两个函数，才不会让排障时误判成同一处故障。
pub async fn inference_get(
    region: Region,
    path: &str,
    token: &str,
    what: &str,
) -> Result<Value, GatewayError> {
    if token.trim().is_empty() {
        return Err(GatewayError::with_status(
            401,
            "ModelScope 账号缺少令牌，无法查询",
        ));
    }
    let url = format!("{}{path}", region.inference_base_url());
    let headers = vec![(
        "Authorization".to_string(),
        format!("Bearer {token}"),
    )];
    let response = send_raw("GET", &url, None, &headers, None, Some(REQUEST_TIMEOUT_MS))
        .await
        .map_err(|error| {
            if error.is_timeout() {
                GatewayError::with_status(504, format!("{what}超时，请稍后重试"))
            } else {
                GatewayError::with_status(502, format!("{what}失败：{error}"))
            }
        })?;
    if !response.ok {
        if response.status == 401 || response.status == 403 {
            return Err(GatewayError::with_status(
                401,
                format!("{what}：ModelScope 令牌已失效，请重新粘贴（HTTP {}）", response.status),
            ));
        }
        return Err(GatewayError::with_status(
            response.status as i32,
            format!("{what}返回 HTTP {}", response.status),
        ));
    }
    Ok(response.payload.unwrap_or(Value::Null))
}

#[cfg(test)]
mod tests {
    use super::{code_of, data_of, message_of, succeeded};
    use serde_json::json;

    /// 大写信封（login/info、rate-limit 的实测形态）
    #[test]
    fn the_uppercase_envelope_is_read() {
        let payload = json!({"Code": 200, "Success": true, "Data": {"a": 1}, "Message": "success"});
        assert!(succeeded(&payload));
        assert_eq!(code_of(&payload), 200);
        assert_eq!(data_of(&payload), json!({"a": 1}));
        assert_eq!(message_of(&payload), "success");
    }

    /// 小写信封（magicubes/balance 的实测形态）—— 只认大写会让余额查询整条挂掉
    #[test]
    fn the_lowercase_envelope_is_read() {
        let payload = json!({"success": true, "data": {"total_balance": 108}});
        assert!(succeeded(&payload));
        assert_eq!(data_of(&payload), json!({"total_balance": 108}));
    }

    /// 显式的失败位必须被认出来（否则签到会把上游的拒绝记成成功）
    #[test]
    fn an_explicit_failure_flag_is_not_a_success() {
        assert!(!succeeded(&json!({"Success": false, "Message": "access forbidden"})));
        assert!(!succeeded(&json!({"success": false})));
        // 上游实测的错误码（`access forbidden`）—— 超过 i32 范围，用 i64 字面量
        assert!(!succeeded(&json!({"Code": 10019901001i64, "Success": false})));
    }

    /// `Code` 是唯一线索时（没有布尔位）按 200 判
    #[test]
    fn the_code_is_the_last_resort() {
        assert!(succeeded(&json!({"Code": 200})));
        assert!(!succeeded(&json!({"Code": 500})));
    }
}