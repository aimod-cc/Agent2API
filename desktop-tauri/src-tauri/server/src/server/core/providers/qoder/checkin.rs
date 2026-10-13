//! Qoder 每日签到（campaign 领取链路；中国版 / 国际版两条分支）。
//!
//! ── 上游长什么样（2026-09-27 在真实账号上实测）────────────────
//! 每日权益以**活动（campaign）**形式下发，走 `sash` 一族接口：
//!
//! ```text
//! GET  {openapi}/sash/api/v1/me/campaigns?forceRefresh=true
//!   → {uid, showCampaign, claimable, campaigns:[{campaignId, campaignKey,
//!        actionType, claimStatus, startAt, endAt,
//!        benefit:{kind:"CREDITS", amount:100,
//!                 validity:{mode:"RELATIVE_DAYS", days:30}}}]}
//! POST {openapi}/sash/api/v1/me/campaigns/{campaignId}/claim   body {}
//!   → {status:"CLAIMED", replayed, benefit:{...},
//!      claimedAt, grantedAt, expiresAt}      // 三个时间都是 ISO 字符串
//! ```
//!
//! 实测（中国版 Pro Trial 账号 `act-20260923-339`）：
//!   - 活动窗口 `startAt=1790474400 → endAt=1790560740`，即**当天 10:00 → 次日 09:59**
//!     （UTC+8），与官方客户端「每天 10:00 刷新」一致；
//!   - 领取返回 `{"status":"CLAIMED","replayed":false,"benefit":{...,"amount":100}}`，
//!     紧接着查 `/api/v2/quota/usage`，`addOnQuota` 从「上游不返回该键」变成
//!     `{total:100, remaining:100, unit:"credits"}` —— **签到额度落在 `addOnQuota`
//!     这个桶里**（余额面板「资源包」那一行，见 `balance.rs`）；
//!   - **领取后活动行不消失**，`claimStatus` 由 `CLAIMABLE` 变 `CLAIMED`。
//!     所以「今天已领取」直接从活动列表读，不需要本地记账；
//!   - 顶层的 `claimable` 标志**不能当判据**：领完之后它仍是 `true`（同账号还有一条
//!     可领的促销活动），必须按 `actionType` 逐行筛。
//!
//! ── 国际版为什么要两段拉取（issue #140，CreditDaddy 实测结论）──
//! 国际版服务端**只对带设备风控头**（`Cosy-MachineToken` / `Cosy-MachineCode` /
//! `Cosy-MachineType`，由本机 Qoder 客户端 / qodercli 的 UMID 组件生成，见
//! `risk.rs`）的请求下发「每天领 100 Credits」活动：不带风控头时活动列表里
//! 永远只有 `VIEW_DETAILS` 促销 —— 本家旧注释「国际版没有签到计划」的真正
//! 原因就是缺头，不是没活动。风控身份按 **uid** 生成，而 uid 在活动列表的
//! **顶层字段**里，所以顺序必须是：先拉一次拿 uid → 叠风控头重拉 → 再领。
//! legacy 的 `daily-check-in` 路径两个地区都已停用（国际版 404、中国版恒
//! `DISABLED`），一次都不碰。
//!
//! ── 为什么不走 legacy 的 daily-check-in ─────────────────────
//! `GET /sash/api/v1/me/daily-check-in/status` 的 `status` 恒为 `DISABLED`
//! （国际版该路径直接 404），streak 与累计全 0；它的 claim 兄弟端点已被上游
//! 全局停用（对未领取的日子也回 409 且不发积分）。因此这里**只读活动列表**，
//! 一次都不碰 legacy。
//!
//! ── 没有签到活动的账号 ─────────────────────────────────────
//! 中国版 Free 套餐实测没有活动下发（Pro/试用才有）：返回一条中性结果
//! （`success:false` + 说明），不是错误 —— 上游给不给活动是账号侧的状态。
//! 国际版缺 UMID 组件时同样给中性说明（缺什么、怎么补），把「没得领」报成
//! 失败会让用户以为接口坏了。
//!
//! ── panic=abort ────────────────────────────────────────────
//! 本文件零 unwrap/expect/panic，取值全走 Option 链。

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::core::auth_http::ApiResponse;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::errors::GatewayError;

