//! 小浣熊新手任务（onboarding）：首次登录奖励（电脑端 / 手机端）的查询与领取。
//!
//! ── 上游契约（逆向确认 + 实测）────────────────────────────
//! 小浣熊没有 Loomy 那种「任务表 + 逐项上报」的新手任务接口，它的新手福利
//! 是两条**各领一次**的首次登录奖励，电脑端与手机端各有一条专属端点
//! （桌面端链路见 `balance.rs`；手机端从官方安卓包逆向，2026-10 实测通过）：
//!
//! ```text
//! POST {主站}/api/web/desktop/v1/login/points/grant   桌面端身份头
//! POST {主站}/api/web/mobile/v1/login/points/grant    手机端身份头（app-android）
//!   头: Authorization: Bearer <token>（两端同一把，token 无平台绑定）
//!   → {code:0, data:{ granted: true|false, popup?:{ source, points, expire_at } }}
//! ```
//!
//! 官方客户端每次启动 / 回前台都会打它；服务端对**首次**登录的账号发放一笔
//! 积分（名义各 3000，实际数额以响应 `popup.points` 为准，取不到按名义值），
//! 已发放过的账号返回 `granted=false`（幂等，不重复加分）。两条端点互不共享
//! 额度：桌面端早已领过的账号，手机端那条照样能拿满（实测确认）。
//!
//! ── 「已领取」状态从哪来（结算台账）────────────────────────
//! 上游没有「这个账号领没领过首登奖励」的只读判定口（账单 event_name 的取值
//! 没有足量样本，按词匹配不可靠；bills 又慢，不能挂在状态查询上）。因此状态
//! 查询**不打任何上游请求**，唯一的真实性来源是本网关自己的**结算台账**
//! （账号记录的 `onboardingGrants`，见 `StoredAccount::onboarding_grants`）：
//! 领取一步无论 `granted` 真假（两种都代表奖励已落定）都落一条台账，此后查询
//! 恒报「已领取」。代价是台账没落之前（奖励早被官方客户端领掉的老账号、且
//! 本网关从未领取过）状态未知、如实显示「待领取」—— 一次领取（手点，或签到
//! 完成后的自动领取）即收敛为「已领取」，之后不再对已结算任务发任何请求。
//!
//! ── 与每日签到的关系 ────────────────────────────────────────
//! 这两条一次性奖励原先混在 `balance::claim_daily_grant` 的第①步里（当时只有
//! 桌面端），现已拆出来挂到新手任务分组（与 Loomy 同一入口，见 `api::onboarding`
//! 的分派与 `api::checkin_center` 快照的 `extras.onboarding`）；每日签到只保留
//! setting_info 触发 + 账单核对两步。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Map, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;

use super::balance::{
    desktop_client_headers, main_site_url, mobile_client_headers, request_json_ex,
    REQUEST_TIMEOUT_MS,
};
use super::credentials;

/// 首次电脑端登录奖励的任务 key（本网关自定义，只在本链路与台账内流转）
pub const TASK_KEY_DESKTOP: &str = "desktop_login_grant";
/// 首次手机端登录奖励的任务 key
pub const TASK_KEY_MOBILE: &str = "mobile_login_grant";
/// 任务分组（对齐 Loomy 任务面板「初识 …」的三段式）
pub const TASK_GROUP: &str = "初识小浣熊";
/// 奖励的名义分值（上游名义值，仅用于界面展示与台账外没有实测时的兜底；
/// 实际入账以响应 popup.points 与账单为准）
pub const TASK_POINTS: i64 = 3000;
/// 全部任务的分值合计（get_tasks / claim_all 的 `total`）
pub const TOTAL_POINTS: i64 = TASK_POINTS * 2;

/// 一条新手任务 = 一条一次性登录奖励端点。
///
/// `mobile` 只决定请求带哪套客户端身份头（`balance` 的 `*_client_headers`），
/// **不**影响凭证 —— 两端认同一把 Bearer token。
struct Task {
    key: &'static str,
    title: &'static str,
    path: &'static str,
    mobile: bool,
}

/// 任务表（数组顺序即界面展示顺序）
const TASKS: [Task; 2] = [
    Task {
        key: TASK_KEY_DESKTOP,
        title: "首次电脑端登录奖励",
        path: "/api/web/desktop/v1/login/points/grant",
        mobile: false,
    },
    Task {
        key: TASK_KEY_MOBILE,
        title: "首次手机端登录奖励",
        path: "/api/web/mobile/v1/login/points/grant",
        mobile: true,
    },
];

/// 台账里某条任务的结算时刻（0 = 未结算，状态未知）
fn settled_at(record: &Value, key: &str) -> i64 {
    record
        .get("onboardingGrants")
        .and_then(|ledger| ledger.get(key))
        .and_then(Value::as_i64)
        .unwrap_or(0)
}

