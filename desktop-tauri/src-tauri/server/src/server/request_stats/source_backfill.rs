use std::collections::BTreeMap;

use rusqlite::{params, Connection};

use super::clock::{date_key, day_of};
use super::record::{DailyEntry, RequestEntry};
use super::source::{ApiKeyAccum, ClientAccum, UsageAccum, SOURCE_STATS_VERSION};
use super::{daily, fold_into_daily, sql, RequestStats};

struct RecoveredDay {
    daily: DailyEntry,
    interrupted_requests: i64,
}

fn usage_fits(total: &DailyEntry, covered: &UsageAccum) -> bool {
    let missing_requests = total.requests - covered.requests;
    let missing_success = total.successful - covered.successful;
    missing_requests >= 0 && missing_success >= 0
        && missing_success <= missing_requests && covered.tokens <= total.tokens
}

/// 日报与保留明细会重叠，不能直接相加，也不能减少已清理明细的历史贡献。
fn merge_usage(existing: &UsageAccum, incoming: &UsageAccum) -> UsageAccum {
    let mut merged = if incoming.token_details_requests > existing.token_details_requests {
        incoming.clone()
    } else {
        existing.clone()
    };
    let details_requests = merged.requests;
    let details_tokens = merged.tokens;
    merged.requests = existing.requests.max(incoming.requests);
    merged.successful = existing.successful.max(incoming.successful);
    merged.tokens = existing.tokens.max(incoming.tokens);
    merged.last_used_at = existing.last_used_at.max(incoming.last_used_at);
    if merged.requests != details_requests || merged.tokens != details_tokens {
        merged.token_details_requests = 0;
    }
    merged
}

fn merge_key_usage(existing: &ApiKeyAccum, incoming: &ApiKeyAccum) -> ApiKeyAccum {
    let mut merged = existing.clone();
    merged.usage = merge_usage(&existing.usage, &incoming.usage);
    if incoming.usage.last_used_at >= existing.usage.last_used_at && !incoming.identity.name.is_empty() {
        merged.identity.name = incoming.identity.name.clone();
    }
    merged.clients = merge_clients(&existing.clients, &incoming.clients);
    for model in &incoming.models {
        if let Some(row) = merged.models.iter_mut().find(|row| row.model == model.model) {
            row.requests = row.requests.max(model.requests);
            row.tokens = row.tokens.max(model.tokens);
        } else {
            merged.models.push(model.clone());
        }
    }
    merged
}

fn merge_clients(existing: &[ClientAccum], incoming: &[ClientAccum]) -> Vec<ClientAccum> {
    let mut merged = existing.to_vec();
    for item in incoming {
        if let Some(row) = merged.iter_mut().find(|row| row.client == item.client) {
            if !item.label.is_empty() && item.usage.last_used_at >= row.usage.last_used_at { row.label = item.label.clone(); }
            row.usage = merge_usage(&row.usage, &item.usage);
        } else {
            merged.push(item.clone());
        }
    }
    merged
}

fn merge_keys(existing: &[ApiKeyAccum], incoming: &[ApiKeyAccum]) -> Vec<ApiKeyAccum> {
    let mut merged = existing.to_vec();
    for item in incoming {
        if let Some(row) = merged.iter_mut().find(|row| {
            row.identity.id == item.identity.id && row.identity.source == item.identity.source
        }) {
            *row = merge_key_usage(row, item);
        } else {
            merged.push(item.clone());
        }
    }
    merged
}

fn total_usage(usages: impl Iterator<Item = UsageAccum>) -> UsageAccum {
    let mut total = UsageAccum::default();
    for usage in usages { total.merge(&usage); }
    total
}

fn is_interrupted(item: &RequestEntry) -> bool {
    item.status == 408 && matches!(item.error.as_deref(),
        Some("客户端在响应完成前断开连接" | "网关重启或流中断，请求未能完成"))
}

fn interruption_gap(existing: &DailyEntry, recovered: &RecoveredDay) -> bool {
    let day = &recovered.daily;
    let extra = day.requests - existing.requests;
    extra > 0 && extra == recovered.interrupted_requests
        && day.successful == existing.successful && day.tokens == existing.tokens
        && day.cache_input_tokens == existing.cache_input_tokens
        && day.cache_hit_tokens == existing.cache_hit_tokens
}

/// 只用仍保留的明细补来源；已清理明细的历史总量和分类必须保留。
pub(super) fn backfill_sources(conn: &Connection) -> rusqlite::Result<()> {
    let mut days = daily::select_daily_map(conn)?;
    days.retain(|_, day| day.source_stats_version < SOURCE_STATS_VERSION);
    if days.is_empty() { return Ok(()); }
    let mut rebuilt: BTreeMap<String, RecoveredDay> = BTreeMap::new();
    {
        let mut stmt = conn.prepare(&format!(
            "SELECT {} FROM requests WHERE is_test = 0 AND status <> 0 ORDER BY ts, row_id",
            sql::REQUEST_COLUMNS,
        ))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let item = sql::decode_request(row)?.normalized_for_aggregate();
            let key = date_key(day_of(item.ts));
            if !days.contains_key(&key) { continue; }
            let recovered = rebuilt.entry(key.clone()).or_insert_with(|| RecoveredDay {
                daily: DailyEntry::new(key), interrupted_requests: 0,
            });
            recovered.interrupted_requests += i64::from(is_interrupted(&item));
            fold_into_daily(&mut recovered.daily, &item);
        }
    }
    for (key, mut day) in days {
        if let Some(recovered) = rebuilt.get(&key) {
            // 旧版断线收尾只更新明细；用量全部对齐且差额恰好为断线行时补齐日报。
            if interruption_gap(&day, recovered) {
                let mut repaired = recovered.daily.clone();
                repaired.source_stats_version = SOURCE_STATS_VERSION;
                daily::upsert_daily(conn, &repaired)?;
                continue;
            }
            let clients = merge_clients(&day.client_stats, &recovered.daily.client_stats);
            let keys = merge_keys(&day.api_key_stats, &recovered.daily.api_key_stats);
            if usage_fits(&day, &total_usage(clients.iter().map(|row| row.usage.clone()))) {
                day.client_stats = clients;
            }
            if usage_fits(&day, &total_usage(keys.iter().map(|row| row.usage.clone()))) {
                day.api_key_stats = keys;
            }
        }
        conn.execute(
            "UPDATE request_daily SET client_stats = ?2, api_key_stats = ?3, source_stats_version = ?4 WHERE date = ?1",
            params![key,
                serde_json::to_string(&day.client_stats).unwrap_or_else(|_| "[]".to_string()),
                serde_json::to_string(&day.api_key_stats).unwrap_or_else(|_| "[]".to_string()),
                SOURCE_STATS_VERSION,
            ],
        )?;
    }
    Ok(())
}

impl RequestStats {
    pub fn backfill_source_dimensions(&self) {
        let guard = self.guard();
        let _ = self.with_conn_mut(&guard, |conn| {
            let tx = conn.transaction()?;
            backfill_sources(&tx)?;
            tx.commit()
        });
    }
}
