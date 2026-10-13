//! CodeArts 的**新手任务**：目前只有「新人注册礼」这一条。
//!
//! ── 上游契约（读的是同一张口，不是新端点）──────────────────
//! 与每日福利共用 `GET /v1/ops/delivery?channel=IDE` → `POST /v1/ops/claim`
//! → `POST /v1/ops/confirm`，执行体也共用 [`welfare::claim_rewards`] ——
//! 幂等键先落盘、领完回读活动列表二次确认这两条硬约束因此自动生效。
//! 唯一的区别是**挑哪一条活动**：[`welfare::Campaign::is_newbie_gift`]。
//!
//! ── 另外两类一次性活动**不给入口**（不是"还没做"）──────────
//! 官方 IDE 的 `ActivityWelfarePane.TYPE_ORDER` 把活动分成四类，除 `USER_LOGIN`
//! 与 `NEW_USER_REGISTER` 外还有两种，本家有意不做：
//!   · `STUDENT_CERTIFIED`（学生认证）—— 真实清单里这一项的 `claimable` 是 false、
//!     `status` 是 `null`（不是字符串，`welfare::parse_delivery` 因此把非字符串一律读成空）。
//!     它的前置是认证
//!     本身，而认证不是 API；给按钮就是给一个必然失败的按钮。
//!   · `INVITE_USER`（邀请好友）—— `status:"ENTRY"`，收益归**邀请人**。由网关替被邀请
//!     那一侧点这一下，等于替一个不是我们的用户做决定。
//!
//! ── 自动补领的边界：面板自动、定时任务不接 ─────────────────
//! 新人礼是**一次性**的，所以两条链分开看：
//!   · 后端的定时签到（`auto_checkin` 的提供商清单）**不含本家** —— 那是无人看守的
//!     动作，本家的每日福利本来也不走那条链（见 `core::billing::checkin` 的排除）；
//!   · 面板上「签到后自动补领新手任务」**含本家**，与 Loomy / 小浣熊同构：领过之后
//!     结清记忆一落（见 [`super::super::onboarding_memory`]），此后的查询与领取都
//!     **零上游请求** —— 这条自动路径总共只发过一次写请求。
//! 手点的「一键领取」仍然在，它和自动补领走的是同一个执行体。
//!
//! ── 面板上的「待领」为什么按 hasWork 数 ─────────────────────
//! 新人礼也可能以 `claimable:false` 且从未到账的形态出现（未达门槛、活动未开始）。
//! 把它算进「待领」就是骗用户点一次然后失败 —— 所以行上带一个 `blocked`，
//! 既不计入 `unclaimed`，也不发领取，只在展开的任务清单里如实显示「暂不可领」。

use serde_json::{Value, json};

use crate::server::core::account_store::codearts_accounts::CODEARTS_PROVIDER_ID;
use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::onboarding_memory;
use crate::server::errors::GatewayError;

use super::welfare::{self, Campaign};

/// 业务基址与每日福利同一条（`api::codearts_welfare` 用的是同一个常量）。
fn base() -> &'static str {
    super::models::DEFAULT_BASE_URL
}

/// 任务分组名（面板按 group 分小节展示，与 Loomy / 小浣熊同层）。
const TASK_GROUP: &str = "新人注册礼";

/// 这一条现在值不值得动手（能领，或领了还没确认）。
fn actionable(item: &Campaign) -> bool {
    item.has_work()
}

/// 到账判定：官方列表里已是 CONFIRMED / CONSUMED。
fn settled(item: &Campaign) -> bool {
    item.is_confirmed()
}