/// 按台账组装任务行（界面直接渲染的形状，与 `loomy::onboarding::task_rows` 对齐）。
fn task_rows(record: &Value) -> Vec<Value> {
    TASKS
        .iter()
        .map(|task| {
            json!({
                "key": task.key,
                "title": task.title,
                "group": TASK_GROUP,
                "points": TASK_POINTS,
                "done": settled_at(record, task.key) > 0,
            })
        })
        .collect()
}

/// 已结算任务的分值合计（`earned`；与 Loomy 同口径：按任务名义值算）。
fn earned_of(record: &Value) -> i64 {
    TASKS
        .iter()
        .filter(|task| settled_at(record, task.key) > 0)
        .map(|_| TASK_POINTS)
        .sum()
}

/// 未结算的任务数。
fn unclaimed_of(rows: &[Value]) -> usize {
    rows.iter()
        .filter(|row| !row.get("done").and_then(Value::as_bool).unwrap_or(false))
        .count()
}

/// 两条台账里较晚的结算时刻（两条都结算时才是有效值；未结算用 0）。
fn settled_at_of(record: &Value) -> i64 {
    TASKS
        .iter()
        .map(|task| settled_at(record, task.key))
        .max()
        .unwrap_or(0)
}

/// 签到时中心快照用：两条都结算过就给一份「已结清」的记忆（零上游），否则 None
/// —— 与 Loomy / CodeArts 的 `onboardingSettled` 同形（见 `providers::onboarding_memory`），
/// 快照消费方不必按 provider 分叉。本家没有单独的结算字段：两条台账都落下即结清，
/// 结算时刻取两条里较晚的那个。
pub fn settled_view(record: Option<&Value>) -> Option<Value> {
    let record = record?;
    let rows = task_rows(record);
    if unclaimed_of(&rows) > 0 {
        return None;
    }
    Some(json!({
        "at": settled_at_of(record),
        "tasks": rows,
        "earned": earned_of(record),
        "total": TOTAL_POINTS,
    }))
}

/// 打一次 grant 探测（幂等），返回上游 `data` 对象。
///
/// 请求失败在这里就是失败（返回 Err），不降级 —— 调用方据此向界面如实报错。
/// 401（登录态失效）由调用方截住整体失败，与 Loomy 的领取同一处理。
async fn probe_grant(
    store: &AccountStore,
    account_id: &str,
    task: &Task,
) -> Result<Value, GatewayError> {
    let credentials = credentials::snapshot_for(store, account_id)?;
    if credentials.token.trim().is_empty() {
        return Err(GatewayError::with_status(
            401,
            "该账号没有可用凭证，无法领取新手任务",
        ));
    }
    let headers = if task.mobile {
        mobile_client_headers()
    } else {
        desktop_client_headers()
    };
    request_json_ex(
        &main_site_url(),
        "POST",
        task.path,
        &credentials.token,
        &headers,
        REQUEST_TIMEOUT_MS,
    )
    .await
}

/// 探测结果 →（是否本轮刚发放，本轮入账分值）。
///
/// 实际数额以响应 `popup.points` 为准（手机端实测带该字段），取不到回落名义值
/// —— `granted=false` 时本轮没有入账，分值记 0。
fn granted_outcome(data: &Value) -> (bool, i64) {
    let granted = data.get("granted").and_then(Value::as_bool) == Some(true);
    let points = if granted {
        data.pointer("/popup/points")
            .and_then(Value::as_i64)
            .filter(|points| *points > 0)
            .unwrap_or(TASK_POINTS)
    } else {
        0
    };
    (granted, points)
}

/// 查询任务状态（`GET /api/accounts/{id}/onboarding` 的小浣熊执行体，只读：
/// 不发任何上游请求）。
///
/// 响应形状与 Loomy 对齐：`{tasks:[…], earned, total, unclaimed, settled}`。`done`
/// 来自结算台账（见模块头）：结算过恒「已领取」，没结算过恒「待领取」—— 状态未知
/// 就说未知，不假装知道上游的真实发放状态。
///
/// `refresh` 在本家**没有实查可做**：上游没有「领没领过」的只读判定口（见模块头），
/// 台账就是唯一真实性来源。参数只为接口形状与另外两家一致（`?refresh=1` 时结论
/// 相同），调用方不必为本家单独分叉。
pub async fn get_tasks(
    store: &AccountStore,
    account_id: &str,
    _refresh: bool,
) -> Result<Value, GatewayError> {
    if account_id.is_empty() {
        return Err(GatewayError::with_status(400, "缺少账号 id"));
    }
    let record = store
        .raccoon_account_record(account_id)
        .ok_or_else(|| GatewayError::with_status(404, "未找到小浣熊账号"))?;
    let rows = task_rows(&record);
    let unclaimed = unclaimed_of(&rows);
    Ok(json!({
        "tasks": rows,
        "earned": earned_of(&record),
        "total": TOTAL_POINTS,
        "unclaimed": unclaimed,
        // 两条都结算过 ⇒ 一次性福利到此为止（界面据此不再重复查询）。
        // `settledAt` 与 Loomy / CodeArts 的记忆路径同义（结算时刻），界面展示
        // 「结算于」时不必按 provider 分叉。
        "settled": unclaimed == 0,
        // 结算时刻：`settled` 为真时给两条台账里较晚的那个（与 Loomy / CodeArts
        // 记忆路径的 `settledAt` 同义），否则 null
        "settledAt": if unclaimed == 0 { json!(settled_at_of(&record)) } else { Value::Null },
    }))
}

