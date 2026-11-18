//! Magpie claude_usage_cli.go 的 /usage 文本窗口 + Claude rate_limit_event。
//! 某些 CLI 版本的 -p /usage 只报会话花费：没有窗口时明确失败，不伪造余额。
use chrono::{Datelike, FixedOffset, NaiveDateTime, TimeZone, Utc};
use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;

use super::{accounts, errors, process};
use crate::server::core::account_store::AccountStore;
use crate::server::{errors::GatewayError, logging};

const LEDGER: &str = "claudeCliLimits";

fn window(key: &str, used: f64, reset: Option<i64>) -> Value {
    let label = match key {
        "five_hour" => "5 小时",
        "seven_day" => "每周（全部模型）",
        "seven_day_sonnet" => "每周（Sonnet）",
        "seven_day_opus" => "每周（Opus）",
        "extra_usage" => "额外用量",
        other => other,
    };
    json!({ "key": key, "label": label, "window": if key == "five_hour" { "5h" } else { "weekly" },
        "remainingPercent": (100.0 - used).clamp(0.0, 100.0), "usedPercent": used,
        "remaining": Value::Null, "limit": Value::Null, "resetsAt": reset })
}

pub(super) fn note(store: &AccountStore, account: &str, info: &Value) {
    let mut windows = Vec::new();
    if let Some(all) = info.get("unifiedWindows").and_then(Value::as_object) {
        for (key, data) in all.iter().take(32) {
            if key.len() > 128 {
                continue;
            }
            if let Some(used) = data.get("utilization").and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0) {
                windows.push(window(key, used * 100.0, data.get("resetsAt").and_then(errors::timestamp)));
            }
        }
    }
    if let Some(key) = info.get("rateLimitType").and_then(Value::as_str).filter(|key| key.len() <= 128) {
        if !windows.iter().any(|row| row.get("key").and_then(Value::as_str) == Some(key)) {
            let used = info
                .get("utilization")
                .and_then(Value::as_f64)
                .or_else(|| (info.get("status").and_then(Value::as_str) == Some("rejected")).then_some(1.0));
            if let Some(used) = used.filter(|v| v.is_finite() && *v >= 0.0) {
                windows.push(window(key, used * 100.0, info.get("resetsAt").and_then(errors::timestamp)));
            }
        }
    }
    if !windows.is_empty() {
        let old = store.plugin_ledger(account, LEDGER);
        for previous in old.get("windows").and_then(Value::as_array).into_iter().flatten().take(32) {
            if windows.len() < 32 && !windows.iter().any(|row| row.get("key") == previous.get("key")) {
                windows.push(previous.clone());
            }
        }
        let value =
            json!({ "windows": windows, "fetchedAt": logging::now_ms(), "source": "official-cli-rate-limit-event" });
        let _ = store.plugin_set_ledger(account, LEDGER, &value);
    }
}

pub(super) async fn query(store: &AccountStore, account: &str) -> Result<Value, GatewayError> {
    let link = accounts::load(store, account)?;
    let proxy = accounts::account_proxy(store, account)?;
    accounts::verify(&link, proxy.as_ref()).await?;
    let mut args = vec!["-p".into(), "/usage".into(), "--output-format".into(), "json".into()];
    args.extend(process::safe_args(None, false));
    let output = process::capture(&link, &args, proxy.as_ref()).await?;
    link.check_profile()?;
    let value: Value =
        serde_json::from_slice(&output).map_err(|_| errors::protocol("Claude /usage 没有返回 JSON 结果"))?;
    if value.get("is_error").and_then(Value::as_bool) == Some(true) {
        return Err(errors::upstream(&value));
    }
    let text =
        value.get("result").and_then(Value::as_str).ok_or_else(|| errors::protocol("Claude /usage 没有返回读数"))?;
    let windows = parse(text)?;
    let report = json!({ "available": Value::Null, "unit": "%", "windows": windows,
        "subscription": {"planName": link.plan}, "fetchedAt": logging::now_ms(), "source": "official-cli-usage" });
    store
        .plugin_set_ledger(account, LEDGER, &report)
        .map_err(|_| errors::local("claude_usage_store_failed", "Claude 用量已读取，但账号台账保存失败"))?;
    Ok(report)
}

