//! ZCode 限时套餐的领取接口（探测 + 领取）。
//!
//! ── 为什么单独成文件（不在 `api::accounts` 里）────────────────
//! `api::accounts` 已经是账号 CRUD + 十来个动作的入口（接近 550 行），
//! 而领取这条链路自带一段**协议知识**（biz code 分类、404 是正常状态、
//! 验证码由前端解），塞进去会让那个文件同时承担「账号簿记」与「套餐协议」
//! 两件事。与 `api::accounts_usage`（余额查询单独拆出去）同一处置。
//!
//! ── 三个接口 ────────────────────────────────────────────────
//! ```text
//!   POST /api/accounts/{id}/zcode-claim/preview          探测可领套餐（**不要验证码**）
//!   POST /api/accounts/{id}/zcode-claim/captcha-config   取风控配置（滑块要不要弹）
//!   POST /api/accounts/{id}/zcode-claim                  领取（**要**验证码参数）
//! ```
//! 拆成两个而不是「一个接口两段」的理由：探测是幂等的只读动作，可以放心反复跑、
//! 也可以在界面上随便刷新；领取是有副作用的写动作，且必须等前端把验证码解出来
//! 才能发。两者的调用时机与失败语义完全不同。
//!
//! ── 这里只做转发 ────────────────────────────────────────────
//! 执行体在**提供商插件**里（`zcode::claim` / `zcode::activation`，动作
//! `claim.preview` / `claim.captcha-config` / `claim.claim`）：biz code 怎么
//! 分类、激活事件长什么样、请求头怎么拼，都是「怎么打 ZCode 的活动接口」的
//! 知识 —— 按项目的风险隔离要求，这类实现不留在本仓库。网关这边只剩账号归属
//! 校验、`claimedAt` 落库（那动的是网关的账号库）与信封。
//!
//! ── 「每天领一次」与「今天已领」的落点 ──────────────────────
//! 2026-09-28 起的那期活动（ZCode Trust Build）是**每天一个新套餐**
//! （`plan_id` 带日期段），所以领取这件事天然按自然日重复。本文件负责把结果
//! 写进账号记录（`claimAt` / `claimPlanId`，见 `mark_zcode_claim`）：
//!   - **成功**要写；
//!   - **`already_claimed` 也要写** —— 上游说这一期的套餐已经领掉了
//!     （可能是另一台设备领的），界面同样该显示「今日已领」，
//!     否则用户会一直点那颗按钮、每次拿回同一句「该账号已领取过」。
//! 响应里带上落库后的状态，前端不必为了刷新那颗按钮再拉一次账号列表。
//!
//! ── 定时领取与手动领取 ──────────────────────────────────────
//! 手动领取由本接口接收界面当次验证结果；定时领取由插件按需发布验证任务，
//! 管理界面经 `zcode_claim_verification` 回传，继续同一轮领取和签到记账。
//! 两条路径都不复用模型调用令牌池，没有界面或验证未完成时如实记录需要人工验证。
//!
//! ── 激活事件：领取链上的**前置动作** ─────────────────────────
//! 资格由上游的激活事件（`app_launch` / `app_daily_active`）发放，而本家不跑
//! 官方客户端、那一步不会自动发生。因此两个入口都会补报（插件的
//! `claim.preview` / `claim.claim` 里）：
//!   · `claim`：领取前必报（失败不阻断）；
//!   · `preview`：**探测为空时**报一次再重探（列表非空时不报，读写分开的口径
//!     因此只在这一处被打破）。
//! 响应里的 `activated` / `activationError` 就是这两件事的读数。
//!
//! ── 出口要不要挂账号代理：**要**（与「余额查询直连」相反）────────
//! 领取打的是活动接口，风控按 IP 判；出口由插件从账号记录里取，网关不参与。

use axum::body::Bytes;
use axum::response::Response;
use serde_json::Value;

use crate::server::errors::management_error;
use crate::server::http::{ok_json, parse_body};
use crate::server::ServerState;

/// 取账号记录（不是 ZCode 系一律报错）。
fn zcode_account_of(
    state: &ServerState,
    account_id: &str,
) -> Result<Value, (i32, String)> {
    let record = state
        .store()
        .list_accounts()
        .get("accounts")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|account| account.get("id").and_then(Value::as_str) == Some(account_id))
        })
        .cloned()
        .ok_or_else(|| (404, "找不到该 ZCode 账号".to_string()))?;
    let provider = record
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !crate::server::core::providers::zcode::region::Region::from_provider_id(provider).is_some()
    {
        return Err((400, "该账号不是 ZCode 账号".to_string()));
    }
    Ok(record)
}

/// 调一次管理动作。
async fn call_action(
    state: &ServerState,
    account_id: &str,
    action: &str,
    payload: &Value,
) -> Result<Value, (i32, String)> {
    let provider = state
        .store()
        .account_provider_of(account_id)
        .ok_or_else(|| (404, "找不到该 ZCode 账号".to_string()))?;
    let Some(adapter) = crate::server::core::providers::adapter::adapter_by_id(&provider) else {
        return Err((
            400,
            format!(
                "{}的提供商插件没有安装，无法执行套餐动作",
                crate::server::core::providers::label_of(&provider)
            ),
        ));
    };
    adapter
        .admin_action(state.store(), account_id, action, payload)
        .await
        .map_err(|error| (error.status_code, error.message))
}

