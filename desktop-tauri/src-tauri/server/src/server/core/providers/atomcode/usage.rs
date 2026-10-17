//! AtomCode 的额度读数：`status-v2` 的**滚动调用次数窗口**。
//!
//! ── 这家的额度口径与别家都不一样（读之前先看这条）─────────────
//! 不是积分、不是 token 余额，而是**每窗口可调用多少次**。实测（2026-10-01，
//! 记录在本地 Go 参考 `credits.go:37-38`）：
//!
//! > The plan's real quota is a CALL COUNT per window, not a token budget:
//! > the free tier measured 2026-10-01 allows 200 calls per 5 hours.
//!
//! `status-v2` 里同时存在 `window_token_limit`（**可空**）与
//! `rate_limit_windows[]`（真正的口径）。本模块只读后者：
//! 前者在免费档上是 null，把它当主读数会让界面显示一片空白。
//!
//! ── 只展示服务端让展示的窗口 ──────────────────────────────────
//! 每个窗口带 `show_enable`，只有 `1` 的才进界面（与本地 Go 参考
//! `credits.go:131-139` 同一口径）。服务端可能在同一个账号上挂多个规则
//! （5 小时 / 日 / 月），把不展示的那些也画出来会让用户看到互相矛盾的额度。
//!
//! ── 归一形状（与 loomy/balance.rs 同款，界面真正读的那一套）──────
//! ```text
//!   { available, unit, wallets: [{type, displayName, balance, remaining, limit, …}], raw }
//! ```
//! ⚠️ 曾经有一家（灵犀）在这处自造了 `total_balance` / `currency` 两个键，
//! 结果账号页余额列**空白**而探针还全绿（探针跟着实现断言）。所以这里
//! **严格照抄 loomy 的键名**，不自造。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{Value, json};

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::credentials::{AtomcodeCredentials, from_record};
use super::endpoints::REQUEST_TIMEOUT_MS;
use super::models::fetch_status;

/// 一个滚动窗口（`rate_limit_windows[]` 的一条）
#[derive(Clone, Debug)]
struct QuotaWindow {
    /// 窗口小时数
    hours: i64,
    /// 本窗口允许的调用次数
    call_limit: i64,
    /// 已用次数
    used: i64,
    /// 服务端给的重置时刻文案（如 `12:13`）
    reset_display: String,
    /// 本窗口是否已耗尽
    exhausted: bool,
}

