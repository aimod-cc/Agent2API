//! LobsterAI 的每日活动签到：**三步、幂等靠客户端**。
//!
//! ── 三步协议（`credits.go:305-368`）──────────────────────────
//! ```text
//!   1. GET  /api/client-activities/slot?placement=…&clientVersion=…&…
//!        -> 找到「每日签到」那个活动位：slotState / activity.activityCode /
//!           activity.configRevision
//!   2. GET  /api/client-activities/{code}/context?configRevision=…
//!        -> 活动状态：state.claimedToday 与 actions[]（含不含 check_in）
//!   3. POST /api/client-activities/{code}/actions/check_in
//!        体 {configRevision, idempotencyKey:<uuid4>, payload:{}}
//! ```
//!
//! ── 幂等是**客户端**的事，所以两道前置检查都必须做 ──────────────
//! 上游没有「今天签过了就返回已签」的幂等语义（否则不需要 idempotencyKey）：
//! 幂等靠 ① 请求里那个 uuid4 的 `idempotencyKey`，② 第二步的两道预检
//! （`state.claimedToday` 与 `actions` 里有没有 `check_in`）。**两道都要**：
//! 只看 `claimedToday` 会漏掉「活动在、但今天不可签」那一格
//! （`credits.go:26-29` 的原话）。预检命中时如实回一句中性的「今天已签到」，
//! **不**再去打签到接口 —— 那会在上游不幂等的情况下把一天的奖励签重。
//!
//! ── 发放额字段有**三段回退** ─────────────────────────────────
//! `data.result` 里的奖励名随活动 / 版本变化：`creditsGranted` → `rewardCredits`
//! → `credits`，取第一个能读到的（`credits.go:349-360` 的原文注释）。
//!
//! ── 返回形状 ────────────────────────────────────────────────
//! `{success, msg, …}` —— 与 `atomcode/checkin.rs` 同款，交给
//! `billing::checkin::claim_result` 翻成签到结果行。「今天已签到」那句必须带
//! 「已签到」三字：`billing::checkin::checkin_completed_today` 是按 msg 子串
//! 判「当日已办」的（那样调度与台账才不会把一次中性结果当成失败去反复重打）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{Value, json};

use crate::server::core::account_store::AccountStore;
use crate::server::core::auth_http::send_raw;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::chat;
use super::credentials::{LobsterCredentials, from_record};
use super::endpoints::{
    REQUEST_TIMEOUT_MS, USER_AGENT, activity_checkin_url, activity_context_url, activity_slot_url,
};
use super::oauth::unwrap_envelope;
use super::uuid_v4;

/// 活动上下文里表示「今天可签」的动作名
pub const CHECK_IN_ACTION: &str = "check_in";

/// 活动位（第一步 `data` 里我们关心的三个字段）
#[derive(Clone, Debug, Default)]
struct ActivitySlot {
    /// `slotState`（`available` 才可能可签）
    slot_state: String,
    /// `activity.activityCode`
    activity_code: String,
    /// `activity.configRevision`（原样留着，写进第二步的 query 与第三步的 body）
    config_revision: Value,
}

/// 从活动位 `data` 里取三个字段
fn parse_slot(data: &Value) -> ActivitySlot {
    let activity = data.get("activity");
    ActivitySlot {
        slot_state: data
            .get("slotState")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string(),
        activity_code: activity
            .and_then(|value| value.get("activityCode"))
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string(),
        config_revision: activity
            .and_then(|value| value.get("configRevision"))
            .cloned()
            .unwrap_or(Value::Null),
    }
}

/// `configRevision` 渲染成 query / body 里那个数（整数不带小数尾巴，与 Go
/// `formatNumber` 同口径；读不出数时给 `0`，不猜）。
fn config_revision_string(value: &Value) -> String {
    if let Some(number) = value.as_i64() {
        return number.to_string();
    }
    if let Some(number) = value.as_f64() {
        if number.fract() == 0.0 {
            return (number as i64).to_string();
        }
        return format!("{number}");
    }
    if let Some(text) = value.as_str() {
        if let Ok(number) = text.trim().parse::<f64>() {
            if number.fract() == 0.0 {
                return (number as i64).to_string();
            }
            return format!("{number}");
        }
    }
    "0".to_string()
}

/// `configRevision` 作为 **JSON 数字**放进签到体（原样透传数字形态；字符串数字
/// 也折成数字，非数字给 0）
fn config_revision_json(value: &Value) -> Value {
    if value.as_f64().is_some() || value.as_i64().is_some() {
        return value.clone();
    }
    if let Some(text) = value.as_str() {
        if let Ok(number) = text.trim().parse::<f64>() {
            return Value::from(number);
        }
    }
    Value::from(0)
}

