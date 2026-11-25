//! NewAPI 余额与订阅额度的响应解析与文档组装。
//!
//! 覆盖三条上游接口：
//!   - `/api/user/self`：钱包剩余配额（`data.quota`）与已用配额（`data.used_quota`）；
//!   - `/api/status`：展示币种与配额换算倍率（`quota_per_unit` / `usd_exchange_rate` 等）；
//!   - `/api/subscription/self`：当前用户的有效订阅额度（`amount_total` / `amount_used` /
//!     `next_reset_time` / `end_time` / `allow_wallet_overflow` / `billing_preference`）。
//!
//! ── 扣费偏好与可用总额口径（对照 QuantumNous/new-api `service/billing_session.go`）──
//!   - `wallet_only`：只花钱包余额；
//!   - `subscription_only`：只花有效订阅额度；
//!   - `wallet_first`：钱包与订阅均可消费（合计可用 = 订阅剩余 + 钱包正余量）；
//!   - `subscription_first`（缺省）：有有效订阅时，若任一有效订阅声明
//!     `allow_wallet_overflow: false` 则只花订阅额度，否则订阅与钱包均可消费。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：非测试代码零 unwrap / expect / panic。

use serde_json::{json, Map, Value};

use super::balance_parse::{
    document, map_of, number_at, reset_text, unix_to_ms, wallet,
};

/// NewAPI `/api/status` 里的配额显示口径（`data.quota_per_unit` 等）。
pub(super) struct QuotaDisplay {
    pub display_type: String,
    pub quota_per_unit: f64,
    pub usd_exchange_rate: f64,
    pub custom_currency_symbol: String,
    pub custom_currency_exchange_rate: f64,
}

/// 解析 `/api/status` 的显示口径（读不出 / 汇率缺失时给 None —— 宁可退回原始配额，
/// 也不用默认汇率算出一个错的金额）。
pub(super) fn new_api_display(payload: &Value) -> Option<QuotaDisplay> {
    let data = payload.get("data").and_then(map_of)?;
    let quota_per_unit = number_at(data, "quota_per_unit").filter(|value| *value > 0.0)?;
    let mut display_type = data
        .get("quota_display_type")
        .and_then(Value::as_str)
        .map(|text| text.trim().to_ascii_uppercase())
        .unwrap_or_default();
    if !["USD", "CNY", "TOKENS", "CUSTOM"].contains(&display_type.as_str()) {
        display_type = if data.get("display_in_currency").and_then(Value::as_bool) == Some(false) {
            "TOKENS".to_string()
        } else {
            "USD".to_string()
        };
    }
    let usd_exchange_rate = number_at(data, "usd_exchange_rate").unwrap_or(1.0);
    let custom_currency_exchange_rate =
        number_at(data, "custom_currency_exchange_rate").unwrap_or(1.0);
    if display_type == "CNY" && !(usd_exchange_rate > 0.0) {
        return None;
    }
    if display_type == "CUSTOM" && !(custom_currency_exchange_rate > 0.0) {
        return None;
    }
    Some(QuotaDisplay {
        display_type,
        quota_per_unit,
        usd_exchange_rate,
        custom_currency_symbol: data
            .get("custom_currency_symbol")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("")
            .to_string(),
        custom_currency_exchange_rate,
    })
}

/// 原始配额 → 展示口径（与源实现 `displayedAmountRemaining` 同链）。
pub(super) fn new_api_display_amount(quota: f64, display: Option<&QuotaDisplay>) -> (f64, String) {
    let Some(display) = display else {
        return (quota, "quota".to_string());
    };
    if display.quota_per_unit <= 0.0 {
        return (quota, "quota".to_string());
    }
    let amount_usd = quota / display.quota_per_unit;
    match display.display_type.as_str() {
        "CNY" if display.usd_exchange_rate > 0.0 => {
            (amount_usd * display.usd_exchange_rate, "CNY".to_string())
        }
        "CUSTOM" if display.custom_currency_exchange_rate > 0.0 => (
            amount_usd * display.custom_currency_exchange_rate,
            if display.custom_currency_symbol.is_empty() {
                "CUSTOM".to_string()
            } else {
                display.custom_currency_symbol.clone()
            },
        ),
        "TOKENS" => (quota, "quota".to_string()),
        _ => (amount_usd, "USD".to_string()),
    }
}