fn parse(text: &str) -> Result<Vec<Value>, GatewayError> {
    static PATTERN: OnceLock<Option<Regex>> = OnceLock::new();
    let regex = PATTERN
        .get_or_init(|| {
            Regex::new(r"^Current (session|week(?: \(([^)]+)\))?):\s*([0-9.]+)% used(?:\s*·\s*resets (.+))?$").ok()
        })
        .as_ref()
        .ok_or_else(|| errors::protocol("Claude 用量解析器不可用"))?;
    static ANSI: OnceLock<Option<Regex>> = OnceLock::new();
    let text = ANSI
        .get_or_init(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").ok())
        .as_ref()
        .map(|pattern| pattern.replace_all(text, "").into_owned())
        .unwrap_or_else(|| text.to_string());
    let lower = text.to_ascii_lowercase();
    if ["not logged in", "not signed in", "unauthorized", "forbidden", "authentication failed", "invalid token"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return Err(GatewayError::with_status(401, "Claude /usage 报告登录无效，请先在官方 CLI 重新登录"));
    }
    let mut rows = Vec::new();
    for line in text.lines() {
        if rows.len() >= 32 {
            return Err(errors::protocol("Claude 用量窗口数量异常"));
        }
        let Some(caps) = regex.captures(line.trim()) else {
            continue;
        };
        let Some(used) =
            caps.get(3).and_then(|m| m.as_str().parse::<f64>().ok()).filter(|n| n.is_finite() && *n >= 0.0)
        else {
            continue;
        };
        let scope = caps.get(2).map(|m| m.as_str().to_ascii_lowercase()).unwrap_or_default();
        let scope = scope.strip_suffix(" only").unwrap_or(&scope).trim();
        let key = if caps.get(1).is_some_and(|m| m.as_str() == "session") {
            "five_hour".into()
        } else if scope.is_empty() || scope == "all models" {
            "seven_day".into()
        } else {
            format!("seven_day_{}", scope.replace(' ', "_"))
        };
        if rows.iter().any(|row: &Value| row.get("key").and_then(Value::as_str) == Some(key.as_str())) {
            continue;
        }
        let reset_text = caps.get(4).map(|m| m.as_str()).unwrap_or("");
        let mut row = window(&key, used, reset(reset_text));
        if !reset_text.is_empty() {
            row["resetText"] = json!(reset_text.chars().take(100).collect::<String>());
        }
        if key.starts_with("seven_day_") {
            row["label"] = json!(format!("每周（{scope}）"));
        }
        rows.push(row);
    }
    if lower.contains("using your overages") {
        let mut row = window("extra_usage", 100.0, None);
        row["view"] = json!("正在使用额外付费用量");
        rows.push(row);
    }
    if rows.is_empty() {
        return Err(errors::local(
            "claude_usage_unavailable",
            "此 Claude Code 版本的非交互 /usage 没有提供订阅窗口；请在官方 CLI 查看。未将会话花费当成余额",
        ));
    }
    Ok(rows)
}

/// 不增加整套时区库；明确认识的时区才换算，其余保留 resetText、不猜本机时区。
fn reset(text: &str) -> Option<i64> {
    let (date, zone) = text.trim().rsplit_once(" (")?;
    let seconds = match zone.trim_end_matches(')') {
        "UTC" | "Etc/UTC" | "GMT" => 0,
        "Asia/Shanghai" | "Asia/Hong_Kong" | "Asia/Taipei" | "Asia/Singapore" => 8 * 3600,
        "Asia/Tokyo" | "Asia/Seoul" => 9 * 3600,
        _ => return None,
    };
    let offset = FixedOffset::east_opt(seconds)?;
    let now = Utc::now().with_timezone(&offset);
    let date = date.replace("AM", "am").replace("PM", "pm");
    for year in [now.year() - 1, now.year(), now.year() + 1] {
        for format in ["%Y %b %e at %I:%M%P", "%Y %b %e at %I%P", "%Y %b %e, %I:%M%P", "%Y %b %e, %I%P"] {
            if let Ok(local) = NaiveDateTime::parse_from_str(&format!("{year} {date}"), format) {
                let at = offset.from_local_datetime(&local).single()?.timestamp_millis();
                if at > logging::now_ms() - 86_400_000 && at < logging::now_ms() + 9 * 86_400_000 {
                    return Some(at);
                }
            }
        }
    }
    for day in [0, 1] {
        let date_prefix = (now.date_naive() + chrono::Duration::days(day)).format("%Y-%m-%d").to_string();
        for format in ["%Y-%m-%d %I:%M%P", "%Y-%m-%d %I%P"] {
            if let Ok(local) = NaiveDateTime::parse_from_str(&format!("{date_prefix} {date}"), format) {
                let at = offset.from_local_datetime(&local).single()?.timestamp_millis();
                if at > logging::now_ms() && at < logging::now_ms() + 7 * 3_600_000 {
                    return Some(at);
                }
            }
        }
    }
    None
}
