//! 智谱清言余额 / 积分查询。
//!
//! ── 三条读数的来源（实测，2026-10）─────────────────────────────
//! ```text
//!   1. 会员积分活动状态  GET /chatglm/member-api/member/score_activity_status
//!        result: { status, remain_days, target_score, current_score, discount,
//!                  expire_at }        ← `current_score` 就是当前积分余额
//!   2. 每日登录积分      （签到接口，只用于展示「今日可得」，见 checkin.rs）
//!   3. 付费侧会员信息    GET /chatglm/payment-api/v1/cs/me
//!        登录账号的会员档位 / 到期时间（访客 401）
//! ```
//! 第 1 条是**主要读数**（本家把积分记在会员活动里）；第 3 条 best-effort
//! （接口失败不影响积分读数，只是少一行会员信息）。
//!
//! ── 访客账号没有积分体系（实测 401 `user needs login`）──────────
//! 访客令牌调上面两个接口一律 401。这不是「查询失败」而是「该账号形态不支持」，
//! 因此报 [`USAGE_NOT_CONFIGURED_CODE`] 那条中性标记（前端显示成「未配置查询」
//! 而不是红色失败）—— 与 CatPaw 的 token2 / ZCode 的 Coding Plan 同一处置。
//! 文案要说清「访客账号本来就没有积分」，否则用户会去找根本没坏的设置。
//!
//! ── 归一化形状（`ProviderAdapter::query_usage` 的契约）────────
//! `{available, unit: "积分", wallets: [...], credits: [...], raw}`
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::adapter::{usage_not_configured, USAGE_NOT_CONFIGURED_CODE};
use crate::server::errors::GatewayError;

use super::{credentials, endpoints, http};

/// 从任意 JSON 里按候选键顺序取一个数字（字符串数字也认）
fn pick_number(value: &Value, keys: &[&str]) -> Option<f64> {
    for key in keys {
        let Some(field) = value.get(*key) else {
            continue;
        };
        if let Some(number) = field.as_f64() {
            if number.is_finite() {
                return Some(number);
            }
        }
        if let Some(text) = field.as_str() {
            if let Ok(number) = text.trim().parse::<f64>() {
                if number.is_finite() {
                    return Some(number);
                }
            }
        }
    }
    None
}

/// 「访客账号没有积分体系」的中性报错（前端按 code 显示成未配置）
fn guest_has_no_points() -> GatewayError {
    let mut error = usage_not_configured("智谱清言", "登录账号（访客账号没有积分体系）");
    // 覆盖掉 usage_not_configured 的通用文案：本家的情况要说清「访客本来就没有」
    error.message = "该账号是智谱清言**访客**，没有积分体系（访客只能对话，不能查询积分 / 签到）。\
                     请添加一个登录账号后再查。"
        .to_string();
    error.code = Some(USAGE_NOT_CONFIGURED_CODE.to_string());
    error
}

/// 查询某账号的积分余额（归一化形状见模块头）。
pub(super) async fn query(
    store: &AccountStore,
    account_id: &str,
) -> Result<Value, GatewayError> {
    let record = store.chatglm_account_record(account_id);
    let credentials = credentials::from_record(record.as_ref())?;
    // 访客账号：直接给中性提示（不去打注定 401 的接口）
    if credentials.is_guest {
        return Err(guest_has_no_points());
    }
    let token = credentials.bearer();

    // 主线：会员积分活动状态（积分余额就在这里）
    let activity = http::get_json(
        endpoints::PATH_SCORE_ACTIVITY_STATUS,
        &token,
        &credentials.device_id,
        None,
        "积分查询",
    )
    .await?;
    let result = http::check_envelope(&activity, "积分查询")?;
    // 40103 / 未登录：同一个中性结论（令牌可能是过期的登录态）
    let current = pick_number(&result, &["current_score"]).unwrap_or(0.0);
    let target = pick_number(&result, &["target_score"]);
    let remain_days = pick_number(&result, &["remain_days"]);
    let activity_status = result.get("status").and_then(Value::as_i64);

    // 支线（best-effort）：会员档位 / 到期时间。失败不影响积分读数
    let member = http::get_json(
        endpoints::PATH_PAYMENT_ME,
        &token,
        &credentials.device_id,
        None,
        "会员信息查询",
    )
    .await
    .ok()
    .and_then(|payload| http::check_envelope(&payload, "会员信息查询").ok());

    // ── 明细行：积分活动一行 + （有会员信息时）会员一行 ──────────
    let mut credits = vec![json!({
        "name": "会员积分活动",
        "remain": current,
    })];
    if let Some(target) = target {
        if let Some(object) = credits[0].as_object_mut() {
            // 「已达 / 目标」是活动进度，界面用它画进度条
            object.insert("target".to_string(), Value::from(target));
        }
    }
    if let Some(days) = remain_days {
        if let Some(object) = credits[0].as_object_mut() {
            object.insert("remainDays".to_string(), Value::from(days));
        }
    }

    let mut wallets = vec![json!({
        "type": "activity_points",
        "displayName": "会员积分",
        "balance": current,
    })];

    // 会员档位（payment-api 的响应形态各家不同，这里只取能认出来的展示字段）
    let mut subscription = Value::Null;
    if let Some(member) = member.as_ref() {
        let plan = member
            .get("vip_level")
            .or_else(|| member.get("level"))
            .or_else(|| member.get("member_type"))
            .or_else(|| member.get("plan_name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty());
        let expire = pick_number(member, &["expire_at", "expireAt", "vip_expire_at"]);
        if plan.is_some() || expire.is_some() {
            subscription = json!({
                "planName": plan.unwrap_or(""),
                "status": member
                    .get("status")
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
                "expireAt": expire.map(|value| value as i64).unwrap_or(0),
            });
            if let Some(object) = credits.get_mut(0).and_then(Value::as_object_mut) {
                object.insert("plan".to_string(), Value::String(plan.unwrap_or("").to_string()));
            }
        }
    }

    wallets.push(json!({
        "type": "member_level",
        "displayName": "会员档位",
        "balance": Value::Null,
    }));

    let mut document = json!({
        "available": current,
        "unit": "积分",
        "wallets": wallets,
        "credits": credits,
        "raw": {
            "activity": activity,
            "member": member,
            "activityStatus": activity_status,
        },
    });
    if subscription != Value::Null {
        if let Some(object) = document.as_object_mut() {
            object.insert("subscription".to_string(), subscription);
        }
    }
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn picks_numbers_from_mixed_shapes() {
        let value = json!({ "a": 12.5, "b": "30", "c": "not a number" });
        assert_eq!(pick_number(&value, &["a"]), Some(12.5));
        assert_eq!(pick_number(&value, &["b"]), Some(30.0));
        assert_eq!(pick_number(&value, &["c"]), None);
        assert_eq!(pick_number(&value, &["missing"]), None);
        // 候选键顺序
        assert_eq!(pick_number(&json!({ "x": 1, "y": 2 }), &["y", "x"]), Some(2.0));
    }

    #[test]
    fn guest_error_is_neutral_and_mentions_guest()  {
        let error = guest_has_no_points();
        assert_eq!(error.status_code, 400, "中性标记用 400（前端显示成未配置）");
        assert_eq!(error.code.as_deref(), Some(USAGE_NOT_CONFIGURED_CODE));
        assert!(error.message.contains("访客"));
    }
}