fn task_row(item: &Campaign) -> Value {
    json!({
        // key 用 type 而不是 campaignId：同一类活动换期会变 id，而面板上的
        // 「这一条领过没有」是按类读的（与 Loomy 用任务名当 key 同一口径）
        "key": item.kind,
        // 标题按 kind 翻成中文（上游 title 是英文的，只有注册礼那条给过中文；
        // 映射范围与回退口径见 `Campaign::display_title`）
        "title": item.display_title(),
        "group": TASK_GROUP,
        "points": item.benefit_amount,
        "unit": item.benefit_unit,
        "done": settled(item),
        // 不可领又不是已到账 ⇒ 资格还没到（未达门槛、活动未开始）。行上如实标出来，
        // 但既不计入待领数、也不发请求。
        "blocked": !actionable(item) && !settled(item),
        "claimable": item.claimable,
        "status": item.status,
    })
}

/// 待领数 = 有活可干且还没确认的那几条。
fn unclaimed_of(rows: &[Value]) -> usize {
    rows.iter()
        .filter(|row| row.get("blocked").and_then(Value::as_bool) != Some(true))
        .filter(|row| row.get("done").and_then(Value::as_bool) != Some(true))
        .count()
}

/// 这条一次性福利**彻底结清**了没：清单非空且每一条都已到账。
///
/// 不能只看 `unclaimed == 0`：`blocked`（资格还没到、活动还没开始）的那些
/// 既不算待领、也不算到账 —— 把它们当成结清，等于给一个将来可能变可领的活动
/// 钉死「不会再变」的记忆，之后既不再查也领不到（见模块头的 `blocked` 说明）。
fn all_settled(rows: &[Value]) -> bool {
    !rows.is_empty()
        && rows
            .iter()
            .all(|row| row.get("done").and_then(Value::as_bool) == Some(true))
}

/// 名义积分合计（已到账的算 earned、全部的算 total）—— 与 Loomy 同口径：
/// 用**上游给的名义值**，不拿余额差值冒充到账数。
fn points_sum(rows: &[Value], only_done: bool) -> f64 {
    rows.iter()
        .filter(|row| !only_done || row.get("done").and_then(Value::as_bool) == Some(true))
        .filter_map(|row| row.get("points").and_then(Value::as_f64))
        .sum()
}

/// 状态响应里固定的那两项说明（记忆路径与实查路径共用一份文案）。
const TASK_NOTE: &str = "一次性奖励：领一次就没了，且到账的是套餐赠送积分（不增加福利模型 token 池）";

/// 把 provider / note 并进一份响应（记忆路径与实查路径共用）。
fn with_meta(mut payload: Value) -> Value {
    if let Some(object) = payload.as_object_mut() {
        object.insert("provider".to_string(), json!(CODEARTS_PROVIDER_ID));
        object.insert("note".to_string(), json!(TASK_NOTE));
    }
    payload
}

/// 记忆里的结算快照（没有 → None）。语义与写入口径见
/// `providers::onboarding_memory`。
fn settled_snapshot(store: &AccountStore, account_id: &str) -> Option<Value> {
    onboarding_memory::snapshot(store.codearts_account_record(account_id).as_ref())
}

/// 签到时中心快照用：已结算就给一份可渲染的记忆（零上游），否则 None。
pub fn settled_view(record: Option<&Value>) -> Option<Value> {
    onboarding_memory::snapshot(record)
}