/// 今天是否已经签过（第一步预检）
fn claimed_today(context_data: &Value) -> bool {
    context_data
        .get("state")
        .and_then(|state| state.get("claimedToday"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// 活动上下文里的 `actions[]` 是否包含 `check_in`（第二步预检）
fn has_check_in_action(context_data: &Value) -> bool {
    context_data
        .get("actions")
        .and_then(Value::as_array)
        .map(|actions| {
            actions
                .iter()
                .filter_map(Value::as_str)
                .any(|action| action.trim() == CHECK_IN_ACTION)
        })
        .unwrap_or(false)
}

/// 从签到结果的 `data.result` 里取发放额（`creditsGranted` → `rewardCredits`
/// → `credits`，见模块头）。返回 `(amount, found)`。
fn credit_from_result(result: &Value) -> (f64, bool) {
    for key in ["creditsGranted", "rewardCredits", "credits"] {
        let Some(value) = result.get(key) else {
            continue;
        };
        let number = value
            .as_f64()
            .or_else(|| value.as_str().and_then(|text| text.trim().parse::<f64>().ok()));
        if let Some(number) = number {
            return (number, true);
        }
    }
    (0.0, false)
}

/// 账号记录 id → 凭证（与别家同口径：点名了不存在的账号就报 401）
fn credentials_for(
    store: &AccountStore,
    account_id: &str,
) -> Result<LobsterCredentials, GatewayError> {
    let record = store.lobsterai_account_record(account_id);
    if !account_id.is_empty() && record.is_none() {
        return Err(GatewayError::with_status(
            401,
            format!("LobsterAI 账号 {account_id} 不存在（请重新添加）"),
        ));
    }
    from_record(record.as_ref())
}

/// 发一次带鉴权的 GET（活动位 / 活动上下文共用；头照 Go `authHeaders`）
async fn get_json(
    url: &str,
    credentials: &LobsterCredentials,
    what: &str,
) -> Result<Value, GatewayError> {
    request_json("GET", url, credentials, None, what).await
}

/// 发一次带鉴权的 POST（签到）
async fn post_json(
    url: &str,
    credentials: &LobsterCredentials,
    body: &Value,
    what: &str,
) -> Result<Value, GatewayError> {
    request_json("POST", url, credentials, Some(body), what).await
}

/// 带鉴权的 JSON 请求（非 JSON 响应给「凭据已失效」这类可读原因，而不是解析错误）
async fn request_json(
    method: &str,
    url: &str,
    credentials: &LobsterCredentials,
    body: Option<&Value>,
    what: &str,
) -> Result<Value, GatewayError> {
    let headers: Vec<(String, String)> = vec![
        (
            "authorization".to_string(),
            format!("Bearer {}", credentials.access_token),
        ),
        ("accept".to_string(), "application/json".to_string()),
        ("content-type".to_string(), "application/json".to_string()),
        ("user-agent".to_string(), USER_AGENT.to_string()),
    ];
    let response = send_raw(method, url, body, &headers, None, Some(REQUEST_TIMEOUT_MS))
        .await
        .map_err(|error| {
            if error.is_timeout() {
                GatewayError::with_status(504, format!("LobsterAI {what}超时"))
            } else {
                GatewayError::with_status(502, format!("LobsterAI {what}请求失败: {error}"))
            }
        })?;
    if response.status == 401 || response.status == 403 {
        return Err(GatewayError::with_status(
            401,
            format!(
                "LobsterAI 凭据已失效（HTTP {}），请重新登录该账号",
                response.status
            ),
        ));
    }
    if !response.ok {
        return Err(GatewayError::with_status(
            response.status as i32,
            format!("LobsterAI {what}返回 HTTP {}", response.status),
        ));
    }
    response
        .payload
        .ok_or_else(|| GatewayError::with_status(502, format!("LobsterAI {what}响应不是 JSON")))
}

/// 中性结果：活动不可签（**不带「已签到」字样** —— 那会被当成当日已办）
fn inactive_result(message: &str, activity_code: &str) -> Value {
    json!({
        "success": false,
        "msg": message,
        "activityCode": activity_code,
        "needsAction": true,
    })
}

/// 中性结果：今天已签到（带「已签到」字样 + `alreadyCompleted`：当日完成）
fn already_claimed_result(activity_code: &str) -> Value {
    json!({
        "success": false,
        "msg": "今天已签到",
        "activityCode": activity_code,
        "alreadyCompleted": true,
        "needsAction": false,
    })
}

/// 每日签到（三步）。返回 `{success, msg, …}`，交给 `billing::checkin::claim_result`。
///
/// ── 三种结局对应用户看到的三种话 ─────────────────────────────
///   · **今天已签**：`success:false` + `alreadyCompleted:true` + 「今天已签到」
///     （中性、不重签）；
///   · **不可签**：`success:false` + 中性文案（活动未下发 / `check_in` 不在
///     `actions` 里）；
///   · **真签上**：`success:true` + 发放积分（字段三段回退）。
/// 传输 / 信封层面的失败才返回 `Err`（由 `claim_result` 放进结果的 `error`）。
pub async fn claim_daily_checkin(
    store: &AccountStore,
    account_id: &str,
) -> Result<Value, GatewayError> {
    let credentials = credentials_for(store, account_id)?;
    // 版本号是活动位 query 的必填参数（解析结果同时写进转发的版本头快照）
    let version = chat::prime_client_version().await;

    // ── 第一步：活动位 ─────────────────────────────────────────
    let slot_payload = get_json(&activity_slot_url(&version), &credentials, "活动槽位查询").await?;
    let slot_data = unwrap_envelope(&slot_payload, "活动槽位查询")?;
    let slot = parse_slot(&slot_data);
    if slot.slot_state != "available" || slot.activity_code.is_empty() {
        let message = format!("今天没有可签到的活动（slotState={}）", slot.slot_state);
        logging::verbose("[Checkin]", &format!("LobsterAI {account_id}: {message}"));
        return Ok(inactive_result(&message, &slot.activity_code));
    }

    // ── 第二步：两道预检（幂等靠客户端，缺一不可，见模块头）─────────
    let revision = config_revision_string(&slot.config_revision);
    let context_payload =
        get_json(&activity_context_url(&slot.activity_code, &revision), &credentials, "活动上下文查询")
            .await?;
    let context_data = unwrap_envelope(&context_payload, "活动上下文查询")?;
    if claimed_today(&context_data) {
        logging::verbose(
            "[Checkin]",
            &format!("LobsterAI {account_id}: 今天已签到（claimedToday）"),
        );
        return Ok(already_claimed_result(&slot.activity_code));
    }
    if !has_check_in_action(&context_data) {
        let message = "当前不可签到（活动未开放 check_in）".to_string();
        logging::verbose("[Checkin]", &format!("LobsterAI {account_id}: {message}"));
        return Ok(inactive_result(&message, &slot.activity_code));
    }

    // ── 第三步：真签（幂等键是 uuid4，客户端生成）───────────────────
    let body = json!({
        "configRevision": config_revision_json(&slot.config_revision),
        "idempotencyKey": uuid_v4(),
        "payload": {},
    });
    let checkin_payload =
        post_json(&activity_checkin_url(&slot.activity_code), &credentials, &body, "签到").await?;
    let data = unwrap_envelope(&checkin_payload, "签到")?;
    let result = data.get("result").cloned().unwrap_or(Value::Null);
    let (credit, granted) = credit_from_result(&result);
    let delayed = result
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let message = if granted {
        format!("签到成功，获得 {credit} 积分")
    } else {
        "签到成功".to_string()
    };
    logging::log(
        "[Checkin]",
        &format!("LobsterAI {account_id}: {message}（活动 {}）", slot.activity_code),
    );
    let mut outcome = json!({
        "success": true,
        "msg": message,
        "activityCode": slot.activity_code,
        "credit": credit,
        "creditGranted": granted,
        "needsAction": false,
    });
    if !delayed.is_empty() {
        if let Some(object) = outcome.as_object_mut() {
            object.insert("delayedMessage".to_string(), Value::String(delayed));
        }
    }
    Ok(outcome)
}

// 纯逻辑单测：活动位 / 两道预检 / 发放额三段回退 / 幂等键形态 / 中性措辞。
// （uuid_v4 的实现与它的形态断言在模块根 `mod.rs`，两处共用同一个生成器。）
#[cfg(test)]
mod tests {
    use super::*;

    /// 活动位解析：三个字段都要取到（`configRevision` 原样留着）。
    #[test]
    fn slot_parsing_reads_state_code_and_revision() {
        let data = json!({
            "slotState": "available",
            "activity": {"activityCode": "daily-checkin", "configRevision": 12}
        });
        let slot = parse_slot(&data);
        assert_eq!(slot.slot_state, "available");
        assert_eq!(slot.activity_code, "daily-checkin");
        assert_eq!(config_revision_string(&slot.config_revision), "12");
        assert_eq!(config_revision_json(&slot.config_revision), json!(12));
        // 缺字段不 panic
        let empty = parse_slot(&json!({}));
        assert!(empty.slot_state.is_empty());
        assert!(empty.activity_code.is_empty());
        assert_eq!(config_revision_string(&empty.config_revision), "0");
    }

    /// `configRevision` 的数值渲染：整数不带尾巴，小数保留（与 Go `formatNumber` 同）。
    #[test]
    fn revision_renders_without_a_trailing_zero() {
        assert_eq!(config_revision_string(&json!(12)), "12");
        assert_eq!(config_revision_string(&json!(12.0)), "12");
        assert_eq!(config_revision_string(&json!(12.5)), "12.5");
        assert_eq!(config_revision_string(&json!("7")), "7");
        assert_eq!(config_revision_string(&json!("abc")), "0");
        assert_eq!(config_revision_string(&Value::Null), "0");
    }

    /// **两道预检**都要能判：`claimedToday` 与 `actions` 里的 `check_in`。
    #[test]
    fn both_prechecks_are_evaluated() {
        assert!(claimed_today(&json!({"state": {"claimedToday": true}})));
        assert!(!claimed_today(&json!({"state": {"claimedToday": false}})));
        assert!(!claimed_today(&json!({})));
        assert!(has_check_in_action(&json!({"actions": ["view", "check_in"]})));
        assert!(!has_check_in_action(&json!({"actions": ["view"]})));
        assert!(!has_check_in_action(&json!({})));
    }

    /// 发放额的**三段回退**：`creditsGranted` → `rewardCredits` → `credits`。
    #[test]
    fn credit_falls_back_through_three_field_names() {
        assert_eq!(
            credit_from_result(&json!({"creditsGranted": 10})),
            (10.0, true)
        );
        assert_eq!(
            credit_from_result(&json!({"rewardCredits": 5.5})),
            (5.5, true)
        );
        assert_eq!(credit_from_result(&json!({"credits": 3})), (3.0, true));
        // 第一个键存在时优先（哪怕 0）
        assert_eq!(credit_from_result(&json!({"creditsGranted": 0, "credits": 9})), (0.0, true));
        // 一个都没有 → 未声明（不是「发了 0」）
        assert_eq!(credit_from_result(&json!({})), (0.0, false));
        // 数字字符串也认
        assert_eq!(credit_from_result(&json!({"credits": "8"})), (8.0, true));
    }

    /// 中性结果的措辞：不可签那句**不能**带「已签到」（否则会被当成当日已办）；
    /// 已签那句**必须**带（`checkin_completed_today` 按子串判）。
    #[test]
    fn neutral_messages_carry_the_right_wording() {
        let inactive = inactive_result("今天没有可签到的活动（slotState=empty）", "c1");
        let msg = inactive.get("msg").and_then(Value::as_str).unwrap_or("");
        assert!(!msg.contains("已签到"), "不可签不该被当成已办：{msg}");
        assert!(!msg.contains("已领取"));
        assert_eq!(inactive.get("success"), Some(&Value::Bool(false)));

        let claimed = already_claimed_result("c1");
        let msg = claimed.get("msg").and_then(Value::as_str).unwrap_or("");
        assert!(msg.contains("已签到"), "已签那句必须带「已签到」：{msg}");
        assert_eq!(claimed.get("alreadyCompleted"), Some(&Value::Bool(true)));
        assert_eq!(claimed.get("success"), Some(&Value::Bool(false)));
    }

    /// 幂等键是 UUID v4 形态（`8-4-4-4-12`，版本位 4、变体位 8/9/a/b）。
    #[test]
    fn idempotency_key_is_uuid_v4_shaped() {
        let key = uuid_v4();
        let parts: Vec<&str> = key.split('-').collect();
        assert_eq!(parts.len(), 5, "{key}");
        assert_eq!(
            parts.iter().map(|part| part.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "{key}"
        );
        assert!(parts[2].starts_with('4'), "版本位不是 4：{key}");
        assert!(
            matches!(parts[3].chars().next(), Some('8' | '9' | 'a' | 'b')),
            "变体位不对：{key}"
        );
        assert_ne!(uuid_v4(), uuid_v4());
    }
}
