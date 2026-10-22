//! 千问办公的每日签到：`POST {gateway}/api/v1/adapter/user/daily-credit-notice/claim`。
//!
//! ── 上游长什么样（实测 2026-10-19）──────────────────────────────
//! ```text
//! POST /api/v1/adapter/user/daily-credit-notice/claim     ← 不带任何 body
//!   → 200 {"code":"ok","data":{"should_show":false,"credits":0}}
//! ```
//!
//! **这不是 Qoder 那套 campaign 链路。** 千问办公没有 `/sash/api/v1/me/campaigns`，
//! 也没有 `CLAIMABLE` / `CLAIMED` 状态机 —— 它就是一条「领每日积分」的写接口，
//! 幂等：当天已领过再打一次回 `credits: 0`，**不报错、不重复发**。
//!
//! ── ★ 两个实测坑（抄 Qoder 的 `claim_campaign` 会双双踩中）────────
//!   1. **不能带 body**。带 `{}` 回
//!      `400 INVALID_MULE_RUN_REQUEST: "claim accepts no query or request body"`
//!      —— 而 Qoder 那边明写 `.json(&json!({}))`，照抄必错；
//!   2. **响应带 `{"code":"ok","data":{...}}` 信封**，`credits` 在 `data` 里。
//!      不解信封会读成 `None` → 按「今天已领」处理 → 用户明明领到了却显示未领。
//!
//! ── 判「今天签没签」为什么不能看 HTTP 状态 ───────────────────────
//! 已领过也回 200。所以只能看响应里的 `credits`：
//!   * `credits > 0` → 本次真的到账了，`success: true`；
//!   * `credits == 0` → 今天已经领过（或上游没发），`success: false` +
//!     `alreadyCompleted: true`，让界面把按钮变成「已签到」。
//!
//! ── 没有配套的查询端点 ────────────────────────────────────
//! `GET /api/v1/adapter/user/daily-credit-notice` 实测 **404**，
//! 所以「今天有没有奖励可领」只能靠 claim 自己的返回值回答，先查再领做不到。
//!
//! ── panic=abort ────────────────────────────────────────────
//! 本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::endpoints;

/// 领取每日积分（本家的「签到」）。
///
/// 返回 `{success, msg, ...}` —— 与 `billing::checkin::claim_result` 认的形状
/// 一致，于是汇总、日志与界面三处不需要为这一家再加分支。
pub async fn claim_daily_checkin(
    store: &AccountStore,
    account_id: &str,
) -> Result<Value, GatewayError> {
    let jwt = session_jwt(store, account_id)?;
    let url = endpoints::gateway_url(endpoints::DAILY_CREDIT_CLAIM_PATH);
    let client = crate::server::core::egress::client_for_system_proxy();
    // ★ 不带 body 也不带 Content-Type（见模块头第 1 条坑）
    let response = client
        .post(&url)
        .timeout(super::CONTROL_TIMEOUT)
        .header("Authorization", format!("Bearer {jwt}"))
        .header("Accept", "application/json")
        .header("User-Agent", super::user_agent())
        .send()
        .await
        .map_err(|err| GatewayError::with_status(502, format!("千问办公签到接口不可达：{err}")))?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        logging::log_event(
            "warn",
            "qwenwork",
            "签到接口返回非 2xx",
            Some(json!({ "status": status, "body": super::truncate(&text, 300) })),
        );
        return Err(GatewayError::with_status(
            if status == 401 { 401 } else { 502 },
            format!("千问办公签到接口返回 HTTP {status}"),
        ));
    }
    let body: Value = serde_json::from_str(&text)
        .map_err(|_| GatewayError::with_status(502, "千问办公签到接口返回的不是合法 JSON"))?;
    // ★ 解信封：`credits` 在 `data` 里（见模块头第 2 条坑）
    let data = body.get("data").filter(|v| v.is_object()).unwrap_or(&body);
    Ok(claim_result(data))
}

/// 从会话里取 JWT（签到与额度都打同一个域，取法一致）。
fn session_jwt(store: &AccountStore, account_id: &str) -> Result<String, GatewayError> {
    let session = store
        .get_session_by_id(account_id)
        .map(|entry| entry.session)
        .ok_or_else(|| GatewayError::with_status(404, "没有找到该千问办公账号"))?;
    session
        .get("auth")
        .and_then(|auth| auth.get("accessToken"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            GatewayError::with_status(401, "千问办公账号缺少登录令牌，请重新从本机导入")
        })
}

/// 把领取响应翻成签到结果行（入参是已解信封的 `data` 段）。
///
/// `credits` 可能以数字或字符串两种形态出现（上游对 0 有时给 `"0"`），
/// 两种都认；读不到就按「今天已领」处理（宁可少报一次成功，也不虚报）。
fn claim_result(data: &Value) -> Value {
    let credits = data
        .get("credits")
        .and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_str().and_then(|text| text.trim().parse::<f64>().ok()))
        })
        .unwrap_or(0.0);

    if credits > 0.0 {
        return json!({
            "success": true,
            "msg": format!("获得 {credits} 积分"),
            "rewardPoints": credits,
            // 中文口径名（与 `usage::merge_balance` 的 `unit` 同一套词汇：
            // 签到领到的和余额读到的必须是同一个单位说法）
            "unit": "积分",
        });
    }
    json!({
        "success": false,
        "alreadyCompleted": true,
        "msg": "今日已领取",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真领到：`success: true` + 金额（界面显示「本次 +N」）。
    #[test]
    fn positive_credits_is_a_real_claim() {
        let out = claim_result(&json!({ "credits": 50 }));
        assert_eq!(out.get("success").and_then(Value::as_bool), Some(true));
        assert_eq!(out.get("rewardPoints").and_then(Value::as_f64), Some(50.0));
        assert_eq!(out.get("unit").and_then(Value::as_str), Some("积分"));
        assert!(out.get("alreadyCompleted").is_none());
    }

    /// 实测形态：今天已领回 `{"should_show":false,"credits":0}`。
    /// 关键：必须带 `alreadyCompleted`，否则定时链会把「已签」记成「未签」反复重打。
    #[test]
    fn zero_credits_means_already_claimed() {
        let out = claim_result(&json!({ "should_show": false, "credits": 0 }));
        assert_eq!(out.get("success").and_then(Value::as_bool), Some(false));
        assert_eq!(out.get("alreadyCompleted").and_then(Value::as_bool), Some(true));
    }

    /// 上游把 0 写成字符串时同样判成已领（两种都出现过）。
    #[test]
    fn string_zero_is_also_already_claimed() {
        let out = claim_result(&json!({ "credits": "0" }));
        assert_eq!(out.get("alreadyCompleted").and_then(Value::as_bool), Some(true));
    }

    /// 响应里根本没有 `credits` —— 不虚报成功。
    #[test]
    fn missing_credits_is_not_a_success() {
        let out = claim_result(&json!({ "ok": true }));
        assert_eq!(out.get("success").and_then(Value::as_bool), Some(false));
        assert_eq!(out.get("alreadyCompleted").and_then(Value::as_bool), Some(true));
    }
}

