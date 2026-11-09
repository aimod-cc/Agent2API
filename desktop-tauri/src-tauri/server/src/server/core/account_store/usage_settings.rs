use serde_json::{json, Value};

use super::store_util::js_truthy;
use super::{AccountStore, AccountStoreError};
use crate::server::core::usage_records::{
    query_retries_of, query_timeout_of, DEFAULT_QUERY_INTERVAL_SECONDS, DEFAULT_QUERY_RETRIES, DEFAULT_QUERY_TIMEOUT_SECONDS, MAX_QUERY_TIMEOUT_SECONDS,
    MAX_QUERY_INTERVAL_SECONDS, MAX_QUERY_RETRIES, MIN_QUERY_INTERVAL_SECONDS,
};

impl AccountStore {
    pub(crate) fn normalize_usage_query(value: &Value) -> Result<Value, AccountStoreError> {
        let Some(fields) = value.as_object() else {
            return Ok(usage_query_public(None));
        };
        let enabled = fields.get("enabled").map(js_truthy).unwrap_or(false);
        let interval = match fields.get("interval") {
            None | Some(Value::Null) => 0,
            Some(raw) => raw.as_i64().ok_or_else(|| {
                AccountStoreError::bad_request("查询间隔必须是整数（单位：秒）")
            })?,
        };
        if interval != 0 && !(MIN_QUERY_INTERVAL_SECONDS..=MAX_QUERY_INTERVAL_SECONDS).contains(&interval) {
            return Err(AccountStoreError::bad_request(format!(
                "查询间隔必须是 {MIN_QUERY_INTERVAL_SECONDS}~{MAX_QUERY_INTERVAL_SECONDS} 秒"
            )));
        }
        if enabled && interval == 0 {
            return Err(AccountStoreError::bad_request(format!(
                "开启自动查询需要填写 {MIN_QUERY_INTERVAL_SECONDS}~{MAX_QUERY_INTERVAL_SECONDS} 秒的间隔"
            )));
        }
        let retries = match fields.get("retries") {
            None => DEFAULT_QUERY_RETRIES,
            Some(raw) => raw.as_u64().filter(|value| *value <= MAX_QUERY_RETRIES)
                .ok_or_else(|| AccountStoreError::bad_request(format!(
                    "失败重试次数必须是 0~{MAX_QUERY_RETRIES} 的整数"
                )))?,
        };
        let timeout = match fields.get("timeoutSeconds") {
            None => DEFAULT_QUERY_TIMEOUT_SECONDS,
            Some(value) => value.as_u64().filter(|value| (15..=MAX_QUERY_TIMEOUT_SECONDS).contains(value))
                .ok_or_else(|| AccountStoreError::bad_request("整轮查询时限必须是 15~180 秒的整数"))?,
        };
        Ok(json!({ "enabled": enabled, "interval": interval, "retries": retries, "timeoutSeconds": timeout }))
    }

    pub(super) fn normalize_usage_query_lenient(value: Option<&Value>) -> Value {
        usage_query_public(value)
    }

    pub(crate) fn describe_usage_query(next: &Value) -> String {
        let enabled = next.get("enabled").and_then(Value::as_bool).unwrap_or(false);
        let mut description = if enabled {
            let seconds = next.get("interval").and_then(Value::as_i64).unwrap_or(0);
            let span = if seconds > 0 && seconds % 3600 == 0 {
                format!("{} 小时", seconds / 3600)
            } else if seconds > 0 && seconds % 60 == 0 {
                format!("{} 分钟", seconds / 60)
            } else {
                format!("{seconds} 秒")
            };
            format!("自动查询余额 → 开启（每 {span}）")
        } else {
            "自动查询余额 → 关闭".to_string()
        };
        if let Some(retries) = next.get("retries").and_then(Value::as_u64) {
            description.push_str(&format!("；失败重试 → {retries} 次"));
        }
        if let Some(timeout) = next.get("timeoutSeconds").and_then(Value::as_u64) {
            description.push_str(&format!("；整轮时限 → {timeout} 秒"));
        }
        description
    }
}

pub(super) fn usage_query_public(value: Option<&Value>) -> Value {
    let retries = query_retries_of(value);
    let timeout = query_timeout_of(value);
    let Some(fields) = value.and_then(Value::as_object) else {
        return json!({ "enabled": true, "interval": DEFAULT_QUERY_INTERVAL_SECONDS, "retries": retries, "timeoutSeconds": timeout });
    };
    let enabled = fields.get("enabled").map(js_truthy).unwrap_or(true);
    let interval = fields.get("interval").and_then(Value::as_i64).unwrap_or(0);
    let interval = if enabled
        && !(MIN_QUERY_INTERVAL_SECONDS..=MAX_QUERY_INTERVAL_SECONDS).contains(&interval)
    {
        DEFAULT_QUERY_INTERVAL_SECONDS
    } else {
        interval
    };
    json!({ "enabled": enabled, "interval": interval, "retries": retries, "timeoutSeconds": timeout })
}