use super::credentials::Credentials;
use super::{auth, endpoints, refresh, risk};
use super::endpoints::Region;

/// 活动类型：可领取权益（促销类活动是 `VIEW_DETAILS`，不是签到，不能领）
const ACTION_CLAIM_BENEFIT: &str = "CLAIM_BENEFIT";
/// 活动状态：可领取
const STATUS_CLAIMABLE: &str = "CLAIMABLE";
/// 活动状态：已领取
const STATUS_CLAIMED: &str = "CLAIMED";
/// 领取响应的 `result` 字段（上游在并发/重放时会给这个值）
const RESULT_ALREADY_CLAIMED: &str = "ALREADY_CLAIMED";
/// 上游没给 `benefit.amount` 时的缺省奖励额（实测就是 100）
const DEFAULT_REWARD: f64 = 100.0;

/// Qoder 的每日签到（中国版直领；国际版带设备风控身份两段拉取）。
///
/// 返回 `{success, msg, ...}` —— 与 WorkBuddy / 小浣熊 / AutoClaw 三家同形状
/// （见 `billing::checkin::claim_result`），于是汇总、日志与界面三处不需要
/// 为第四家再加分支。
///
/// `success` 的口径是「**本次真的领到了**」：
///   - 领到 → `true`，`msg` 带金额与有效期，另给 `rewardPoints`（界面显示「本次 +N」）
///     与 `unit`（单位是 credits，不是积分）；
///   - 今天已领 / 没有活动 → `false` + `msg`，其中「已领取」另带
///     `alreadyCompleted: true`，`billing::checkin` 据此落 `checkinAt`（行上的按钮
///     变成「已签到」）；「没有活动」不带它 —— 不能把「今天没得领」记成已签到。
pub async fn claim_daily_checkin(
    store: &AccountStore,
    region: Region,
    account_id: &str,
) -> Result<Value, GatewayError> {
    let mut credentials = refresh::ensure_fresh(store, region, account_id, false).await?;
    let (record, _) = refresh::snapshot(store, region, account_id)?;
    let proxy = auth::account_proxy(&record)?;

    let mut response = fetch_campaigns(&credentials, proxy.as_ref()).await?;
    if response.status == 401 && credentials.can_refresh() {
        credentials = refresh::ensure_fresh(store, region, account_id, true).await?;
        response = fetch_campaigns(&credentials, proxy.as_ref()).await?;
    }
    let list = auth::payload(response, "签到活动查询")?;

    // ── 国际版：先拿顶层 uid，叠风控头重拉一次（中国版不需要）────
    // 401 重拉后的重试同样要叠头 —— 那时令牌换了，但流程不变。
    let mut headers = endpoints::sash_headers(&credentials.access_token);
    let mut risk_note: Option<String> = None;
    if region == Region::Global {
        let uid = list.get("uid").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let (augmented, note) = risk::augment_headers(headers, region, &uid).await;
        headers = augmented;
        risk_note = note;
        let second = fetch_campaigns_with(&credentials, proxy.as_ref(), &headers).await?;
        if second.status == 401 && credentials.can_refresh() {
            credentials = refresh::ensure_fresh(store, region, account_id, true).await?;
            let third = fetch_campaigns(&credentials, proxy.as_ref()).await?;
            let third = intl_refetch(&credentials, proxy.as_ref(), third).await?;
            return claim_from(store, region, account_id, credentials, proxy, third, risk_note).await;
        }
        let second = auth::payload(second, "签到活动查询（风控重拉）")?;
        return claim_from(store, region, account_id, credentials, proxy, second, risk_note).await;
    }

    claim_from(store, region, account_id, credentials, proxy, list, risk_note).await
}