/// `POST /api/accounts/{id}/zcode-claim/preview`：探测当前可领的套餐。
///
/// 成功响应的形状（前端据此渲染列表与「领取」按钮）：
/// ```json
/// { "plans": [ { "planId", "name", "description", "priority",
///                "startsAt", "endsAt", "entitlements": [...] } ],
///   "deployed": true, "activated": true, "activationError": null }
/// ```
/// `deployed: false` 表示上游活动接口尚未部署（404）—— **这不是错误**，
/// 是活动开抢前的正常状态。前端应当据此显示「当前没有可领套餐」而不是报错。
///
/// ── `activated` / `activationError` 的取值 ────────────────────
///   · `null` —— 没报（探测到了套餐：不必报；接口未部署：报也无从解锁；
///     账号没有 `userId`：报不出去）。**也没出错**；
///   · `true` / `false` —— 报了 / 报失败（原因在第二个字段里）。
pub async fn preview(state: &ServerState, account_id: &str) -> Response {
    if let Err((status, message)) = zcode_account_of(state, account_id) {
        return management_error(status, message);
    }
    match call_action(state, account_id, "claim.preview", &Value::Null).await {
        Ok(body) => {
            record_daily_checkin(state, account_id, &body);
            ok_json(body)
        },
        Err((status, message)) => management_error(status, message),
    }
}

/// `POST /api/accounts/{id}/zcode-claim/captcha-config`：取阿里云风控配置。
///
/// 前端拿它去初始化滑块 SDK（`prefix` / `region` / `sceneId` 三样缺一不可）。
/// 与 AutoClaw OAuth 的 `/api/session/login/oauth/captcha-config` 是同一个位置
/// 的同一件事，只是来源不同。
///
/// ── 不缓存 ──────────────────────────────────────────────────
/// 领取是低频动作（一次领一次），不值得在这里维护带 TTL 的进程级缓存。
///
/// ── 这一支**不要求账号有 jwt** ──────────────────────────────
/// 风控配置是「上游此刻要不要验证码」的公共信息，与哪个账号无关。提前报
/// 「缺 jwt」会让用户以为账号有问题，而真正的原因是他还没登录过。
pub async fn captcha_config(state: &ServerState, account_id: &str) -> Response {
    if let Err((status, message)) = zcode_account_of(state, account_id) {
        return management_error(status, message);
    }
    match call_action(state, account_id, "claim.captcha-config", &Value::Null).await {
        Ok(body) => ok_json(body),
        Err((status, message)) => management_error(status, message),
    }
}

/// `POST /api/accounts/{id}/zcode-claim`：领取一个套餐。
///
/// 请求体：
/// ```json
/// { "planId": "...", "captchaVerifyParam": "...", "captchaRegion": "..." }
/// ```
/// `planId` 省略或为空时取**优先级最高**的那个可领套餐（插件侧实现）。
/// 因此前端可以「先探测、用户直接点领取」，也可以「探测后让用户选一个再领」。
///
/// ── 失败也返回 HTTP 200（业务结果不是 HTTP 错误）─────────────
/// 「已领过」「额度用尽」「验证码没过」都是**正常的业务结果**，前端要靠
/// `failure` 字段给不同提示。只有「请求本身没打成」（账号不存在、缺 jwt、
/// 网络故障）才用非 2xx。
pub async fn claim_plan(state: &ServerState, account_id: &str, body: &Bytes) -> Response {
    if let Err((status, message)) = zcode_account_of(state, account_id) {
        return management_error(status, message);
    }
    let payload = parse_body(body).ok().unwrap_or(Value::Null);
    let outcome = match call_action(state, account_id, "claim.claim", &payload).await {
        Ok(outcome) => outcome,
        Err((status, message)) => return management_error(status, message),
    };
    record_daily_checkin(state, account_id, &outcome);
    if let Some(object) = outcome.as_object() {
        let ok = object.get("ok").and_then(Value::as_bool).unwrap_or(false);
        let failure = object.get("failure").and_then(Value::as_str).unwrap_or("");
        let plan_id = object.get("planId").and_then(Value::as_str).unwrap_or("");
        // ── 落一次领取状态 ────────────────────────────────────────
        // 成功的、以及「上游说已领过」，都要写：后者同样是「这一期到手了」。
        // 其余失败不落 —— 验证码没过、额度耗尽都不意味着套餐已经领到。
        let claimed_at = if ok || failure == "already_claimed" {
            record_claim(state, account_id, plan_id, object.get("claimedAt").and_then(Value::as_i64))
        } else {
            0
        };
        let mut result = outcome;
        if let Some(object) = result.as_object_mut() {
            object.insert("claimedAt".to_string(), Value::from(claimed_at));
        }
        return ok_json(result);
    }
    ok_json(outcome)
}

fn record_daily_checkin(state: &ServerState, account_id: &str, body: &Value) {
    let Some(snapshot) = body.get("dailyCheckin").filter(|value| value.is_object()) else { return; };
    if let Err(message) = state.store().plugin_set_ledger(account_id, "dailyCheckin", snapshot) {
        crate::server::logging::verbose("[Claim]", &message);
    }
}

/// 落一次领取状态，返回落库的时刻（毫秒；0 = 没落成，界面按「未领取」处理）。
///
/// 写盘失败只记日志、不改领取结果：额度在上游已经到账，因为一次落盘失败把
/// 成功的领取报成失败是本末倒置（与 `billing::checkin::run_checkin` 同一口径）。
fn record_claim(state: &ServerState, account_id: &str, plan_id: &str, claimed_at: Option<i64>) -> i64 {
    let now = crate::server::logging::now_ms();
    let at = claimed_at.filter(|at| *at > 0 && *at <= now).unwrap_or(now);
    if state.store().mark_zcode_claim(account_id, plan_id, at) {
        at
    } else {
        crate::server::logging::verbose(
            "[Claim]",
            &format!("账号 {account_id} 的领取时间未能落盘（账号可能已被删除）"),
        );
        0
    }
}
