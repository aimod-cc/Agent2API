//! KukuAI 余额查询：`GET /bizapi/gfpro/getgfvipremain` → `data.list[*].totalPoint`。
//!
//! ── 接口事实（2026-10-07 用真实凭证实测，与参考实现 `Acankao/zhengwuji-workbuddy`
//!    的 `internal/upstream/kuku.go` 逐字段对读）────────────────
//!   - 路径在 `/bizapi/` 一级。query 按客户端形态带 `channel/clienttype/version`
//!     （实测**不带 query 也能返回**，但客户端与 Go 参考都带，带上更稳）；
//!   - 响应是**顶层包络**（不是初版误判的双层）：
//!     ```json
//!     { "status": { "code": 0, "msg": "success" },
//!       "data": { "uname": "…",
//!                 "list": [ { "assetType": 1, "assetName": "token",
//!                             "totalPoint": "1100", "bonusPoint": "1100", … },
//!                           { "assetName": "duration", … }, … ] } }
//!     ```
//!     判据是 `status.code === 0`；积分取 `list` 里 `assetName == "token"` 的
//!     `totalPoint`（Go 参考同口径，回落第一项）—— 初版按双层 `data.status.code`
//!     解析，`data.status` 不存在取了默认 `-1`，这就是「status.code=-1：未知错误」
//!     的根因（真实凭证实测复现）；
//!   - `totalPoint` 是 **token 额度**（字符串）。
//!
//! ── 归一化形状（`ProviderAdapter::query_usage` 的契约）────────
//! `{available, unit: "积分", wallets: [{type, displayName, balance}], raw}`
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

use serde_json::{Map, Value, json};

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;

use super::credentials::{self, request_headers};
use super::{BASE_URL, CHANNEL};

/// 余额请求超时（给 15 秒；参考实现同量级）
const BALANCE_TIMEOUT_MS: u64 = 15_000;

/// 查询某账号的余额 / 积分（归一化形状见 `ProviderAdapter::query_usage` 文档）。
pub(super) async fn query_usage(
    store: &AccountStore,
    account_id: &str,
) -> Result<Value, GatewayError> {
    let credentials = credentials::snapshot_for(store, account_id)?;
    // query 按客户端形态：channel/clienttype/version（bizapi 不认 app_id，
    // 参考实现明确删掉它 —— 见 `Acankao/zhengwuji-workbuddy` 的 Points()）。
    let query = format!("channel={CHANNEL}&clienttype=400&version=1.4.4");
    let url = format!("{BASE_URL}/bizapi/gfpro/getgfvipremain?{query}");
    let headers = request_headers(&credentials);
    let value = super::http::get_json_value(&url, &headers, None, Some(BALANCE_TIMEOUT_MS)).await?;
    // 顶层包络：判据在 status.code（2026-10-07 真实凭证实测）
    let status = value.get("status").cloned().unwrap_or(Value::Null);
    let code = status
        .get("code")
        .and_then(Value::as_i64)
        .unwrap_or(-1);
    if code != 0 {
        let message = status
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("未知错误")
            .to_string();
        return Err(GatewayError::with_status(
            401,
            format!("KukuAI 积分查询失败（status.code={code}：{message}）"),
        ));
    }
    // data.list 是各类资产（token / duration / scheduled_task …）：取
    // assetName == "token" 的那项（Go 参考同口径），找不到回落第一项。
    let list = value
        .get("data")
        .and_then(|inner| inner.get("list"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let token_item = list
        .iter()
        .find(|item| item.get("assetName").and_then(Value::as_str) == Some("token"))
        .cloned()
        .or_else(|| list.first().cloned())
        .unwrap_or(Value::Null);
    let total_point = token_item
        .get("totalPoint")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    let available = parse_number(&total_point);
    let wallets: Vec<Value> = vec![json!({
        "type": "points",
        "displayName": "积分",
        "balance": available,
    })];
    let mut raw = Map::new();
    // 整份资产清单进 raw（诊断用；token 那一项的 totalPoint 已取到 available）
    raw.insert("vipRemain".to_string(), Value::Array(list));
    Ok(json!({
        "available": available,
        "unit": "积分",
        "wallets": wallets,
        "subscription": Value::Null,
        "raw": Value::Object(raw),
    }))
}

/// 上游把 `totalPoint` 给成字符串（`"1234"`），这里必须同时接受数字形态。
fn parse_number(text: &str) -> Value {
    let text = text.trim();
    if text.is_empty() {
        return Value::Null;
    }
    if let Ok(number) = text.parse::<f64>() {
        return Value::from(number);
    }
    Value::Null
}