/// NewAPI `/api/user/self`：`data.quota`（剩余配额）/ `data.used_quota`（已用）。
pub(super) fn parse_new_api_account(payload: &Value) -> Option<(f64, Option<f64>)> {
    let data = payload.get("data").and_then(map_of)?;
    let remaining = number_at(data, "quota")?;
    Some((remaining, number_at(data, "used_quota")))
}

/// NewAPI `/api/subscription/self` 汇总后的有效订阅读数（原始配额单位，与 `data.quota` 同刻度）。
#[derive(Clone, Debug, PartialEq)]
pub(super) struct NewApiSubscriptionSummary {
    pub count: usize,
    pub has_unlimited: bool,
    pub total_raw: Option<f64>,
    pub used_raw: f64,
    pub remaining_raw: Option<f64>,
    pub next_reset_ms: Option<i64>,
    pub expire_at_ms: Option<i64>,
    pub allow_wallet_overflow: bool,
    pub billing_preference: String,
    pub plan_name: Option<String>,
}

/// 解析 `/api/subscription/self`：兼容官方 `QuantumNous/new-api` 的
/// `{ data: { billing_preference, subscriptions: [{ subscription: {...} }] } }`
/// 以及部分派生站点直接返回数组或扁平条目的写法。
///
/// 无有效订阅（列表为空、全部已过期/取消、或 `success: false`）时返回 `None`，
/// 调用方保持纯钱包余额形态回退。
pub(super) fn parse_new_api_subscription(
    payload: &Value,
    now_ms: i64,
) -> Option<NewApiSubscriptionSummary> {
    let root = map_of(payload)?;
    if root.get("success").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    let data = root.get("data").unwrap_or(payload);
    let billing_preference = data
        .as_object()
        .and_then(|map| map.get("billing_preference"))
        .and_then(Value::as_str)
        .map(normalize_billing_preference)
        .unwrap_or_else(|| "subscription_first".to_string());

    let owned_single;
    let items: &[Value] = if let Some(array) = data.as_array() {
        array.as_slice()
    } else if let Some(map) = data.as_object() {
        // 官方结构中 `subscriptions` 仅含有效订阅、`all_subscriptions` 含历史过期项；
        // 只要存在 `subscriptions` 键就绝不回落去读 `all_subscriptions`。
        if let Some(subs) = map.get("subscriptions") {
            subs.as_array().map(Vec::as_slice).unwrap_or(&[])
        } else if let Some(list) = map
            .get("items")
            .or_else(|| map.get("list"))
            .and_then(Value::as_array)
        {
            list.as_slice()
        } else if map.contains_key("subscription")
            || map.contains_key("amount_total")
            || map.contains_key("amount_used")
        {
            owned_single = [data.clone()];
            &owned_single
        } else {
            return None;
        }
    } else {
        return None;
    };

    let mut count = 0usize;
    let mut has_unlimited = false;
    let mut finite_total = 0.0f64;
    let mut finite_remaining = 0.0f64;
    let mut has_finite = false;
    let mut used_sum = 0.0f64;
    let mut future_reset_ms: Option<i64> = None;
    let mut any_reset_ms: Option<i64> = None;
    let mut expire_at_ms: Option<i64> = None;
    let mut allow_wallet_overflow = true;
    let mut plan_names: Vec<String> = Vec::new();

    for item in items {
        let Some(item_map) = map_of(item) else {
            continue;
        };
        let (sub, outer_plan) = match item_map.get("subscription").and_then(map_of) {
            Some(inner) => (inner, item_map.get("plan").and_then(map_of)),
            None => (item_map, item_map.get("plan").and_then(map_of)),
        };
        let plan = outer_plan.or_else(|| sub.get("plan").and_then(map_of));

        if let Some(status) = sub
            .get("status")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            if !status.eq_ignore_ascii_case("active") {
                continue;
            }
        }
        let end_ms = unix_to_ms(sub.get("end_time"));
        if end_ms.is_some_and(|end| end <= now_ms) {
            continue;
        }

        let amount_total = number_at(sub, "amount_total");
        let amount_used = number_at(sub, "amount_used");
        if amount_total.is_none() && amount_used.is_none() {
            continue;
        }

        count += 1;
        let used = amount_used.unwrap_or(0.0).max(0.0);
        used_sum += used;

        match amount_total {
            // NewAPI 约定 `amount_total == 0` 表示不限量订阅。
            Some(total) if total <= 0.0 => {
                has_unlimited = true;
            }
            Some(total) => {
                has_finite = true;
                finite_total += total;
                finite_remaining += (total - used).max(0.0);
            }
            None => {}
        }

        if let Some(reset_ms) = unix_to_ms(sub.get("next_reset_time")) {
            any_reset_ms = Some(any_reset_ms.map_or(reset_ms, |prev| prev.min(reset_ms)));
            if reset_ms > now_ms {
                future_reset_ms =
                    Some(future_reset_ms.map_or(reset_ms, |prev| prev.min(reset_ms)));
            }
        }
        if let Some(end) = end_ms {
            expire_at_ms = Some(expire_at_ms.map_or(end, |prev| prev.max(end)));
        }
        if sub.get("allow_wallet_overflow").and_then(Value::as_bool) == Some(false) {
            allow_wallet_overflow = false;
        }
        if let Some(name) = extract_plan_name(plan, sub) {
            if !plan_names.iter().any(|existing| existing == &name) {
                plan_names.push(name);
            }
        }
    }

    if count == 0 {
        return None;
    }

    let plan_name = match plan_names.len() {
        0 => None,
        1 => plan_names.into_iter().next(),
        _ => Some(plan_names.join(" / ")),
    };

    Some(NewApiSubscriptionSummary {
        count,
        has_unlimited,
        total_raw: has_finite.then_some(finite_total),
        used_raw: used_sum,
        remaining_raw: has_finite.then_some(finite_remaining),
        next_reset_ms: future_reset_ms.or(any_reset_ms),
        expire_at_ms,
        allow_wallet_overflow,
        billing_preference,
        plan_name,
    })
}

