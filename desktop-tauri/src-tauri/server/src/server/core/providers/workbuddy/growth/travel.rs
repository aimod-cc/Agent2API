//! WorkBuddy **猫猫旅行**（growth 域的 buddy 子系统；从参考项目
//! `internal/upstream/travel.go` + `internal/scheduler/travel.go` 移植）。
//!
//! ── 这是什么 ────────────────────────────────────────────────
//! 上游的养成玩法：账号领养一只猫（Buddy），每天把猫派出去旅行一次，
//! 到站后领奖（积分）。状态机很小：
//!
//! ```text
//! 无猫          → 同意协议 + 领养（buddy/first，+300c+8e；门槛未达时预期 400）
//! idle          → 派出（每日 1 次；已达当日上限则跳过）
//! traveling     → 在途，跳过（等下一趟）
//! arrived       → 领奖（带 record_id；reward_credit 入账）
//! ```
//! 参考项目把它挂成每日 09:00 的排程；本仓同样支持**定时排程**
//! （见 [`run_for_accounts`]，由 `core::growth_scheduler` 调用）与界面上的
//! 「巡检一次」（手动触发）。一趟只推进一步，不轮询 —— 定时排程一天跑两趟
//! （默认 09:00 / 21:00）就足够覆盖「早上派出、晚上领奖」的闭环。
//!
//! ── 跳过与失败为什么必须分开 ────────────────────────────────
//! 跳过是**预期**的正常态（在途、已达当日上限、领养门槛未达、防抖挡下），
//! 失败是上游真的报错。混在一起会让界面天天显示「失败」，用户对真正的
//! 故障就麻木了（参考项目 ledger.go 的同一取舍）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use super::{GrowthAccount, chat_call};

/// 派出地点固定 4（古镇客栈）：4 个地点的收益/时长区间完全相同，无最优解。
const TRAVEL_LOCATION_ID: i64 = 4;

/// 领养门槛未达的业务错误关键词（HTTP 400 时出现，属预期正常态）。
const BUDDY_TASK_INCOMPLETE_MARKER: &str = "first_buddy task not completed yet";

/// 猫猫旅行状态。
#[derive(Clone, Debug, Default)]
pub struct TravelState {
    /// idle / traveling / arrived / 未知
    pub state: String,
    /// 今日已派出过（自然日 00:00 CST 重置）
    pub daily_limit_reached: bool,
    /// 在途/到站记录 id（claim 必带）
    pub record_id: i64,
    /// 到站可领奖励积分
    pub reward_credit: i64,
}

impl TravelState {
    fn from_value(data: &Value) -> Self {
        Self {
            state: data
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            daily_limit_reached: data
                .get("daily_limit_reached")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            record_id: data.get("record_id").and_then(Value::as_i64).unwrap_or(0),
            reward_credit: data.get("reward_credit").and_then(Value::as_i64).unwrap_or(0),
        }
    }
}

/// 猫档案（id + 名字）。
pub struct Buddy {
    pub id: i64,
    pub name: String,
}

/// 查当前猫档案：`Ok(None)` = 无猫（data.buddy 为 null / 缺失）。
pub async fn buddy_info(account: &GrowthAccount) -> Result<Option<Buddy>, String> {
    let call = chat_call(account, "GET", "/activity/growth/buddy/info", None, &[]).await?;
    if !call.ok() {
        return Err(format!("猫档案查询失败（HTTP {} code={:?}）", call.status, call.code()));
    }
    let buddy = call.data().get("buddy").cloned().unwrap_or(Value::Null);
    if buddy.is_null() {
        return Ok(None);
    }
    // 空对象也按「无猫」处理（上游的 null / {} 两种形态都出现过）
    if let Some(object) = buddy.as_object() {
        if object.is_empty() {
            return Ok(None);
        }
    }
    Ok(Some(Buddy {
        id: buddy.get("id").and_then(Value::as_i64).unwrap_or(0),
        name: buddy
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    }))
}

/// 查旅行状态（GET /activity/growth/buddy/travel/status）。
pub async fn travel_status(account: &GrowthAccount) -> Result<TravelState, String> {
    let call = chat_call(account, "GET", "/activity/growth/buddy/travel/status", None, &[]).await?;
    if !call.ok() {
        return Err(format!("旅行状态查询失败（HTTP {} code={:?}）", call.status, call.code()));
    }
    Ok(TravelState::from_value(call.data()))
}