/// 从 `status-v2` 里取出「服务端让展示」的窗口
fn shown_windows(payload: &Value) -> Vec<QuotaWindow> {
    let mut windows = Vec::new();
    let Some(items) = payload.get("rate_limit_windows").and_then(Value::as_array) else {
        return windows;
    };
    for item in items {
        // 只有 show_enable == 1 的窗口才展示
        if item.get("show_enable").and_then(Value::as_i64) != Some(1) {
            continue;
        }
        let call_limit = item.get("call_limit").and_then(Value::as_i64).unwrap_or(0);
        if call_limit <= 0 {
            continue;
        }
        windows.push(QuotaWindow {
            hours: item.get("window_hours").and_then(Value::as_i64).unwrap_or(0),
            call_limit,
            used: item.get("calls_used").and_then(Value::as_i64).unwrap_or(0),
            reset_display: item
                .get("reset_at_display")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            exhausted: item
                .get("quota_exhausted")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
    }
    windows
}

/// 套餐名与到期文案（`codingplan_free` 块）
fn plan_of(payload: &Value) -> (String, String) {
    let plan = payload.get("codingplan_free");
    let name = plan
        .and_then(|plan| plan.get("plan_name"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let expires = plan
        .and_then(|plan| plan.get("expires_at"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    (name, expires)
}

/// 把 `status-v2` 归成界面认的额度形状（**纯函数**，便于测试）
///
/// ── 一条**必须**遵守的契约：读不到额度就不要写 `available` ──────
/// 限额判定走 `core::usage_records::extract_remaining`，它的规则是
/// 「`unlimited` → 判不出；否则看 `totalLeft`；否则看 `available`」——
/// 也就是说 **`available` 一旦出现，就会被当成真实读数**。
///
/// 2026-10-17 我在这一点上写错过：没有窗口时也写 `available: 0`，于是「读不到额度」
/// 被限制器理解成「余额为 0」，账号当场被标成「余额不足 / 已跳过」——
/// 用户看到的现象是「领完套餐还是余额不足」。**没有窗口 ≠ 余额为 0**：
/// 前者是「这个响应我没读懂」，后者是「确实用完了」，两者必须分开。
/// 所以这里只在**确实解析出窗口**时才写 `available` / `subscription` 的那两个数；
/// 一个都没有就只回 `wallets: []`（记录会被 `usage_records` 摘除、不拦账号，
/// 余额列留空 —— 与本仓「读不到的数值留空显示」同一口径）。
pub fn shape_usage(payload: &Value) -> Value {
    let windows = shown_windows(payload);
    let (plan_name, plan_expires) = plan_of(payload);

    // 诊断：把上游的真实字段形态记进**详细日志**。额度是按字段名解析的，
    // 名字一变就静默归零（今天就栽在这），所以留一条能事后对账的痕迹。
    // 只记键名与两条额度数组（都不含凭据），且不进普通日志。
    logging::verbose(
        "[AtomCode]",
        &format!(
            "status-v2 形态：顶层键={:?} rate_limit_windows={} current_usage={}",
            payload
                .as_object()
                .map(|object| object.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default(),
            payload
                .get("rate_limit_windows")
                .map(|value| serde_json::to_string(value).unwrap_or_default())
                .map(|text| text.chars().take(600).collect::<String>())
                .unwrap_or_else(|| "<缺>".to_string()),
            payload
                .get("current_usage")
                .map(|value| serde_json::to_string(value).unwrap_or_default())
                .map(|text| text.chars().take(400).collect::<String>())
                .unwrap_or_else(|| "<缺>".to_string()),
        ),
    );

    let wallets: Vec<Value> = windows
        .iter()
        .map(|window| {
            let remaining = (window.call_limit - window.used).max(0);
            let label = if window.hours > 0 {
                format!("近 {} 小时调用次数", window.hours)
            } else {
                "调用次数窗口".to_string()
            };
            let mut wallet = json!({
                "type": "rolling_window",
                "displayName": label,
                "balance": remaining,
                "remaining": remaining,
                "limit": window.call_limit,
            });
            if let Some(object) = wallet.as_object_mut() {
                object.insert("used".to_string(), Value::from(window.used));
                if !window.reset_display.is_empty() {
                    object.insert(
                        "resetAt".to_string(),
                        Value::String(window.reset_display.clone()),
                    );
                }
                if window.exhausted {
                    object.insert("exhausted".to_string(), Value::Bool(true));
                }
            }
            wallet
        })
        .collect();

    // 主读数 = 第一个窗口的剩余次数。
    // ⚠️ **只在真的解析出窗口时才写**（读到 0 条 = 这份响应我没读懂，
    // 不是「余额为 0」）—— 理由见本函数文档「必须遵守的契约」那一节。
    let available = windows
        .first()
        .map(|window| (window.call_limit - window.used).max(0));

    let mut subscription = json!({ "status": if plan_name.is_empty() { "unclaimed" } else { "active" } });
    if let Some(object) = subscription.as_object_mut() {
        if !plan_name.is_empty() {
            object.insert("planName".to_string(), Value::String(plan_name));
        }
        // 上游给的是**日期字符串**，而 `expireAt` 在别家是毫秒数字。
        // 混用会让界面把一串日期当毫秒渲染，所以把原文放进独立的键，
        // 不占用 `expireAt`（宁可这一格留空，也不要显示一个错的时刻）。
        if !plan_expires.is_empty() {
            object.insert("planExpiresAt".to_string(), Value::String(plan_expires));
        }
        // 同上：没有窗口就不写这两个数（写了 `available` 的兄弟键也一样会被
        // 当成读数来源）
        if let Some(first) = windows.first() {
            object.insert(
                "remainQuota".to_string(),
                Value::from((first.call_limit - first.used).max(0)),
            );
            object.insert("totalQuota".to_string(), Value::from(first.call_limit));
        }
    }

    let mut shaped = json!({
        "unit": "次",
        "wallets": wallets,
        "subscription": subscription,
        "raw": payload,
    });
    // 只有解析出窗口才回 `available`（见函数文档）：这是限额判定唯一的读数来源
    if let Some(available) = available {
        if let Some(object) = shaped.as_object_mut() {
            object.insert("available".to_string(), Value::from(available));
        }
    }
    shaped
}

/// 查额度：读记录 → 取 `status-v2` → 归一。
pub async fn query_usage(
    store: &AccountStore,
    account_id: &str,
) -> Result<Value, GatewayError> {
    let record = store.atomcode_account_record(account_id);
    if !account_id.is_empty() && record.is_none() {
        return Err(GatewayError::with_status(
            401,
            format!("AtomCode 账号 {account_id} 不存在（请重新添加）"),
        ));
    }
    let credentials: AtomcodeCredentials = from_record(record.as_ref())?;
    let payload = fetch_status(&credentials, REQUEST_TIMEOUT_MS).await?;
    Ok(shape_usage(&payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status_with_windows() -> Value {
        json!({
            "codingplan_free": {"plan_name": "CodingPlan Lite-体验版", "expires_at": "2026-11-08 12:00:00"},
            "rate_limit_windows": [
                {"show_enable": 1, "window_hours": 5, "call_limit": 200, "calls_used": 37,
                 "reset_at_display": "12:13", "quota_exhausted": false},
                {"show_enable": 0, "window_hours": 24, "call_limit": 1000, "calls_used": 900}
            ]
        })
    }

    /// **核心口径**：额度是调用次数窗口，`available` 是剩余次数。
    #[test]
    fn available_is_remaining_calls_of_the_first_shown_window() {
        let shaped = shape_usage(&status_with_windows());
        assert_eq!(shaped.get("available"), Some(&Value::from(163)));
        assert_eq!(shaped.get("unit"), Some(&Value::from("次")));
    }

    /// `show_enable != 1` 的窗口不进界面（服务端不让展示的规则画出来会互相矛盾）。
    #[test]
    fn windows_the_server_hides_are_not_drawn() {
        let shaped = shape_usage(&status_with_windows());
        let wallets = shaped.get("wallets").and_then(Value::as_array).expect("wallets");
        assert_eq!(wallets.len(), 1, "只该有一个展示窗口");
        assert_eq!(wallets[0].get("limit"), Some(&Value::from(200)));
        assert_eq!(wallets[0].get("remaining"), Some(&Value::from(163)));
        assert_eq!(wallets[0].get("used"), Some(&Value::from(37)));
        assert!(wallets[0].get("displayName").and_then(Value::as_str).unwrap_or("").contains("5 小时"));
    }

    /// 归一形状的键名必须与别家一致（灵犀曾自造键名导致界面空白）。
    #[test]
    fn shape_uses_the_shared_key_names() {
        let shaped = shape_usage(&status_with_windows());
        for key in ["available", "unit", "wallets", "subscription", "raw"] {
            assert!(shaped.get(key).is_some(), "缺键 {key}");
        }
        let wallet = &shaped["wallets"][0];
        for key in ["type", "displayName", "balance", "remaining", "limit"] {
            assert!(wallet.get(key).is_some(), "wallet 缺键 {key}");
        }
        // 自造键不得出现（它们正是上一家空白的原因）
        assert!(shaped.get("total_balance").is_none());
        assert!(shaped.get("currency").is_none());
    }

    /// 上游给的到期是**日期字符串**，不能塞进别家读作毫秒的 `expireAt`。
    #[test]
    fn plan_expiry_text_does_not_impersonate_a_millisecond_timestamp() {
        let shaped = shape_usage(&status_with_windows());
        let subscription = &shaped["subscription"];
        assert!(subscription.get("expireAt").is_none(), "不要把日期字符串写进 expireAt");
        assert_eq!(
            subscription.get("planExpiresAt").and_then(Value::as_str),
            Some("2026-11-08 12:00:00")
        );
        assert_eq!(subscription.get("planName").and_then(Value::as_str), Some("CodingPlan Lite-体验版"));
        assert_eq!(subscription.get("status").and_then(Value::as_str), Some("active"));
    }

    /// 没有套餐时如实标 unclaimed（不编造一个 0 额度窗口）。
    #[test]
    fn account_without_a_plan_reports_unclaimed() {
        let shaped = shape_usage(&json!({}));
        assert_eq!(
            shaped["subscription"].get("status").and_then(Value::as_str),
            Some("unclaimed")
        );
        assert_eq!(shaped.get("wallets").and_then(Value::as_array).map(Vec::len), Some(0));
        // ⚠️ **不许**回 `available: 0` —— 读不到窗口是「这份响应没读懂」，
        // 不是「余额为 0」。写成 0 会被 `usage_records::extract_remaining` 当成
        // 真实读数，账号当场被标成「余额不足/已跳过」（2026-10-17 的实际故障）。
        assert!(
            shaped.get("available").is_none(),
            "读不到窗口时不得写 available：那会被限额判定当成余额为 0"
        );
    }

    /// 读不到窗口时 `subscription` 里也不该出现额度数字（同一条契约的兄弟键）。
    #[test]
    fn no_windows_means_no_quota_numbers_anywhere() {
        let shaped = shape_usage(&json!({ "codingplan_free": {"plan_name": "CodingPlan Lite-体验版", "expires_at": "2026-11-08 12:00:00"} }));
        assert!(shaped.get("available").is_none());
        let subscription = &shaped["subscription"];
        assert!(subscription.get("remainQuota").is_none(), "{subscription}");
        assert!(subscription.get("totalQuota").is_none(), "{subscription}");
        // 套餐名仍然要透出去（那是读到了的字段）
        assert_eq!(subscription.get("planName").and_then(Value::as_str), Some("CodingPlan Lite-体验版"));
    }

    /// 耗尽的窗口要能看出来（界面把它收进折叠区）。
    #[test]
    fn exhausted_windows_are_flagged() {
        let payload = json!({
            "rate_limit_windows": [
                {"show_enable": 1, "window_hours": 5, "call_limit": 200, "calls_used": 200,
                 "quota_exhausted": true}
            ]
        });
        let shaped = shape_usage(&payload);
        assert_eq!(shaped["wallets"][0].get("remaining"), Some(&Value::from(0)));
        assert_eq!(shaped["wallets"][0].get("exhausted"), Some(&Value::Bool(true)));
        assert_eq!(shaped.get("available"), Some(&Value::from(0)));
    }

    /// 已用超过上限（上游口径变化）时剩余不得为负。
    #[test]
    fn remaining_never_goes_negative() {
        let payload = json!({
            "rate_limit_windows": [
                {"show_enable": 1, "window_hours": 5, "call_limit": 100, "calls_used": 130}
            ]
        });
        let shaped = shape_usage(&payload);
        assert_eq!(shaped.get("available"), Some(&Value::from(0)));
        assert_eq!(shaped["wallets"][0].get("remaining"), Some(&Value::from(0)));
    }
}