/// `GET /api/accounts/{id}/onboarding` —— 状态快照。
///
/// ── 一次性福利的记忆（`refresh = false` 时）───────────────────
/// 结算过（见 `providers::onboarding_memory`）就直接用记忆回答，**零上游请求**
/// —— 进签到中心不该为一条早就领完的福利反复查上游。手动「查询任务」按钮走
/// `?refresh=1` 强制实查（拿到新事实时覆盖或清除记忆）。
pub async fn get_tasks(
    store: &AccountStore,
    account_id: &str,
    refresh: bool,
) -> Result<Value, GatewayError> {
    let previous = settled_snapshot(store, account_id);
    if !refresh {
        if let Some(snapshot) = previous.as_ref() {
            return Ok(with_meta(onboarding_memory::serve(snapshot)));
        }
    }
    let items = welfare::newbie_gift(store, account_id, base()).await?;
    let rows: Vec<Value> = items.iter().map(task_row).collect();
    // 上游这次一条新人礼都没回，而记忆里**有**已结算的结论 ⇒ 用记忆回答。
    // 活动被领完之后从运营列表里消失是上游的正当行为（`CONSUMED` 之后不再下发），
    // 那时实查得到一张空清单，而记忆才是这条福利的真实历史 —— 不回落到记忆会让
    // 同一张卡片在「已领取一条」与「什么都没有」之间随刷新来回跳。
    if rows.is_empty() {
        if let Some(snapshot) = previous.as_ref() {
            return Ok(with_meta(onboarding_memory::serve(snapshot)));
        }
    }
    let unclaimed = unclaimed_of(&rows);
    let mut payload = with_meta(json!({
        "tasks": rows,
        "earned": points_sum(&rows, true),
        "total": points_sum(&rows, false),
        "unclaimed": unclaimed,
        // 界面据此判断「这条一次性福利到此为止」（`remember` 也按它落 / 清记忆）：
        // 与 Loomy / 小浣熊同名字段，消费方不必按 provider 分叉。判据是
        // `all_settled`（每一条都已到账）而不是 `unclaimed == 0` —— 后者会把
        // `blocked` 的行误当成结清，见那里的说明。
        "settled": all_settled(&rows),
    }));
    // 走到这里清单必然非空（空清单且无记忆 = 这账号压根没有新人礼，不该落记忆
    // —— 那会让它此后永远不再实查；空清单但有记忆已在上面的回落里消化掉了）。
    // `remember` 自己按 settled 决定写还是清，并把结算时刻补回响应（与记忆路径
    // 的 `settledAt` 同义）。
    onboarding_memory::remember(
        store,
        account_id,
        "CodeArts",
        &mut payload,
        previous.as_ref(),
    );
    Ok(payload)
}

/// `POST /api/accounts/{id}/onboarding/claim` —— 领新人注册礼并回读确认。
///
/// 执行体是每日那条同一个（`welfare::claim_rewards`），差别只在挑哪一条活动。
/// `manual = true`：这是用户手点的动作，不受「每天 6 次 / 间隔 10 分钟」那两道
/// 保护无人值守重试的闸约束，但**资格每次仍从上游重读**。
///
/// ── 结算过就直接回答（零上游）───────────────────────────────
/// 一次性福利没有再领一次的可能：记忆里已结算 ⇒ 不再打上游（`claim_all` 也是
/// 签到后自动补领的执行体，那才是这条短路真正省下的请求）。要实查请走状态
/// 查询的 `?refresh=1`，它会把新事实写回记忆（含"上游又出了新一期"的清账情形）。
pub async fn claim_all(store: &AccountStore, account_id: &str) -> Result<Value, GatewayError> {
    let previous = settled_snapshot(store, account_id);
    if let Some(snapshot) = previous.as_ref() {
        return Ok(settled_claim_response(snapshot));
    }
    let now_ms = crate::server::logging::now_ms();
    let run = welfare::claim_rewards(
        store,
        account_id,
        base(),
        now_ms,
        true,
        welfare::Rewards::NewbieGift,
    )
    .await?;
    // 领取流程只在「全部回读确认」时才交出 after；其余分支（已领过 / 没有活 /
    // 被限流）用 `before`（就是本轮挑中的那几条）当当前真相 —— 两条路都**不再
    // 额外打一次 delivery**（旧实现在 after 为空时回落到 `newbie_gift`，
    // 那是一次多余的只读请求）。
    let after: Vec<Campaign> = if run.after.is_empty() { run.before.clone() } else { run.after };
    // 任务清单恒为「新人礼那一条」：`after` 在成功路径上是回读的**全量**活动
    // 列表（执行体要它逐条确认），直接拿它铺行会让卡片在领取后多出邀请 /
    // 学生认证 / 每日签到三条 —— 同一张卡片「查询」一条、「领取」四条，
    // 点一次变一次。这里与 `get_tasks` 同用 `is_newbie_gift` 一把筛子，
    // 两个接口的 tasks / earned / total / unclaimed 因此永远同源。
    let rows: Vec<Value> = after
        .iter()
        .filter(|item| item.is_newbie_gift())
        .map(task_row)
        .collect();
    // 本轮真正到手的那几条：领取前有活、回读之后已确认
    let landed: Vec<&Campaign> = run
        .before
        .iter()
        .filter(|item| actionable(item))
        .filter(|item| after.iter().any(|seen| seen.key() == item.key() && settled(seen)))
        .collect();
    let results: Vec<Value> = run
        .before
        .iter()
        .filter(|item| actionable(item))
        .map(|item| {
            let done = landed.iter().any(|seen| seen.key() == item.key());
            json!({
                "key": item.kind,
                "ok": done,
                "already": item.is_confirmed(),
                "error": if done { Value::Null } else { Value::String("官方活动列表尚未确认到账".to_string()) },
            })
        })
        .collect();
    let claimed = landed.len();
    let has_tasks = !rows.is_empty();
    let mut payload = with_meta(json!({
        "results": results,
        "claimed": claimed,
        "failed": results.len() - claimed,
        // 按**上游名义值**合计，不拿余额差值冒充（余额变动可能掺着别的来源）
        "claimedPoints": landed.iter().map(|item| item.benefit_amount).sum::<f64>(),
        "tasks": rows,
        "earned": points_sum(&rows, true),
        "total": points_sum(&rows, false),
        "unclaimed": unclaimed_of(&rows),
        "settled": all_settled(&rows),
        "outcome": run.outcome.label(),
    }));
    // 本轮确实有这一条活动 ⇒ 顺手同步记忆（判据与 `get_tasks` 同一把：非空才
    // 记，`remember` 自己按 settled 决定写还是清）。`previous` 传 None 是确定的：
    // 上面已有记忆就短路返回了，走到这里必然还没有快照，结算时刻按现在计。
    if has_tasks {
        onboarding_memory::remember(store, account_id, "CodeArts", &mut payload, None);
    }
    Ok(payload)
}

