use std::collections::{BTreeMap, HashMap};

use chrono::Duration as ChronoDuration;
use rusqlite::{params, params_from_iter, Connection};
use serde_json::{json, Value};

use super::clock::{hour_floor, hour_key, ms_to_local};
use super::report::{model_stat_key, safe_rate, UNKNOWN_MODEL_LABEL};
use super::source::UsageAccum;
use super::source_report::usage_json;

#[derive(Clone, Debug)]
pub(super) struct SummaryRow {
    pub ts: i64,
    pub model: String,
    pub upstream_model: String,
    pub total_tokens: i64,
    pub cache_read_tokens: i64,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub successful: bool,
}

pub(super) fn select_summary_rows(
    conn: &Connection,
    from_ms: i64,
    to_ms: i64,
) -> rusqlite::Result<Vec<SummaryRow>> {
    let mut stmt = conn.prepare(
        "SELECT ts, model, upstream_model, total_tokens, cache_read_tokens, prompt_tokens, \
         completion_tokens, (status >= 200 AND status < 300 AND error IS NULL) \
         FROM requests WHERE ts >= ?1 AND ts <= ?2 AND is_test = 0 AND status <> 0",
    )?;
    let rows = stmt.query_map(params![from_ms, to_ms], |row| Ok(SummaryRow {
        ts: row.get(0)?,
        model: row.get(1)?,
        upstream_model: row.get(2)?,
        total_tokens: row.get(3)?,
        cache_read_tokens: row.get(4)?,
        prompt_tokens: row.get(5)?,
        completion_tokens: row.get(6)?,
        successful: row.get(7)?,
    }))?;
    rows.collect()
}

pub(super) fn cache_trend_24h(entries: &[SummaryRow], now: i64) -> Vec<Value> {
    #[derive(Default)]
    struct HourBucket {
        usage: UsageAccum,
        models: BTreeMap<String, UsageAccum>,
    }

    let start = hour_floor(ms_to_local(now)) - ChronoDuration::hours(23);
    let start_ms = start.timestamp_millis();
    let mut buckets: HashMap<String, HourBucket> = HashMap::new();
    for item in entries {
        if item.ts < start_ms || item.ts > now { continue; }
        let usage = UsageAccum {
            requests: 1,
            successful: i64::from(item.successful),
            tokens: item.total_tokens,
            prompt_tokens: item.prompt_tokens,
            completion_tokens: item.completion_tokens,
            cache_read_tokens: item.cache_read_tokens,
            token_details_requests: 1,
            last_used_at: Some(item.ts),
        };
        let slot = buckets.entry(hour_key(ms_to_local(item.ts))).or_default();
        slot.usage.merge(&usage);
        let model = model_stat_key(&item.model, &item.upstream_model);
        slot.models.entry(model.to_string()).or_default().merge(&usage);
    }

    (0..24).map(|step| {
        let key = hour_key(start + ChronoDuration::hours(step));
        let slot = buckets.remove(&key).unwrap_or_default();
        let mut models: Vec<_> = slot.models.into_iter().collect();
        models.sort_by(|left, right| right.1.tokens.cmp(&left.1.tokens)
            .then(right.1.requests.cmp(&left.1.requests)).then(left.0.cmp(&right.0)));
        let models: Vec<_> = models.into_iter().map(|(model, usage)| {
            let mut row = usage_json(&usage);
            row["label"] = json!(if model.is_empty() { UNKNOWN_MODEL_LABEL } else { &model });
            row["model"] = json!(model);
            row["tokens"] = json!(usage.tokens);
            row
        }).collect();
        let mut row = usage_json(&slot.usage);
        row["hour"] = json!(key);
        row["hitTokens"] = json!(slot.usage.cache_read_tokens);
        row["inputTokens"] = json!(slot.usage.prompt_tokens);
        row["rate"] = json!(safe_rate(slot.usage.cache_read_tokens, slot.usage.prompt_tokens));
        row["models"] = json!(models);
        row
    }).collect()
}

/// 一次扫描覆盖最长窗口，各档只在自己的时间边界内累加。
pub(super) fn sum_cache_windows(
    conn: &Connection,
    now_ms: i64,
    windows_ms: &[i64],
) -> rusqlite::Result<Vec<(i64, i64)>> {
    let Some(longest) = windows_ms.iter().copied().max() else { return Ok(Vec::new()); };
    let mut sql = String::from("SELECT ");
    for index in 0..windows_ms.len() {
        if index > 0 { sql.push_str(", "); }
        let slot = index + 2;
        sql.push_str(&format!(
            "COALESCE(SUM(CASE WHEN ts >= ?{slot} THEN cache_read_tokens END), 0), \
             COALESCE(SUM(CASE WHEN ts >= ?{slot} THEN prompt_tokens END), 0)"
        ));
    }
    let last = windows_ms.len() + 2;
    sql.push_str(&format!(" FROM requests WHERE ts >= ?1 AND ts <= ?{last} AND is_test = 0"));
    let mut binds = Vec::with_capacity(windows_ms.len() + 2);
    binds.push(now_ms - longest);
    binds.extend(windows_ms.iter().map(|window| now_ms - window));
    binds.push(now_ms);
    conn.query_row(&sql, params_from_iter(binds.iter()), |row| {
        (0..windows_ms.len()).map(|index| Ok((row.get(index * 2)?, row.get(index * 2 + 1)?))).collect()
    })
}