/// NewAPI 整套读数组装：有有效订阅时结合订阅额度与钱包余额计算可用读数，
/// 无有效订阅时保持既有钱包余额口径不变。
pub(super) fn new_api_document(
    wallet_remaining_raw: f64,
    wallet_used_raw: Option<f64>,
    display: Option<&QuotaDisplay>,
    account_payload: &Value,
    status_payload: Option<&Value>,
    subscription: Option<&NewApiSubscriptionSummary>,
    subscription_payload: Option<&Value>,
) -> Value {
    let (wallet_remaining, unit) = new_api_display_amount(wallet_remaining_raw, display);
    let wallet_used = wallet_used_raw.map(|used| new_api_display_amount(used, display).0);

    let Some(sub) = subscription else {
        let wallets = vec![
            wallet("remaining", "剩余配额", Some(wallet_remaining), &unit),
            wallet("used", "已用配额", wallet_used, &unit),
        ]
        .into_iter()
        .flatten()
        .collect();
        let mut doc = document(Some(wallet_remaining), &unit, wallets, account_payload);
        attach_raw_fields(
            &mut doc,
            wallet_remaining_raw,
            wallet_used_raw,
            status_payload,
            subscription_payload,
        );
        return doc;
    };

    let sub_remaining = sub
        .remaining_raw
        .map(|value| new_api_display_amount(value, display).0);
    let sub_total = sub
        .total_raw
        .map(|value| new_api_display_amount(value, display).0);
    let sub_used = new_api_display_amount(sub.used_raw, display).0;

    let (use_subscription, use_wallet) = match sub.billing_preference.as_str() {
        "wallet_only" => (false, true),
        "subscription_only" => (true, false),
        "wallet_first" => (true, true),
        _ => (true, sub.allow_wallet_overflow),
    };

    let unlimited = use_subscription && sub.has_unlimited;
    let available = if unlimited {
        None
    } else if use_subscription {
        let sub_avail = sub_remaining.unwrap_or(0.0);
        Some(if use_wallet {
            sub_avail + wallet_remaining.max(0.0)
        } else {
            sub_avail
        })
    } else {
        Some(wallet_remaining)
    };

    // 只有「当前主要靠订阅消费且钱包没有额外正余额」时，让订阅行带 `total` 走进度条卡片；
    // 钱包与订阅同时有可用正余额（或 wallet_only）时，表格格直接展示合计可用余额，
    // 避免进度条只画订阅而掩盖钱包余额。
    let lead_with_subscription_pool = use_subscription
        && !unlimited
        && sub_total.is_some_and(|total| total > 0.0)
        && (!use_wallet || wallet_remaining <= 0.0);

    let sub_label = sub
        .plan_name
        .as_deref()
        .unwrap_or("订阅额度");
    let reset_label = reset_text(sub.next_reset_ms);

    let mut wallets: Vec<Value> = Vec::new();
    let mut sub_row = Map::new();
    sub_row.insert("type".to_string(), json!("subscription"));
    sub_row.insert(
        "displayName".to_string(),
        json!(if use_subscription {
            sub_label.to_string()
        } else {
            format!("{sub_label}（未启用）")
        }),
    );
    sub_row.insert("unit".to_string(), json!(&unit));
    if sub.has_unlimited {
        sub_row.insert("unlimited".to_string(), json!(true));
        let mut view = "不限".to_string();
        if let Some(reset) = &reset_label {
            view.push_str(&format!(" · {reset} 重置"));
        }
        sub_row.insert("balanceView".to_string(), json!(view));
    } else if let Some(remaining) = sub_remaining {
        sub_row.insert("balance".to_string(), json!(remaining));
        sub_row.insert("used".to_string(), json!(sub_used));
        let base_view = match sub_total {
            Some(total) if total > 0.0 => format!(
                "剩 {} / {} {unit}",
                amount_text(remaining),
                amount_text(total)
            ),
            _ => format!("剩 {} {unit}", amount_text(remaining)),
        };
        if lead_with_subscription_pool {
            if let Some(total) = sub_total {
                sub_row.insert("planName".to_string(), json!(sub_label));
                sub_row.insert("total".to_string(), json!(total));
                sub_row.insert(
                    "remainingPercent".to_string(),
                    json!((remaining / total * 100.0).clamp(0.0, 100.0)),
                );
            }
            // 进度条格子空间窄，重置时间放到 document.balanceView（仅进悬停明细）
            sub_row.insert("balanceView".to_string(), json!(base_view));
        } else {
            let full_view = match &reset_label {
                Some(reset) => format!("{base_view} · {reset} 重置"),
                None => base_view,
            };
            sub_row.insert("balanceView".to_string(), json!(full_view));
        }
    }
    if let Some(reset_ms) = sub.next_reset_ms {
        sub_row.insert("resetsAt".to_string(), json!(reset_ms));
    }
    if sub_row.contains_key("balance") || sub_row.contains_key("unlimited") {
        wallets.push(Value::Object(sub_row));
    }

    if let Some(mut wallet_row) = wallet("wallet", "钱包余额", Some(wallet_remaining), &unit) {
        if let Some(obj) = wallet_row.as_object_mut() {
            obj.insert(
                "balanceView".to_string(),
                json!(format!("{} {unit}", amount_text(wallet_remaining))),
            );
        }
        wallets.push(wallet_row);
    }
    if let Some(mut used_row) = wallet("used", "累计已用", wallet_used, &unit) {
        if let Some(obj) = used_row.as_object_mut() {
            if let Some(used_val) = wallet_used {
                obj.insert(
                    "balanceView".to_string(),
                    json!(format!("{} {unit}", amount_text(used_val))),
                );
            }
        }
        wallets.push(used_row);
    }

    let mut sub_meta = Map::new();
    sub_meta.insert("planName".to_string(), json!(sub_label));
    sub_meta.insert("count".to_string(), json!(sub.count));
    sub_meta.insert(
        "billingPreference".to_string(),
        json!(sub.billing_preference),
    );
    if sub.has_unlimited {
        sub_meta.insert("unlimited".to_string(), json!(true));
    }
    if let Some(remaining) = sub_remaining {
        sub_meta.insert("remainQuota".to_string(), json!(remaining));
    }
    if let Some(total) = sub_total {
        sub_meta.insert("totalQuota".to_string(), json!(total));
    }
    if let Some(reset_ms) = sub.next_reset_ms {
        sub_meta.insert("resetsAt".to_string(), json!(reset_ms));
    }
    if let Some(expire_ms) = sub.expire_at_ms {
        sub_meta.insert("expireAt".to_string(), json!(expire_ms));
    }

    let mut doc = document(available, &unit, wallets, account_payload);
    if let Some(object) = doc.as_object_mut() {
        object.insert("subscription".to_string(), Value::Object(sub_meta));
        object.insert(
            "billingPreference".to_string(),
            json!(sub.billing_preference),
        );
        if unlimited {
            object.insert("unlimited".to_string(), json!(true));
            object.insert("availableView".to_string(), json!("不限"));
        }
        if lead_with_subscription_pool {
            if let Some(reset) = reset_label {
                object.insert("balanceView".to_string(), json!(format!("下次重置 {reset}")));
            }
        }
        let mut sub_quota = Map::new();
        sub_quota.insert("count".to_string(), json!(sub.count));
        sub_quota.insert("used".to_string(), json!(sub_used));
        if let Some(remaining) = sub_remaining {
            sub_quota.insert("remaining".to_string(), json!(remaining));
        }
        if let Some(total) = sub_total {
            sub_quota.insert("total".to_string(), json!(total));
        }
        if sub.has_unlimited {
            sub_quota.insert("unlimited".to_string(), json!(true));
        }
        if let Some(reset_ms) = sub.next_reset_ms {
            sub_quota.insert("nextResetAt".to_string(), json!(reset_ms));
        }
        object.insert("subscriptionQuota".to_string(), Value::Object(sub_quota));
    }
    attach_raw_fields(
        &mut doc,
        wallet_remaining_raw,
        wallet_used_raw,
        status_payload,
        subscription_payload,
    );
    doc
}