/// 国际版的活动列表重取（叠风控头）。首段的 `uid` 缺失（上游没给）时按空
/// uid 走 —— 风控身份生成不出来，头束退化为「只有客户端身份」，与缺组件
/// 同一表现（上游不下发积分活动，落到 `no_claim` 的说明文案）。
async fn intl_refetch(
    credentials: &Credentials,
    proxy: Option<&ResolvedProxy>,
    first: ApiResponse,
) -> Result<Value, GatewayError> {
    let first = auth::payload(first, "签到活动查询")?;
    let uid = first.get("uid").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let (headers, _) = risk::augment_headers(endpoints::sash_headers(&credentials.access_token), credentials.region, &uid).await;
    let second = fetch_campaigns_with(credentials, proxy, &headers).await?;
    auth::payload(second, "签到活动查询（风控重拉）")
}

/// 活动列表解析 + 领取（两个地区共用的收尾）。
async fn claim_from(
    store: &AccountStore,
    region: Region,
    account_id: &str,
    mut credentials: Credentials,
    proxy: Option<ResolvedProxy>,
    list: Value,
    risk_note: Option<String>,
) -> Result<Value, GatewayError> {
    let rows: Vec<Value> = list
        .get("campaigns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut claimable: Option<&Value> = None;
    let mut claimed = false;
    for row in &rows {
        if row.get("actionType").and_then(Value::as_str) != Some(ACTION_CLAIM_BENEFIT) {
            continue;
        }
        match row.get("claimStatus").and_then(Value::as_str) {
            Some(STATUS_CLAIMABLE) if claimable.is_none() => claimable = Some(row),
            Some(STATUS_CLAIMED) => claimed = true,
            _ => {}
        }
    }

    let Some(target) = claimable else {
        return Ok(no_claim(region, claimed, risk_note));
    };
    let campaign_id = target.get("campaignId").and_then(Value::as_str).unwrap_or("");
    if campaign_id.is_empty() {
        return Err(GatewayError::with_status(502, "Qoder 签到活动缺少活动标识，无法领取"));
    }

    let url = format!(
        "{}{}/{}/claim",
        region.open_api(),
        endpoints::CAMPAIGNS_PATH,
        campaign_id
    );
    let mut headers = endpoints::sash_headers(&credentials.access_token);
    if region == Region::Global {
        let uid = list.get("uid").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let (augmented, _) = risk::augment_headers(headers, region, &uid).await;
        headers = augmented;
    }
    let mut claim = claim_campaign(&url, &credentials, proxy.as_ref(), &headers).await?;
    // 上游并发/重放时给 409 + `{"result":"ALREADY_CLAIMED"}`（参考实现实测），
    // 那不是失败，是「今天已经领过了」。
    if !claim.ok && (claim.status == 409 || result_of(&claim).as_deref() == Some(RESULT_ALREADY_CLAIMED))
    {
        return Ok(json!({
            "success": false,
            "alreadyCompleted": true,
            "msg": "今日已领取",
        }));
    }
    if claim.status == 401 && credentials.can_refresh() {
        credentials = refresh::ensure_fresh(store, region, account_id, true).await?;
        // 重签一个新头束（令牌换了，旧头的 Bearer 已失效）
        let mut fresh_headers = endpoints::sash_headers(&credentials.access_token);
        if region == Region::Global {
            let uid = list.get("uid").and_then(Value::as_str).unwrap_or("").trim().to_string();
            let (augmented, _) = risk::augment_headers(fresh_headers, region, &uid).await;
            fresh_headers = augmented;
        }
        claim = claim_campaign(&url, &credentials, proxy.as_ref(), &fresh_headers).await?;
    }
    let body = auth::payload(claim, "签到领取")?;

    if body.get("status").and_then(Value::as_str) != Some(STATUS_CLAIMED) {
        let status = body.get("status").and_then(Value::as_str).unwrap_or("未知");
        return Ok(json!({ "success": false, "msg": format!("签到未完成（上游状态 {status}）") }));
    }
    if body.get("replayed").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(json!({
            "success": false,
            "alreadyCompleted": true,
            "msg": "今日已领取",
        }));
    }

    let reward = body
        .get("benefit")
        .and_then(|benefit| benefit.get("amount"))
        .and_then(Value::as_f64)
        .unwrap_or(DEFAULT_REWARD);
    let valid_days = body
        .get("benefit")
        .and_then(|benefit| benefit.get("validity"))
        .and_then(|validity| validity.get("days"))
        .and_then(Value::as_f64);
    // 文案不重复「签到成功」：调用方 `claim_result` 已经用它作日志前缀
    // （「账号 X: 签到成功（…）」），这里只说领到了什么。
    let msg = match valid_days {
        Some(days) if days > 0.0 => format!("获得 {reward} credits，{days} 天有效"),
        _ => format!("获得 {reward} credits"),
    };
    Ok(json!({
        "success": true,
        "msg": msg,
        "rewardPoints": reward,
        "unit": "credits",
    }))
}

