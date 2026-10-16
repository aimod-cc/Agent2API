//! AutoClaw 的**活动福利到账确认**（Weekly Perks / 登录奖励这类「活跃即发」活动）。
//!
//! ── 上游是什么（从客户端产物 + 实测确认，2026-10-13 / 10-15）──────
//! 这类活动**没有领取接口**：服务端在用户「登录活跃」时自动入账。实测证据：
//!
//!   ```text
//!   2026-10-13 16:18:46  桌面端打开 → token 刷新（JWT iat 同时刻）
//!   2026-10-13 16:18:48  流水 +800「AutoClaw Weekly Perks +800 credits」
//!   2026-10-15 22:04:27  user-info 打下去（登录）
//!   2026-10-15 22:04:28  +20000「AutoClaw 用户登录奖励积分，7天有效」
//!                        +500  （同日、同 event_type，国际版是 800）
//!   ```
//!
//! 2026-10-15 逐条测过：签到本身、`refresh` 的 force 刷新、home-fetch 都**不**
//! 触发发放，只有 **`POST /userapi/v1/user-info`**（客户端启动的固定动作）会 ——
//! 服务端把「登录」的判据挂在它上面，入账流水带 `event_type: autoclaw_user_active`、
//! `grant_date`（北京日期）与 `source: campaign`。触发动作在 `checkin.rs` 的
//! `trigger_login_active`（见那边模块头）；本模块只管读积分流水回答
//! 「今天的活动福利到账了吗」。
//!
//! ── 判据：event_type 为主、activity_id 前缀兜底 ─────────────
//! 10-15 实测的两笔：20000 的 `activity_id` 是 `autoclaw_user_active_bonus_20261015`
//! （一次性大礼包），500/800 的是 `autoclaw_user_active_reward_202609`（日福利）。
//! 同一活动期内**两笔前缀不同** —— 只按单一前缀匹配会漏掉 20000 那笔。因此判据
//! 以 `metadata.event_type == "autoclaw_user_active"` 为主（两笔都带），前缀列表
//! 只作 event_type 缺失时的兜底（活动换期换后缀 / 换家族也不受影响）。
//!
//! ── 一天可能多笔：全部收回、给出合计与明细 ────────────────────
//! `granted: true` 时 `points` 是**当日全部活动入账的合计**（如 20000 + 500 =
//! 20500），`entries` 逐笔给出 amount / description / activity_id —— 签到消息用
//! 合计（「…（活动福利 +20500 已到账）」），需要拆分的调用方读 `entries`。
//!
//! ── 查询失败不影响签到结论 ──────────────────────────────────
//! 与 `balance.rs` 里「订阅失败不影响积分结果」同一取向：活动福利是附加
//! 信息，查不到（网络抖 / 活动结束）就给 `null`，绝不把它升级成签到失败。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：零 unwrap/expect/panic（取首元素用 `first()`）。

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::beijing;
use crate::server::errors::GatewayError;

use super::balance::userapi_get;
use super::credentials;
use super::region::Region;

/// 积分流水路径（只取**入账**方向：模型消费的 debit 不会挤占窗口）。
///
/// `flow_direction` 的合法值是 `income`（实测：`credit` / `in` 都是 400001）。
/// `page_size` 取 50：活动期一天可能多笔（签到 / 登录奖励 / 任务），给「今天」
/// 留足回溯余量。
const LEDGERS_PATH: &str = "/agent-assetmgr/api/v1/ledgers_std?asset_type=point&wallet_type=all&cursor=&page_size=50&flow_direction=income";

/// 「活跃即发」活动的流水事件名前缀（`metadata.event_type`）。
///
/// 2026-10-15 实测：登录奖励的每一笔（一次性大礼包与日福利）都带
/// `autoclaw_user_active`，是比 activity_id 前缀更稳的判据 —— 活动换期会换
/// activity_id 后缀，甚至换前缀家族（见模块头）。
/// `autoclaw_user_signup_day`：登录动作还会带出**新手期**的「注册 Day 1-7 奖励」
/// （event_type 形如 `autoclaw_user_signup_day_1`，实测 2026-09-22 国际版
/// +10000「New-user credits (Day 1/7)」）—— 同一个发放钩子，一并确认。
const ACTIVE_EVENT_TYPE_PREFIXES: [&str; 2] =
    ["autoclaw_user_active", "autoclaw_user_signup_day"];

