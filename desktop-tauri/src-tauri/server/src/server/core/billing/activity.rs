//! 运营活动与组合动作（对照 Node 版 workbuddy-billing.mjs 的「活动」与
//! checkinAndReport 部分）。
//!
//!   GET  /v2/activity/workbuddy/banner    运营 banner（需客户端白名单头）
//!   GET  /v2/activity/ambassador/status    大使状态
//!   签到 + 查余额的组合动作（"签到并回报最新积分"）
//!   WorkBuddy 国际版的每日活跃任务（探测 → 条件领取 → 免费模型保活 + 网页会话）
//!
//! 从 billing/mod.rs 拆出（单文件行数约定）。banner / ambassador **吞掉所有错误
//! 返回 null**（装饰性内容，拉不到不该让首屏报错）；组合动作里的签到失败则收敛成
//! `{success:false}` 交给前端显示成 warn，只有国际版无签到是硬错误 ——
//! 而国际版现在走的是下面的 `workbuddy_daily_activity`（活跃任务不是普通签到，
//! 它的所有失败都收敛为结果字段，不让自动签到任务因为上游活动开关而中断）。

use serde_json::{json, Map, Value};

use crate::server::core::endpoints::RESPONSE_CODE_OK;
use crate::server::core::upstream::request::new_request_id;
use crate::server::logging;

use super::request::{
    BillingCall, CallOptions, ACTIVITY_AMBASSADOR, ACTIVITY_BANNER,
    BILLING_ACTIVITY_CHECKIN_STATUS, BILLING_DAILY_CHECKIN, BILLING_REPORT_ACTIVITY,
    GROWTH_STREAK_STATUS, js_truthy,
};
use super::{assert_checkin_supported, BillingError, BillingService};

/// 国际版每日活跃任务的**执行粒度**（对应签到中心的三颗手动按钮；
/// 定时与批量签到恒走 [`WorkbuddyActivity::Full`]，粒度细分只是手动入口的事）。
///
/// 三种粒度的边界就是"哪几步要打上游"：
///   * `Full`：探测 + 条件领取 + 保活 —— 一次点击完整跑一遍；
///   * `Claim`：探测 + 条件领取 —— 只想要奖励，不多打保活那发对话；
///   * `Keepalive`：只保活 —— 活动没开时只维持账号活跃，不打探测与领取。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkbuddyActivity {
    Full,
    Claim,
    Keepalive,
}

impl WorkbuddyActivity {
    /// API 层传来的 mode 字符串 → 粒度。未知值回落 Full —— 手动入口的
    /// 缺省行为是"完整跑一遍"，比"悄悄少做一步"更接近点击者的本意。
    pub fn parse(value: Option<&str>) -> Self {
        match value {
            Some("claim") => Self::Claim,
            Some("keepalive") => Self::Keepalive,
            _ => Self::Full,
        }
    }

    /// 结果行里回带的 mode（前端排查"这轮实际跑了哪几步"用）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Claim => "claim",
            Self::Keepalive => "keepalive",
        }
    }
}

impl BillingService {
    // ─── 活动 ───────────────────────────────────────────────

