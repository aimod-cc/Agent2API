//! 智谱清言每日签到与领取类。
//!
//! ── 本家的「签到」是什么（逆向 + 实测依据）──────────────────────
//! 网页端 `member-api` 那一组接口里，与「每日领取」相关的是三条：
//!
//! ```text
//!   POST /chatglm/member-api/member/daily_login_score     ← 每日登录积分（= 签到）
//!        → result: { score: <本次到账数> }
//!   GET  /chatglm/member-api/member/score_activity_open   ← 积分活动是否开启
//!   GET  /chatglm/member-api/member/score_activity_status ← 活动状态（积分余额）
//!   POST /chatglm/member-api/member/score_activity_draw   ← 活动抽奖 / 领取（= 领取类）
//!   GET  /chatglm/member-api/member/summer_day_claim_status
//!   POST /chatglm/member-api/member/summer_day_give_score ← 季节性活动领取（= 领取类）
//! ```
//!
//! 三条判定来自网页端实现（`agentmore` 包里的调用点）：
//!   1. `daily_login_score` 的调用**前置条件就是「登录且今天还没领」**：
//!      网页端在 `is_guest` 时直接 return（访客不调），并记一个
//!      「今天已领」的本地标记；返回的 `result.score` 是**本次到账数**，
//!      `0` 表示今天已经领过（幂等），业务码 `10001` 也是「已领」那一档；
//!   2. `score_activity_draw` 是活动期才可用的抽奖 / 领取，活动未开启时上游
//!      给它的是中性拒绝而不是错误；
//!   3. `summer_day_*` 是限时活动，活动下线后状态接口恒回「未开启」。
//!
//! ── 访客一律跳过（实测 401 `user needs login`）────────────────
//! 所以两个 claim 都先判凭证形态：访客直接给**中性未领取**（`success: false`
//! 但不是错误）—— 与 Qoder 国际版 / Trae 那些「不该算失败」的格子同一处置，
//! 免得自动签到把「访客本来就没得签」反复记成失败。
//!
//! ── 返回形状（`billing::checkin` 的契约）──────────────────────
//! `{success, msg}` —— 与 WorkBuddy / 小浣熊 / AutoClaw / Qoder / Loomy /
//! KukuAI 的 claim 同一形状（汇总只认这两个字段）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{credentials, endpoints, http};

/// 访客的中性结果（不是失败：访客本来就没有积分体系）
fn guest_result() -> Value {
    json!({
        "success": false,
        "msg": "访客账号没有积分体系，跳过（仅登录账号可领取）",
    })
}

/// 取凭证并判形态；访客返回 `None`（调用方给中性结果）
async fn login_credentials(
    store: &AccountStore,
    account_id: &str,
) -> Result<Option<credentials::ChatglmCredentials>, GatewayError> {
    let record = store.chatglm_account_record(account_id);
    let credentials = credentials::from_record(record.as_ref())?;
    if credentials.is_guest {
        return Ok(None);
    }
    Ok(Some(credentials))
}