/// activity_id 前缀兜底（event_type 缺失的老数据 / 变体）。
///
/// `autoclaw_user_active_reward`：日福利（Weekly Perks，如 202609 期）；
/// `autoclaw_user_active_bonus`：一次性大礼包（如 20261015 那次 20000）；
/// `autoclaw_new_user_day_1_to_7_bonus`：新手 7 天登录礼。
const ACTIVE_ACTIVITY_PREFIXES: [&str; 3] = [
    "autoclaw_user_active_reward",
    "autoclaw_user_active_bonus",
    "autoclaw_new_user_day_1_to_7_bonus",
];

/// 今天的活动福利到账状态。
///
/// 返回形状（`promotionReward` 字段的值）：
///   - 到账：`{"granted": true, "points": 20500, "description": "...",
///     "grantDate": "YYYY-MM-DD", "entries": [{"points": 20000, ...}, ...]}`
///     —— `points` 是当日全部活动入账的**合计**，`entries` 是逐笔明细；
///   - 未到账：`{"granted": false, "grantDate": "YYYY-MM-DD"}`。
///
/// 判据是**流水里存在今天的**活动入账：`metadata.grant_date` 等于北京今天的
/// 日期键（`core::beijing`，与签到链路同一个「今天」口径），且 `event_type`
/// 命中 [`ACTIVE_EVENT_TYPE_PREFIXES`]（缺失时退回 activity_id 前缀匹配，见模块头）。
pub async fn today_reward(
    region: Region,
    store: &AccountStore,
    account_id: &str,
) -> Result<Value, GatewayError> {
    let record = store.autoclaw_account_record(region, account_id);
    if !account_id.is_empty() && record.is_none() {
        return Err(GatewayError::with_status(
            404,
            format!(
                "AutoClaw {}账号 {account_id} 不存在或不属于该地区",
                region.label()
            ),
        ));
    }
    let credentials = credentials::snapshot_for(record.as_ref(), region)?;
    if credentials.token.trim().is_empty() {
        return Err(GatewayError::with_status(401, "该账号没有可用凭证，无法查询活动福利"));
    }

    let payload = userapi_get(&credentials, LEDGERS_PATH, "活动福利查询").await?;
    let today = beijing::today_key();

    let entries: Vec<&Value> = payload
        .get("data")
        .and_then(|data| data.get("entries"))
        .and_then(Value::as_array)
        .map(|entries| entries.iter().filter(|entry| is_today_reward(entry, &today)).collect())
        .unwrap_or_default();

    let Some(first) = entries.first() else {
        return Ok(json!({ "granted": false, "grantDate": today }));
    };

    let total: i64 = entries
        .iter()
        .filter_map(|entry| entry.get("amount").and_then(Value::as_i64))
        .sum();
    let detail: Vec<Value> = entries.iter().map(|entry| entry_detail(entry)).collect();

    Ok(json!({
        "granted": true,
        "points": total,
        "description": first.get("description").and_then(Value::as_str).unwrap_or(""),
        "grantDate": today,
        "entries": detail,
    }))
}

/// 一笔活动入账的明细（`entries` 数组的元素）。
fn entry_detail(entry: &Value) -> Value {
    let metadata = entry.get("metadata");
    json!({
        "points": entry.get("amount").and_then(Value::as_i64).unwrap_or(0),
        "description": entry.get("description").and_then(Value::as_str).unwrap_or(""),
        "activityId": metadata
            .and_then(|metadata| metadata.get("activity_id"))
            .and_then(Value::as_str)
            .unwrap_or(""),
    })
}

/// 这条流水是不是「今天的活动福利」。
///
/// 判据见模块头：`grant_date` 必须是今天，且 `event_type` 命中前缀
/// （activity_id 前缀兜底）。
fn is_today_reward(entry: &Value, today: &str) -> bool {
    let Some(metadata) = entry.get("metadata") else {
        return false;
    };
    let grant_date = metadata.get("grant_date").and_then(Value::as_str).unwrap_or("");
    if grant_date != today {
        return false;
    }
    let event_type = metadata.get("event_type").and_then(Value::as_str).unwrap_or("");
    if ACTIVE_EVENT_TYPE_PREFIXES.iter().any(|prefix| event_type.starts_with(prefix)) {
        return true;
    }
    let activity_id = metadata.get("activity_id").and_then(Value::as_str).unwrap_or("");
    ACTIVE_ACTIVITY_PREFIXES
        .iter()
        .any(|prefix| activity_id.starts_with(prefix))
}
