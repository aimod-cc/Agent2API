//! AtomCode 的「签到」：**领取免费 CodingPlan**。
//!
//! ── 它与别家的「签到」不是一回事，先说清楚 ────────────────────
//! 其它家的签到多是「每天领一次积分」。AtomCode 的免费套餐是**一次性领取**的
//! 授权（`POST /coding-plan/claim-v2`，档位 Max / Pro / Lite），领到之后
//! 按窗口滚额度、不再需要每天领。之所以挂在签到链上，是因为：
//!   · 新账号加进来时必须领一次，否则模型目录为空、什么都调不动；
//!   · 套餐会**到期**（`codingplan_free.expires_at`），到期后要能自动续领；
//!   · 签到链本来就有「每天跑一次、逐账号、结果进台账」的调度与界面，
//!     不必再为这一家单起一个定时器。
//! 所以本实现的口径是「**按需领取**」：先探测，只有确实没有有效套餐时才领。
//!
//! ── 顺序与停规则（照官方 `setup.rs:825-918` 与本地 Go 参考 `credits.go:287-330`）──
//! 档位按 **Max → Pro → Lite** 走，**命中即停**，且三种停法各有理由：
//!
//!   | 响应 | 含义 | 动作 |
//!   | --- | --- | --- |
//!   | `duplicate: true` | 该档位（或更高档）**已经持有** | **停** —— 继续往下走会再报一次 duplicate，甚至把用户降级 |
//!   | `success: true` | 本次发放成功 | **停** |
//!   | 两者皆 false | 该档位被拒（额度 / 不在活动期 / 不符合条件） | 记下文案，试下一档 |
//!   | 传输/5xx/解析失败 | 连接坏了 | **整条级联中止** —— 换个档位重试修不好连接，只会加压 |
//!
//! ⚠️ **别只看 HTTP 200 判成功**。`duplicate: true` 是**成功语义**（已持有），
//! 把它当失败会让级联继续往下走并把用户降级。这是本模块最容易写错的一处。
//! （我先前据插件 README 说过「claim-v2 没有幂等标记、重复领取只会再回
//! success:true」—— **那句是错的**，源码里 `credits.go:149` 就有 `Duplicate bool
//! json:"duplicate"`，官方 CLI 的 `types.rs:526` 同款。）
//!
//! ── 领到之后为什么要刷目录 ──────────────────────────────────
//! `models-v2` 的 `plan_available` 是**相对被问的档位**算的 ⇒ 档位一变，
//! 可用模型集合就变。所以任何一次「真领到」或「档位变了」之后必须让目录失效，
//! 否则界面会继续用旧档位下的那份清单（表现是「领到了却看不到新模型」）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{Value, json};

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::credentials::from_record;
use super::endpoints::REQUEST_TIMEOUT_MS;
use super::models::{PLAN_LITE, PLAN_MAX, PLAN_PRO, claim_plan, fetch_status, refresh};

/// 档位级联顺序（官方 `types.rs:48` 的 `CASCADE_ORDER`）
pub const CASCADE_ORDER: [&str; 3] = [PLAN_MAX, PLAN_PRO, PLAN_LITE];

/// 一次 `claim-v2` 响应的判定结果
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TierVerdict {
    /// 该档位（或更高档）已持有 —— **终止级联**（照官方：继续往下会重复报或降级）
    AlreadyHeld {
        /// 上游文案
        message: String,
        /// 上游给的套餐名（可能缺）
        plan_name: String,
    },
    /// 本次发放成功 —— **终止级联**
    Granted {
        /// 上游文案
        message: String,
        /// 上游给的套餐名（可能缺）
        plan_name: String,
    },
    /// 该档位被拒 —— 记下文案，试下一档
    Refused {
        /// 上游文案
        message: String,
    },
}

