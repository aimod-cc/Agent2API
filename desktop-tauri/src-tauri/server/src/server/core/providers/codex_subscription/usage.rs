//! Magpie subscription_usage.go/codexWindows 的额度读取：订阅窗口与另购 credits 分开。
//! 只读 /wham/usage，不自动兑换重置次数、购买额度或触发其他扣费管理动作。

use serde_json::{json, Value};

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;
use crate::server::logging;

use super::{account, credentials, http, ID};

pub(super) async fn query(store: &AccountStore, account_id: &str) -> Result<Value, GatewayError> {
    let initial = account::snapshot(store, account_id)?;
    credentials::ensure(store, &initial.id, false).await?;
    let snapshot = account::snapshot(store, &initial.id)?;
    let headers = http::account_headers(&snapshot.tokens.access, &snapshot.tokens.account_id);
    let request = http::with_headers(
        http::client(snapshot.proxy.as_ref())?.get(http::USAGE_URL),
        &headers,
    );
    let reply = http::request(request, 4 * 1024 * 1024, "读取额度").await?;
    // 401 保留给 usage_query 的共用刷新后重试一次，不在这里叠加另一轮刷新。
    if !(200..300).contains(&reply.status) {
        return Err(http::response_error("读取额度", &reply));
    }
    let current = account::snapshot(store, &snapshot.id)?;
    if current.tokens.access != snapshot.tokens.access
        || current.tokens.identity() != snapshot.tokens.identity()
    {
        return Err(GatewayError::with_status(
            409,
            "读取 Codex 额度时凭据已变化，请重新查询",
        ));
    }
    parse(&reply.body)
}

pub(super) fn number(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse().ok())
        .filter(|number| number.is_finite())
}

pub(super) fn reset(window: &Value) -> Option<i64> {
    ["reset_at", "resets_at", "resetAt"]
        .into_iter()
        .find_map(|key| {
            let value = window.get(key)?;
            if let Some(text) = value.as_str() {
                if let Ok(date) = chrono::DateTime::parse_from_rfc3339(text) {
                    return Some(date.timestamp_millis());
                }
            }
            let at = value.as_i64().or_else(|| value.as_str()?.parse().ok())?;
            if at <= 0 {
                return None;
            }
            if at < 100_000_000_000 {
                at.checked_mul(1000)
            } else {
                Some(at)
            }
        })
        .or_else(|| {
            [
                "reset_after_seconds",
                "resets_in_seconds",
                "retry_after_seconds",
            ]
            .into_iter()
            .find_map(|key| {
                let seconds = number(window.get(key))?;
                if !(0.0..=31_536_000.0).contains(&seconds) {
                    return None;
                }
                logging::now_ms().checked_add((seconds * 1000.0) as i64)
            })
        })
}

fn window_name(seconds: Option<f64>, fallback: &str) -> (String, String) {
    match seconds {
        Some(seconds) if seconds == 18_000.0 => ("5 小时".into(), "5h".into()),
        Some(seconds) if seconds == 604_800.0 => ("每周".into(), "weekly".into()),
        Some(seconds) if seconds >= 86_400.0 && seconds % 86_400.0 == 0.0 => {
            (format!("{} 天", seconds / 86_400.0), fallback.into())
        }
        Some(seconds) if seconds >= 3600.0 && seconds % 3600.0 == 0.0 => {
            (format!("{} 小时", seconds / 3600.0), fallback.into())
        }
        _ => (
            if fallback == "primary" {
                "主要额度窗口"
            } else {
                "次要额度窗口"
            }
            .into(),
            fallback.into(),
        ),
    }
}

fn add_window(
    key: &str,
    prefix: &str,
    raw: &Value,
    model: Option<&str>,
    windows: &mut Vec<Value>,
    wallets: &mut Vec<Value>,
    sources: &mut Vec<Value>,
) {
    if !raw.is_object() {
        return;
    }
    let seconds = number(raw.get("limit_window_seconds"))
        .or_else(|| number(raw.get("window_minutes")).map(|minutes| minutes * 60.0));
    let (label, window) = window_name(seconds, key);
    let label = if prefix.is_empty() {
        label
    } else {
        format!("{prefix} · {label}")
    };
    let id = if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}:{key}")
    };
    let used = number(raw.get("used_percent")).filter(|used| *used >= 0.0);
    let remaining = used.map(|used| (100.0 - used).clamp(0.0, 100.0));
    let resets_at = reset(raw);
    let mut row = json!({ "key": id, "label": label, "window": window });
    if let Some(seconds) = seconds {
        row["windowSeconds"] = json!(seconds);
    }
    if let Some(used) = used {
        row["usedPercent"] = json!(used);
    }
    if let Some(remaining) = remaining {
        row["remainingPercent"] = json!(remaining);
        row["view"] = json!(format!("剩 {:.1}%", remaining));
        wallets.push(json!({ "type": id, "displayName": label, "balance": remaining, "unit": "%", "resetsAt": resets_at }));
    }
    if let Some(resets_at) = resets_at {
        row["resetsAt"] = json!(resets_at);
    }
    windows.push(row);
    let mut source = json!({ "id": format!("window:{id}"), "provider": ID, "label": label,
        "unit": "%", "remainingPercent": remaining });
    if let Some(resets_at) = resets_at {
        source["resetsAt"] = json!(resets_at);
    }
    if let Some(model) = model {
        source["models"] = json!([model]);
    }
    sources.push(source);
}