/// 活动列表（默认头束：`forceRefresh` 拿实时 `claimStatus`）。
async fn fetch_campaigns(
    credentials: &Credentials,
    proxy: Option<&ResolvedProxy>,
) -> Result<ApiResponse, GatewayError> {
    let headers = endpoints::sash_headers(&credentials.access_token);
    fetch_campaigns_with(credentials, proxy, &headers).await
}

/// 活动列表（指定头束 —— 国际版第二段要叠风控头）。
async fn fetch_campaigns_with(
    credentials: &Credentials,
    proxy: Option<&ResolvedProxy>,
    headers: &[(String, String)],
) -> Result<ApiResponse, GatewayError> {
    let url = format!(
        "{}{}{}",
        credentials.region.open_api(),
        endpoints::CAMPAIGNS_PATH,
        endpoints::CAMPAIGNS_QUERY
    );
    auth::request("GET", &url, None, headers, proxy).await
}

/// 领取一个活动。请求体是空对象（抓包确认：无参数），`Origin` 指向该地区门户
/// （参考实现只在 POST 上带它）。
async fn claim_campaign(
    url: &str,
    credentials: &Credentials,
    proxy: Option<&ResolvedProxy>,
    headers: &[(String, String)],
) -> Result<ApiResponse, GatewayError> {
    let mut headers = headers.to_vec();
    headers.push(("content-type".to_string(), "application/json".to_string()));
    headers.push(("origin".to_string(), credentials.region.open_api().to_string()));
    auth::request("POST", url, Some(&json!({})), &headers, proxy).await
}

/// 领取响应的 `result` 字段（并发/重放标记），拿不到给 None。
fn result_of(response: &ApiResponse) -> Option<String> {
    let value = response.payload.as_ref()?.get("result")?.as_str()?.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// 「这次没领到」的结果：区分「今天已领」「没有可领的活动」与「缺风控身份」。
fn no_claim(region: Region, claimed: bool, risk_note: Option<String>) -> Value {
    if claimed {
        return json!({
            "success": false,
            "alreadyCompleted": true,
            "msg": "今日已领取",
        });
    }
    if region == Region::Global {
        // 国际版：要么缺 UMID 组件（风控头没带上，上游不下发积分活动），
        // 要么组件在但活动列表里此刻没有积分活动。10router 作者也在国际版
        // 上观察到「长期只回 VIEW_DETAILS 促销」（见它的 qoderCheckin.js），
        // 官方文档确认每日 100 Credits 的窗口每天 10:00（UTC+8）开启 ——
        // 文案如实说明现状与时机，不要暗示「已经领过」。
        if let Some(note) = risk_note {
            return json!({
                "success": false,
                "msg": format!("未领取：{note}。国际版签到需要本机安装 Qoder 客户端或 qodercli（Linux 可在签到中心一键安装组件）"),
            });
        }
        return json!({
            "success": false,
            "msg": "上游暂未给该账号下发每日积分活动（官方活动窗口每天 10:00 开启，若今天已在 Qoder 客户端领过则活动行会消失）。明天 10 点后再试",
        });
    }
    // 中国版没有活动是账号侧状态（实测 Free 套餐账号如此，Pro/试用账号才有），
    // 明确说「没有可领的」而不是报错。
    json!({
        "success": false,
        "msg": "当前没有可领取的签到活动",
    })
}
