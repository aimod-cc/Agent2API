//! 5 段 Cron 定时计划与 IANA 时区校验、下次执行时间计算。

use std::str::FromStr;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use croner::Cron;

pub const DEFAULT_TIMEZONE: &str = "Asia/Shanghai";

pub fn validate_timezone(raw: &str) -> Result<String, String> {
    let tz_name = raw.trim();
    let tz_name = if tz_name.is_empty() {
        DEFAULT_TIMEZONE
    } else {
        tz_name
    };
    tz_name
        .parse::<Tz>()
        .map(|_| tz_name.to_string())
        .map_err(|_| format!("无效时区：{tz_name}"))
}

pub fn validate_cron(raw: &str) -> Result<String, String> {
    let normalized = raw
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let parts: Vec<&str> = normalized.split(' ').filter(|s| !s.is_empty()).collect();
    if parts.len() != 5 {
        return Err("定时计划必须是 5 段 Cron 表达式（分 时 日 月 周）".to_string());
    }
    Cron::from_str(&normalized)
        .map_err(|error| format!("定时计划格式无效：{error}"))?;
    Ok(normalized)
}

pub fn next_run_ms(cron_expr: &str, timezone: &str, after_ms: i64) -> Result<i64, String> {
    let expr = validate_cron(cron_expr)?;
    let tz_name = validate_timezone(timezone)?;
    let tz: Tz = tz_name
        .parse()
        .map_err(|_| format!("无效时区：{tz_name}"))?;
    let cron = Cron::from_str(&expr).map_err(|error| format!("定时计划格式无效：{error}"))?;
    let base_utc = DateTime::<Utc>::from_timestamp_millis(after_ms).unwrap_or_else(Utc::now);
    let base_tz = base_utc.with_timezone(&tz);
    let next = cron
        .find_next_occurrence(&base_tz, false)
        .map_err(|error| format!("无法计算下次执行时间：{error}"))?;
    Ok(next.timestamp_millis())
}

pub fn preview_next_runs(cron_expr: &str, timezone: &str, count: usize) -> Result<Vec<i64>, String> {
    let mut cursor = crate::server::logging::now_ms();
    let mut list = Vec::with_capacity(count.min(10));
    for _ in 0..count.min(10) {
        let next = next_run_ms(cron_expr, timezone, cursor)?;
        list.push(next);
        cursor = next;
    }
    Ok(list)
}