fn attach_raw_fields(
    doc: &mut Value,
    wallet_remaining_raw: f64,
    wallet_used_raw: Option<f64>,
    status_payload: Option<&Value>,
    subscription_payload: Option<&Value>,
) {
    let Some(object) = doc.as_object_mut() else {
        return;
    };
    object.insert("quota".to_string(), json!(wallet_remaining_raw));
    if let Some(used_raw) = wallet_used_raw {
        object.insert("usedQuota".to_string(), json!(used_raw));
    }
    if let Some(status) = status_payload {
        object.insert("status".to_string(), status.clone());
    }
    if let Some(subscription) = subscription_payload {
        object.insert("subscriptionRaw".to_string(), subscription.clone());
    }
}

fn normalize_billing_preference(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "subscription_first" | "wallet_first" | "subscription_only" | "wallet_only" => {
            raw.trim().to_ascii_lowercase()
        }
        _ => "subscription_first".to_string(),
    }
}

fn extract_plan_name(
    plan: Option<&Map<String, Value>>,
    sub: &Map<String, Value>,
) -> Option<String> {
    let from_plan = plan.and_then(|map| {
        map.get("title")
            .or_else(|| map.get("name"))
            .and_then(Value::as_str)
    });
    let from_sub = sub
        .get("plan_title")
        .or_else(|| sub.get("plan_name"))
        .or_else(|| sub.get("title"))
        .or_else(|| sub.get("name"))
        .and_then(Value::as_str);
    from_plan
        .or(from_sub)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn amount_text(value: f64) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    let abs = value.abs();
    let formatted = if abs > 0.0 && abs < 0.01 {
        format!("{value:.4}")
    } else {
        format!("{value:.2}")
    };
    let trimmed = formatted.trim_end_matches('0').trim_end_matches('.');
    if trimmed == "-0" || trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
#[path = "balance_new_api_tests.rs"]
mod tests;
