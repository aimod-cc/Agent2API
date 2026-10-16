//! AutoClaw 的每日签到（移植来源：AutoClaw 桌面端的「每日签到」Banner 链路）。
//!
//! ── 上游是什么（无参考项目可抄，从客户端产物里挖出来的）──────────
//! 老项目 `D:\APP\AutoClaw\autoclaw-local-proxy` **没有签到**（它的全部接口只有
//! 账号管理、积分钱包、订阅查询与模型转发），所以这一份不是移植而是**逆向**：
//! 接口从安装包 `D:\Program Files\AutoClaw\resources\app.asar` 里读出并实测确认。
//!
//! 桌面端首页侧栏有一个「每日签到」Banner，点击后走通用任务接口：
//!
//! ```text
//! POST {userapi}/autoclaw-proxy/proxy/autoclaw-task-complete
//! body: {"task_id":"daily_signin"}
//! → {"data":{"already_completed":true,"reward_points":0,"success":false,
//!            "task_id":"daily_signin"}}
//! ```
//!
//! 三个要点：
//!   1. **不是签到专用接口** —— `autoclaw-task-complete` 是「完成客户端任务」的
//!      通用入口，`daily_signin` 只是其中一个 task_id（同级的还有
//!      `daily_inspiration_center` / `upgrade_pc_app`）。所以请求体只有一个字段。
//!   2. **HTTP 200 + body 里的成功标记**：实测（2026-09-19）当天已签到时返回
//!      HTTP 200、`success: false`、`already_completed: true`、`reward_points: 0`。
//!      判「签到成功」要看 `reward_points > 0`（或 `success === true`），
//!      **不能只看 HTTP 状态码** —— 那会把「今天已领过」当成一次成功领取。
//!   3. 上游是**按天幂等**的：重复调用不会重复入账，返回「已领取」。
//!
//! ── 为什么复用积分与订阅那套请求封装 ─────────────────────────
//! 同一个 userapi 域、同一套 `X-Auth-Sign` 签名头（`refresh::signed_auth_headers`）、
//! 同一份超时口径。因此这里直接用 `balance.rs` 里那两个 `userapi_post` /
//! `userapi_get` 的对等实现 —— 但它们目前是 `balance.rs` 的私有函数。
//! **不复制**（复制会让 appId/appKey、时间戳单位与品牌头分叉，而签名一旦分叉
//! 就是稳定 400002），改为把它们提升为 `pub(super)` 供本模块调用。
//!
//! ── claim 形状与其他两家对齐 ───────────────────────────────
//! `billing::checkin` 的汇总只认 `{success, msg}` 两个字段（见那里的 `succeeded`
//! 统计）。本模块返回同一形状，于是工作流/日志/界面三处的口径与 WorkBuddy、
//! 小浣熊完全一致，不需要为第三家再加分支。
//!
//! ── 活动福利（Weekly Perks / 登录奖励等「活跃即发」活动，2026-10 加）──
//! 这类活动没有领取接口、由服务端在账号**登录活跃**时自动入账。签到链的做法：
//! 签到完成后读一次积分流水，**今天还没到账**就顺带打一次「登录」接口
//! （`POST /userapi/v1/user-info`，客户端每次启动都会打的那个），复核之后把
//! 状态放进 `promotionReward` 字段（读侧实现见 `promotion` 模块头）。
//!
//! ── 为什么触发动作是 user-info（2026-10-15 实测，别改回刷新 token）──
//! M10W3 活动（10.15-10.21 一次性 20000；10.15-10.25 每日 500/800）当天逐条
//! 测过：签到本身、`refresh` 的 force 刷新（含带调高的 X-Version）、home-fetch
//! 都**不触发**发放；只有 user-info 打下去，20000 + 500 立刻入账（国际版是
//! 800，没有 20000 档）。user-info 是客户端启动时的固定动作，服务端把「登录」
//! 的判据挂在它上面（流水元数据 `event_type: autoclaw_user_active`）。
//! 版本无关（X-Version 1.18.5 照发）、幂等（重复调用不重复入账）。
//!
//! user-info 只需要 access token（不需要 refreshToken），也就没有 token 轮换
//! 这类副作用 —— 比原来的「强制刷新」触发更干净、覆盖面更大。失败只记日志、
//! 不阻断签到（活动福利是附加收益，见 `attach_today_reward` 的兜底说明）。
//!
//! 已到账时**不触发**（用户当天开过客户端就已经拿到，这里的调用次数零浪费）。
//!
//! ── 为什么带一个只读的任务列表调用 ─────────────────────────
//! `GET autoclaw-task-list` 能拿到所有任务的完成状态。它在**签到失败时**提供
//! 归因：拿不到列表说明登录态有问题，能拿到而 `daily_signin` 已完成，则说明
//! 是「今天已签到」而不是「接口坏了」。因此只在需要解释失败时才拉它，
//! 正常路径不多花一次往返。

