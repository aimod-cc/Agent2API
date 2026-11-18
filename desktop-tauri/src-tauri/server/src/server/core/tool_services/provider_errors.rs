use std::sync::OnceLock;

use chrono::{DateTime, TimeZone, Utc};
use regex::Regex;
use reqwest::header::HeaderMap;
use serde_json::Value;

use super::search::{SearchError, SearchErrorCode};

fn error_texts(value: &Value, depth: usize, out: &mut Vec<String>) {
    if depth > 4 {
        return;
    }
    match value {
        Value::String(text) => out.push(text.clone()),
        Value::Object(object) => {
            for field in [
                "error",
                "error_msg",
                "detail",
                "message",
                "reason",
                "code",
                "type",
            ] {
                if let Some(value) = object.get(field) {
                    error_texts(value, depth + 1, out);
                }
            }
        }
        _ => {}
    }
}

fn detail(body: Option<&Value>, raw: &str, api_key: &str, status: u16) -> String {
    let mut texts = Vec::new();
    if let Some(body) = body {
        error_texts(body, 0, &mut texts);
    }
    if texts.is_empty() && body.is_none() && !raw.trim_start().starts_with('<') {
        texts.push(raw.to_string());
    }
    let text = texts
        .into_iter()
        .find(|text| !text.trim().is_empty())
        .unwrap_or_else(|| format!("HTTP {status}"));
    let text = text.replace(api_key, "[REDACTED]");
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(500)
        .collect()
}

fn quota_message(text: &str) -> bool {
    [
        "insufficient credits",
        "insufficient_credit",
        "insufficient balance",
        "insufficient_balance",
        "insufficient quota",
        "insufficient_quota",
        "quota exhausted",
        "quota_exhausted",
        "credits exhausted",
        "credits_exhausted",
        "credit balance exhausted",
        "credit_balance_exhausted",
        "out of credits",
        "out_of_credits",
        "no credits",
        "exhausted your credits",
        "exhausted your quota",
        "usage limit exceeded",
        "credit limit exceeded",
        "quota exceeded",
        "quota_exceeded",
        "exceeded your quota",
        "quota is exhausted",
        "quota has been exhausted",
        "quota has been exceeded",
        "quota has been reached",
        "quota limit reached",
        "quota limit exceeded",
        "credit limit reached",
        "credit_limit_reached",
        "credits limit reached",
        "not enough credits",
        "not enough balance",
        "prepaid quota is exhausted",
        "余额已用完",
        "额度不足",
        "额度已用完",
        "额度耗尽",
        "积分不足",
        "积分已用完",
        "积分耗尽",
        "余额不足",
        "配额不足",
        "配额耗尽",
        "配额已用完",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}

fn timestamp(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => {
            let number = number.as_f64()?;
            if !number.is_finite() || number <= 0.0 {
                return None;
            }
            let millis = if number >= 100_000_000_000.0 {
                number
            } else {
                number * 1000.0
            };
            Utc.timestamp_millis_opt(millis as i64)
                .single()
                .map(|date| date.timestamp_millis())
        }
        Value::String(text) => DateTime::parse_from_rfc3339(text.trim())
            .ok()
            .or_else(|| DateTime::parse_from_rfc2822(text.trim()).ok())
            .map(|date| date.timestamp_millis())
            .or_else(|| {
                text.trim()
                    .parse::<f64>()
                    .ok()
                    .and_then(|number| timestamp(&serde_json::json!(number)))
            }),
        _ => None,
    }
}

fn structured_reset(value: &Value, depth: usize) -> Option<i64> {
    if depth > 5 {
        return None;
    }
    let object = value.as_object()?;
    for field in [
        "reset_at",
        "resets_at",
        "resetAt",
        "resetsAt",
        "reset_time",
        "resetTime",
        "reset_timestamp",
        "retry_at",
        "retryAt",
        "retry_time",
        "retryTime",
        "quota_reset_at",
        "quotaResetAt",
        "creditsResetAt",
        "credits_reset_at",
        "planCreditsResetAt",
        "plan_credits_reset_at",
        "retryAfterDate",
    ] {
        if let Some(time) = object.get(field).and_then(timestamp) {
            return Some(time);
        }
    }
    for field in [
        "error",
        "detail",
        "data",
        "quota",
        "usage",
        "rate_limit",
        "rateLimit",
    ] {
        if let Some(time) = object
            .get(field)
            .and_then(|node| structured_reset(node, depth + 1))
        {
            return Some(time);
        }
    }
    None
}

fn reset_from_message(text: &str) -> Option<i64> {
    static RESET: OnceLock<Regex> = OnceLock::new();
    let pattern = RESET.get_or_init(|| Regex::new(
        r"(?i)(?:reset(?:s|ting)?(?:\s+(?:at|on|time))?|renews?(?:\s+at)?|retry\s+at|恢复时间|重置时间)[^0-9]{0,24}(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2}))"
    ).expect("valid credit reset pattern"));
    let captured = pattern.captures(text)?.get(1)?.as_str();
    DateTime::parse_from_rfc3339(captured)
        .ok()
        .map(|date| date.timestamp_millis())
}

