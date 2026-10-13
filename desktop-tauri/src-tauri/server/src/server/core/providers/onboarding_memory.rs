//! 一次性新手任务的**本地结算记忆**（Loomy / CodeArts 用；小浣熊同族的逐条
//! 台账见 `onboardingGrants`，它由 [`super::raccoon::onboarding::settled_view`]
//! 派生出同一份「已结算」视图）。
//!
//! ── 存什么 ──────────────────────────────────────────────────
//! 账号记录上的 `onboardingSettled`：`{ at, tasks: [...], earned, total }`，
//! `tasks` 与状态查询返回的行同形（结算那一刻全部 `done`）。语义只有一条：
//! **这个账号的新手任务（一次性福利）已经全部领完，之后不会再变。**
//!
//! ── 为什么要有它 ────────────────────────────────────────────
//! 新手任务是一次性福利：领完就没有了。没有这份记忆，每次进签到中心、每次
//! 签到后的自动补领都要为它重新打一次上游查询（Loomy 的任务表、CodeArts 的
//! 活动列表），而答案永远是「已领取」。小浣熊早已用结算台账消灭了同样的多余
//! 动作（见 `raccoon::onboarding` 的模块头），这里把同一套口径给另外两家：
//! 结算过 ⇒ 状态查询与一键领取都**零上游请求**，面板进页面直接用记忆渲染。
//!
//! ── 何时写 / 何时信 / 何时清 ────────────────────────────────
//!   · 写：[`remember`]，查询或领取拿到「没有待领」的事实那一刻。包括「别处
//!     （官方客户端）早就领过」的情形 —— 那同样说明这条一次性福利已经结清；
//!   · 信：[`snapshot`] / [`serve`]，`?refresh=1` 之外的查询与全部领取；
//!   · 清：[`remember`] 收到 `unclaimed > 0` 时把旧快照抹掉 —— 上游出了新一期
//!     活动之类的新事实，记忆失效，回到「每次都查」的状态。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::logging;

/// 从账号记录里读结算快照；没有 / 形状读不懂（非对象）→ None。
pub fn snapshot(record: Option<&Value>) -> Option<Value> {
    record
        .and_then(|record| record.get("onboardingSettled"))
        .filter(|value| value.is_object())
        .cloned()
}

/// 用记忆回答一次状态查询：`tasks / earned / total` 来自快照，`unclaimed` 恒 0
/// （全部已领），另附 `settled` / `settledAt` 让界面能标出「这是记忆、不是刚
/// 查的」。调用方把自己那几项（provider / note）并进来。
pub fn serve(snapshot: &Value) -> Value {
    json!({
        "tasks": snapshot.get("tasks").cloned().unwrap_or(Value::Null),
        "earned": snapshot.get("earned").cloned().unwrap_or(Value::Null),
        "total": snapshot.get("total").cloned().unwrap_or(Value::Null),
        "unclaimed": 0,
        "settled": true,
        "settledAt": snapshot.get("at").cloned().unwrap_or(Value::Null),
    })
}

/// 按**响应自己的结论**落记忆，并把结果如实标回响应（`payload` 至少含
/// tasks / settled）。
///
/// `settled == true`（这次的事实是「一次性福利全部到账」）⇒ 写入并返回快照本体，
/// 同时在响应里补 `settledAt`（结算时刻）—— 界面无论走记忆路径还是实查路径看到
/// 的都是同一个含义的字段。否则**清掉**旧快照并返回 None —— 上游出了新事实
/// （新一期活动、又出现待领项），记忆失效，回到「每次都查」的状态。读法是
/// `payload.settled` 而不是本地再判一遍「unclaimed 是不是 0」：各家对这一条的
/// 判定不一样（CodeArts 的 `blocked` 行既不算待领也不算到账，只有它自己知道那
/// 算不算结清），事实由产出它的那家声明。
///
/// `previous` 是**落库前**那份快照（调用方用 [`snapshot`] 读，没有就传 `None`）。
/// 结论与旧快照一致（任务行逐字相同）时沿用旧的 `at`：`refresh=1` 手点查询会用
/// 同一份事实重新落一遍记忆，若每次刷新时间，「结算于」就变成了「刚才查过」，
/// 那不是它想说的意思（这条福利是什么时候结清的）。
///
/// 落库失败只记一条日志、仍返回快照 —— 上游那边福利已经结清，界面不该因为一次
/// 写盘失败就退回「还没查过」。
pub fn remember(
    store: &AccountStore,
    account_id: &str,
    label: &str,
    payload: &mut Value,
    previous: Option<&Value>,
) -> Option<Value> {
    let settled = payload.get("settled").and_then(Value::as_bool) == Some(true);
    if !settled {
        if !store.mark_onboarding_settled(account_id, Value::Null) {
            logging::verbose(
                "[Onboarding]",
                &format!("{label} 账号 {account_id}：结算记忆清除失败（不影响本次结果）"),
            );
        }
        return None;
    }
    let tasks = payload.get("tasks").cloned().unwrap_or(Value::Null);
    let at = match previous {
        // 同一份事实已经在记忆里 ⇒ 沿用原来的结算时刻
        Some(old) if old.get("tasks") == Some(&tasks) => old
            .get("at")
            .and_then(Value::as_i64)
            .unwrap_or_else(logging::now_ms),
        _ => logging::now_ms(),
    };
    let snapshot = json!({
        "at": at,
        "tasks": tasks,
        "earned": payload.get("earned").cloned().unwrap_or(Value::Null),
        "total": payload.get("total").cloned().unwrap_or(Value::Null),
    });
    if !store.mark_onboarding_settled(account_id, snapshot.clone()) {
        logging::log(
            "[Onboarding]",
            &format!(
                "⚠️ {label} 账号 {account_id}：新手任务结算记忆落库失败（本次结果不受影响，下次查询会重试）"
            ),
        );
    }
    if let Some(object) = payload.as_object_mut() {
        object.insert("settledAt".to_string(), json!(at));
    }
    Some(snapshot)
}