fn workspace_overage_open(raw: &Value, plan: &str, credits: Option<&Value>) -> bool {
    // Magpie codexWorkspacePlans / codexReservePlans：保留 reserve 实验的两种套餐例外。
    let workspace = matches!(
        plan,
        "free_workspace"
            | "self_serve_business_usage_based"
            | "business"
            | "ent26"
            | "enterprise_cbp_automation"
            | "enterprise_cbp_trial"
            | "enterprise_cbp_usage_based"
            | "enterprise_cbp_view_only"
            | "enterprise"
            | "hc"
            | "finserv"
            | "law"
            | "sci"
            | "education"
            | "edu"
            | "edu_plus"
            | "edu_pro"
            | "quorum"
            | "k12"
    );
    workspace
        && credits
            .and_then(|credits| credits.get("overage_limit_reached"))
            .and_then(Value::as_bool)
            == Some(false)
        && raw
            .pointer("/spend_control/reached")
            .and_then(Value::as_bool)
            != Some(true)
        && raw
            .pointer("/rate_limit_reached_type/type")
            .and_then(Value::as_str)
            .is_none_or(|reason| reason == "rate_limit_reached")
}

fn parse(raw: &Value) -> Result<Value, GatewayError> {
    if !raw.is_object()
        || ["rate_limit", "credits", "plan_type"]
            .iter()
            .all(|key| raw.get(*key).is_none())
    {
        return Err(GatewayError::with_status(
            502,
            "Codex 没有返回可识别的额度数据，未把未知额度记为零",
        ));
    }
    let plan = account::text(raw, &["plan_type"]);
    let mut windows = Vec::new();
    let mut wallets = Vec::new();
    let mut sources = Vec::new();
    if let Some(rate) = raw.get("rate_limit") {
        for (key, field) in [
            ("primary", "primary_window"),
            ("secondary", "secondary_window"),
        ] {
            if let Some(window) = rate.get(field) {
                add_window(
                    key,
                    "",
                    window,
                    None,
                    &mut windows,
                    &mut wallets,
                    &mut sources,
                );
            }
        }
    }
    let available = windows
        .iter()
        .filter_map(|window| window.get("remainingPercent").and_then(Value::as_f64))
        .reduce(f64::min);
    let credits = raw.get("credits").filter(|credits| credits.is_object());
    let credit_balance = credits
        .and_then(|credits| number(credits.get("balance")))
        .filter(|balance| *balance >= 0.0);
    let unlimited = credits
        .and_then(|credits| credits.get("unlimited"))
        .and_then(Value::as_bool)
        == Some(true);
    let has_credits = credits
        .and_then(|credits| credits.get("has_credits"))
        .and_then(Value::as_bool)
        == Some(true);
    if let Some(balance) = credit_balance {
        wallets.push(json!({ "type": "credits", "displayName": "额外点数", "balance": balance, "unit": "credits" }));
    }
    if credits.is_some() {
        sources.push(
            json!({ "id": "credits", "provider": ID, "label": "额外点数", "unit": "credits",
            "remaining": credit_balance, "unlimited": unlimited }),
        );
    }
    let additional = raw.get("additional_rate_limits").and_then(Value::as_array);
    for rate in additional.into_iter().flatten() {
        let label = account::text(rate, &["limit_name", "metered_feature", "name"]);
        if label.is_empty() {
            continue;
        }
        let data = rate.get("rate_limit").unwrap_or(rate);
        let model = data.get("model").and_then(Value::as_str);
        for (key, field) in [
            ("primary", "primary_window"),
            ("secondary", "secondary_window"),
        ] {
            if let Some(window) = data.get(field) {
                add_window(
                    key,
                    label,
                    window,
                    model,
                    &mut windows,
                    &mut wallets,
                    &mut sources,
                );
            }
        }
    }
    // 100% 订阅窗口不代表整个账号不可用：credits / 工作区超额可能仍可服务。
    let can_spend_credits =
        unlimited || has_credits && credit_balance.is_none_or(|balance| balance > 0.0);
    let overage_open = workspace_overage_open(raw, plan, credits);
    let mut report = json!({ "available": if can_spend_credits || overage_open { None } else { available },
        "unit": "%", "windows": windows, "wallets": wallets, "quotaSources": sources,
        "subscription": if plan.is_empty() { Value::Null } else { json!({ "planName": plan }) },
        "unlimited": unlimited, "allowed": raw.pointer("/rate_limit/allowed").and_then(Value::as_bool),
        "hasCredits": has_credits, "creditsBalance": credit_balance });
    if can_spend_credits {
        report["availableView"] = json!(if unlimited {
            "额外点数不限量"
        } else {
            "订阅窗口之外仍有额外点数"
        });
    }
    if let Some(count) = raw
        .pointer("/rate_limit_reset_credits/available_count")
        .and_then(Value::as_u64)
    {
        report["resetCreditsAvailable"] = json!(count);
    }
    // 不把原始账号资料直接公开，只保留额度排障所需的读数子树。
    report["raw"] = json!({ "plan_type": plan, "rate_limit": raw.get("rate_limit"), "credits": credits,
        "spend_control": raw.get("spend_control"), "rate_limit_reached_type": raw.get("rate_limit_reached_type"),
        "rate_limit_reset_credits": raw.get("rate_limit_reset_credits") });
    Ok(report)
}
