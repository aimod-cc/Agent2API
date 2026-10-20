//! 签到中心的聚合快照（GET /api/checkin-center）。
//!
//! ── 为什么单独一条聚合端点 ──────────────────────────────────
//! 签到中心一页要看四块数据：每日签到的账号分组、自动签到设置、签到历史台账、
//! 一次性/手动项（新手任务 / 福利 / 套餐）的账号清单。前两块已有各自接口
//! （/api/accounts、/api/auto-checkin），但「按提供商分组 + 支持签到判定 +
//! 今日已签」的口径在后端（`supports_checkin` / `CHECKIN_PROVIDERS`），前端拼
//! 会抄一份判定逻辑 —— 那正是「两套行为」的开头。所以这里在服务端聚合成一份，
//! 一次请求拉全，页面打开只打这一条。
//!
//! 惰性查询的**上游资格类状态**（Loomy / 小浣熊的新手任务、ZCode 可领套餐）
//! **不在**这份快照里：它们每查一个账号就要打一次上游，聚合快照必须保持
//! 「打开页面零上游请求」。快照只给账号入口清单，前端拿到清单后按需调既有的
//! 查询接口（/onboarding、/zcode-claim/preview）—— 那两条接口原样复用，一条不改。
//! （CodeArts 的 `/codearts-welfare/preview` 不再出现在本页：它的每日福利
//! 2026-10 起已并入每日签到链，走 `/api/accounts/checkin` 那一套，见
//! `core::billing::checkin` 的 codearts 分支。）
//!
//! ── 分组口径与批量签到完全一致 ─────────────────────────────
//! 进「每日签到」分组的账号 = 提供商 ∈ `CHECKIN_PROVIDERS` **且**
//! `supports_checkin` 为真 —— 与 `resolve_checkin_targets` 的过滤是同一对判据，
//! 签到中心看到的「可签 9 个」与点「立即全部签到」实际签到的集合不会分叉。
//! 其余账号进 `outOfScope`（按提供商聚合，附原因文案）。
//!
//! ── 响应形状 ───────────────────────────────────────────────
//! ```json
//! {
//!   "daily": {
//!     "providers": [ { "id", "label",
//!                      "accounts": [ { "id", "name", "available", "checkinAt",
//!                                      "checkedInToday" } ],
//!                      "doneCount", "totalCount" } ],
//!     "outOfScope": [ { "label", "reason", "count" } ],
//!     "todayDone": 6, "todayEligible": 9
//!   },
//!   "extras": {
//!     "onboarding": [ { "id", "name", "provider" } ],
//!     "plans":      [ { "id", "name", "provider", "claimAt", "claimPlans" } ],
//!     "autoGrant":  [ { "id", "name", "provider", "note" } ]
//!   },
//!   "auto": { …与 GET /api/auto-checkin 同形… },
//!   "history": [ { "at", "date", "reason", "succeeded", "total", "skipped",
//!                  "failed", "failedCount" } ]
//! }
//! ```
//! `history` 新条目在前（`checkin_history` 的落盘顺序），前端直接按序渲染时间线。
//!
//! ── 「自动发放」为什么是独立一段，而不是塞进每日签到 ──────────
//! QClaw 的积分由服务端**随登录自动发放**（实测账号里躺着一笔
//! 「老用户一次性回馈」，没有任何「领取」动作可调：客户端 bundle 里
//! 没有签到、没有领奖端点，jprx 的 `4110` 只读余额）。它的账号如果按
//! `CHECKIN_PROVIDERS` 进每日签到分组，界面会长出一颗「签到」按钮 ——
//! 那颗按钮点下去必然 404/报错。**一个点了必然报错的复选框比没有更糟**。
//!
//! 但它也不能默默消失：用户要在这页看到「这个账号有多少积分、什么时候到期」。
//! 所以它进 `extras.autoGrant`（与 `onboarding` / `plans` 同一层级），界面
//! 渲染成**只读行 + 余额惰性查询**，并在「每日签到」的折叠说明里点名指路。
//! 归属理由：积分的**获得方式**是登录即发（日频、无需动作），语义上更接近
//! 每日签到而非一次性新手任务；只是它没有「领」这个动作，所以只借用入口不
//! 借用按钮。

use std::collections::BTreeMap;

use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::core::auto_checkin::{self, CHECKIN_PROVIDERS};
use crate::server::core::beijing;
use crate::server::core::billing::checkin;
use crate::server::core::checkin_history;
use crate::server::http::ok_json;
use crate::server::ServerState;