use std::time::Duration;

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::balance::{userapi_get, userapi_post};
use super::credentials;
use super::promotion;
use super::region::Region;

/// 每日签到任务的 id（源实现 `DAILY_SIGNIN_TASK_ID`）
const DAILY_SIGNIN_TASK_ID: &str = "daily_signin";

/// 触发「登录活跃」后、复核活动福利到账前等的一拍。
///
/// 实测（2026-10-15）：user-info 打下去 → 服务端入账（流水 occurred_at）在同一
/// 秒内（20000 + 500 两笔同秒）。签到本身耗时通常已覆盖一部分等待，所以这里
/// 只补 1.5 秒再复核 —— 复核不到也不影响结论（下一次签到/查询会看到，活动福利
/// 按天入账、当天都在流水里）。
const PROMOTION_SETTLE_MS: u64 = 1500;

/// 签到接口路径（源实现 `completeClientTask`）
const TASK_COMPLETE_PATH: &str = "/autoclaw-proxy/proxy/autoclaw-task-complete";

/// 任务列表路径（源实现 `getTaskList`；`?lang=` 可选，省略时上游给默认语言）
const TASK_LIST_PATH: &str = "/autoclaw-proxy/proxy/autoclaw-task-list";

/// 「登录」接口路径（客户端启动同款：`withWebInfo` 的 user-info）。
///
/// 活动福利的发放钩子挂在它上面（见模块头「为什么触发动作是 user-info」）。
const USER_INFO_PATH: &str = "/userapi/v1/user-info";

/// AutoClaw 账号的每日签到（**含活动福利触发与附加**）。
///
/// 返回 `{success, msg, ...}` —— `success` 的口径是「**本次真的领到了积分**」，
/// 而不是「HTTP 通了」。今天已领过时 `success: false` 且 msg 说明原因；
/// 前端把这条当 warn 提示显示（与 WorkBuddy 的「今天已签到」同一处理）。
///
/// `region` 决定账号在哪一家的记录里找、请求打哪个 userapi 域：两地的任务体系
/// 各自独立（同一套接口路径、两个站点），不能跨地区查。
///
/// ── 执行顺序（活动福利见模块头）──────────────────────────────
///   1. **签到先行**：task-complete 的成败就是这条结果行的成败（账号不存在 /
///      凭证失效等在这里直接返回错误，不做多余的查询与触发）；
///   2. 读一次积分流水，判断今天的活动福利是否已到账（只读）；
///   3. 未到账 → 打一次「登录」接口（user-info）—— **已到账不触发**，
///      用户当天开过客户端时这里零副作用；签到耗时已充当服务端发放延迟的
///      一部分，触发后再等一拍复核一次。
pub async fn claim_daily_signin(
    region: Region,
    store: &AccountStore,
    account_id: &str,
) -> Result<Value, GatewayError> {
    let claim = run_daily_signin(region, store, account_id).await?;

    let mut reward = promotion::today_reward(region, store, account_id)
        .await
        .unwrap_or(Value::Null);
    let granted = reward.get("granted").and_then(Value::as_bool) == Some(true);

    if !granted && trigger_login_active(region, store, account_id).await {
        tokio::time::sleep(Duration::from_millis(PROMOTION_SETTLE_MS)).await;
        if let Ok(retry) = promotion::today_reward(region, store, account_id).await {
            reward = retry;
        }
    }
    Ok(attach_today_reward(claim, reward))
}