/// 派出（POST /activity/growth/buddy/travel/depart，地点固定 4）。
pub async fn travel_depart(account: &GrowthAccount) -> Result<(), String> {
    let call = chat_call(
        account,
        "POST",
        "/activity/growth/buddy/travel/depart",
        Some(&json!({ "location_id": TRAVEL_LOCATION_ID })),
        &[],
    )
    .await?;
    if call.ok() {
        Ok(())
    } else {
        Err(format!("派出失败（HTTP {} code={:?} {}）", call.status, call.code(), call.msg()))
    }
}

/// 领奖（POST /activity/growth/buddy/travel/claim，带 record_id）→ reward_credit。
pub async fn travel_claim(account: &GrowthAccount, record_id: i64) -> Result<i64, String> {
    let call = chat_call(
        account,
        "POST",
        "/activity/growth/buddy/travel/claim",
        Some(&json!({ "record_id": record_id })),
        &[],
    )
    .await?;
    if !call.ok() {
        return Err(format!("领奖失败（HTTP {} code={:?} {}）", call.status, call.code(), call.msg()));
    }
    Ok(call.data().get("reward_credit").and_then(Value::as_i64).unwrap_or(0))
}

/// 领养（同意协议 → buddy/first；见 growth::adopt_buddy）。这里只做「门槛未达」
/// 的识别包装 —— 领养实现是单一事实源，在 `growth` 主模块。
async fn adopt(account: &GrowthAccount) -> (bool, bool, String) {
    let (ok, message) = super::adopt_buddy(account).await;
    // 门槛未达的判据：adopt_buddy 把它翻成中文文案；英文 marker 兜底
    // （上游文案若变化，仍按关键词识别，避免把预期正常态报成失败）
    let threshold_short = message.contains("门槛未达")
        || message.to_lowercase().contains(BUDDY_TASK_INCOMPLETE_MARKER);
    (ok, threshold_short, message)
}

// ─── 当日防抖（领养门槛未达时不反复打上游）──────────────────

/// uid → 判定日期（北京时间日键）。参考项目 `adoptTriedToday` 的对等物：
/// 领养门槛未达是「今天对话量不够」的状态，当天重试只会得到同一个 400 ——
/// 记下来，当天不再试（次日自然重来）。
fn adopt_tried() -> &'static Mutex<HashMap<String, String>> {
    static TRIED: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    TRIED.get_or_init(|| Mutex::new(HashMap::new()))
}

fn adopt_tried_today(uid: &str) -> bool {
    let today = crate::server::core::beijing::today_key();
    adopt_tried()
        .lock()
        .map(|guard| guard.get(uid).map(|day| day == &today).unwrap_or(false))
        .unwrap_or(false)
}

fn mark_adopt_tried(uid: &str) {
    let today = crate::server::core::beijing::today_key();
    if let Ok(mut guard) = adopt_tried().lock() {
        guard.insert(uid.to_string(), today);
    }
}

// ─── 单账号一趟巡检 ─────────────────────────────────────────

/// 一趟巡检的结论（界面按它渲染）。
pub struct TravelReport {
    /// no_buddy / adopt / depart / claim / skip / failed
    pub outcome: &'static str,
    /// 界面文案（如实描述这一步做了什么 / 为什么跳过）
    pub message: String,
    /// 本次入账积分（领奖 / 领养时可能非零）
    pub credit: i64,
}

