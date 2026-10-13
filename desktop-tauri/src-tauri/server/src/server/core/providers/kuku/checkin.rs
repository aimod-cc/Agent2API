//! KukuAI 每日签到：免费领积分活动（freepoint）的每日任务领取。
//!
//! ── 上游是什么（2026-10-08 真实凭证实测 + kuku2api 同款）────────
//! ```text
//! GET  /api/genflowpro/freepoint/homenew?{WEB_QUERY}
//!      → {errno, data:{activities:[{activity_key, activity_name, period_no,
//!         tabs:[{tasks:[{task_type, task_name, task_status, single_reward_point,
//!         claimable_point}]}]}]}}
//! POST /api/genflowpro/freepoint/taskComplete?{WEB_QUERY}
//!      body: task_type=LOGIN|CHAT        （x-www-form-urlencoded）
//!      → {errno, data:{complete_status, reward_point}}
//! ```
//! 实测要点：
//!   - `homenew` 的任务状态 `FINISHED` / `UNFINISHED`，`claimable_point` 恒 0
//!     —— **不能拿它当领取门槛**（「完成一次对话」UNFINISHED 照样领到 +50）；
//!   - `taskComplete` **幂等**：重复领返回 `SUCCESS` + `reward_point=0`（或
//!     kuku2api 记录的 `BADGE_ALREADY_RECEIVED`），因此「已领过」不算失败；
//!   - 每日可自助完成的是 `LOGIN`（每日登录）与 `CHAT`（完成一次对话）；
//!     `INVITE_VISIT` / `INVITE_DOWNLOAD` 要真实受邀人，接口上强领只会失败，
//!     不进领取清单（kuku2api 同款取舍）。
//!
//! ── 会话前提 ────────────────────────────────────────────────
//! freepoint 是业务接口，与 userreport 同一道会话门：调 `with_business_stoken`
//! 先换发 genflowpro STOKEN（`engine.rs`），老账号 / 通行证级 STOKEN 也能签。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::credentials;
use super::http;
use super::{BASE_URL, WEB_QUERY};

/// 每日可自助完成的任务（类型，展示名）。展示名用于结果文案。
const DAILY_TASKS: [(&str, &str); 2] = [("LOGIN", "每日登录"), ("CHAT", "完成一次对话")];

/// KukuAI 账号的每日签到（自动签到框架的 claim）。
///
/// 返回 `{success, msg}` —— 与 WorkBuddy / 小浣熊 / AutoClaw / Qoder / Loomy
/// 的 claim 同一形状（`billing::checkin` 的汇总只认这两个字段）。
pub async fn claim_daily_checkin(
    store: &AccountStore,
    account_id: &str,
) -> Result<Value, GatewayError> {
    let credentials = credentials::snapshot_for(store, account_id)?;
    let prepared = credentials::with_business_stoken(&credentials).await;
    let headers = credentials::request_headers(&prepared);

    // 1) 任务面板：先探一次会话（freepoint 与 userreport 同一道门，-6 就是
    //    登录态问题），顺带确认活动面板可读。
    let panel_url = format!("{BASE_URL}/api/genflowpro/freepoint/homenew?{WEB_QUERY}");
    let panel = http::get_json_value(&panel_url, &headers, None, Some(30_000)).await?;
    let errno = panel.get("errno").and_then(Value::as_i64).unwrap_or(-1);
    if errno != 0 {
        let message = panel
            .get("show_msg")
            .or_else(|| panel.get("errmsg"))
            .and_then(Value::as_str)
            .unwrap_or("未知错误");
        return Err(GatewayError::with_status(
            401,
            format!("KukuAI 签到失败（freepoint errno={errno}：{message}）——请重新登录该账号"),
        ));
    }

    // 2) 逐个领取。单个任务失败只记日志继续（一个任务被风控不该拖垮另
    //    一个），与「个别账号查询失败不再拖垮整轮定时查询」同一取舍。
    let mut newly: Vec<String> = Vec::new();
    let mut already_claimed = false;
    let mut total = 0i64;
    for (task_type, task_label) in DAILY_TASKS {
        let url = format!("{BASE_URL}/api/genflowpro/freepoint/taskComplete?{WEB_QUERY}");
        let value = match http::post_form_value(
            &url,
            &headers,
            &[("task_type", task_type)],
            None,
            Some(30_000),
        )
        .await
        {
            Ok(value) => value,
            Err(error) => {
                logging::log(
                    "[Checkin]",
                    &format!("⚠️ KukuAI「{task_label}」领取失败：{}", error.message),
                );
                continue;
            }
        };
        let errno = value.get("errno").and_then(Value::as_i64).unwrap_or(-1);
        if errno != 0 {
            let message = value
                .get("show_msg")
                .or_else(|| value.get("errmsg"))
                .and_then(Value::as_str)
                .unwrap_or("未知错误");
            logging::log(
                "[Checkin]",
                &format!("⚠️ KukuAI「{task_label}」领取失败（errno={errno}：{message}）"),
            );
            continue;
        }
        let data = value.get("data").cloned().unwrap_or(Value::Null);
        let status = data
            .get("complete_status")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let reward = data
            .get("reward_point")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if status.eq_ignore_ascii_case("SUCCESS") && reward > 0 {
            newly.push(format!("{task_label}+{reward}"));
            total += reward;
        } else {
            // SUCCESS + reward_point=0（重复领取）或非 SUCCESS（已领标记）：
            // 都是「今天已经领过」，不算失败（幂等，与 Loomy 同一口径）。
            already_claimed = true;
        }
    }

    let msg = if !newly.is_empty() {
        format!("签到完成：{}（共 +{total}）", newly.join("、"))
    } else if already_claimed {
        "今日积分已领过（任务接口幂等，无新增）".to_string()
    } else {
        "本次没有可领取的每日任务（活动可能未开始或已结束）".to_string()
    };
    logging::log("[Checkin]", &format!("✅ {msg}"));
    Ok(json!({ "success": true, "msg": msg }))
}
