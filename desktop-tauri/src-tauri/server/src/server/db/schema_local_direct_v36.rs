//! Schema v36：把历史「本机直连采集」的 Antigravity 记录提供商归正为 `antigravity`。
//!
//! ── 为什么需要这次迁移 ────────────────────────────────────────
//! 早期采集器将所有 Antigravity 本机直连请求的 `provider` 写为占位值 `"local-direct"`、
//! `accountName` 写为 `"本机直连"`；列表里已经通过 `origin = 'local_direct'` 与「直连」徽章
//! 标识渠道身份，再在提供商列写「本机直连」会造成多列文字重复，也导致报表与筛选把
//! 同一个 Antigravity 的直连/网关用量割裂开。
//!
//! 本迁移幂等且不破坏 `origin = 'local_direct'` 标记：
//!   1. `requests` 表里 `origin = 'local_direct' AND provider = 'local-direct'` 的
//!      Antigravity 记录改写为 `provider = 'antigravity'`，并根据 `local_direct_refs.root_kind`
//!      将占位账号名 `"本机直连"` 收敛为更清晰的目录来源（如「桌面版」「CLI (agy)」）。
//!   2. `request_daily.provider_stats` JSON 数组里的 `"local-direct"` 项合并进 `"antigravity"`。

use rusqlite::{params, Connection};
use serde_json::{Map, Value};

fn fallback_label_for_kind(kind: &str) -> &'static str {
    match kind.trim() {
        "desktop" => "桌面版",
        "insiders" => "Insiders",
        "cli" => "CLI (agy)",
        "t3_acp" => "T3 Code ACP",
        "wsl" => "WSL 目录",
        _ => "本机直连",
    }
}

fn as_i64(map: &Map<String, Value>, key: &str) -> i64 {
    map.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn as_f64(map: &Map<String, Value>, key: &str) -> f64 {
    map.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

fn merge_provider_stats_json(raw: &str) -> Option<String> {
    let mut items: Vec<Value> = serde_json::from_str(raw).ok()?;
    let mut ag_idx: Option<usize> = None;
    let mut ld_indices: Vec<usize> = Vec::new();

    for (idx, item) in items.iter().enumerate() {
        let Some(provider) = item.get("provider").and_then(Value::as_str) else {
            continue;
        };
        if provider == "antigravity" && ag_idx.is_none() {
            ag_idx = Some(idx);
        } else if provider == "local-direct" {
            ld_indices.push(idx);
        }
    }

    if ld_indices.is_empty() {
        return None;
    }

    if ag_idx.is_none() {
        let first = ld_indices.remove(0);
        if let Some(obj) = items[first].as_object_mut() {
            obj.insert("provider".to_string(), Value::String("antigravity".to_string()));
        }
        ag_idx = Some(first);
    }

    let target_idx = ag_idx?;
    let mut extra_requests = 0i64;
    let mut extra_successful = 0i64;
    let mut extra_tokens = 0i64;
    let mut extra_cost = 0.0f64;
    let mut extra_unpriced = 0i64;

    for idx in &ld_indices {
        if let Some(obj) = items[*idx].as_object() {
            extra_requests += as_i64(obj, "requests");
            extra_successful += as_i64(obj, "successful");
            extra_tokens += as_i64(obj, "tokens");
            extra_cost += as_f64(obj, "costUsd");
            extra_unpriced += as_i64(obj, "unpricedRequests");
        }
    }

    if let Some(target) = items[target_idx].as_object_mut() {
        let next_req = as_i64(target, "requests") + extra_requests;
        let next_ok = as_i64(target, "successful") + extra_successful;
        let next_tok = as_i64(target, "tokens") + extra_tokens;
        let next_cost = as_f64(target, "costUsd") + extra_cost;
        let next_unpriced = as_i64(target, "unpricedRequests") + extra_unpriced;
        target.insert("requests".to_string(), Value::from(next_req));
        target.insert("successful".to_string(), Value::from(next_ok));
        target.insert("tokens".to_string(), Value::from(next_tok));
        target.insert("costUsd".to_string(), Value::from(next_cost));
        target.insert("unpricedRequests".to_string(), Value::from(next_unpriced));
    }

    for idx in ld_indices.into_iter().rev() {
        items.remove(idx);
    }

    serde_json::to_string(&items).ok()
}

pub(super) fn migrate_local_direct_antigravity(conn: &Connection) -> rusqlite::Result<()> {
    // 1. 关联 local_direct_refs 的 Antigravity 明细行：更新 provider 与占位账号名。
    let mut ref_stmt = conn.prepare(
        "SELECT r.row_id, l.root_kind, r.account_id, r.account_name \
         FROM requests r \
         JOIN local_direct_refs l ON l.request_id = r.id \
         WHERE r.origin = 'local_direct' \
           AND r.provider = 'local-direct' \
           AND l.agent_id = 'antigravity'",
    )?;
    let mut updates: Vec<(i64, String, String)> = Vec::new();
    let mut rows = ref_stmt.query([])?;
    while let Some(row) = rows.next()? {
        let row_id: i64 = row.get(0)?;
        let root_kind: String = row.get::<_, Option<String>>(1)?.unwrap_or_default();
        let account_id: String = row.get::<_, Option<String>>(2)?.unwrap_or_default();
        let account_name: String = row.get::<_, Option<String>>(3)?.unwrap_or_default();

        let kind_slug = if root_kind.trim().is_empty() {
            "unknown"
        } else {
            root_kind.trim()
        };
        let next_account_id = if account_id == "local-direct" || account_id.is_empty() {
            format!("local-direct:antigravity:{kind_slug}")
        } else {
            account_id
        };
        let next_account_name = if account_name.trim().is_empty() || account_name == "本机直连" {
            fallback_label_for_kind(&root_kind).to_string()
        } else {
            account_name
        };
        updates.push((row_id, next_account_id, next_account_name));
    }
    drop(rows);
    drop(ref_stmt);

    if !updates.is_empty() {
        let mut upd = conn.prepare(
            "UPDATE requests SET provider = 'antigravity', account_id = ?1, account_name = ?2 \
             WHERE row_id = ?3",
        )?;
        for (row_id, account_id, account_name) in updates {
            upd.execute(params![account_id, account_name, row_id])?;
        }
    }

    // 兜底：存在没有对应 ref 的旧本地直连行（例如仅保留明细时）也一并归正 provider。
    conn.execute(
        "UPDATE requests SET provider = 'antigravity' \
         WHERE origin = 'local_direct' AND provider = 'local-direct' AND client = 'Antigravity'",
        [],
    )?;

    // 2. 合并 request_daily.provider_stats 中的 local-direct 到 antigravity。
    let mut daily_stmt = conn.prepare(
        "SELECT date, provider_stats FROM request_daily \
         WHERE provider_stats LIKE '%local-direct%'",
    )?;
    let mut daily_updates: Vec<(String, String)> = Vec::new();
    let mut daily_rows = daily_stmt.query([])?;
    while let Some(row) = daily_rows.next()? {
        let date: String = row.get(0)?;
        let raw_stats: String = row.get(1)?;
        if let Some(merged) = merge_provider_stats_json(&raw_stats) {
            daily_updates.push((date, merged));
        }
    }
    drop(daily_rows);
    drop(daily_stmt);

    if !daily_updates.is_empty() {
        let mut upd = conn.prepare("UPDATE request_daily SET provider_stats = ?1 WHERE date = ?2")?;
        for (date, merged) in daily_updates {
            upd.execute(params![merged, date])?;
        }
    }

    Ok(())
}