/// 判定一次 `claim-v2` 的响应。
///
/// **判定顺序是契约**：先看 `duplicate`（成功语义），再看 `success`。
/// 反过来写的话，上游同时给两个 false 之外的所有组合都会走错分支。
pub fn classify_tier_response(payload: &Value) -> TierVerdict {
    let message = payload
        .get("message")
        .or_else(|| payload.get("msg"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let plan_name = payload
        .get("plan_name")
        .or_else(|| payload.get("planName"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if payload.get("duplicate").and_then(Value::as_bool).unwrap_or(false) {
        return TierVerdict::AlreadyHeld { message, plan_name };
    }
    if payload.get("success").and_then(Value::as_bool).unwrap_or(false) {
        return TierVerdict::Granted { message, plan_name };
    }
    TierVerdict::Refused { message }
}

/// 当前套餐是否**已经有效、不需要领取**。
///
/// 判据与额度读数同源（`usage.rs` 的 `plan_of`）：`codingplan_free` 存在
/// 且 `expires_at` 非空 —— 上游用「`expires_at` 为空」表达**待激活**，
/// 那正是需要去领的状态。`remaining_days` 作为第二信号（同样由到期推出）。
pub fn active_plan_of(status: &Value) -> Option<(String, String, i64)> {
    let plan = status.get("codingplan_free")?;
    let name = plan
        .get("plan_name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let expires_at = plan
        .get("expires_at")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let remaining = plan
        .get("remaining_days")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if expires_at.is_empty() && remaining <= 0 {
        return None;
    }
    Some((name, expires_at, remaining))
}

/// 首个展示窗口的读数文案（`已用/上限 次，N 小时后重置`）。
///
/// 与 `usage.rs` 同一口径：只读 `show_enable == 1` 的窗口、额度是**调用次数**。
/// 读不到就返回空串（签到结果里少一行，不编造）。
pub fn quota_summary(status: &Value) -> String {
    let Some(items) = status.get("rate_limit_windows").and_then(Value::as_array) else {
        return String::new();
    };
    let Some(window) = items.iter().find(|item| {
        item.get("show_enable").and_then(Value::as_i64) == Some(1)
            && item.get("call_limit").and_then(Value::as_i64).unwrap_or(0) > 0
    }) else {
        return String::new();
    };
    let limit = window.get("call_limit").and_then(Value::as_i64).unwrap_or(0);
    let used = window.get("calls_used").and_then(Value::as_i64).unwrap_or(0);
    let hours = window.get("window_hours").and_then(Value::as_i64).unwrap_or(0);
    let reset = window
        .get("reset_at_display")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut text = format!("已用 {used}/{limit} 次");
    if hours > 0 {
        text.push_str(&format!("（每 {hours} 小时滚动）"));
    }
    if !reset.is_empty() {
        text.push_str(&format!("，{reset} 重置"));
    }
    text
}

/// 按需领取：探测 → 无效才级联 → 领到后刷目录。
///
/// 返回的形状与其它家的签到实现一致（`{success, msg, ...}`），交给
/// `billing::checkin::claim_result` 翻成签到结果行。
///
/// ── 三种结局对应用户看到的三种话 ─────────────────────────────
///   · **已经有效**：`success:false` + 中性文案（`套餐有效中…无需领取`）+ 额度读数。
///     刻意**不**回 `success:true` —— 那会被日志与界面说成「签到成功」，
///     而这一天其实什么都没发生（与本仓 chatglm 对访客账号的中性处置同一口径）。
///   · **真领到**：`success:true` + 上游文案。
///   · **被拒**：`success:false` + 各档位文案汇总（例如不在活动期）。
pub async fn claim_plan_if_needed(
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
    let credentials = from_record(record.as_ref())?;

    // ── 第一步：探测。已经有效就别动上游的领取接口 ────────────────
    let status = fetch_status(&credentials, REQUEST_TIMEOUT_MS).await?;
    if let Some((plan_name, expires_at, remaining)) = active_plan_of(&status) {
        let quota = quota_summary(&status);
        let mut msg = String::from("套餐有效中，无需领取");
        if !plan_name.is_empty() {
            msg.push_str(&format!("（{plan_name}）"));
        }
        if remaining > 0 {
            msg.push_str(&format!("，剩余 {remaining} 天"));
        } else if !expires_at.is_empty() {
            msg.push_str(&format!("，到期 {expires_at}"));
        }
        logging::verbose("[Checkin]", &format!("AtomCode {account_id}: {msg}"));
        return Ok(json!({
            "success": false,
            "msg": msg,
            "planName": plan_name,
            "expiresAt": expires_at,
            "quota": quota,
            "needsAction": false,
        }));
    }

    // ── 第二步：级联领取（Max → Pro → Lite，命中即停）─────────────
    let mut attempts: Vec<Value> = Vec::new();
    let mut last_refusal = String::new();
    for tier in CASCADE_ORDER {
        let payload = claim_plan(&credentials, tier).await.map_err(|error| {
            // 传输/HTTP 层失败：**整条级联中止**（换档位修不好连接）
            GatewayError::with_status(
                error.status_code,
                format!("AtomCode 套餐领取在 {} 档失败：{}", tier, error.message),
            )
        })?;
        match classify_tier_response(&payload) {
            TierVerdict::AlreadyHeld { message, plan_name } => {
                attempts.push(json!({ "tier": tier, "outcome": "already-held", "message": message }));
                let msg = if message.is_empty() {
                    format!("已持有 {tier} 档套餐，无需重复领取")
                } else {
                    message
                };
                logging::log("[Checkin]", &format!("AtomCode {account_id}: {msg}"));
                return Ok(json!({
                    "success": false,
                    "msg": msg,
                    "tier": tier,
                    "planName": plan_name,
                    "attempts": attempts,
                    "needsAction": false,
                }));
            }
            TierVerdict::Granted { message, plan_name } => {
                attempts.push(json!({ "tier": tier, "outcome": "claimed", "message": message }));
                let msg = if message.is_empty() {
                    format!("已领取 {tier} 档免费套餐")
                } else {
                    message
                };
                // 档位变了 ⇒ 目录必须失效（`plan_available` 相对档位算）
                let outcome = refresh(&credentials, true).await;
                logging::log(
                    "[Checkin]",
                    &format!(
                        "AtomCode {account_id}: {msg}（目录已刷新：{:?}）",
                        outcome
                    ),
                );
                return Ok(json!({
                    "success": true,
                    "msg": msg,
                    "tier": tier,
                    "planName": plan_name,
                    "attempts": attempts,
                    "needsAction": true,
                }));
            }
            TierVerdict::Refused { message } => {
                attempts.push(json!({ "tier": tier, "outcome": "refused", "message": message }));
                if !message.is_empty() {
                    last_refusal = message;
                }
            }
        }
    }

    // ── 三档都被拒：如实报告（常见成因是不在活动期或名额已满）──────
    let msg = if last_refusal.is_empty() {
        "三档免费套餐都未能领取（上游未说明原因）".to_string()
    } else {
        format!("三档免费套餐都未能领取：{last_refusal}")
    };
    logging::log("[Checkin]", &format!("AtomCode {account_id}: {msg}"));
    Ok(json!({
        "success": false,
        "msg": msg,
        "attempts": attempts,
        "needsAction": true,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// **首条护栏**：`duplicate:true` 是成功语义（已持有），必须终止级联。
    ///
    /// 把它当失败会让级联继续往下试，最终可能把用户从 Pro 降到 Lite ——
    /// 官方 `client.rs:203-207` 专门写了这条理由。
    #[test]
    fn duplicate_means_held_and_stops_the_cascade() {
        let verdict = classify_tier_response(&json!({
            "success": false, "duplicate": true, "message": "已领取", "plan_name": "CodingPlan Pro"
        }));
        assert_eq!(
            verdict,
            TierVerdict::AlreadyHeld {
                message: "已领取".into(),
                plan_name: "CodingPlan Pro".into(),
            }
        );
    }

    /// `success:true` 是发放，同样终止。
    #[test]
    fn success_means_granted_and_stops() {
        let verdict = classify_tier_response(&json!({
            "success": true, "message": "领取成功", "plan_name": "CodingPlan Lite-体验版"
        }));
        assert_eq!(
            verdict,
            TierVerdict::Granted {
                message: "领取成功".into(),
                plan_name: "CodingPlan Lite-体验版".into(),
            }
        );
    }

    /// 两个都为 false = 该档位被拒 ⇒ 记文案、试下一档。
    #[test]
    fn neither_flag_means_refused_so_the_cascade_continues() {
        let verdict = classify_tier_response(&json!({
            "success": false, "duplicate": false, "message": "名额已满"
        }));
        assert_eq!(
            verdict,
            TierVerdict::Refused {
                message: "名额已满".into()
            }
        );
    }

    /// 判定顺序是契约：`duplicate` 先于 `success`（上游理论上不会同时给真，
    /// 但真给了必须按「已持有」处置 —— 那不会误发、也不会降级）。
    #[test]
    fn duplicate_wins_over_success_because_it_is_checked_first() {
        let verdict = classify_tier_response(&json!({"success": true, "duplicate": true}));
        assert!(matches!(verdict, TierVerdict::AlreadyHeld { .. }));
    }

    /// 字段缺失 / 类型不对时按「被拒」处置，不 panic。
    #[test]
    fn malformed_payload_is_treated_as_refused_not_a_crash() {
        assert_eq!(
            classify_tier_response(&json!({})),
            TierVerdict::Refused {
                message: String::new()
            }
        );
        assert_eq!(
            classify_tier_response(&json!({"success": "true", "duplicate": 1})),
            TierVerdict::Refused {
                message: String::new()
            }
        );
    }

    /// 套餐有效性判据：`expires_at` 非空或 `remaining_days > 0` 才算有效。
    /// 上游用「`expires_at` 为空」表达**待激活** —— 那正是需要去领的状态。
    #[test]
    fn active_plan_requires_an_expiry_or_remaining_days() {
        let active = json!({
            "codingplan_free": {"plan_name": "CodingPlan Lite-体验版", "expires_at": "2026-11-08 12:00:00", "remaining_days": 22}
        });
        assert_eq!(
            active_plan_of(&active),
            Some(("CodingPlan Lite-体验版".into(), "2026-11-08 12:00:00".into(), 22))
        );

        // 待激活：expires_at 空、没有剩余天数 ⇒ 需要领
        let pending = json!({"codingplan_free": {"plan_name": "CodingPlan Lite-体验版", "expires_at": ""}});
        assert_eq!(active_plan_of(&pending), None);

        // 整块缺失 ⇒ 需要领
        assert_eq!(active_plan_of(&json!({})), None);

        // 只剩剩余天数也能判有效（expires_at 缺失的容错）
        let by_days = json!({"codingplan_free": {"plan_name": "X", "remaining_days": 3}});
        assert!(active_plan_of(&by_days).is_some());
    }

    /// 额度文案只读**服务端让展示**的窗口，且是调用次数口径。
    #[test]
    fn quota_summary_reads_the_shown_window_only() {
        let status = json!({
            "rate_limit_windows": [
                {"show_enable": 0, "window_hours": 24, "call_limit": 1000, "calls_used": 900},
                {"show_enable": 1, "window_hours": 5, "call_limit": 200, "calls_used": 37, "reset_at_display": "12:13"}
            ]
        });
        let text = quota_summary(&status);
        assert!(text.contains("已用 37/200 次"), "{text}");
        assert!(text.contains("每 5 小时滚动"), "{text}");
        assert!(text.contains("12:13 重置"), "{text}");

        // 没有可展示的窗口 ⇒ 空串（不编造）
        assert_eq!(quota_summary(&json!({})), "");
        assert_eq!(
            quota_summary(&json!({"rate_limit_windows": [{"show_enable": 0, "call_limit": 10}]})),
            ""
        );
    }

    /// 级联顺序就是官方那一串（不许改成 Lite 优先：那会把用户降到最低档）。
    #[test]
    fn cascade_order_is_max_then_pro_then_lite() {
        assert_eq!(CASCADE_ORDER, [PLAN_MAX, PLAN_PRO, PLAN_LITE]);
        assert_eq!(CASCADE_ORDER, ["Max", "Pro", "Lite"]);
    }
}