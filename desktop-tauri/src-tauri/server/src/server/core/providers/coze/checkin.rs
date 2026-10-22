//! Coze 每日签到 / 领取（自动签到框架的 claim）。
//!
//! ── 上游是什么（2026-10-21 活凭证实测）──────────────────────
//! Coze 的「签到」不是一次打卡，而是**两条互相独立的领奖链**：
//! ```text
//! POST /api/marketplace/incentive/activity/reward     （只能 Cookie）
//!      body {"activity_types":[1,2]}
//!      → {code:0, data:{activity_points:[{activity_type, point}]}}
//!        activity_type 1 = 30 天有效的活动积分
//!                      2 = 7 天有效的活动积分
//! POST /api/ug/v1/activity/client_download_reward     （PAT）
//!      body {"activity_id":"client_download_2026","client_type":"desktop"}
//!      → {code:0, msg:"success"}
//! ```
//! 实测要点：
//!   - 第一条**只能 Cookie 打**（PAT 一律 `700012006 登录认证失败`），第二条
//!     **要 PAT**（Cookie 打会缺设备标识）—— 所以本文件两条凭证都用：Cookie
//!     取自账号记录，PAT 现铸一次。这也是本家 claim 比别家多一步的原因；
//!   - `client_type` 是**字符串** `"desktop"`：传数字 2 上游回
//!     `ClientType: invalid request value, expected string`（踩过一次）；
//!   - 两条都是**幂等**的：当天已领会回 `code:0` 但 `activity_points` 为空
//!     （或 msg 不变），因此「已领过」不算失败；
//!   - `activity_types:[1,2]` 一次提交两个类型，上游按「还能领的」逐个发，
//!     不返回逐类型明细时分不清哪个领到了 —— 文案按「本次共 +N」表述，
//!     不假装知道来源。
//!
//! ── 为什么不读任务中心 ──────────────────────────────────────
//! `/api/marketplace/incentive/user_task/list`（Cookie）能列「每日登录 / 分享」
//! 这类任务，但那些任务的完成判据与奖励发放都在客户端里，接口上强领只会失败。
//! 本家可自助领取的就是上面两条，与 kuku 只领 `LOGIN`/`CHAT` 同一取舍。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::errors::GatewayError;

use super::credentials;
use super::http;
use super::WEB_BASE;

/// 签到请求超时
const CHECKIN_TIMEOUT_MS: u64 = 20_000;

/// 每日登录奖励的活动类型（1 = 30 天有效期，2 = 7 天有效期）
const ACTIVITY_TYPES: [i64; 2] = [1, 2];

/// 下载客户端奖励的活动 id（客户端 `main.js` 里写死的那个）
const CLIENT_DOWNLOAD_ACTIVITY: &str = "client_download_2026";

/// Coze 账号的每日签到（自动签到框架的 claim）。
///
/// 返回 `{success, msg}` —— 与 WorkBuddy / 小浣熊 / AutoClaw / Qoder / Loomy
/// 的 claim 同一形状（`billing::checkin` 的汇总只认这两个字段）。
pub async fn claim_daily_checkin(
    store: &AccountStore,
    account_id: &str,
) -> Result<Value, GatewayError> {
    let credentials = credentials::snapshot_for(store, account_id)?;
    // 出口跟着账号走（与转发链同源，见 `credentials::account_proxy`）：签到是
    // 一次写请求，用户配的代理同样不该被绕过 —— 否则「转发正常、签到天天失败」。
    let proxy = credentials::account_proxy(store, account_id);

    // 1) 活动奖励（Cookie 链）
    let activity = claim_activity_reward(&credentials, proxy.as_ref()).await;
    // 2) 下载客户端奖励（PAT 链）—— PAT 铸不出来不该拖垮第 1 条
    let download = claim_client_download(store, account_id, proxy.as_ref()).await;

    let mut gained: Vec<String> = Vec::new();
    let mut already = false;
    let mut total = 0i64;

    match activity {
        Ok(points) => {
            if points > 0 {
                gained.push(format!("活动奖励+{points}"));
                total += points;
            } else {
                already = true;
            }
        }
        Err(error) => {
            // 401 = 登录态失效，是要让用户看见的硬错误；其余只记日志继续
            if error.status_code == 401 {
                return Err(error);
            }
            crate::server::logging::log(
                "[Checkin]",
                &format!("⚠️ Coze 活动奖励领取失败：{}", error.message),
            );
        }
    }

    match download {
        Ok(true) => {
            gained.push("下载客户端奖励".to_string());
        }
        Ok(false) => already = true,
        Err(error) => {
            // 下载奖励没领到不影响「今天签过了」这个结论（活动可能已结束）
            crate::server::logging::verbose(
                "[Checkin]",
                &format!("Coze 下载客户端奖励跳过：{}", error.message),
            );
        }
    }

    let msg = if !gained.is_empty() {
        if total > 0 {
            format!("签到完成：{}（共 +{total} 积分）", gained.join("、"))
        } else {
            format!("签到完成：{}", gained.join("、"))
        }
    } else if already {
        "今日积分已领过（上游接口幂等，无新增）".to_string()
    } else {
        "本次没有可领取的签到奖励（活动可能未开始或已结束）".to_string()
    };
    crate::server::logging::log("[Checkin]", &format!("✅ Coze：{msg}"));
    Ok(json!({ "success": true, "msg": msg }))
}