/// 触发一次「登录活跃」—— 打 user-info（客户端启动同款「登录」动作），
/// 让服务端把今天的活动福利发出来。
///
/// ── 为什么是 user-info 而不是刷新 token ─────────────────────
/// 见模块头「为什么触发动作是 user-info」：它是服务端「登录即发」的钩子
/// （2026-10-15 实测：只有它能让 20000 + 500 立刻入账，refresh / home-fetch
/// 都不行），且只带 access token、不涉及 token 轮换 —— 比强制刷新更干净。
///
/// ── 返回值与容错 ────────────────────────────────────────────
/// 返回是否**成功**打出了这次登录（调用方据此决定要不要多等一拍复核）。
/// 失败（没有可用凭证 / 网络问题 / 登录态失效）只记日志并返回 false ——
/// 活动福利是附加收益，绝不因它把一次真签到判成失败。
async fn trigger_login_active(region: Region, store: &AccountStore, account_id: &str) -> bool {
    let record = store.autoclaw_account_record(region, account_id);
    // 与签到同一判据：非空 id 必须命中记录，否则不该拿桌面端登录态去打
    if !account_id.is_empty() && record.is_none() {
        return false;
    }
    let Ok(credentials) = credentials::snapshot_for(record.as_ref(), region) else {
        return false;
    };
    if credentials.token.trim().is_empty() {
        logging::verbose(
            "[AutoClaw]",
            &format!("账号 {account_id} 没有可用凭证，跳过活动福利触发"),
        );
        return false;
    }
    // body 与客户端 `withWebInfo` 一致：source_id + device_id（device_id 可能为空串）
    let body = json!({ "source_id": "autoclaw", "device_id": credentials.device_id });
    match userapi_post(&credentials, USER_INFO_PATH, &body, "登录").await {
        Ok(_) => {
            logging::verbose(
                "[AutoClaw]",
                &format!("账号 {account_id} 已触发登录（user-info），复核活动福利"),
            );
            true
        }
        Err(error) => {
            logging::verbose(
                "[AutoClaw]",
                &format!("账号 {account_id} 活动福利触发失败: {}", error.message),
            );
            false
        }
    }
}

