//! 签到中心的聚合快照（GET /api/checkin-center）。
//!
//! ── 为什么单独一条聚合端点 ──────────────────────────────────
//! 签到中心一页要看四块数据：每日签到的账号分组、自动签到设置、签到历史台账、
//! 一次性/手动项（新手任务 / 福利 / 套餐）的账号清单。前两块已有各自接口
//! （/api/accounts、/api/auto-checkin），但「按提供商分组 + 支持签到判定 +
//! 今日已签」的口径在后端（`supports_checkin` / `checkin_providers`），前端拼
//! 会抄一份判定逻辑 —— 那正是「两套行为」的开头。所以这里在服务端聚合成一份，
//! 一次请求拉全，页面打开只打这一条。
//!
//! 惰性查询的**上游资格类状态**（Loomy / 小浣熊的新手任务、ZCode 可领套餐）
//! **不在**这份快照里：它们每查一个账号就要打一次上游，聚合快照必须保持
//! 「打开页面零上游请求」。快照只给账号入口清单，前端拿到清单后按需调既有的
//! 查询接口（/onboarding、/zcode-claim/preview）—— 那两条接口原样复用，一条不改。
//! （CodeArts 的 `/codearts-welfare/preview` 不再出现在本页：它的每日福利
//! 2026-10 起已并入每日签到链，走 `/api/accounts/checkin` 那一套 ——
//! 执行体在插件里，网关只做分派。）
//!
//! ── 分组口径与批量签到完全一致 ─────────────────────────────
//! 进「每日签到」分组的账号 = 提供商 ∈ 可签清单（`auto_checkin::checkin_providers()`，
//! 内置常量表 + 运行时装入的插件家）**且** `supports_checkin` 为真 —— 与 `resolve_checkin_targets` 的过滤是同一对判据，
//! 签到中心看到的「可签 9 个」与点「立即全部签到」实际签到的集合不会分叉。
//! 其余账号进 `outOfScope`（按提供商聚合，附原因文案）。
//!
//! ── 响应形状 ───────────────────────────────────────────────
//! ```json
//! {
//!   "daily": {
//!     "providers": [ { "id", "label",
//!                      "accounts": [ { "id", "name", "available", "checkinAt",
//!                                      "checkedInToday", "streakDays" } ],
//!                      "doneCount", "totalCount" } ],
//!     "outOfScope": [ { "label", "reason", "count" } ],
//!     "todayDone": 6, "todayEligible": 9
//!   },
//!   "extras": {
//!     "onboarding": [ { "id", "name", "provider" } ],
//!     "plans":      [ { "id", "name", "provider", "claimAt", "claimPlans" } ]
//!   },
//!   "auto": { …与 GET /api/auto-checkin 同形… },
//!   "history": [ { "at", "date", "reason", "succeeded", "total", "skipped",
//!                  "failed", "failedCount" } ]
//! }
//! ```
//! `history` 新条目在前（`checkin_history` 的落盘顺序），前端直接按序渲染时间线。

use std::collections::BTreeMap;

use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::core::auto_checkin::{self};
use crate::server::core::billing::checkin;
use crate::server::core::checkin_history;
use crate::server::core::providers::{kind_id, ProviderKind};
use crate::server::http::ok_json;
use crate::server::ServerState;

