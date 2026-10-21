//! ModelScope 每日签到：**读一次登录信息就完成**（2026-10-16 实测 + 插件源码对照）。
//!
//! ── 上游是什么 ──────────────────────────────────────────────
//! 魔搭**没有独立的签到接口**（官方网页的「每日签到」按钮背后就是这条）：
//!
//! ```text
//! GET {网页站}/api/v1/users/login/info          Cookie: m_session_id=<ms-令牌>
//!   → {"Code":200,"Success":true,"Data":{"Name":"…","NickName":"…",
//!                                          "HavanaId":"…","Email":"…"}, …}
//! ```
//!
//! **调用这个端点本身就触发当日的魔粒签到**（激活当日魔粒额度），
//! 返回体里给的是账号身份，而不是「签到成功」这四个字 ——
//! 这是本家签到与别家最大的不同，也是「看起来只是读了个用户信息」的原因。
//!
//! ── 依据来自哪里（不是猜的）─────────────────────────────────
//! CPA 的 `modelscope-ratelimit` 插件（v2.4.10，`cmd/plugin/main.go` 的午夜签到段）
//! 就是按这个口径做的：遍历账号 → 调 `/api/v1/users/login/info` →
//! 把这次调用登记成当日签到（`store.RecordCheckIn(aid, now)`）。
//! 本实现照同一条链路，只是把触发时机交给本项目的自动签到框架。
//!
//! ── 幂等 ────────────────────────────────────────────────────
//! 一天内调多次是安全的（插件同样每天只调一次，但没有「今天已签过」的拒绝返回）。
//! 因此「今天已经触发过」**不算失败** —— 如实报成功并说明是重复调用，
//! 与 Loomy 的 `first-login` 同一口径。
//!
//! ── 两个地区打各自的站 ──────────────────────────────────────
//! `region` 决定站点。打错了会拿到 `401 access forbidden`（实测：国内站的令牌打
//! 国际站就是这个响应），这里把它如实翻成「令牌不属于该站点」而不是笼统的失败 ——
//! 那正是用户把两地账号加混时最需要看到的一句话。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::client;
use super::credentials;
use super::region::Region;

/// 登录信息路径（相对网页站）。**它就是签到**，见模块头。
pub const LOGIN_INFO_PATH: &str = "/api/v1/users/login/info";

/// ModelScope 账号的每日签到（自动签到框架的 claim）。
///
/// 返回 `{success, msg}` —— 与 WorkBuddy / 小浣熊 / AutoClaw / Qoder / Loomy
/// 的 claim 同一形状（`billing::checkin` 的汇总只认这两个字段）。
pub async fn claim_daily_checkin(
    region: Region,
    store: &AccountStore,
    account_id: &str,
) -> Result<Value, GatewayError> {
    let record = store.modelscope_account_record(region, account_id);
    if !account_id.is_empty() && record.is_none() {
        return Err(GatewayError::with_status(
            404,
            format!(
                "ModelScope {}账号 {account_id} 不存在（请重新添加）",
                region.label()
            ),
        ));
    }
    let credentials = credentials::from_record(region, record.as_ref())?;

    let payload = match client::site_get(
        region,
        LOGIN_INFO_PATH,
        &credentials.token,
        "每日签到",
    )
    .await
    {
        Ok(payload) => payload,
        // 401 原样透出（调用方按「刷新后重试」的既有处置走；本家没有续期，
        // 最终会如实报「请重新粘贴令牌」）
        Err(error) => return Err(error),
    };

    if !client::succeeded(&payload) {
        let message = client::message_of(&payload);
        let message = if message.trim().is_empty() {
            format!("每日签到失败（上游 Code={}）", client::code_of(&payload))
        } else {
            format!("每日签到失败：{message}")
        };
        logging::log("[Checkin]", &format!("⚠️ ModelScope {message}"));
        return Ok(json!({ "success": false, "msg": message }));
    }

    // 成功时上游给的是账号身份（不是「签到成功」）—— 把显示名播报出来，
    // 用户一眼能确认「这条记录签的是哪个账号」（多账号场景下这是有用的信息）
    let data = client::data_of(&payload);
    let display = data
        .get("NickName")
        .or_else(|| data.get("Name"))
        .or_else(|| data.get("nickname"))
        .or_else(|| data.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("")
        .to_string();
    let msg = if display.is_empty() {
        format!("魔搭{}每日签到已触发", region.label())
    } else {
        format!("魔搭{}每日签到已触发（{display}）", region.label())
    };
    logging::log("[Checkin]", &format!("✅ {msg}"));
    Ok(json!({ "success": true, "msg": msg }))
}