/// 领取（`POST /api/accounts/{id}/onboarding/claim` 的小浣熊执行体）。
///
/// 只对**未结算**的任务逐条探测（串行，与签到同一条防风控口径）；台账里已结算
/// 的不发请求也不进 `results`（与 Loomy「已完成的 key 不发请求」同一行为）。
/// 探测成功（无论 `granted` 真假）都落结算台账 —— 这一步就是「已领取」标记的
/// 写入点。响应形状与 `loomy::onboarding::claim_all` 对齐（results / claimed /
/// failed / claimedPoints / tasks / earned / total / unclaimed）。
pub async fn claim_all(store: &AccountStore, account_id: &str) -> Result<Value, GatewayError> {
    if account_id.is_empty() {
        return Err(GatewayError::with_status(400, "缺少账号 id"));
    }
    let record = store
        .raccoon_account_record(account_id)
        .ok_or_else(|| GatewayError::with_status(404, "未找到小浣熊账号"))?;

    let now = crate::server::logging::now_ms();
    let mut rows: Vec<Value> = Vec::new();
    let mut claimed = 0_i64;
    let mut failed = 0_i64;
    let mut claimed_points = 0_i64;
    // 结算台账的本次增量：探测成功才写（失败的任务保持未结算，可重试）
    let mut settled: Map<String, Value> = Map::new();

    for task in TASKS.iter() {
        if settled_at(&record, task.key) > 0 {
            continue;
        }
        match probe_grant(store, account_id, task).await {
            Ok(data) => {
                let (granted, points) = granted_outcome(&data);
                claimed += 1;
                claimed_points += points;
                settled.insert(task.key.to_string(), Value::from(now));
                rows.push(json!({
                    "key": task.key,
                    "ok": true,
                    "already": !granted,
                    "points": points,
                }));
                if granted {
                    crate::server::logging::log(
                        "[Onboarding]",
                        &format!("小浣熊账号 {account_id}：「{}」已发放（+{points}）", task.title),
                    );
                } else {
                    crate::server::logging::log(
                        "[Onboarding]",
                        &format!("小浣熊账号 {account_id}：「{}」此前已发放，跳过重复加分", task.title),
                    );
                }
            }
            Err(error) => {
                if error.status_code == 401 {
                    return Err(error);
                }
                failed += 1;
                crate::server::logging::verbose(
                    "[Onboarding]",
                    &format!("小浣熊账号 {account_id}：「{}」领取失败: {}", task.title, error.message),
                );
                rows.push(json!({
                    "key": task.key,
                    "ok": false,
                    "error": error.message,
                }));
            }
        }
    }

    if !settled.is_empty() {
        let saved = store.mark_onboarding_grant_batch(account_id, &settled);
        if !saved {
            crate::server::logging::log(
                "[Onboarding]",
                &format!("⚠️ 小浣熊账号 {account_id}：结算台账落库失败（本次领取不受影响，下次会重复探测一次）"),
            );
        }
    }

    // 响应里的任务行 = 台账旧状态 ∪ 本次增量（落库失败也按已结算播报 ——
    // 上游已经结清，界面不该再显示「待领取」引着用户去点一颗注定幂等的按钮）
    let mut merged = record.clone();
    if let Some(ledger) = merged.get_mut("onboardingGrants").and_then(Value::as_object_mut) {
        for (key, at) in settled.iter() {
            ledger.insert(key.clone(), at.clone());
        }
    } else if !settled.is_empty() {
        merged["onboardingGrants"] = Value::Object(settled.clone());
    }
    let rows_all = task_rows(&merged);
    let unclaimed = rows_all
        .iter()
        .filter(|row| !row.get("done").and_then(Value::as_bool).unwrap_or(false))
        .count();

    Ok(json!({
        "results": rows,
        "claimed": claimed,
        "failed": failed,
        "claimedPoints": claimed_points,
        "tasks": rows_all,
        "earned": earned_of(&merged),
        "total": TOTAL_POINTS,
        "unclaimed": unclaimed,
        "settled": unclaimed == 0,
        "settledAt": if unclaimed == 0 { json!(settled_at_of(&merged)) } else { Value::Null },
    }))
}