/// 单账号单趟状态机：查有无猫 → 查状态 → 最多一个动作，不轮询不等待。
pub async fn run_once(account: &GrowthAccount) -> TravelReport {
    let buddy = match buddy_info(account).await {
        Ok(buddy) => buddy,
        Err(error) => {
            return TravelReport { outcome: "failed", message: error, credit: 0 };
        }
    };
    let Some(buddy) = buddy else {
        // 无猫：走领养链路（当日门槛未达过则跳过，不反复打）
        if adopt_tried_today(&account.uid) {
            return TravelReport {
                outcome: "skip",
                message: "领养门槛今日已判定未达（对话量不够），明天自动重试".to_string(),
                credit: 0,
            };
        }
        let (ok, threshold_short, message) = adopt(account).await;
        if ok {
            return TravelReport { outcome: "adopt", message, credit: 300 };
        }
        if threshold_short {
            mark_adopt_tried(&account.uid);
            return TravelReport { outcome: "skip", message, credit: 0 };
        }
        return TravelReport { outcome: "failed", message, credit: 0 };
    };

    let status = match travel_status(account).await {
        Ok(status) => status,
        Err(error) => {
            return TravelReport { outcome: "failed", message: error, credit: 0 };
        }
    };
    match status.state.as_str() {
        "arrived" => {
            if status.record_id == 0 {
                return TravelReport {
                    outcome: "skip",
                    message: format!("已到站但缺 record_id，跳过（猫：{}）", buddy.name),
                    credit: 0,
                };
            }
            match travel_claim(account, status.record_id).await {
                Ok(reward) => TravelReport {
                    outcome: "claim",
                    message: format!("到站领奖成功，积分 +{reward}（猫：{}）", buddy.name),
                    credit: reward,
                },
                Err(error) => TravelReport { outcome: "failed", message: error, credit: 0 },
            }
        }
        "idle" => {
            if status.daily_limit_reached {
                return TravelReport {
                    outcome: "skip",
                    message: format!("今日已派出过（猫：{}），明天再来", buddy.name),
                    credit: 0,
                };
            }
            match travel_depart(account).await {
                Ok(()) => TravelReport {
                    outcome: "depart",
                    message: format!("已派出旅行（猫：{}，地点：古镇客栈）", buddy.name),
                    credit: 0,
                },
                Err(error) => TravelReport { outcome: "failed", message: error, credit: 0 },
            }
        }
        "traveling" => TravelReport {
            outcome: "skip",
            message: format!("猫在旅行途中（记录 {}），等下一趟", status.record_id),
            credit: 0,
        },
        other => TravelReport {
            outcome: "skip",
            message: format!("未知状态「{other}」，保守跳过"),
            credit: 0,
        },
    }
}

// ─── 多账号批量巡检（定时排程用）────────────────────────────

/// 一个账号的巡检汇总。
pub struct AccountTravelReport {
    pub account_id: String,
    pub name: String,
    /// run_once 的 outcome（no_buddy 不会出现：无猫走 adopt）
    pub outcome: &'static str,
    pub message: String,
    pub credit: i64,
}

/// 对一批账号跑一趟巡检（**串行** + 账号间错峰）。
///
/// ── 与单账号入口的分工 ──────────────────────────────────────
/// `api::growth::travel_run` 是「用户点了某一个账号的按钮」，本函数是
/// 「定时排程扫一遍全部账号」。两者共用 [`run_once`] 这一份状态机 ——
/// 区别只在遍历与门控，不在动作本身。
///
/// ── 门控 ────────────────────────────────────────────────────
/// 只服务 WorkBuddy 国内版的个人账号（企业版没有成长体系，国际版没有
/// CN 任务中心）—— 与 `growth::streak::run_for_accounts` 同一口径。
///
/// ── 为什么跳过也要进报告 ────────────────────────────────────
/// 旅行的「跳过」分两种：`traveling`（在途，等下一趟，正常）与
/// `daily_limit_reached`（今天派出过了，正常）。两者都是**用户可能想确认
/// 的状态**（「为什么今天没派出」），所以不像 streak 那样把无事可做的账号
/// 整条吞掉 —— 但定时任务只把「有动作 / 有失败」的写进摘要，避免日志刷屏
/// （见 `core::growth_scheduler`）。
pub async fn run_for_accounts(
    store: &crate::server::core::account_store::AccountStore,
    accounts: &[serde_json::Value],
) -> Vec<AccountTravelReport> {
    /// 账号间错峰（毫秒）：参考项目 `travelAccountDelay` 同口径。
    const ACCOUNT_GAP_MS: u64 = 800;

    let mut reports: Vec<AccountTravelReport> = Vec::new();
    let mut first = true;
    for account in accounts {
        let id = account.get("id").and_then(Value::as_str).unwrap_or("");
        if id.is_empty() || !super::supports_growth(account) {
            continue;
        }
        // 企业版没有个人成长体系（上游固定 400 code 10001）
        if account
            .get("enterpriseId")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
        {
            continue;
        }
        let name = account
            .get("name")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .or_else(|| account.get("nickname").and_then(Value::as_str))
            .unwrap_or(id)
            .to_string();
        let Some(growth_account) = GrowthAccount::from_store(store, id) else {
            continue;
        };
        if !first {
            tokio::time::sleep(std::time::Duration::from_millis(ACCOUNT_GAP_MS)).await;
        }
        first = false;
        let report = run_once(&growth_account).await;
        reports.push(AccountTravelReport {
            account_id: id.to_string(),
            name,
            outcome: report.outcome,
            message: report.message,
            credit: report.credit,
        });
    }
    reports
}
