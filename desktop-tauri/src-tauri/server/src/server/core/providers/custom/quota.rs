use serde_json::{json, Value};

use super::{balance::Scheme, balance_parse::{number_of, unix_to_ms}};

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

pub(super) fn sources(provider: &str, scheme: Scheme, report: &Value) -> Vec<Value> {
    let mut sources = Vec::new();
    match scheme {
        Scheme::Opencode => {
            for window in report.get("windows").and_then(Value::as_array).into_iter().flatten() {
                sources.push(json!({ "id": format!("window:{}", text(window, "key")),
                    "provider": provider, "label": text(window, "label"), "unit": "%",
                    "remainingPercent": number_of(window.get("remainingPercent")),
                    "resetsAt": window.get("resetsAt").and_then(Value::as_i64).unwrap_or(0) }));
            }
        }
        Scheme::Sensenova => {
            let plan = report.pointer("/raw/plan").and_then(Value::as_str).unwrap_or("");
            for pool in report.pointer("/raw/pools").and_then(Value::as_array).into_iter().flatten() {
                let id = if text(pool, "id").is_empty() { text(pool, "pool_type") } else { text(pool, "id") };
                if id.is_empty() { continue; }
                let name = text(pool, "name");
                let models = pool.get("model_ids").and_then(Value::as_array).cloned().unwrap_or_default();
                for (key, label) in [("window_5h", "5 小时"), ("window_7d", "7 天")] {
                    let Some(window) = pool.get(key).filter(|value| value.is_object()) else { continue };
                    let remaining = number_of(window.get("remaining"));
                    let total = number_of(window.get("limit"));
                    let used = number_of(window.get("used"));
                    sources.push(json!({ "id": format!("pool:{id}:{key}"), "provider": provider,
                        "label": format!("{name} · {label}"), "planId": plan, "planName": plan,
                        "models": models, "unit": "积分", "remaining": remaining, "total": total,
                        "used": used, "resetsAt": unix_to_ms(window.get("reset_at")).unwrap_or(0) }));
                }
                if let Some(remaining) = number_of(pool.get("grant_balance")) {
                    sources.push(json!({ "id": format!("pool:{id}:grant"), "provider": provider,
                        "label": format!("{name} · 活动返赠"), "models": models,
                        "planId": plan, "planName": plan, "unit": "积分", "remaining": remaining }));
                }
            }
        }
        Scheme::NewApi => {
            if let Some(sub) = report.get("subscriptionQuota").filter(|value| value.is_object()) {
                let unit = text(report, "unit");
                let plan_name = report
                    .pointer("/subscription/planName")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .unwrap_or("订阅额度");
                sources.push(json!({
                    "id": "subscription",
                    "provider": provider,
                    "label": plan_name,
                    "planName": plan_name,
                    "unit": unit,
                    "remaining": number_of(sub.get("remaining")),
                    "total": number_of(sub.get("total")),
                    "used": number_of(sub.get("used")),
                    "unlimited": sub.get("unlimited").and_then(Value::as_bool).unwrap_or(false),
                    "resetsAt": sub.get("nextResetAt").and_then(Value::as_i64).unwrap_or(0),
                }));
                if let Some(wallet_row) = report
                    .get("wallets")
                    .and_then(Value::as_array)
                    .and_then(|rows| rows.iter().find(|row| text(row, "type") == "wallet"))
                {
                    sources.push(json!({
                        "id": "wallet",
                        "provider": provider,
                        "label": "钱包余额",
                        "unit": unit,
                        "remaining": number_of(wallet_row.get("balance")),
                    }));
                }
            }
        }
        _ => {}
    }
    sources
}