    /// 运营 banner：需客户端白名单头，否则被拦截（返回 null，与桌面端一致）。
    ///
    /// 这里**吞掉所有错误并返回 null** —— 与 Node 版一致：banner 是页面上
    /// 的一块装饰，拉不到就不显示，不该让首屏出现一条红色报错。
    pub async fn get_activity_banner(&self, session: Option<&Value>) -> Value {
        let active = match session {
            Some(session) => Some(session.clone()),
            None => match self.require_session().await {
                Ok(session) => Some(session),
                Err(error) => {
                    logging::log("[Activity]", &format!("banner 拉取失败: {}", error.message));
                    return Value::Null;
                }
            },
        };
        let result = match self
            .call_billing(
                ACTIVITY_BANNER,
                CallOptions { session: active.as_ref(), expect_code_ok: false, ..Default::default() },
            )
            .await
        {
            Ok(result) => result,
            Err(error) => {
                logging::log("[Activity]", &format!("banner 拉取失败: {}", error.message));
                return Value::Null;
            }
        };
        if result.code != Some(RESPONSE_CODE_OK) || result.data.is_null() {
            return Value::Null;
        }
        let raw = result.data;
        // action 只在 type 是 open_url / switch_model 时透出（其余类型前端不认识）。
        // 键的取舍照抄 Node 的对象字面量：`url` / `model_id` / `fallback_url`
        // 在缺失时是 undefined → **键整个消失**。实测上游的 open_url 就是
        // `{type, label, fallback_url}`（没有 url），所以响应里也不该有 url 键 ——
        // 前端 `action.url` 两种形态都读不到东西，但 `'url' in action` 会分叉。
        let action = raw.get("action").and_then(|action| {
            let action_type = action.get("type").and_then(Value::as_str).unwrap_or("");
            if action_type != "open_url" && action_type != "switch_model" {
                return None;
            }
            let mut object = Map::new();
            object.insert("type".to_string(), Value::String(action_type.to_string()));
            object.insert(
                "label".to_string(),
                action.get("label").cloned().unwrap_or(Value::Null),
            );
            if let Some(url) = action.get("url") {
                object.insert("url".to_string(), url.clone());
            }
            if let Some(model_id) = action.get("model_id") {
                object.insert("modelId".to_string(), model_id.clone());
            }
            if let Some(fallback) = action.get("fallback_url") {
                object.insert("fallbackUrl".to_string(), fallback.clone());
            }
            Some(Value::Object(object))
        });
        let mut banner = Map::new();
        banner.insert(
            "id".to_string(),
            raw.get("activity_id").cloned().unwrap_or(Value::Null),
        );
        // `raw.activity_online_status && raw.status === 'None'` ——
        // 前者是 JS 真值判定，后者是严格字符串比较
        banner.insert(
            "active".to_string(),
            Value::Bool(
                raw.get("activity_online_status").map(js_truthy).unwrap_or(false)
                    && raw.get("status").and_then(Value::as_str) == Some("None"),
            ),
        );
        banner.insert(
            "bannerContent".to_string(),
            raw.get("banner_content").cloned().unwrap_or(Value::Null),
        );
        banner.insert(
            "link".to_string(),
            raw.get("detail_link").cloned().unwrap_or(Value::Null),
        );
        banner.insert(
            "level".to_string(),
            raw.get("activity_level").cloned().unwrap_or(Value::Null),
        );
        banner.insert(
            "startTime".to_string(),
            raw.get("start_time").cloned().unwrap_or(Value::Null),
        );
        banner.insert(
            "endTime".to_string(),
            raw.get("end_time").cloned().unwrap_or(Value::Null),
        );
        // Node 的 `let action` 在类型不认识时保持 undefined → 键整个消失；
        // 能识别时一定是个对象（上面已构造）
        if let Some(action) = action {
            banner.insert("action".to_string(), action);
        }
        banner.insert("raw".to_string(), raw);
        Value::Object(banner)
    }

    /// 大使（推广）状态。同样吞错返回 null（verbose 级别日志）。
    pub async fn get_ambassador_status(&self, session: Option<&Value>) -> Value {
        let active = match session {
            Some(session) => Some(session.clone()),
            None => match self.require_session().await {
                Ok(session) => Some(session),
                Err(error) => {
                    logging::verbose(
                        "[Activity]",
                        &format!("ambassador 状态拉取失败: {}", error.message),
                    );
                    return Value::Null;
                }
            },
        };
        let result = match self
            .call_billing(
                ACTIVITY_AMBASSADOR,
                CallOptions { session: active.as_ref(), expect_code_ok: false, ..Default::default() },
            )
            .await
        {
            Ok(result) => result,
            Err(error) => {
                logging::verbose(
                    "[Activity]",
                    &format!("ambassador 状态拉取失败: {}", error.message),
                );
                return Value::Null;
            }
        };
        if result.code != Some(RESPONSE_CODE_OK) || result.data.is_null() {
            return Value::Null;
        }
        json!({
            "isAmbassador": result
                .data
                .get("isAmbassador")
                .map(js_truthy)
                .unwrap_or(false),
            "raw": result.data,
        })
    }

    // ─── 组合动作 ───────────────────────────────────────────