/// 记忆命中时领取接口的回答：这次**一次上游都没打**，因此没有一条 results。
/// `outcome` 用「已领取并确认」—— 这是真话（记忆就是上游确认过的结论），
/// 也正是界面希望看到的「别再点了」。
fn settled_claim_response(snapshot: &Value) -> Value {
    let mut payload = onboarding_memory::serve(snapshot);
    if let Some(object) = payload.as_object_mut() {
        object.insert("results".to_string(), json!([]));
        object.insert("claimed".to_string(), json!(0));
        object.insert("failed".to_string(), json!(0));
        object.insert("claimedPoints".to_string(), json!(0));
        object.insert("outcome".to_string(), json!(welfare::Outcome::Already.label()));
    }
    with_meta(payload)
}

#[cfg(test)]
mod shaping {
    use super::*;
    use serde_json::json;

    fn campaign(id: &str, kind: &str, title: &str, claimable: bool, status: &str, amount: f64) -> Campaign {
        Campaign {
            id: json!(id),
            kind: kind.to_string(),
            title: title.to_string(),
            claimable,
            status: status.to_string(),
            benefit_amount: amount,
            benefit_unit: "CREDIT".to_string(),
        }
    }

    #[test]
    fn only_the_newbie_gift_is_a_task_and_the_other_three_kinds_stay_out() {
        assert!(campaign("4", "NEW_USER_REGISTER", "新人注册礼", true, "", 4000.0).is_newbie_gift());
        // 每日那条不是新手任务（混领就是把每天的事当成只有一次的事做）
        assert!(!campaign("1", "USER_LOGIN", "每日签到领1000积分", true, "", 1000.0).is_newbie_gift());
        // 这两类**有意不给入口**（理由见模块头），不是漏做：学生认证的前置是认证本身、
        // 邀请礼的收益归邀请人。判据收窄到一条，界面上就不会出现必然失败的按钮。
        assert!(!campaign("2", "STUDENT_CERTIFIED", "学生认证", true, "", 4000.0).is_newbie_gift());
        assert!(!campaign("3", "INVITE_USER", "邀请好友", true, "ENTRY", 1000.0).is_newbie_gift());
        // 认不出键的条目不算任务（没有键就没法记台账、也没法回读确认）
        assert!(!Campaign { id: Value::Null, ..campaign("4", "NEW_USER_REGISTER", "", true, "", 1.0) }.is_newbie_gift());
        assert!(!campaign("", "NEW_USER_REGISTER", "空键", true, "", 1.0).is_newbie_gift());
    }