/// `POST incentive/activity/reward`（Cookie）→ 本次领到的积分合计。
///
/// `activity_points` 缺失 / 空数组 = 今天已经领过（上游幂等，不算失败）。
async fn claim_activity_reward(
    credentials: &credentials::CozeCredentials,
    proxy: Option<&ResolvedProxy>,
) -> Result<i64, GatewayError> {
    let url = format!("{WEB_BASE}/api/marketplace/incentive/activity/reward");
    let headers = credentials::cookie_headers(credentials);
    let body = json!({ "activity_types": ACTIVITY_TYPES });
    let value = http::post_json_value(&url, &headers, &body, proxy, Some(CHECKIN_TIMEOUT_MS)).await?;
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let message = value
            .get("msg")
            .or_else(|| value.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("未知错误");
        if code == 700012006 || message.contains("登录") || message.contains("认证") {
            return Err(GatewayError::with_status(
                401,
                format!("Coze 签到失败（code={code}：{message}）——请重新登录该账号"),
            ));
        }
        return Err(GatewayError::with_status(
            502,
            format!("Coze 签到失败（code={code}：{message}）"),
        ));
    }
    let mut total = 0i64;
    if let Some(items) = value
        .get("data")
        .and_then(|data| data.get("activity_points"))
        .and_then(Value::as_array)
    {
        for item in items {
            // 字段是 `point`（不是 `points`）；字符串形态也认
            let point = match item.get("point") {
                Some(Value::String(text)) => text.trim().parse::<i64>().unwrap_or(0),
                Some(other) => other.as_i64().unwrap_or(0),
                None => 0,
            };
            total += point.max(0);
        }
    }
    Ok(total)
}

/// `POST ug/v1/activity/client_download_reward`（PAT）→ 是否本次新领到。
async fn claim_client_download(
    store: &AccountStore,
    account_id: &str,
    proxy: Option<&ResolvedProxy>,
) -> Result<bool, GatewayError> {
    let pat = credentials::ensure_pat(store, account_id, false).await?;
    let headers = credentials::pat_headers(&pat);
    let url = format!("{WEB_BASE}/api/ug/v1/activity/client_download_reward");
    // ⚠️ client_type 必须是字符串 "desktop"（传数字会被上游判非法值）
    let body = json!({
        "activity_id": CLIENT_DOWNLOAD_ACTIVITY,
        "client_type": "desktop",
    });
    let value = http::post_json_value(&url, &headers, &body, proxy, Some(CHECKIN_TIMEOUT_MS)).await?;
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 {
        let message = value
            .get("msg")
            .or_else(|| value.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("未知错误");
        return Err(GatewayError::with_status(
            502,
            format!("Coze 下载客户端奖励失败（code={code}：{message}）"),
        ));
    }
    // `data` 里能看出是否新发（`reward_points` / `points`），读不到就按
    // 「接口返回了成功」当作领到了 —— 幂等重领也回 code:0，这里不假装区分。
    let reward = value
        .get("data")
        .and_then(|data| {
            data.get("reward_points")
                .or_else(|| data.get("points"))
                .or_else(|| data.get("point"))
        })
        .and_then(|value| match value {
            Value::String(text) => text.trim().parse::<i64>().ok(),
            other => other.as_i64(),
        })
        .unwrap_or(0);
    Ok(reward != 0 || value.get("data").is_some())
}