    /// 签到 + 查余额的组合动作（"签到并回报最新积分"）。
    ///
    /// 已签到时不重复领取，直接返回当前额度。
    /// 国际版没有签到活动：不吞成「签到失败」，而是直接抛出，
    /// 让调用方拿到明确原因（`checkinStatus` 那一步也会先抛）。
    ///
    /// 并发语义：Node 用 `Promise.all` 并发跑「查状态 + 领取」。
    /// 这里必须**顺序**执行 —— 两者共享同一个底层连接池没问题，
    /// 但并发会在上游留下两条几乎同时到达的签到请求，而 Node 版
    /// `checkinAndReport` 的注释明确说「额度可能因签到变化，稍等一下再查」，
    /// 顺序执行才保证 `usage` 反映的是领取之后的额度。
    pub async fn checkin_and_report(
        &self,
        session: Option<&Value>,
        locale: Option<&str>,
    ) -> Result<Value, BillingError> {
        let active = match session {
            Some(session) => session.clone(),
            None => self.require_session().await?,
        };

        // WorkBuddy 国际版没有国内版的普通签到接口；组合入口沿用同一条活跃任务
        // 链路，并把探测/领取/保活结果一起返回，避免重复打上游。
        if super::is_international(&active) {
            let activity = self.workbuddy_daily_activity(&active, WorkbuddyActivity::Full).await;
            let usage = match self.query_credits_summary(Some(&active), locale).await {
                Ok(value) => value,
                Err(error) => json!({ "error": error.message }),
            };
            return Ok(json!({
                "checkinStatus": activity.get("status").cloned().unwrap_or(Value::Null),
                "claim": activity.get("claim").cloned().unwrap_or(Value::Null),
                "activity": activity.get("activity").cloned().unwrap_or(Value::Null),
                "usage": usage,
            }));
        }
        assert_checkin_supported(&active)?;

        // 查状态与领取：Node 并发，这里顺序（理由见上）。
        // 领取失败不抛出 —— 收敛成 `{success:false, code:-1, msg}`，与 Node 的
        // `.catch(error => ({ success:false, code:-1, msg: error.message }))` 一致
        let status = self.get_checkin_status(Some(&active)).await?;
        let claim = match self.claim_daily_checkin(Some(&active)).await {
            Ok(value) => value,
            Err(error) => json!({ "success": false, "code": -1, "msg": error.message }),
        };
        // 额度可能因签到变化，稍等一下再查（对照 Node 的 sleep(500)）
        if claim.get("success").and_then(Value::as_bool).unwrap_or(false) {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        let usage = match self.query_credits_summary(Some(&active), locale).await {
            Ok(value) => value,
            Err(error) => json!({ "error": error.message }),
        };
        Ok(json!({
            "checkinStatus": status,
            "claim": claim,
            "usage": usage,
        }))
    }

    // ─── WorkBuddy 国际版：每日活跃任务 ─────────────────────

    /// 国际版每日活跃任务：活动探测 → 条件领取 → 免费模型保活 → 网页会话。
    ///
    /// [`WorkbuddyActivity`] 决定这次跑到哪一步 —— 界面上「保活 / 领取 /
    /// 保活+领取」三颗按钮各对应一种粒度；**定时与批量签到恒走 [`WorkbuddyActivity::Full`]**
    /// （自动任务要完整跑一遍，粒度细分只是手动入口的事）。
    ///
    /// 参考客户端把这三步挂在既有签到调度器上（rockswang/wild-work 的执行顺序），
    /// 本仓照同一顺序编排，并有三条纪律：
    ///   * 国际版活动未开放属于**正常状态**，所有探测/保活失败都收敛为结果字段
    ///     并记 verbose 日志，不让自动签到任务因为上游活动开关而中断；
    ///   * 活跃保活成功**不伪造普通签到成功** —— 调用方只有在 `claim.success`
    ///     或 `alreadyCompleted` 时才落 `checkinAt`（见 `billing::checkin` 的
    ///     `checkin_completed_today`），保活读数单独放在 `activity` 段；
    ///   * 领取只在「活动开放且今日未领」时打：`todayCheckedIn` 直接给
    ///     `alreadyCompleted`（上游幂等，但少一发是一发），活动没开就不打领取。
    ///
    /// 保活的模型链来自 `keepalive::models()`（签到中心可自定义，空回落缺省链），
    /// 逐个尝试、第一个成功的收口（见 `poke_daily_activity` 与
    /// `create_web_conversation`）。
    pub async fn workbuddy_daily_activity(&self, session: &Value, mode: WorkbuddyActivity) -> Value {
        // 只保活：跳过探测与领取。活动状态没查过，读数给 **null** 而不是 false
        // —— 「没查」与「查了说没开」是两回事；前端只认 `pokeSucceeded`。
        // 保活三件事：活跃上报（点亮连登的那条 /v2/report 通道）、桌面端
        // 免费模型对话（维持「账号有调用」）、网页通道会话（issue #277：
        // **每日活跃计数只认它**）—— 全都做，读数分别透出。
        if mode == WorkbuddyActivity::Keepalive {
            let report = self.report_chat_activity(session).await;
            let (poke_succeeded, poke_model) = self.poke_daily_activity(session).await;
            let web = self.create_web_conversation(session).await;
            let web_ok = web.get("succeeded").and_then(Value::as_bool) == Some(true);
            if !poke_succeeded && !web_ok {
                logging::verbose("[Checkin]", "WorkBuddy 国际版免费模型与网页会话保活均未成功");
            }
            // msg 交代两发保活各自的走向：网页会话才是「当日活跃」的判定
            // 通道，桌面端那发失败不致命，网页会话失败要给出原因。
            let msg = if web_ok {
                "活跃保活完成（网页会话已建）".to_string()
            } else if poke_succeeded {
                format!(
                    "免费模型保活完成，但网页会话失败：{}",
                    web.get("msg").and_then(Value::as_str).unwrap_or("未知原因")
                )
            } else {
                "活跃保活失败".to_string()
            };
            return json!({
                "status": Value::Null,
                "claim": {
                    "success": false,
                    "code": 0,
                    "msg": msg,
                },
                "activity": {
                    "mode": mode.as_str(),
                    "active": Value::Null,
                    "todayCheckedIn": Value::Null,
                    "statusAvailable": Value::Null,
                    "report": report,
                    "pokeSucceeded": poke_succeeded,
                    "pokeModel": poke_model,
                    "webConversation": web,
                },
            });
        }

        let status = self
            .call_billing(
                BILLING_ACTIVITY_CHECKIN_STATUS,
                CallOptions {
                    session: Some(session),
                    expect_code_ok: false,
                    ..Default::default()
                },
            )
            .await;

        let (status_available, active, today_checked_in, status_value) = match status {
            Ok(result) => {
                let status_available = result.code == Some(RESPONSE_CODE_OK);
                let active = result
                    .data
                    .get("active")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let today_checked_in = result
                    .data
                    .get("today_checked_in")
                    .or_else(|| result.data.get("todayCheckedIn"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !status_available {
                    logging::verbose(
                        "[Checkin]",
                        &format!(
                            "WorkBuddy 国际版活跃探测未开启（code={}）",
                            result.code.unwrap_or(-1)
                        ),
                    );
                }
                (
                    status_available,
                    active,
                    today_checked_in,
                    normalize_activity_status(&result),
                )
            }
            Err(error) => {
                logging::verbose(
                    "[Checkin]",
                    &format!("WorkBuddy 国际版活跃探测失败: {}", error.message),
                );
                (
                    false,
                    false,
                    false,
                    json!({
                        "active": false,
                        "todayCheckedIn": false,
                        "statusAvailable": false,
                        "code": -1,
                        "msg": error.message,
                    }),
                )
            }
        };

        // 活跃上报（Claim / Full 都做，Keepalive 不做）。
        //
        // 这是国际版每日活跃的**主动作**：`POST /v2/report` 的一条
        // chat_request_send 事件点亮成长中心连登（growth streak）。实测
        // （2026-10，真实账号）上报后 streak 从 0 → 1；而签到活动探测的
        // `active` 字段与免费模型保活对话都**不**点亮连登 —— 后者只是维持
        // 「账号有调用」，上游的活跃计数只认 /v2/report 事件。
        // 上报按天幂等（同一自然日重复上报不重复计分），失败不阻断领取分支。
        let report = self.report_chat_activity(session).await;

        let mut claim = if today_checked_in {
            // 上游幂等，但「今日已领」就不用再打一发领取 —— 少一发是一发。
            json!({
                "success": false,
                "code": 0,
                "msg": "今日已领取",
                "alreadyCompleted": true,
            })
        } else if active {
            match self
                .call_billing(
                    BILLING_DAILY_CHECKIN,
                    CallOptions {
                        session: Some(session),
                        expect_code_ok: false,
                        ..Default::default()
                    },
                )
                .await
            {
                Ok(result) => super::normalize_daily_claim(result),
                Err(error) => {
                    logging::verbose(
                        "[Checkin]",
                        &format!("WorkBuddy 国际版活跃奖励领取失败: {}", error.message),
                    );
                    json!({ "success": false, "code": -1, "msg": error.message })
                }
            }
        } else {
            json!({ "success": false, "code": 0, "msg": "活动未开启" })
        };
        let claim_succeeded = claim
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        // 保活只在完整模式做：「只领取」的用户明确不多打这次对话。
        // pokeSucceeded 是 bool | null —— null = 这次没跑保活（不是失败）。
        // 网页会话与保活对话同一档粒度（都是「保活」的一部分，issue #277），
        // 只在 Full 跑；它的成败不影响 claim（那是探测/领取的事），只透出读数。
        let (poke_succeeded, poke_model, web) = if mode == WorkbuddyActivity::Full {
            let (succeeded, model) = self.poke_daily_activity(session).await;
            if !succeeded {
                logging::verbose("[Checkin]", "WorkBuddy 国际版免费模型活跃保活未成功");
            }
            let web = self.create_web_conversation(session).await;
            if web.get("succeeded").and_then(Value::as_bool) != Some(true) {
                logging::verbose("[Checkin]", "WorkBuddy 国际版网页会话保活未成功");
            }
            (
                Value::Bool(succeeded),
                model.map(Value::String).unwrap_or(Value::Null),
                web,
            )
        } else {
            (Value::Null, Value::Null, Value::Null)
        };
        let web_ok = web.get("succeeded").and_then(Value::as_bool) == Some(true);

        // 领取没有明确的成功/已领读数时，msg 给出这一轮**实际发生**的事。
        // ⚠️ 完整模式里「上报 + 保活成功」必须把「活动未开启」说在同一句里：
        // 能走到这里的分支必然 !active（活动没开放），只写「已完成活跃上报」
        // 会让人以为积分该加没加 —— 两个事实都交代，提示才不会自相矛盾。
        // 保活完成 = 两发任一建成（网页会话是「当日活跃」的判定通道，桌面端
        // 对话维持「账号有调用」，互补而非冗余）。
        // 「只领取」模式没有保活可讲，msg 保留探测/领取的原话（活动未开启 /
        // 具体错误），那正是这颗按钮唯一要回答的问题。
        if mode == WorkbuddyActivity::Full && !claim_succeeded && !today_checked_in && !active {
            let report_ok = report.get("succeeded").and_then(Value::as_bool) == Some(true);
            let poke = poke_succeeded.as_bool().unwrap_or(false);
            claim["msg"] = Value::String(if report_ok && (poke || web_ok) {
                "已完成活跃上报（连登已点亮）与保活，但活动未开启（本次没有可领的奖励）".to_string()
            } else if report_ok {
                "活跃上报完成，但保活未成功且活动未开启".to_string()
            } else if poke || web_ok {
                "活跃保活完成，但活跃上报失败且活动未开启".to_string()
            } else {
                "活跃上报与保活均未成功，且活动未开启".to_string()
            });
        }

        json!({
            "status": status_value,
            "claim": claim,
            "activity": {
                "mode": mode.as_str(),
                "active": active,
                "todayCheckedIn": today_checked_in,
                "statusAvailable": status_available,
                "report": report,
                "pokeSucceeded": poke_succeeded,
                "pokeModel": poke_model,
                "webConversation": web,
            },
        })
    }

    /// 国际版对话活跃上报：`POST {endpoint}/v2/report` 的一条
    /// `chat_request_send` 事件。
    ///
    /// 这是点亮成长中心连登（growth streak）的唯一实测通道（见
    /// `BILLING_REPORT_ACTIVITY` 的注释与 `workbuddy_daily_activity` 的编排）。
    ///
    /// 事件形状照抄官方客户端的完整 24 字段（conversationId / mode /
    /// inputLength / requestModelId …），**`userId` 必填**（= 账号 uid）——
    /// 缺失时上游返回 200 但**静默丢弃**事件（参考实现 REPORT-active-map
    /// 的实测记录，本仓 streak 回读闭环同款验证）。conversationId 无需真实
    /// 会话，随机生成即可；每号每天一条（按天幂等），多号之间靠调度串行自然
    /// 限速，这里不再额外加 sleep。
    ///
    /// 返回 `{succeeded: bool, streakDays: number|null, msg: string}`：
    /// `streakDays` 是上报后的回读（`GET /activity/growth/streak`），null 表示
    /// 回读失败 —— 回读是**只读 oracle**（参考实现同款口径），失败不撤销
    /// 上报本身的结果，也不做重试（上报按天幂等，明天还会再发）。
    async fn report_chat_activity(&self, session: &Value) -> Value {
        let uid = session
            .get("account")
            .and_then(|account| account.get("uid"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if uid.is_empty() {
            return json!({
                "succeeded": false,
                "streakDays": Value::Null,
                "msg": "会话缺少账号 uid，无法上报活跃",
            });
        }
        let conversation_id = format!("agent2api-{}", new_request_id());
        let now = logging::now_ms();
        let event = json!({
            "eventCode": "chat_request_send",
            "timestamp": now,
            "reportDelay": 0,
            "mode": "craft",
            "conversationId": conversation_id,
            "requestId": conversation_id,
            "inputLength": 12,
            "requestModelId": "deepseek-v4.1-flash",
            "requestModelName": "DeepSeek V4.1 Flash",
            "isPlan": false,
            "isAutoExecuteTerminal": false,
            "isAutoModify": false,
            "codebaseEnable": false,
            "maxToken": 0,
            "maxSteps": 0,
            "temperature": 0,
            "maxRetries": 0,
            "mentionContexts": [],
            "knowledgeId": [],
            "knowledgeName": [],
            "codebaseId": "",
            "mentionContextCount": 0,
            "command": "",
            "expertId": "",
            "recommendId": "",
            "skillId": "",
            "skillCount": 0,
            "totalCount": 0,
            "fileUri": "",
            "presentAt": now,
            "traceId": "",
            "rootRequestId": conversation_id,
            "parentConversationId": conversation_id,
            "agentName": "default",
            "agentType": "conversation",
            "userId": uid,
        });
        let body = json!([event]);
        let result = self
            .call_billing(
                BILLING_REPORT_ACTIVITY,
                CallOptions {
                    session: Some(session),
                    body: Some(&body),
                    expect_code_ok: true,
                    ..Default::default()
                },
            )
            .await;
        let succeeded = match &result {
            Ok(_) => true,
            Err(error) => {
                logging::verbose(
                    "[Checkin]",
                    &format!("WorkBuddy 国际版活跃上报失败: {}", error.message),
                );
                false
            }
        };
        // 回读连登天数（只读 oracle）：上报 200 ≠ 计分，闭环验证用。
        let streak_days = if succeeded {
            match self.read_growth_streak(session).await {
                Ok(days) => Some(days),
                Err(error) => {
                    logging::verbose(
                        "[Checkin]",
                        &format!("WorkBuddy 国际版连登回读失败（上报本身已成功）: {}", error.message),
                    );
                    None
                }
            }
        } else {
            None
        };
        let streak_text = streak_days
            .map(|days| format!("，连登 {days} 天"))
            .unwrap_or_default();
        json!({
            "succeeded": succeeded,
            "streakDays": streak_days.map(Value::from).unwrap_or(Value::Null),
            "msg": if succeeded {
                format!("活跃上报成功{streak_text}")
            } else {
                "活跃上报失败".to_string()
            },
        })
    }

    /// 回读成长中心连登状态（`GET /activity/growth/streak`），返回
    /// `streak.days`。只读、失败即 Err，调用方决定是否容忍。
    async fn read_growth_streak(&self, session: &Value) -> Result<i64, BillingError> {
        let result = self
            .call_billing(
                GROWTH_STREAK_STATUS,
                CallOptions {
                    session: Some(session),
                    expect_code_ok: true,
                    ..Default::default()
                },
            )
            .await?;
        Ok(result
            .data
            .get("streak")
            .and_then(|streak| streak.get("days"))
            .and_then(Value::as_i64)
            .unwrap_or(0))
    }

    /// 用免费模型发一次最小流式对话，维持账号「活跃」。
    ///
    /// 模型链按 `keepalive::models()` 逐个尝试，第一个拿到 2xx 且响应体可读的
    /// 收口；每个模型的请求与读体各自限时（`TIMEOUT_MS`）—— 保活挂在批量签到
    /// 的串行链上，一个挂住的模型不能把整轮拖住。响应体读到 `MAX_POKE_BYTES`
    /// 就算数（流式 SSE 没必要读完，但要**读**：不读的请求上游可能没记活跃）。
    async fn poke_daily_activity(&self, session: &Value) -> (bool, Option<String>) {
        const TIMEOUT_MS: u64 = 20_000;
        const MAX_POKE_BYTES: usize = 1 << 20;
        for model in super::keepalive::models() {
            let plan = match crate::server::core::providers::workbuddy::keepalive::build_daily_activity_request(
                session, &model,
            ) {
                Ok(plan) => plan,
                Err(error) => {
                    logging::verbose("[Checkin]", &error);
                    continue;
                }
            };
            let response = match tokio::time::timeout(
                std::time::Duration::from_millis(TIMEOUT_MS),
                crate::server::core::upstream::request::send_chat_request(&plan),
            )
            .await
            {
                Ok(Ok(response)) => response,
                Ok(Err(error)) => {
                    logging::verbose(
                        "[Checkin]",
                        &format!(
                            "WorkBuddy 国际版活跃模型 {model} 请求失败: {}",
                            error.message
                        ),
                    );
                    continue;
                }
                Err(_) => {
                    logging::verbose(
                        "[Checkin]",
                        &format!("WorkBuddy 国际版活跃模型 {model} 请求超时"),
                    );
                    continue;
                }
            };
            let status = response.status();
            let body_ok = tokio::time::timeout(
                std::time::Duration::from_millis(TIMEOUT_MS),
                consume_activity_response(response, MAX_POKE_BYTES),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(false);
            if !(200..300).contains(&status.as_u16()) || !body_ok {
                logging::verbose(
                    "[Checkin]",
                    &format!(
                        "WorkBuddy 国际版活跃模型 {model} 返回 HTTP {}",
                        status.as_u16()
                    ),
                );
                continue;
            }
            logging::verbose(
                "[Checkin]",
                &format!("WorkBuddy 国际版活跃保活成功（model={model}）"),
            );
            return (true, Some(model));
        }
        (false, None)
    }

    /// 网页通道建一个带 prompt 的 agent 会话（issue #277 的修复主体）。
    ///
    /// 国际版上游把「当日活跃」（每日 30 积分的判定）记在网页通道的会话上：
    /// 参考实现 workbuddy2api-hub 的抓包结论 —— `POST /console/as/conversations/`
    /// 建会话即计活跃，桌面端 `chat/completions` 与 `/v2/report` 上报都**不进
    /// 这个计数**（后者只点亮成长中心连登）。网页链路只认 Bearer + X-User-Id
    /// 两个头（见 `keepalive::build_web_conversation_request`），同一份账号
    /// 凭据直接可用，无需网页登录。
    ///
    /// 模型沿用 `keepalive::models()`（与桌面端保活同一份免费链）逐个尝试，
    /// 第一个建成的收口；成功判定沿用参考实现口径 —— 响应 `code ∈ {0, 缺失}`
    /// 且 `data.id` 存在。HTTP 非 2xx / 业务码非 0 / 建不出会话换下一个模型，
    /// 全部失败收敛为 `succeeded:false`，不阻断领取分支。请求与读体各自限时
    /// （保活挂在批量签到的串行链上，一个挂住的模型不能把整轮拖住）；响应是
    /// 一次性 JSON，读满 `MAX_WEB_BYTES` 即截断（上游正常响应远小于上限）。
    ///
    /// 返回 `{succeeded: bool, conversationId: string|null, msg: string}`，
    /// 原样进 `activity.webConversation` 给前端与台账读。
    async fn create_web_conversation(&self, session: &Value) -> Value {
        const TIMEOUT_MS: u64 = 20_000;
        const MAX_WEB_BYTES: usize = 1 << 16;
        for model in super::keepalive::models() {
            let plan = match crate::server::core::providers::workbuddy::keepalive::build_web_conversation_request(
                session, &model,
            ) {
                Ok(plan) => plan,
                // 构造失败是凭据层面的缺失（缺 token / uid），换模型也救不了，直接收口
                Err(error) => {
                    logging::verbose(
                        "[Checkin]",
                        &format!("WorkBuddy 国际版网页会话请求构造失败: {error}"),
                    );
                    return json!({ "succeeded": false, "conversationId": Value::Null, "msg": error });
                }
            };
            let response = match tokio::time::timeout(
                std::time::Duration::from_millis(TIMEOUT_MS),
                crate::server::core::upstream::request::send_chat_request(&plan),
            )
            .await
            {
                Ok(Ok(response)) => response,
                Ok(Err(error)) => {
                    logging::verbose(
                        "[Checkin]",
                        &format!(
                            "WorkBuddy 国际版网页会话（{model}）请求失败: {}",
                            error.message
                        ),
                    );
                    continue;
                }
                Err(_) => {
                    logging::verbose(
                        "[Checkin]",
                        &format!("WorkBuddy 国际版网页会话（{model}）请求超时"),
                    );
                    continue;
                }
            };
            let status = response.status();
            if !(200..300).contains(&status.as_u16()) {
                logging::verbose(
                    "[Checkin]",
                    &format!(
                        "WorkBuddy 国际版网页会话（{model}）返回 HTTP {}",
                        status.as_u16()
                    ),
                );
                continue;
            }
            let body = match tokio::time::timeout(
                std::time::Duration::from_millis(TIMEOUT_MS),
                consume_response_bytes(response, MAX_WEB_BYTES),
            )
            .await
            {
                Ok(Ok(body)) => body,
                _ => {
                    logging::verbose(
                        "[Checkin]",
                        &format!("WorkBuddy 国际版网页会话（{model}）响应读取失败"),
                    );
                    continue;
                }
            };
            let payload: Value = match serde_json::from_slice(&body) {
                Ok(value) => value,
                Err(error) => {
                    logging::verbose(
                        "[Checkin]",
                        &format!("WorkBuddy 国际版网页会话（{model}）响应不是 JSON: {error}"),
                    );
                    continue;
                }
            };
            // 参考实现的判定口径：code 缺失 / null / 0 都算业务成功（其余是失败）
            let code_ok = match payload.get("code") {
                None | Some(Value::Null) => true,
                Some(code) => code.as_i64() == Some(0),
            };
            let conversation_id = payload
                .get("data")
                .and_then(|data| data.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if !code_ok || conversation_id.is_empty() {
                let msg = payload.get("msg").and_then(Value::as_str).unwrap_or("未知原因");
                logging::verbose(
                    "[Checkin]",
                    &format!(
                        "WorkBuddy 国际版网页会话（{model}）未建成: msg={msg}"
                    ),
                );
                continue;
            }
            logging::verbose(
                "[Checkin]",
                &format!("WorkBuddy 国际版网页会话建成（model={model}）"),
            );
            return json!({
                "succeeded": true,
                "conversationId": conversation_id,
                "model": model,
                "msg": "网页会话已创建",
            });
        }
        json!({
            "succeeded": false,
            "conversationId": Value::Null,
            "msg": "网页会话创建失败（模型链全部未建成）",
        })
    }
}

/// 国际版活跃探测响应归一化为管理 API 能直接消费的形态。
///
/// 上游字段是蛇形（`today_checked_in` / `daily_credit`），归一成驼峰与国内版
/// 签到状态的键风格一致；`statusAvailable` 区分「接口说不活动」与「接口本身
/// 没答上」—— 前者是正常状态，后者要留给排查。原始 data 原样带出（`raw`）。
pub(super) fn normalize_activity_status(result: &BillingCall) -> Value {
    let data = &result.data;
    json!({
        "active": data
            .get("active")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        "todayCheckedIn": data
            .get("today_checked_in")
            .or_else(|| data.get("todayCheckedIn"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        "dailyCredit": data
            .get("daily_credit")
            .or_else(|| data.get("dailyCredit"))
            .cloned()
            .unwrap_or(Value::Null),
        "statusAvailable": result.code == Some(RESPONSE_CODE_OK) && !data.is_null(),
        "code": result.code,
        "msg": result.msg,
        "raw": data,
    })
}

/// 消费活跃保活的 SSE 响应，限制最大读取量，避免上游异常时无界缓冲。
///
/// 读满 `max_bytes` 或流自然结束都算成功（返回 Ok(true)）；网络层断流 /
/// 非法字节返回 Err，由调用方按该模型失败处理并尝试链上的下一个。
async fn consume_activity_response(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<bool, ()> {
    consume_response_bytes(response, max_bytes).await.map(|_| true)
}

/// 有界读体：最多读 `max_bytes`（超出即截断返回），网络层断流返回 Err。
///
/// SSE 那路只求「读过了」，JSON 那路（网页会话）要拿到完整字节去解析 ——
/// 同一条纪律：响应体必须**读**（不读的请求上游可能没记活跃），但绝不无界缓冲。
async fn consume_response_bytes(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, ()> {
    use futures::StreamExt;

    let mut stream = response.bytes_stream();
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ())?;
        let remaining = max_bytes - body.len();
        let take = chunk.len().min(remaining);
        body.extend_from_slice(&chunk[..take]);
        if body.len() >= max_bytes {
            return Ok(body);
        }
    }
    Ok(body)
}