fn retry_after(headers: &HeaderMap, body: Option<&Value>, now: i64) -> Option<i64> {
    let header = headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok());
    if let Some(text) = header {
        if let Ok(seconds) = text.trim().parse::<f64>() {
            if seconds.is_finite() && seconds >= 0.0 {
                return Some(now.saturating_add((seconds * 1000.0).ceil() as i64));
            }
        }
        if let Some(time) = timestamp(&Value::String(text.to_string())) {
            return Some(time);
        }
    }
    let body = body?;
    for field in [
        "retry_after",
        "retryAfter",
        "retry_after_seconds",
        "retryAfterSeconds",
    ] {
        let value = body
            .get(field)
            .or_else(|| body.get("error").and_then(|error| error.get(field)));
        let seconds = value.and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_str()?.parse::<f64>().ok())
        });
        if let Some(seconds) = seconds.filter(|value| value.is_finite() && *value >= 0.0) {
            return Some(now.saturating_add((seconds * 1000.0).ceil() as i64));
        }
    }
    None
}

fn rate_reset(headers: &HeaderMap, now: i64) -> Option<i64> {
    ["ratelimit-reset", "x-ratelimit-reset"]
        .into_iter()
        .find_map(|field| {
            let text = headers.get(field)?.to_str().ok()?;
            let seconds = text.trim().parse::<f64>().ok();
            if let Some(seconds) = seconds.filter(|value| value.is_finite() && *value >= 0.0) {
                if field == "ratelimit-reset" || seconds < 1_000_000_000.0 {
                    return Some(now.saturating_add((seconds * 1000.0).ceil() as i64));
                }
            }
            timestamp(&Value::String(text.to_string())).filter(|time| *time > now)
        })
}

fn reset_at(
    headers: &HeaderMap,
    body: Option<&Value>,
    raw: &str,
    code: SearchErrorCode,
) -> Option<i64> {
    let now = crate::server::logging::now_ms();
    let explicit = body
        .and_then(|value| structured_reset(value, 0))
        .filter(|time| *time > now)
        .or_else(|| reset_from_message(raw).filter(|time| *time > now))
        .or_else(|| {
            ["x-quota-reset", "x-credits-reset"]
                .into_iter()
                .filter_map(|field| headers.get(field).and_then(|value| value.to_str().ok()))
                .find_map(|text| {
                    timestamp(&Value::String(text.to_string())).filter(|time| *time > now)
                })
        });
    match code {
        SearchErrorCode::RateLimited => [
            explicit,
            retry_after(headers, body, now),
            rate_reset(headers, now),
        ]
        .into_iter()
        .flatten()
        .max()
        .map(|time| {
            if time <= now {
                now.saturating_add(1000)
            } else {
                time
            }
        })
        .or(Some(now.saturating_add(60_000))),
        SearchErrorCode::QuotaExhausted => explicit
            .or_else(|| retry_after(headers, body, now))
            .filter(|time| *time > now),
        _ => None,
    }
}

pub(super) fn business_status(body: &Value) -> Option<u16> {
    let value = body.get("error_code")?;
    let code = value
        .as_u64()
        .or_else(|| value.as_str()?.parse::<u64>().ok())?;
    if code == 0 || (200..300).contains(&code) {
        None
    } else {
        Some(u16::try_from(code).unwrap_or(500))
    }
}

pub(super) fn response_error(
    provider: &str,
    status: u16,
    headers: &HeaderMap,
    body: Option<&Value>,
    raw: &str,
    api_key: &str,
) -> Option<SearchError> {
    let business_status = if (200..300).contains(&status) {
        body.and_then(business_status)
    } else {
        None
    };
    let effective = business_status.unwrap_or(status);
    let reported_failure = body.is_some_and(|body| {
        body.get("success").and_then(Value::as_bool) == Some(false)
            || body
                .get("error")
                .is_some_and(|error| !error.is_null() && error != "" && error != false)
            || body
                .get("detail")
                .and_then(|detail| detail.get("error"))
                .is_some_and(|error| !error.is_null() && error != "" && error != false)
    });
    if (200..300).contains(&effective) && !reported_failure {
        return None;
    }
    let text = detail(body, raw, api_key, effective);
    let mut texts = Vec::new();
    if let Some(body) = body {
        error_texts(body, 0, &mut texts);
    }
    let markers = if texts.is_empty() {
        text.to_lowercase()
    } else {
        texts.join(" ").to_lowercase()
    };
    // Tavily 432/433 为预算限制；Firecrawl 402 为积分不足。Querit 的 403 也可能是套餐权限。
    let quota = (provider == "tavily" && matches!(effective, 432 | 433))
        || (provider == "firecrawl" && effective == 402)
        || quota_message(&markers);
    let code = if quota {
        SearchErrorCode::QuotaExhausted
    } else if effective == 401 {
        SearchErrorCode::InvalidProviderKey
    } else if effective == 429 {
        SearchErrorCode::RateLimited
    } else if effective == 400 || effective == 422 {
        SearchErrorCode::InvalidInput
    } else {
        SearchErrorCode::ProviderUnavailable
    };
    let message = match code {
        SearchErrorCode::InvalidProviderKey => {
            "The provider API key is invalid or unauthorized".to_string()
        }
        SearchErrorCode::QuotaExhausted => format!("The provider quota is exhausted: {text}"),
        SearchErrorCode::RateLimited => format!("The provider rate limit was exceeded: {text}"),
        _ => format!("The upstream provider returned {text}"),
    };
    Some(SearchError {
        code,
        message,
        reset_at: reset_at(headers, body, raw, code),
    })
}