/// `checkedInToday` 的判定：`checkinAt`（ms）落在今天（**北京时间**，
/// 上游自然日口径 —— 见 `core::beijing`；跟机器时区走会让海外部署下
/// 的「今日已签到」错位，issue #138）。
/// 时间戳缺失 / 非法都算「今天没签」—— 与账号页「checkinAt 落在今天即算签过」
/// 的口径一致（见 store_admin::mark_checkin 的说明）。
fn checked_in_today(checkin_at: Option<i64>) -> bool {
    checkin_at
        .and_then(beijing::date_key_of_ms)
        .is_some_and(|key| key == beijing::today_key())
}

/// 账号快照里的字段（缺失按空值兜底，与 billing::checkin 的读取口径一致）
fn account_text(account: &Value, key: &str) -> String {
    account
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// 账号行的展示名：统一展示名优先（与账号页 / 请求日志同口径，见
/// `store_view::display_name_of`），缺键（旧快照）按旧口径回落 `name`。
/// 本文件四处行组装共用这一份，签到中心的账号名不再另立口径。
fn account_display_name(account: &Value) -> String {
    let display = account_text(account, "displayName");
    if display.is_empty() {
        account_text(account, "name")
    } else {
        display
    }
}

/// outOfScope 的原因文案（按 `supports_checkin` 的排除分支逐一对应）：
/// 判据改了这里要跟着改 —— 两处写的都是「这个账号为什么不能签到」。
fn out_of_scope_reason(account: &Value) -> &'static str {
    let provider = account_text(account, "provider");
    // 自动发放类（QClaw）不进 outOfScope：它们有自己的只读分组（`extras.autoGrant`），
    // 文案在那边给。走到这里说明 `auto_grant_providers` 与下面的分流判据不同步 ——
    // 兜底给一句实情，不误导成「没有签到链路」。
    if auto_grant_providers().iter().any(|id| *id == provider) {
        return "积分随登录自动发放，无需签到";
    }
    // WorkBuddy / Qoder 两个地区的国际版都在 `supports_checkin` 里显式放行
    //（前者走日活链、后者带风控领积分），还落在范围外的 intl 只可能是 Accio。
    if account.get("edition").and_then(Value::as_str) == Some("intl") {
        return "国际版账号没有签到活动";
    }
    if crate::server::core::account_store::is_accio_family(&provider) {
        return "未接入签到链路";
    }
    "没有签到链路"
}

/// 「积分随登录自动发放、没有可领取动作」的提供商清单。
///
/// ── 判据（加一家之前先逐条确认上游）────────────────────────
/// 1. 账号上**真实存在**积分余额（能查到数），否则这一行只是个空壳；
/// 2. 上游**没有任何**「领取 / 签到」端点可调 —— 有的话就该进
///    `CHECKIN_PROVIDERS` 走真正的签到链，而不是在这里当只读行；
/// 3. 积分有到期概念（余额里带 `expireAt`），否则「什么时候到期」这一栏没内容。
///
/// 目前只有 QClaw：服务端随登录发放、`jprx 4110` 只读余额、`items[]` 带
/// `expire_at`（实证见 `core::providers::qclaw::points`）。
pub const AUTO_GRANT_PROVIDERS: [&str; 1] = ["qclaw"];

fn auto_grant_providers() -> &'static [&'static str] {
    &AUTO_GRANT_PROVIDERS
}

/// 自动发放行上的说明文案（后端给，前端原样透出 —— 与 outOfScope 的原因同一处置：
/// 「这家的积分怎么来的」是后端事实，不该在前端再抄一份）。
fn auto_grant_note(provider: &str) -> &'static str {
    match provider {
        "qclaw" => "积分随登录自动发放，无需签到；余额与到期时间在账号页「积分」里查",
        _ => "积分随登录自动发放，无需签到",
    }
}

