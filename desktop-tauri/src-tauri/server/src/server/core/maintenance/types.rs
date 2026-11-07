use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::server::core::{account_store::AccountStore, providers};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub provider: String,
    pub provider_label: String,
    pub account_id: Option<String>,
    pub account_name: Option<String>,
    pub status: String,
    pub message: Option<String>,
    pub count: Option<u64>,
    pub refreshed_at: Option<i64>,
    pub expires_before: Option<i64>,
    pub expires_after: Option<i64>,
    pub credential_changed: Option<bool>,
    pub duration_ms: Option<i64>,
    pub fixed: Option<bool>,
    pub retry_at: Option<i64>,
    #[serde(default)]
    pub forced: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub task_id: String,
    pub source: String,
    pub retry_of: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub status: String,
    pub refreshed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub total: usize,
    pub error: Option<String>,
    pub metadata: Option<Item>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Detail {
    #[serde(flatten)]
    pub run: Run,
    pub results: Vec<Item>,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Query {
    pub task_id: String,
    pub source: String,
    pub status: String,
    pub keyword: String,
    pub offset: usize,
    pub limit: usize,
}

#[derive(Clone)]
pub struct AccountSnapshot {
    pub name: String,
    pub expires_at: Option<i64>,
}

pub fn accounts(store: &AccountStore) -> HashMap<String, AccountSnapshot> {
    store.list_accounts()["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|account| {
            let id = account.get("id")?.as_str()?;
            let name = if account.get("nameCustom").and_then(Value::as_bool) == Some(true) {
                text(account, "name")
            } else {
                ["nickname", "name", "email"]
                    .into_iter()
                    .find_map(|key| text(account, key))
            }
            .unwrap_or_else(|| id.to_string());
            let expires_at = [
                "accessTokenExpiresAt",
                "tokenExpiresAt",
                "expiresAt",
                "tokenExpireAt",
            ]
            .into_iter()
            .find_map(|key| millis(account.get(key)));
            Some((id.to_string(), AccountSnapshot { name, expires_at }))
        })
        .collect()
}

pub fn item(raw: &Value, index: usize, snapshots: &HashMap<String, AccountSnapshot>) -> Item {
    let provider = text(raw, "provider").unwrap_or_default();
    let account_id = text(raw, "accountId").or_else(|| text(raw, "id"));
    let snapshot = account_id.as_ref().and_then(|id| snapshots.get(id));
    let status = match raw.get("status").and_then(Value::as_str) {
        Some("refreshed") => "refreshed",
        Some("skipped") => "skipped",
        _ => "failed",
    };
    Item {
        id: index.to_string(),
        provider_label: text(raw, "providerLabel")
            .unwrap_or_else(|| providers::label_of(&provider)),
        provider,
        account_id,
        account_name: snapshot
            .map(|account| account.name.clone())
            .or_else(|| text(raw, "accountName")),
        status: status.to_string(),
        message: text(raw, "message").map(|message| safe_message(&message)),
        count: raw.get("count").and_then(Value::as_u64),
        refreshed_at: millis(raw.get("refreshedAt")),
        expires_before: millis(raw.get("expiresBefore"))
            .or_else(|| snapshot.and_then(|account| account.expires_at)),
        expires_after: millis(raw.get("expiresAfter")).or_else(|| {
            (status == "skipped")
                .then(|| snapshot.and_then(|account| account.expires_at))
                .flatten()
        }),
        credential_changed: raw.get("credentialChanged").and_then(Value::as_bool),
        duration_ms: raw.get("durationMs").and_then(Value::as_i64),
        fixed: raw.get("fixed").and_then(Value::as_bool),
        retry_at: millis(raw.get("retryAt")),
        forced: raw.get("forced").and_then(Value::as_bool).unwrap_or(false),
    }
}

pub fn summary(results: &[Item]) -> (usize, usize, usize) {
    let count = |status: &str| results.iter().filter(|item| item.status == status).count();
    (count("refreshed"), count("skipped"), count("failed"))
}

pub fn status(refreshed: usize, failed: usize, metadata: Option<&Item>) -> &'static str {
    if failed > 0 {
        if refreshed > 0 {
            "partial"
        } else {
            "failed"
        }
    } else if metadata.is_some_and(|item| item.status == "failed") {
        "partial"
    } else if refreshed > 0 || metadata.is_some_and(|item| item.status == "refreshed") {
        "completed"
    } else {
        "skipped"
    }
}

pub fn report(detail: &Detail) -> Value {
    json!({
        "results": detail.results.iter().map(|item| json!({
            "id": item.account_id,
            "provider": item.provider,
            "providerLabel": item.provider_label,
            "accountId": item.account_id,
            "status": item.status,
            "message": item.message,
            "count": item.count,
            "refreshedAt": item.refreshed_at,
            "fixed": item.fixed,
            "retryAt": item.retry_at,
            "durationMs": item.duration_ms,
            "expiresBefore": item.expires_before,
            "expiresAfter": item.expires_after,
            "credentialChanged": item.credential_changed,
        })).collect::<Vec<_>>(),
        "refreshed": detail.run.refreshed,
        "skipped": detail.run.skipped,
        "failed": detail.run.failed,
        "runId": detail.run.id,
    })
}

pub fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(str::to_string)
}

fn millis(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    let number = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()));
    if let Some(number) = number.filter(|number| number.is_finite() && *number > 0.0) {
        return Some(if number < 100_000_000_000.0 {
            number * 1000.0
        } else {
            number
        } as i64);
    }
    value
        .as_str()
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .map(|time| time.timestamp_millis())
}

pub fn safe_message(message: &str) -> String {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| [
        r"(?i)\bbearer\s+[^\s,;]+",
        r#"(?i)\b(?:authorization|proxy-authorization|cookie|set-cookie)["'\s]*[:=]\s*(?:"[^"]*"|'[^']*'|[^\r\n}]*)"#,
        r#"(?i)(?:access[_-]?token|refresh[_-]?token|id[_-]?token|auth[_-]?token|passport[_-]?token|token|api[_-]?key|password|client[_-]?secret|secret)["'\s]*[:=][\s"']*[^\s"',;&}]+"#,
        r"\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+",
        r"\bsk-[A-Za-z0-9_-]{12,}",
        r"[A-Za-z0-9_+/=-]{48,}",
    ].into_iter().filter_map(|pattern| Regex::new(pattern).ok()).collect());
    let mut safe = message.chars().take(4000).collect::<String>();
    for pattern in patterns {
        safe = pattern.replace_all(&safe, "[已隐藏凭证]").into_owned();
    }
    safe
}