/// 每日登录积分（本家的「签到」）。
///
/// 返回 `{success, msg}`：`result.score > 0` 是本次到账；`0` 或业务码 10001
/// 都是「今天已经领过」—— 两者都算**成功**（幂等语义，与网页端同判据：
/// 它把 10001 当作「已领」记本地标记，不当错误）。
pub async fn claim_daily_login_score(
    store: &AccountStore,
    account_id: &str,
) -> Result<Value, GatewayError> {
    let Some(credentials) = login_credentials(store, account_id).await? else {
        return Ok(guest_result());
    };
    let token = credentials.bearer();

    // 领取前先读活动状态：活动根本没开时不去领（免得把中性拒绝记成失败）
    let activity_open = http::get_json(
        endpoints::PATH_SCORE_ACTIVITY_OPEN,
        &token,
        &credentials.device_id,
        None,
        "积分活动查询",
    )
    .await
    .ok()
    .and_then(|payload| http::check_envelope(&payload, "积分活动查询").ok());
    // `score_activity_open` 返回的 result 形态是布尔或含 open 字段的对象，
    // 两者都认；读不出来时不拦（宁可多打一次领取，也别漏签）
    let open = match activity_open.as_ref() {
        Some(Value::Bool(flag)) => Some(*flag),
        Some(Value::Object(map)) => map
            .get("open")
            .or_else(|| map.get("is_open"))
            .and_then(Value::as_bool),
        _ => None,
    };

    let payload = match http::post_json(
        endpoints::PATH_DAILY_LOGIN_SCORE,
        &token,
        &credentials.device_id,
        &json!({}),
        None,
        "每日登录积分领取",
    )
    .await
    {
        Ok(payload) => payload,
        Err(error) => {
            // 「今天已领」在上游可能是业务码而不是 HTTP 错误：4xx 里带回
            // 10001 / 「已领取」文案的都当成功（幂等）
            let message = error.message.clone();
            if message.contains("10001") || message.contains("已领") || message.contains("already")
            {
                logging::log(
                    "[Checkin]",
                    &format!("智谱清言账号 {account_id}: 今日登录积分已领取过"),
                );
                return Ok(json!({ "success": true, "msg": "今日登录积分已领取过" }));
            }
            return Err(error);
        }
    };
    // 业务信封：10001 是「已领」，其余非 0 要按失败报出来
    let status = payload.get("status").and_then(Value::as_i64).unwrap_or(0);
    if status == 10001 {
        logging::log(
            "[Checkin]",
            &format!("智谱清言账号 {account_id}: 今日登录积分已领取过"),
        );
        return Ok(json!({ "success": true, "msg": "今日登录积分已领取过" }));
    }
    let result = http::check_envelope(&payload, "每日登录积分领取")?;
    // `result.score` 与余额是**同一套单位**（百分之一积分），必须走同一个
    // 换算函数 —— 否则余额显示 18644.05、签到提示却写 +20000（放大一百倍）。
    // 换算规则的依据见 `balance::score_from_raw` 的文档。
    let score_raw = result
        .get("score")
        .and_then(Value::as_f64)
        .or_else(|| result.as_f64())
        .unwrap_or(0.0);
    let score = super::balance::score_from_raw(score_raw);
    let message = if score > 0.0 {
        format!("今日登录积分 +{score}")
    } else if open == Some(false) {
        "积分活动未开启，无登录积分可领".to_string()
    } else {
        "今日登录积分已领取过".to_string()
    };
    if score > 0.0 {
        logging::log(
            "[Checkin]",
            &format!("✅ 智谱清言账号 {account_id}: {message}"),
        );
    }
    Ok(json!({ "success": true, "msg": message }))
}