/// 插件提供了当天的逐项完成状态时优先采用；旧插件回落到签到时间戳。
fn checked_in_today(account: &Value) -> bool {
    crate::server::core::account_store::checkin_state::checked_in_today(account)
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
///
/// ── 插件申报优先 ────────────────────────────────────────────
/// 「访客没有积分体系」这条由插件申报（`CheckinInfo::exclude_guest`）：
/// 哪一家有访客概念、访客能不能签，是 provider 知识。下面按 `isGuest` /
/// `edition` / Accio 家族的三条特判只服务**没申报的旧插件**。
fn out_of_scope_reason(account: &Value) -> &'static str {
    let provider = account_text(account, "provider");
    let guest = account.get("isGuest").and_then(Value::as_bool) == Some(true);
    // 申报过的家：只有「访客排除」一种情况会让它的账号落到范围外
    // （其余排除条件都是网关侧的内置特判，申报家不会走到）
    if let Some(declared) = crate::server::core::plugin_host::checkin_of(&provider) {
        if declared.exclude_guest && guest {
            return "访客账号没有积分体系";
        }
        // 申报了 `plans` / `onboarding` / `growth` 但没申报每日签到的家
        // （ZCode 只有「活动福利」）：理由要说准，别让人以为这家整个没接。
        if !declared.daily && (declared.plans || declared.onboarding || declared.growth) {
            return "本家没有每日签到，福利在下方对应卡片里";
        }
        return "没有签到链路";
    }
    // 智谱清言的**访客账号**：与「没有签到链路」是两回事 —— 上游这一站有签到
    // （登录账号就能签），只是访客没有积分体系（相关接口对访客一律 401）。
    // 判在 provider / edition 之前：本家的 edition 恒 cn，不会与下面那条撞；
    // 文案与 `resolve_checkin_targets` 单账号拒绝时那句同义。
    if guest {
        return "访客账号没有积分体系";
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

/// 签到中心的聚合快照（不发任何上游请求 —— 资格类状态由前端按需惰性查询）
pub async fn get_center(State(state): State<ServerState>) -> Response {
    // 界面视图：插件不在的家不出现（账号仍在库里，装上插件原样回来）
    let accounts: Vec<Value> = state
        .store()
        .list_visible_accounts()
        .get("accounts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|account| {
            account.get("provider").and_then(Value::as_str) != Some(kind_id(ProviderKind::Chatglm))
                || account.get("isGuest").and_then(Value::as_bool) != Some(true)
        })
        .collect();

    // ── 每日签到分组：提供商 ∈ **可用**清单 且 supports_checkin ──
    // 按注册顺序分桶（界面行序稳定）；范围外账号按「提供商 + 原因」聚合
    // （label 用注册表名，理由取排除分支的文案）。
    //
    // 用 `available_checkin_providers`（不是 `checkin_providers`）：后者是
    // 配置校验的白名单、含全部已知家，直接拿来分组会在没装插件时列出十几行
    // 点了必然失败的空行（2026-10 用户反馈）。可用清单的判据是「插件正接管
    // 这家，或库里已有它的账号」—— 见 `providers::serves_or_has_accounts`。
    let checkin_providers = auto_checkin::available_checkin_providers(state.store());
    let mut buckets: Vec<Vec<Value>> = vec![Vec::new(); checkin_providers.len()];
    let mut out_of_scope: BTreeMap<String, (String, String, i64)> = BTreeMap::new();
    for account in &accounts {
        let provider = account_text(account, "provider");
        let provider = if provider.is_empty() {
            crate::server::core::providers::DEFAULT_PROVIDER_ID.to_string()
        } else {
            provider
        };
        let slot = checkin_providers
            .iter()
            .position(|id| id == &provider)
            .filter(|_| checkin::supports_checkin(account));
        if let Some(index) = slot {
            buckets[index].push(json!({
                "id": account_text(account, "id"),
                "name": Value::from(account_display_name(account)),
                "available": account.get("available").and_then(Value::as_bool).unwrap_or(true),
                "checkinAt": account.get("checkinAt").cloned().unwrap_or(Value::Null),
                "checkedInToday": checked_in_today(account),
                "dailyCheckin": account.get("dailyCheckin").cloned().unwrap_or(Value::Null),
                // 版本（cn / intl，缺失 null）：WorkBuddy 国际版在这张表里执行的是
                // 「领日活」（活跃保活），按钮文案与提示要跟国内版的「签到」分开
                "edition": account.get("edition").cloned().unwrap_or(Value::Null),
                // 成长中心连登天数（WorkBuddy 国内版；签到后回读落库，见
                // `AccountStore::mark_streak_days`）。恒为数字（0 = 还没读到过），
                // 与公开形态同一口径 —— 界面按数字读，不必判「字段缺失」。
                "streakDays": account.get("streakDays").cloned().unwrap_or(Value::from(0)),
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

    let mut provider_rows: Vec<Value> = checkin_providers
        .iter()
        .zip(buckets.into_iter())
        .map(|(id, rows)| {
            let done = rows
                .iter()
                .filter(|row| row.get("checkedInToday").and_then(Value::as_bool) == Some(true))
                .count();
            // 插件申报的链路信息（说明 / 执行档位 / 附加 UI 钩子）。
            // 没申报的旧插件不带这几个键，界面走自己的兜底（与申报前一致）。
            let declared = crate::server::core::plugin_host::checkin_of(id);
            let manual_plans = declared.as_ref().is_some_and(|declared| declared.daily && declared.plans);
            let mut row = json!({
                "id": id,
                "label": auto_checkin::provider_label(id),
                "accounts": rows,
                "doneCount": done,
                "totalCount": rows.len(),
                "manualPlans": manual_plans,
            });
            if let Some(declared) = declared {
                if let Some(object) = row.as_object_mut() {
                    object.insert(
                        "description".to_string(),
                        declared.description.map(Value::String).unwrap_or(Value::Null),
                    );
                    object.insert(
                        "modes".to_string(),
                        serde_json::to_value(&declared.modes).unwrap_or(Value::Null),
                    );
                    object.insert(
                        "uiHooks".to_string(),
                        serde_json::to_value(&declared.ui_hooks).unwrap_or(Value::Null),
                    );
                }
            }
            row
        })
        .collect();
    provider_rows.sort_by_key(|row| row.get("totalCount").and_then(Value::as_u64).unwrap_or(0) == 0);
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

    // 新手任务分组：**按插件申报生成**（`CheckinInfo::onboarding`）。
    // 原先是一串 `onboarding_rows_of("loomy")` 之类的硬编码 —— 那是 provider
    // 知识（这一家有没有一次性福利），加一家就得改网关。改成申报之后，
    // 新插件只要打开这一位，它的账号自动出现在这张表里。
    //
    // 访客排除仍走申报（`exclude_guest`）：智谱清言的访客没有积分体系
    // （任务中心一律 401），列进来只会白打查询。
    let mut onboarding_rows: Vec<Value> = Vec::new();
    for account in &accounts {
        let provider = account_text(account, "provider");
        let Some(declared) = crate::server::core::plugin_host::checkin_of(&provider) else {
            continue;
        };
        if !declared.onboarding {
            continue;
        }
        if declared.exclude_guest && account.get("isGuest").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let id = account_text(account, "id");
        onboarding_rows.push(json!({
            "id": id,
            "name": Value::from(account_display_name(account)),
            "provider": provider,
            // `settled` 是**一次性福利的结算记忆**（已结清才非空）：本地事实、
            // 零上游请求，界面据此直接渲染「已领取」并在进页面 / 签到后跳过
            // 自动查询，不必为一条早就领完的福利反复问上游。
            "settled": super::onboarding::settled_view(state.store(), &id)
                .unwrap_or(Value::Null),
        }));
    }
    // ZCode 的福利行带**本地领取台账**（`account.claimPlans`）—— 后端落盘的本地
    // 事实，带出来零上游请求，不违反「快照零上游」；界面的「今天领过哪几份」按它
    // 逐份比对（北京时间的日界判定在前端，不在后端再抄一份）。
    // 判据是插件申报的 `plans` 位（原先按 zcode / zcode-intl 两个 id 硬编码）。
    let plan_rows: Vec<Value> = accounts
        .iter()
        .filter(|account| {
            crate::server::core::plugin_host::checkin_of(&account_text(account, "provider"))
                .is_some_and(|declared| declared.plans)
        })
        .map(|account| {
            json!({
                "id": account_text(account, "id"),
                "name": Value::from(account_display_name(account)),
                "provider": account_text(account, "provider"),
                "claimAt": account.get("claimAt").cloned().unwrap_or(Value::Null),
                // 领取台账（{planId: 毫秒}）原样带出：界面「今天领过哪几份」按
                // 逐份比对（claimPlans 的日界判定在前端已有同款实现，复用它，
                // 后端不再抄一份北京时间的日界逻辑）
                "claimPlans": account.get("claimPlans").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();

    // 成长任务分组：判据是插件申报的 `growth` 位（原先按 `supports_growth` 的
    // 「provider == workbuddy 且非企业号」硬编码）。企业号的排除仍留在
    // `supports_growth` 里 —— 那是**账号形态**，不是 provider 能力。
    // 与新手福利同一先例：快照只给账号入口清单，任务详情由前端展开时按需调
    // `GET /api/accounts/{id}/growth`（那是一次上游查询）。
    let growth_rows: Vec<Value> = accounts
        .iter()
        .filter(|account| {
            crate::server::core::plugin_host::checkin_of(&account_text(account, "provider"))
                .is_some_and(|declared| declared.growth)
                && crate::server::api::growth::supports_growth(account)
        })
        .map(|account| {
            json!({
                "id": account_text(account, "id"),
                "name": Value::from(account_display_name(account)),
                "provider": account_text(account, "provider"),
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
        },
        "auto": state.auto_checkin().state(),
        // WorkBuddy 国际版日活保活的模型链（签到中心可编辑；空清单回落缺省链）
        "keepalive": crate::server::core::billing::keepalive::state(),
        // 猫猫旅行定时排程（国内版）：开关 / 时刻 / 今日已跑时刻 / 上一轮结果。
        // 与 `auto`（签到排程）并列 —— 两者是独立排程，各自开关与时刻。
        "travel": crate::server::core::growth_scheduler::state(),
        "history": checkin_history::list(),
        // 插件申报的管理面板：界面据此决定「猫猫旅行」「保活模型链」这两块
        // 显不显示。它们的能力由 WorkBuddy 插件的不同地区版本提供（国内版
        // 有成长任务、国际版有每日保活），没装插件时两块都不该出现。
        "adminPanels": crate::server::core::plugin_host::admin_panels(),
        // 各家申报的签到链路信息（说明 / 执行档位 / UI 钩子），按 provider id 索引。
        // `daily.providers[]` 里也带了一份（按组取用更方便）；这里再给一份完整的，
        // 供界面对「还没出现分组」的家也拿到说明（例如没账号时的占位行）。
        "checkinFeatures": crate::server::core::plugin_host::checkin_features(),
    }))
}