    #[test]
    fn an_unavailable_task_shows_as_blocked_and_is_not_counted_as_unclaimed() {
        let rows = vec![
            // 实测会出现的形状：条目在列表里，但 claimable:false 且从未到账
            task_row(&campaign("4", "NEW_USER_REGISTER", "新人注册礼", false, "", 4000.0)),
            task_row(&campaign("5", "NEW_USER_REGISTER", "新人注册礼（第二期）", true, "", 4000.0)),
            task_row(&campaign("6", "NEW_USER_REGISTER", "新人注册礼（已领）", false, "CONFIRMED", 4000.0)),
        ];
        assert_eq!(Some(true), rows[0].get("blocked").and_then(Value::as_bool));
        assert_eq!(0, unclaimed_of(&rows[..1]), "blocked 的行不能算待领：点一次就失败一次");
        assert_eq!(1, unclaimed_of(&rows), "只有能领且没到账的那条算待领");
        // 已到账的那条：不再 blocked（没活可干也不是"卡住"，是完事了），也不算待领
        assert_eq!(Some(false), rows[2].get("blocked").and_then(Value::as_bool));
        assert_eq!(Some(true), rows[2].get("done").and_then(Value::as_bool));
    }

    #[test]
    fn a_settled_task_counts_into_earned_but_not_into_unclaimed() {
        let rows = vec![
            task_row(&campaign("4", "NEW_USER_REGISTER", "新人注册礼", false, "CONFIRMED", 4000.0)),
            task_row(&campaign("5", "NEW_USER_REGISTER", "新人注册礼（第二期）", true, "", 1000.0)),
        ];
        assert_eq!(4000.0, points_sum(&rows, true), "已到账的按上游名义值合计");
        assert_eq!(5000.0, points_sum(&rows, false));
        assert_eq!(1, unclaimed_of(&rows));
    }

    #[test]
    fn titles_are_mapped_by_kind_and_fall_back_to_upstream_when_unknown() {
        // 认得的类：按 kind 给中文，忽略上游那个英文 title（金额不重复写进标题）
        assert_eq!(
            "每日签到",
            task_row(&campaign("1", "DAILY_CLAIM", "Daily Check-in: Claim 1000 Credits", true, "", 1000.0))["title"]
        );
        assert_eq!(
            "用户登录送积分",
            task_row(&campaign("1", "USER_LOGIN", "Login reward for credits", true, "", 100.0))["title"]
        );
        assert_eq!(
            "新用户注册礼",
            task_row(&campaign("4", "NEW_USER_REGISTER", "新用户注册送 4000 积分", true, "", 4000.0))["title"]
        );
        assert_eq!(
            "学生认证",
            task_row(&campaign("2", "STUDENT_CERTIFIED", "Student Certification: Claim 4000 Credits", false, "", 4000.0))["title"]
        );
        assert_eq!(
            "邀请好友",
            task_row(&campaign("3", "INVITE_USER", "Invite & Earn with Referral Codes", true, "ENTRY", 1000.0))["title"]
        );
        // 认不出的类：回退上游原文；上游也没给才回退 kind
        assert_eq!(
            "Some New Campaign",
            task_row(&campaign("9", "SOME_NEW_KIND", "Some New Campaign", true, "", 1.0))["title"]
        );
        assert_eq!(
            "SOME_NEW_KIND",
            task_row(&campaign("9", "SOME_NEW_KIND", "   ", true, "", 1.0))["title"]
        );
    }
}
