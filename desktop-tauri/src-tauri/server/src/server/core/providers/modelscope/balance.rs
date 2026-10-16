//! ModelScope 余额 / 用量查询：魔粒钱包 + 今日已用调用数（2026-10-16 实测）。
//!
//! ── 两个接口（都在**网页站**，cookie 鉴权）───────────────────────
//! ```text
//! GET {网页站}/openapi/v1/magicubes/balance
//!   → {"success":true,"data":{"total_balance":108,"available_balance":106,"frozen_amount":2}}
//!
//! GET {网页站}/api/v1/inference/rate-limit
//!   → {"Code":200,"Success":true,"Data":{"currentUsagePerDay":72}, …}
//! ```
//! 第一个是**主读数**（魔粒钱包，单位的「魔粒」）；第二个是「今天已经用掉多少次
//! 调用」——它决定今天还能不能继续打上游，是限流链路要看的数。
//!
//! ── 为什么第二个接口失败只降级、不整条失败 ────────────────────
//! 主问题是「我还有多少魔粒」，`rate-limit` 是附加读数。让一个附加接口的抖动
//! 把余额整条变成红色失败，用户会以为钱包也读不到了 —— 所以它失败只记 verbose 日志、
//! 少给那一行，`raw` 里如实标出没取到。
//!
//! ── 量纲：三行明细里只有前两行是「魔粒」────────────────────────
//! `currentUsagePerDay` 的单位是**调用次数**，不是魔粒。它不进 `wallets`
//! （那是钱包明细，同一量纲），而是进 `credits[]`（逐份读数，名字自带量纲说明），
//! 与 `loomy::balance` 把「永久 / 每日赠送」并列的处置同形。名字里写明「次」
//! 是为了不让界面把它读成「今天用掉了 72 魔粒」。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::client;
use super::credentials;
use super::region::Region;

/// 魔粒钱包路径（相对网页站）
pub const BALANCE_PATH: &str = "/openapi/v1/magicubes/balance";

/// 今日用量路径（相对网页站）
pub const RATE_LIMIT_PATH: &str = "/api/v1/inference/rate-limit";

/// 从一份 JSON 里取一个数字（字段缺失 / 非法时给 None）。
///
/// 上游有把数字序列化成字符串的形态，两种都认（与 `loomy::balance` 同名函数同款）。
fn pick_number(data: &Value, key: &str) -> Option<f64> {
    let value = data.get(key)?;
    if let Some(number) = value.as_f64() {
        return Some(number);
    }
    value
        .as_str()
        .and_then(|text| text.trim().parse::<f64>().ok())
}

/// 查询某 ModelScope 账号的魔粒余额与今日用量。
pub async fn query_usage(
    region: Region,
    store: &AccountStore,
    account_id: &str,
) -> Result<Value, GatewayError> {
    let record = store.modelscope_account_record(region, account_id);
    if !account_id.is_empty() && record.is_none() {
        return Err(GatewayError::with_status(
            404,
            format!(
                "ModelScope {}账号 {account_id} 不存在（请重新添加）",
                region.label()
            ),
        ));
    }
    let credentials = credentials::from_record(region, record.as_ref())?;

    // ── 主读数：魔粒钱包 ────────────────────────────────────
    let balance_payload =
        client::site_get(region, BALANCE_PATH, &credentials.token, "魔粒余额查询").await?;
    if !client::succeeded(&balance_payload) {
        let message = client::message_of(&balance_payload);
        return Err(GatewayError::with_status(
            502,
            if message.trim().is_empty() {
                format!("魔粒余额查询失败（上游 Code={}）", client::code_of(&balance_payload))
            } else {
                format!("魔粒余额查询失败：{message}")
            },
        ));
    }
    let wallet = client::data_of(&balance_payload);
    let available = pick_number(&wallet, "available_balance").unwrap_or(0.0);
    let total = pick_number(&wallet, "total_balance");
    let frozen = pick_number(&wallet, "frozen_amount").unwrap_or(0.0);

    // ── 附加读数：今日已用调用数（失败只降级，见模块头）──────────
    // 原始报文一并留下（`raw.rateLimit`）：没取到给 `Value::Null` —— 排障时
    // 一眼能分辨「上游给了 0」与「这次根本没问到」，缺键则分辨不出。
    let (used_today, rate_limit_raw) = match client::site_get(
        region,
        RATE_LIMIT_PATH,
        &credentials.token,
        "今日用量查询",
    )
    .await
    {
        Ok(payload) if client::succeeded(&payload) => (
            pick_number(&client::data_of(&payload), "currentUsagePerDay"),
            payload,
        ),
        Ok(payload) => (None, payload),
        Err(error) => {
            logging::verbose(
                "[Usage]",
                &format!("ModelScope 今日用量未取到（余额照常返回）：{}", error.message),
            );
            (None, Value::Null)
        }
    };

    let mut wallets = vec![json!({
        "type": "magicubes",
        "displayName": "魔粒余额",
        "balance": available,
    })];
    if let Some(total) = total {
        if let Some(object) = wallets.first_mut().and_then(Value::as_object_mut) {
            object.insert("total".to_string(), Value::from(total));
        }
    }
    if let Some(object) = wallets.first_mut().and_then(Value::as_object_mut) {
        object.insert("frozen".to_string(), Value::from(frozen));
    }

    let mut credits = vec![
        json!({ "name": "可用魔粒", "remain": available }),
        json!({ "name": "冻结魔粒", "remain": frozen }),
    ];
    // 今日已用的量纲是「次」（见模块头）—— 名字里写明，别让界面读成魔粒
    if let Some(used) = used_today {
        credits.push(json!({ "name": "今日已用（次）", "remain": used, "daily": true }));
    }

    let mut document = json!({
        "available": available,
        "unit": "魔粒",
        "wallets": wallets,
        "raw": {
            "balance": balance_payload,
            // 没取到时为 null（如实标出，而不是缺键），见上面的取值处
            "rateLimit": rate_limit_raw,
        },
    });
    if let Some(object) = document.as_object_mut() {
        object.insert("credits".to_string(), Value::Array(credits));
    }
    Ok(document)
}