//! Loomy 积分 / 余额查询：`GET {集成网关}/api/v2/points/records` 的摘要字段。
//!
//! ── 为什么余额藏在流水接口里（客户端就是这么读的）──────────────
//! Loomy **没有独立的余额接口**。客户端的积分详情页读的是 V2 流水接口——
//! 它的响应 `data` 里同时带账户摘要：
//!
//! ```text
//! GET /api/v2/points/records?pageNo=1&pageSize=1&direction=debit&granularity=chat
//!   → {code:"000000", data:{ balance,           ← 永久积分（长期积分）
//!                          dailyBalance,       ← 每日赠送积分（每日登录刷回 5000）
//!                          dailyLimitPoints, dailyConsumedPoints, dailyRemainingPoints,
//!                          list:[…], total }}
//! ```
//!
//! 积分详情页的两张卡片（永久 5000 / 每日赠送 5000）读的就是 `balance` 与
//! `dailyBalance`（渲染层字段名经 babel 产物核对）。这里按账号页的统一形状
//! （`{available, unit, wallets, raw}`，见 `ProviderAdapter::query_usage` 的契约）
//! 归一化：两个钱包各一行，`available` 取两者之和（实际可用的总量）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;

use super::client;
use super::credentials;

/// 流水查询路径（相对集成网关；pageSize 取 1 只为拿摘要，不看流水本身）
const RECORDS_PATH: &str =
    "/api/v2/points/records?pageNo=1&pageSize=1&direction=debit&granularity=chat";

/// 从 `data.data` 里取一个非负数字（字段缺失 / 非法时给 None）
fn pick_number(data: &Value, key: &str) -> Option<f64> {
    let value = data.get(key)?;
    if let Some(number) = value.as_f64() {
        return Some(number);
    }
    // 上游有把数字序列化成字符串的形态（如 "5000"）
    value
        .as_str()
        .and_then(|text| text.trim().parse::<f64>().ok())
}

/// 查询某 Loomy 账号的双账户余额。
pub async fn query_usage(store: &AccountStore, account_id: &str) -> Result<Value, GatewayError> {
    let record = store.loomy_account_record(account_id);
    if !account_id.is_empty() && record.is_none() {
        return Err(GatewayError::with_status(
            404,
            format!("Loomy 账号 {account_id} 不存在（请重新添加）"),
        ));
    }
    let credentials = credentials::from_record(record.as_ref())?;

    let payload = client::token_request("GET", RECORDS_PATH, &credentials.session, None, "积分查询").await?;

    let code = client::business_code(&payload);
    if client::is_auth_error_code(&code) {
        return Err(GatewayError::with_status(
            401,
            "Loomy 登录态已失效，无法查询积分（请重新登录）",
        ));
    }
    if code != client::SUCCESS_CODE {
        let message = client::upstream_message(&payload);
        return Err(GatewayError::with_status(
            502,
            if message.trim().is_empty() {
                format!("积分查询失败（上游 code={code}）")
            } else {
                format!("积分查询失败：{message}")
            },
        ));
    }

    // 摘要可能在 data 或 data.data 两层（V2 接口的形态在不同版本里出现过两种）
    let data = payload
        .get("data")
        .and_then(|value| value.get("data"))
        .unwrap_or_else(|| payload.get("data").unwrap_or(&Value::Null));

    let permanent = pick_number(data, "balance").unwrap_or(0.0);
    let daily = pick_number(data, "dailyBalance").unwrap_or(0.0);
    let daily_remaining = pick_number(data, "dailyRemainingPoints");
    let daily_limit = pick_number(data, "dailyLimitPoints");

    let mut wallets = vec![
        json!({
            "type": "permanent_points",
            "displayName": "永久积分",
            "balance": permanent,
        }),
        json!({
            "type": "daily_points",
            "displayName": "每日赠送积分",
            "balance": daily,
        }),
    ];
    // 赠送额度的「剩余 / 上限」是独立的展示口径（客户端在团队卡片上用它），
    // 有就带上，没有不编造
    if let (Some(remaining), Some(limit)) = (daily_remaining, daily_limit) {
        if let Some(object) = wallets.get_mut(1).and_then(Value::as_object_mut) {
            object.insert("remaining".to_string(), Value::from(remaining));
            object.insert("limit".to_string(), Value::from(limit));
        }
    }

    // ── 逐份明细（`credits[]`，账号页「积分明细」弹层的输入）────────
    // 两行与 `wallets` 同序同口径：永久积分在前、每日赠送在后。`daily: true` 是界面
    // 「每日重置」的判据（余额读到 0 时也不折进「已用完」—— 明天回满，这是对的）。
    //
    // `dailyRemainingPoints` / `dailyLimitPoints` **不折进这两行**：它们是赠送额度
    // 的另一个展示口径（客户端在团队卡片上用），与 `dailyBalance` 是否同源未经证实
    // ——混进来的「剩 x / 共 y」可能对不上真正的余额（宁可少给，不编）。这两个数仍在
    // `wallets[1]` 的 `remaining` / `limit` 里透出，位置没变。
    //
    // 这里没有"缺失不写键"的分支：上游没给时余额列本来就显示 0（`pick_number` 的
    // `unwrap_or(0.0)`），明细跟着同一份数走，不与上方读数打架。
    let credits = vec![
        json!({ "name": "永久积分", "remain": permanent }),
        json!({ "name": "每日赠送积分", "remain": daily, "daily": true }),
    ];

    let mut document = json!({
        "available": permanent + daily,
        "unit": "积分",
        "wallets": wallets,
        "raw": payload,
    });
    // credits 固定两行（见上），没有"空数组"这一档；键名与其余几家一致，
    // 前端按「键在不在」挂「积分明细」入口
    if let Some(object) = document.as_object_mut() {
        object.insert("credits".to_string(), Value::Array(credits));
    }
    Ok(document)
}