/// 签到中心的聚合快照（不发任何上游请求 —— 资格类状态由前端按需惰性查询）
pub async fn get_center(State(state): State<ServerState>) -> Response {
    let accounts: Vec<Value> = state
        .store()
        .list_accounts()
        .get("accounts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    // ── 每日签到分组：提供商 ∈ CHECKIN_PROVIDERS 且 supports_checkin ──
    // 按 CHECKIN_PROVIDERS 的注册顺序分桶（界面行序稳定）；范围外账号按
    // 「提供商 + 原因」聚合（label 用注册表名，理由取排除分支的文案）。
    let mut buckets: Vec<Vec<Value>> = vec![Vec::new(); CHECKIN_PROVIDERS.len()];
    let mut out_of_scope: BTreeMap<String, (String, String, i64)> = BTreeMap::new();
    // 自动发放类按 provider 分桶（BTreeMap 保证界面行序稳定，与本文件别处一致）
    let mut auto_grant: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for account in &accounts {
        let provider = account_text(account, "provider");
        let provider = if provider.is_empty() {
            crate::server::core::providers::DEFAULT_PROVIDER_ID.to_string()
        } else {
            provider
        };
        let slot = CHECKIN_PROVIDERS
            .iter()
            .position(|id| *id == provider)
            .filter(|_| checkin::supports_checkin(account));
        if let Some(index) = slot {
            buckets[index].push(json!({
                "id": account_text(account, "id"),
                "name": Value::from(account_display_name(account)),
                "available": account.get("available").and_then(Value::as_bool).unwrap_or(true),
                "checkinAt": account.get("checkinAt").cloned().unwrap_or(Value::Null),
                "checkedInToday": checked_in_today(
                    account.get("checkinAt").and_then(Value::as_i64),
                ),
                // 版本（cn / intl，缺失 null）：WorkBuddy 国际版在这张表里执行的是
                // 「领日活」（活跃保活），按钮文案与提示要跟国内版的「签到」分开
                "edition": account.get("edition").cloned().unwrap_or(Value::Null),
            }));
        } else if auto_grant_providers().iter().any(|id| *id == provider) {
            // 自动发放类（QClaw）：进自己的只读分组，见模块头。
            // 这里只收集账号行；余额由前端按需惰性查询（快照零上游请求的铁律）
            auto_grant
                .entry(provider.clone())
                .or_default()
                .push(json!({
                    "id": account_text(account, "id"),
                    "name": Value::from(account_display_name(account)),
                    "provider": provider,
                    "note": auto_grant_note(&provider),
                }));
        } else {
            let label = crate::server::core::providers::label_of(&provider);
            let reason = out_of_scope_reason(account);
            let entry = out_of_scope
                .entry(format!("{provider}|{reason}"))
                .or_insert_with(|| (label, reason.to_string(), 0));
            entry.2 += 1;
        }
    }

    let provider_rows: Vec<Value> = CHECKIN_PROVIDERS
        .iter()
        .zip(buckets.into_iter())
        .map(|(id, rows)| {
            let done = rows
                .iter()
                .filter(|row| row.get("checkedInToday").and_then(Value::as_bool) == Some(true))
                .count();
            json!({
                "id": id,
                "label": auto_checkin::provider_label(id),
                "accounts": rows,
                "doneCount": done,
                "totalCount": rows.len(),
            })
        })
        .collect();
    let today_done: u64 = provider_rows
        .iter()
        .map(|row| row.get("doneCount").and_then(Value::as_u64).unwrap_or(0))
        .sum();
    let today_eligible: u64 = provider_rows
        .iter()
        .map(|row| row.get("totalCount").and_then(Value::as_u64).unwrap_or(0))
        .sum();
    let out_rows: Vec<Value> = out_of_scope
        .into_values()
        .map(|(label, reason, count)| json!({ "label": label, "reason": reason, "count": count }))
        .collect();

    // ── 一次性 / 手动项的账号入口清单（资格状态由前端惰性查询）──
    // 各家的账号行都是 {id, name, provider, settled}；provider 供界面选图标与文案
    // （新手任务分组里 Loomy、小浣熊与 CodeArts 共用一张卡）。
    //
    // `settled` 是**一次性福利的结算记忆**（已结清才非空，形状三家一致，
    // 见 `core::providers::onboarding_memory`）：与下面 CodeArts 行的 `welfare`
    // 台账同一先例 —— 本地事实、零上游请求，界面据此直接渲染「已领取」并在
    // 进页面 / 签到后跳过自动查询，不必为一条早就领完的福利反复问上游。
    let onboarding_rows_of = |provider: &str| -> Vec<Value> {
        accounts
            .iter()
            .filter(|account| account_text(account, "provider") == provider)
            .map(|account| {
                let id = account_text(account, "id");
                json!({
                    "id": id,
                    "name": Value::from(account_display_name(account)),
                    "provider": provider,
                    "settled": super::onboarding::settled_view(state.store(), &id, provider)
                        .unwrap_or(Value::Null),
                })
            })
            .collect()
    };
    // 新手任务分组：Loomy（任务表）+ 小浣熊（首次桌面登录奖励）+ CodeArts
    // （运营活动里的新人注册礼那一条），执行体按 provider 分派（api::onboarding）。
    // CodeArts 的账号在这里出现的是**一次性**那部分（新人注册礼）；它的**每日**
    // 福利已并入每日签到链（上面 `daily` 段里就是一组普通分组）—— 同一家的两类
    // 奖励分别归属「每日签到」与「新手福利」两块，互不重复。
    // 签到中心「签到后自动补领」会替用户领这一条一次性新人礼 —— 领过之后上游
    // 不再回 claimable，而结算记忆一落，这条自动路径连查询都不再发
    // （见 codearts::onboarding）。
    let mut onboarding_rows = onboarding_rows_of("loomy");
    onboarding_rows.extend(onboarding_rows_of("raccoon"));
    onboarding_rows.extend(onboarding_rows_of("codearts"));
    // OfficeAce 的「新手任务」与每日签到是同一个上游动作（见 officeace::onboarding），
    // 所以它也在这张分组表里 —— 前端按 server 给的 extras.onboarding 驱动，不硬编码家。
    onboarding_rows.extend(onboarding_rows_of("officeace"));
    // 智谱清言的新手任务是任务中心里的 `newbie` 分类（见 `chatglm::tasks`）。
    // 它的**每日**任务与每日登录积分一起走上面的每日签到链，两块互不重复 ——
    // 与 CodeArts「一次性归新手福利、每日归签到」同一分法。
    //
    // **访客排除**：本家的任务中心只对登录账号开放（访客实测 401），列进来会让
    // 「签到后自动补领」每次都为它白打一次注定失败的查询。
    onboarding_rows.extend(
        accounts
            .iter()
            .filter(|account| {
                account_text(account, "provider") == "chatglm"
                    && !crate::server::core::providers::chatglm::tasks::is_guest_account(
                        state.store(),
                        &account_text(account, "id"),
                    )
            })
            .map(|account| {
                let id = account_text(account, "id");
                json!({
                    "id": id,
                    "name": Value::from(account_display_name(account)),
                    "provider": "chatglm",
                    "settled": super::onboarding::settled_view(state.store(), &id, "chatglm")
                        .unwrap_or(Value::Null),
                })
            }),
    );
    // ZCode 的福利行带**本地领取台账**（`account.claimPlans`）—— 后端落盘的本地
    // 事实，带出来零上游请求，不违反「快照零上游」；界面的「今天领过哪几份」按它
    // 逐份比对（北京时间的日界判定在前端，不在后端再抄一份）。
    // （CodeArts 曾也有一条同样的 welfare 行，2026-10 它的每日福利并入每日签到后
    // 撤掉了：那一组在 `daily` 里已经是普通分组，「活动福利」再列一遍就是同一个
    // 账号两处入口。一块 UI 只留一个入口，领与看都 unambiguous。）
    let plan_rows: Vec<Value> = accounts
        .iter()
        .filter(|account| {
            let provider = account_text(account, "provider");
            provider == "zcode" || provider == "zcode-intl"
        })
        .map(|account| {
            json!({
                "id": account_text(account, "id"),
                "name": Value::from(account_display_name(account)),
                "claimAt": account.get("claimAt").cloned().unwrap_or(Value::Null),
                // 领取台账（{planId: 毫秒}）原样带出：界面「今天领过哪几份」按
                // 逐份比对（claimPlans 的日界判定在前端已有同款实现，复用它，
                // 后端不再抄一份北京时间的日界逻辑）
                "claimPlans": account.get("claimPlans").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();

    // 成长任务分组：WorkBuddy 国内版账号（`growth::supports_growth`）——它是
    // 上游任务树（建画布/用专家/发对话…）的载体，判据与执行体在
    // `providers::workbuddy::growth`（从参考项目 workbuddy2api 移植）。
    // 与新手福利同一先例：快照只给账号入口清单（零上游请求），任务详情由
    // 前端展开时按需调 `GET /api/accounts/{id}/growth`（那是一次上游查询）。
    let growth_rows: Vec<Value> = accounts
        .iter()
        .filter(|account| {
            crate::server::core::providers::workbuddy::growth::supports_growth(account)
        })
        .map(|account| {
            json!({
                "id": account_text(account, "id"),
                "name": Value::from(account_display_name(account)),
                "provider": "workbuddy",
            })
        })
        .collect();

    ok_json(json!({
        "daily": {
            "providers": provider_rows,
            "outOfScope": out_rows,
            "todayDone": today_done,
            "todayEligible": today_eligible,
        },
        "extras": {
            "onboarding": onboarding_rows,
            "plans": plan_rows,
            "growth": growth_rows,
            // 自动发放类（QClaw）：只读行 —— 积分随登录到账、没有可领动作。
            // 形状与 onboarding 同构（id/name/provider），多一个 note 说明文案
            "autoGrant": auto_grant
                .into_values()
                .flatten()
                .collect::<Vec<Value>>(),
        },
        "auto": state.auto_checkin().state(),
        // WorkBuddy 国际版日活保活的模型链（签到中心可编辑；空清单回落缺省链）
        "keepalive": crate::server::core::billing::keepalive::state(),
        // 猫猫旅行定时排程（国内版）：开关 / 时刻 / 今日已跑时刻 / 上一轮结果。
        // 与 `auto`（签到排程）并列 —— 两者是独立排程，各自开关与时刻。
        "travel": crate::server::core::growth_scheduler::state(),
        // 成长任务每日排程（国内版）：同上，独立于签到与旅行
        //（配置键 `growthTaskSchedule`，见 core::growth_task_scheduler）。
        "growthTask": crate::server::core::growth_task_scheduler::state(),
        "history": checkin_history::list(),
    }))
}