/// 签到任务本身（原主体；活动福利的触发与附加在外面那层包装里）。
async fn run_daily_signin(
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
    // 与积分查询同一条取凭证链（账号记录 → 桌面端实时登录态 → 环境变量）。
    let credentials = credentials::snapshot_for(record.as_ref(), region)?;
    if credentials.token.trim().is_empty() {
        return Err(GatewayError::with_status(
            401,
            "该账号没有可用凭证，无法签到",
        ));
    }

    let body = json!({ "task_id": DAILY_SIGNIN_TASK_ID });
    let payload = userapi_post(&credentials, TASK_COMPLETE_PATH, &body, "签到").await?;

    // 业务码判定：签到走的是通用任务接口，登录态失效同样以业务码表达
    // （与积分查询同一组 AUTH_EXPIRED_CODES，见 balance.rs）。
    if let Some(code) = payload.get("code").and_then(Value::as_i64) {
        if super::balance::is_auth_expired_code(code) {
            return Err(GatewayError::with_status(
                401,
                "登录态已过期，无法签到",
            ));
        }
        if code != 0 {
            let message = payload
                .get("msg")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .unwrap_or("签到失败");
            return Ok(json!({ "success": false, "msg": message, "code": code }));
        }
    }

    let data = payload.get("data").cloned().unwrap_or(Value::Null);
    let already_completed = data
        .get("already_completed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let reported_success = data.get("success").and_then(Value::as_bool).unwrap_or(false);
    let reward_points = data.get("reward_points").and_then(Value::as_i64).unwrap_or(0);

    // 「本次领到了」的判据是 reward_points > 0 或显式 success。**已领过不算成功**：
    // 上游对重复调用返回 success:false + already_completed:true，把它算成成功会让
    // 「今天已签到」在界面上显示成「签到成功」，与实际入账不符。
    let claimed = reward_points > 0 || (reported_success && !already_completed);
    if claimed {
        return Ok(json!({
            "success": true,
            "msg": if reward_points > 0 {
                format!("签到成功，获得 {reward_points} 积分")
            } else {
                "签到成功".to_string()
            },
            "rewardPoints": reward_points,
            "alreadyCompleted": false,
        }));
    }

    // 未领到：区分「今天已领过」与「上游异常」，前者是正常的幂等结果
    let msg = if already_completed {
        "今天已签到".to_string()
    } else {
        let task_hint = describe_task_state(&credentials).await;
        match task_hint {
            Some(hint) => format!("签到未领取（{hint}）"),
            None => "签到未领取".to_string(),
        }
    };
    Ok(json!({
        "success": false,
        "msg": msg,
        "rewardPoints": reward_points,
        "alreadyCompleted": already_completed,
    }))
}

/// 签到未领取时，拉一次任务列表给出归因（**只读**，且失败不升级为错误）。
///
/// 返回 None 表示「列表里没有可用信息」——调用方据此退回通用文案，
/// 不把一次辅助查询的失败拼接进用户看到的消息里。
async fn describe_task_state(
    credentials: &credentials::AutoClawCredentials,
) -> Option<String> {
    let payload = userapi_get(credentials, TASK_LIST_PATH, "任务列表查询").await.ok()?;
    if payload.get("code").and_then(Value::as_i64).unwrap_or(0) != 0 {
        return None;
    }
    let tasks = payload.get("data").and_then(Value::as_array)?;
    let task = tasks
        .iter()
        .find(|item| item.get("task_id").and_then(Value::as_str) == Some(DAILY_SIGNIN_TASK_ID))?;
    let completed = task
        .get("completed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if completed {
        Some("任务列表显示今天已完成".to_string())
    } else {
        None
    }
}

/// 把今日活动福利状态附加到签到结果上（`reward` 由调用方查明）。
///
/// ── 为什么查询失败只给 null、不升级为错误 ─────────────────────
/// 活动福利是签到结果之外的**附加信息**（来源不同：签到打 task-complete，
/// 福利读积分流水）。把它做成「查不到就签到失败」会让一次网络抖动否决一次
/// 真实签到 —— 与 `balance.rs` 给「订阅失败不影响积分结果」的兜底同一取向。
///
/// 已到账时把收益拼进 `msg`（「…（活动福利 +20500 已到账）」，`points` 是当日
/// 全部活动入账的**合计**，逐笔明细在 `promotionReward.entries` 里）：签到记录
/// 台账与面板展示读的都是这一条 msg，用户在一个地方就能看清「今天到底拿到了
/// 什么」。未到账**不改 msg** —— 活动有期限，结束后每天提示「未到账」只会变成
/// 噪音；状态仍在 `promotionReward` 字段里，给需要的地方读。
fn attach_today_reward(mut claim: Value, reward: Value) -> Value {
    let granted = reward.get("granted").and_then(Value::as_bool).unwrap_or(false);
    let points = reward.get("points").and_then(Value::as_i64).unwrap_or(0);
    if let Some(object) = claim.as_object_mut() {
        if granted && points > 0 {
            let base = object.get("msg").and_then(Value::as_str).unwrap_or("").to_string();
            if !base.is_empty() {
                object.insert(
                    "msg".to_string(),
                    json!(format!("{base}（活动福利 +{points} 已到账）")),
                );
            }
        }
        object.insert("promotionReward".to_string(), reward);
    }
    claim
}