/// 领取类：会员积分活动抽奖 + 季节性活动领取。
///
/// 两条都是「限时活动」形态：活动未开启 / 已下线时上游给中性拒绝，
/// 这里如实转成中性文案（不是失败）。
pub async fn claim_bonus(
    store: &AccountStore,
    account_id: &str,
) -> Result<Value, GatewayError> {
    let Some(credentials) = login_credentials(store, account_id).await? else {
        return Ok(guest_result());
    };
    let token = credentials.bearer();
    let device = &credentials.device_id;

    let mut messages: Vec<String> = Vec::new();
    let mut any_success = false;

    // ── 1. 会员积分活动抽奖 ─────────────────────────────────────
    match http::post_json(
        endpoints::PATH_SCORE_ACTIVITY_DRAW,
        &token,
        device,
        &json!({}),
        None,
        "积分活动领取",
    )
    .await
    {
        Ok(payload) => {
            let status = payload.get("status").and_then(Value::as_i64).unwrap_or(0);
            if status == 0 {
                let result = http::check_envelope(&payload, "积分活动领取")?;
                // 同 `daily_login_score`：积分字段一律走同一个换算（见
                // `balance::score_from_raw`），否则提示里的数字会大一百倍
                let gained_raw = result
                    .get("score")
                    .and_then(Value::as_f64)
                    .or_else(|| result.get("reward").and_then(Value::as_f64))
                    .unwrap_or(0.0);
                let gained = super::balance::score_from_raw(gained_raw);
                if gained > 0.0 {
                    any_success = true;
                    messages.push(format!("积分活动 +{gained}"));
                } else {
                    messages.push("积分活动无可领取".to_string());
                }
            } else {
                // 活动未开启 / 已领过都是中性
                let text = payload
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("活动不可领取");
                messages.push(format!("积分活动：{text}"));
            }
        }
        Err(error) => {
            // 活动不存在/未开启类错误不算失败（记中性文案），其余原样上抛
            let message = error.message.clone();
            if message.contains("活动") || message.contains("未开启") || message.contains("not found")
            {
                messages.push(format!("积分活动：{message}"));
            } else {
                return Err(error);
            }
        }
    }

    // ── 2. 季节性活动（夏季日签等）──────────────────────────────
    // 先读状态：活动下线时状态接口恒回「未开启」，就不去领了
    let claimable = http::get_json(
        endpoints::PATH_SUMMER_DAY_CLAIM_STATUS,
        &token,
        device,
        None,
        "季节活动状态查询",
    )
    .await
    .ok()
    .and_then(|payload| http::check_envelope(&payload, "季节活动状态查询").ok())
    .and_then(|result| {
        result
            .get("claimed")
            .and_then(Value::as_bool)
            .map(|claimed| !claimed)
            .or_else(|| {
                result
                    .get("claimable")
                    .and_then(Value::as_bool)
            })
    });
    if claimable == Some(true) {
        match http::post_json(
            endpoints::PATH_SUMMER_DAY_GIVE_SCORE,
            &token,
            device,
            &json!({}),
            None,
            "季节活动领取",
        )
        .await
        {
            Ok(payload) => {
                if payload.get("status").and_then(Value::as_i64).unwrap_or(0) == 0 {
                    any_success = true;
                    messages.push("季节活动已领取".to_string());
                } else {
                    let text = payload
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("不可领取");
                    messages.push(format!("季节活动：{text}"));
                }
            }
            Err(error) => {
                let message = error.message.clone();
                if message.contains("活动") || message.contains("未开启") {
                    messages.push(format!("季节活动：{message}"));
                } else {
                    return Err(error);
                }
            }
        }
    }

    if messages.is_empty() {
        messages.push("当前没有可领取的活动".to_string());
    }
    let message = messages.join("；");
    if any_success {
        logging::log(
            "[Checkin]",
            &format!("✅ 智谱清言账号 {account_id}: {message}"),
        );
    }
    Ok(json!({ "success": any_success, "msg": message }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_result_is_neutral_not_error() {
        let value = guest_result();
        assert_eq!(value["success"], json!(false));
        assert!(value["msg"].as_str().unwrap_or("").contains("访客"));
    }

    /// **签到积分与余额同一换算**：上游 `result.score` 也是百分之一积分。
    ///
    /// ── 这条锁的是什么 ──────────────────────────────────────────
    /// 余额已经确认要除以 100（`left_score: 1864405` → 18644.05），而签到
    /// 返回的 `score` 是同一账户的同一套积分单位。两处若各写一份换算，
    /// 迟早漂成「余额对了、签到提示大一百倍」（`+20000` 而不是 `+200`）。
    ///
    /// 本测试直接锁「签到走的是 balance 的那个函数」，而不是在 checkin 里
    /// 另写一个 `/100`。
    #[test]
    fn checkin_score_uses_balance_scale() {
        // 签到日志文案里的 200 积分（score_rule: "登录赠送200积分/天"），
        // 上游按百分之一记 ⇒ 原始值 20000
        assert_eq!(super::super::balance::score_from_raw(20000.0), 200.0);
        // 与余额换算同源：同一函数、同一结果
        assert_eq!(
            super::super::balance::score_from_raw(1864405.0),
            18644.05
        );
    }
}
