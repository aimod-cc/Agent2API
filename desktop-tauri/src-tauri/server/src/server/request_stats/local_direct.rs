use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::aggregate::fold_into_daily;
use super::clock::{date_key, day_of};
use super::record::{DailyEntry, NewRequestEntry, RequestEntry, ORIGIN_GATEWAY, ORIGIN_LOCAL_DIRECT};
use super::{daily, sql, RequestStats};

/// 本机直连请求在数据库中的轻量定位索引：只存会话/步骤与本机路径指针，不存对话正文。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalDirectRef {
    pub request_id: String,
    pub agent_id: String,
    pub root_kind: String,
    pub source_key: String,
    pub conversation_id: String,
    pub conversation_title: String,
    pub workspace_uri: String,
    pub db_path: String,
    pub transcript_path: String,
    pub step_index: i64,
    pub response_id: String,
    pub ts: i64,
    pub created_at: i64,
}

/// 单个本地数据源文件的增量扫描水位。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalDirectFileState {
    pub file_path: String,
    pub agent_id: String,
    pub mtime_ms: i64,
    pub size_bytes: i64,
    pub last_step_idx: i64,
    pub updated_at: i64,
}

/// 准备写入请求库的一条本机直连记录及其索引信息。
#[derive(Clone, Debug)]
pub struct LocalDirectIngestItem {
    pub entry: NewRequestEntry,
    pub reference: LocalDirectRef,
}

/// 批量导入统计。
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalDirectIngestReport {
    pub inserted: usize,
    pub deduped_existing: usize,
    pub deduped_gateway: usize,
}

/// 每个本机直连 Agent 在库中的已导入统计摘要。
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalDirectAgentStats {
    pub requests: i64,
    pub total_tokens: i64,
    pub last_ts: Option<i64>,
}

/// 待补全思考等级的历史本机直连请求定位条目。
#[derive(Clone, Debug)]
pub struct LocalDirectReasoningCandidate {
    pub row_id: i64,
    pub db_path: String,
    pub transcript_path: String,
    pub step_index: i64,
    pub response_id: String,
    pub model: String,
}

const GATEWAY_DEDUP_WINDOW_MS: i64 = 180_000;
const ZERO_TOKEN_DEDUP_WINDOW_MS: i64 = 15_000;

struct GatewayCandidate {
    ts: i64,
    model: String,
    upstream_model: String,
    client_model: String,
    prompt_tokens: i64,
    completion_tokens: i64,
    cache_read_tokens: i64,
    cache_creation_tokens: i64,
}

pub(super) fn select_reprice_for_origin(
    conn: &Connection,
    origin: &str,
    from_ms: Option<i64>,
) -> rusqlite::Result<Vec<RequestEntry>> {
    let mut sql = format!(
        "SELECT {} FROM requests WHERE origin = ?1 AND is_test = 0 AND status <> 0",
        sql::REQUEST_REPRICE_COLUMNS
    );
    if from_ms.is_some() {
        sql.push_str(" AND ts >= ?2");
    }
    sql.push_str(" ORDER BY ts ASC, row_id ASC");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = match from_ms {
        Some(ts) => stmt.query(params![origin, ts])?,
        None => stmt.query(params![origin])?,
    };
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(sql::decode_request(row)?);
    }
    Ok(out)
}

pub(super) fn has_local_direct_requests(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM requests WHERE origin = ?1 AND is_test = 0 LIMIT 1)",
        params![ORIGIN_LOCAL_DIRECT],
        |row| row.get::<_, i64>(0),
    )
    .unwrap_or(0)
        != 0
}

pub(super) fn select_ref_by_request_id(
    conn: &Connection,
    id: &str,
) -> rusqlite::Result<Option<LocalDirectRef>> {
    let mut stmt = conn.prepare(
        "SELECT request_id, agent_id, root_kind, source_key, conversation_id, \
         conversation_title, workspace_uri, db_path, transcript_path, step_index, \
         response_id, ts, created_at \
         FROM local_direct_refs WHERE request_id = ?1 LIMIT 1",
    )?;
    let mut rows = stmt.query(params![id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    Ok(Some(decode_ref(row)?))
}

fn decode_ref(row: &rusqlite::Row<'_>) -> rusqlite::Result<LocalDirectRef> {
    Ok(LocalDirectRef {
        request_id: row.get(0)?,
        agent_id: row.get(1)?,
        root_kind: row.get(2)?,
        source_key: row.get(3)?,
        conversation_id: row.get(4)?,
        conversation_title: row.get(5)?,
        workspace_uri: row.get(6)?,
        db_path: row.get(7)?,
        transcript_path: row.get(8)?,
        step_index: row.get(9)?,
        response_id: row.get(10)?,
        ts: row.get(11)?,
        created_at: row.get(12)?,
    })
}

fn normalize_model_key(raw: &str) -> String {
    let lower = raw.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return String::new();
    }
    let after_slash = lower.rsplit('/').next().unwrap_or(&lower);
    let before_colon = after_slash.split(':').next().unwrap_or(after_slash);
    let stripped = before_colon
        .strip_suffix("-thinking")
        .unwrap_or(before_colon);
    if let Some(pos) = stripped.rfind('-') {
        let suffix = &stripped[pos + 1..];
        if (suffix.len() == 1 && suffix.chars().all(|ch| ch.is_ascii_lowercase()))
            || (suffix.len() == 8 && suffix.chars().all(|ch| ch.is_ascii_digit()))
        {
            return stripped[..pos].to_string();
        }
    }
    stripped.to_string()
}

fn models_compatible(entry: &RequestEntry, cand: &GatewayCandidate) -> bool {
    let local_names = [
        normalize_model_key(&entry.model),
        normalize_model_key(&entry.upstream_model),
    ];
    let gw_names = [
        normalize_model_key(&cand.model),
        normalize_model_key(&cand.upstream_model),
        normalize_model_key(&cand.client_model),
    ];
    for local in local_names.iter().filter(|name| !name.is_empty()) {
        for gw in gw_names.iter().filter(|name| !name.is_empty()) {
            if local == gw || local.contains(gw.as_str()) || gw.contains(local.as_str()) {
                return true;
            }
        }
    }
    local_names.iter().all(String::is_empty) || gw_names.iter().all(String::is_empty)
}

fn matches_existing_gateway(conn: &Connection, entry: &RequestEntry) -> rusqlite::Result<bool> {
    let window = if entry.prompt_tokens == 0 && entry.completion_tokens == 0 {
        ZERO_TOKEN_DEDUP_WINDOW_MS
    } else {
        GATEWAY_DEDUP_WINDOW_MS
    };
    let from_ts = entry.ts.saturating_sub(window);
    let to_ts = entry.ts.saturating_add(window);
    let mut stmt = conn.prepare(
        "SELECT ts, model, upstream_model, client_model, prompt_tokens, completion_tokens, \
         cache_read_tokens, cache_creation_tokens \
         FROM requests \
         WHERE origin = ?1 AND ts >= ?2 AND ts <= ?3 AND status <> 0 AND is_test = 0",
    )?;
    let mut rows = stmt.query(params![ORIGIN_GATEWAY, from_ts, to_ts])?;
    let local_uncached = (entry.prompt_tokens - entry.cache_read_tokens - entry.cache_creation_tokens).max(0);
    while let Some(row) = rows.next()? {
        let cand = GatewayCandidate {
            ts: row.get(0)?,
            model: row.get(1)?,
            upstream_model: row.get(2)?,
            client_model: row.get(3)?,
            prompt_tokens: row.get(4)?,
            completion_tokens: row.get(5)?,
            cache_read_tokens: row.get(6)?,
            cache_creation_tokens: row.get(7)?,
        };
        if !models_compatible(entry, &cand) {
            continue;
        }
        if entry.prompt_tokens == 0 && entry.completion_tokens == 0 {
            if cand.prompt_tokens == 0 && cand.completion_tokens == 0 {
                return Ok(true);
            }
            continue;
        }
        if entry.prompt_tokens == 0 && entry.completion_tokens > 0 {
            if (entry.ts - cand.ts).abs() <= 30_000
                && cand.completion_tokens.abs_diff(entry.completion_tokens) <= 4
            {
                return Ok(true);
            }
            continue;
        }
        if cand.completion_tokens.abs_diff(entry.completion_tokens) > 8 {
            continue;
        }
        let cand_uncached = (cand.prompt_tokens - cand.cache_read_tokens - cand.cache_creation_tokens).max(0);
        let prompt_matches = cand.prompt_tokens.abs_diff(entry.prompt_tokens) <= 32
            || cand_uncached.abs_diff(local_uncached) <= 32;
        if !prompt_matches {
            continue;
        }
        let cache_matches = cand.cache_read_tokens.abs_diff(entry.cache_read_tokens) <= 32
            || cand.cache_read_tokens == 0
            || entry.cache_read_tokens == 0;
        if cache_matches {
            return Ok(true);
        }
    }
    Ok(false)
}

impl RequestStats {
    /// 读取指定 Agent 的文件扫描水位表。
    pub fn local_direct_file_states(&self, agent_id: &str) -> HashMap<String, LocalDirectFileState> {
        let guard = self.guard();
        self.with_conn(&guard, |conn| {
            let mut stmt = conn.prepare(
                "SELECT file_path, agent_id, mtime_ms, size_bytes, last_step_idx, updated_at \
                 FROM local_direct_files WHERE agent_id = ?1",
            )?;
            let mut rows = stmt.query(params![agent_id])?;
            let mut out = HashMap::new();
            while let Some(row) = rows.next()? {
                let state = LocalDirectFileState {
                    file_path: row.get(0)?,
                    agent_id: row.get(1)?,
                    mtime_ms: row.get(2)?,
                    size_bytes: row.get(3)?,
                    last_step_idx: row.get(4)?,
                    updated_at: row.get(5)?,
                };
                out.insert(state.file_path.clone(), state);
            }
            Ok(out)
        })
        .unwrap_or_default()
    }

    /// 重置扫描水位（「重扫全部历史」时调用；不会删除已经存在的请求，通过 source_key 自动去重）。
    pub fn clear_local_direct_file_states(&self, agent_id: Option<&str>) {
        let guard = self.guard();
        let _ = self.with_conn_mut(&guard, |conn| {
            match agent_id.map(str::trim).filter(|id| !id.is_empty()) {
                Some(id) => conn.execute("DELETE FROM local_direct_files WHERE agent_id = ?1", params![id])?,
                None => conn.execute("DELETE FROM local_direct_files", [])?,
            };
            Ok(())
        });
    }

    /// 汇总每个已启用/已采集本机 Agent 的已入库条数与最近活动时间。
    pub fn local_direct_agent_stats(&self) -> HashMap<String, LocalDirectAgentStats> {
        let guard = self.guard();
        self.with_conn(&guard, |conn| {
            let mut stmt = conn.prepare(
                "SELECT l.agent_id, COUNT(*), COALESCE(SUM(r.total_tokens), 0), MAX(r.ts) \
                 FROM local_direct_refs l \
                 JOIN requests r ON r.id = l.request_id \
                 GROUP BY l.agent_id",
            )?;
            let mut rows = stmt.query([])?;
            let mut out = HashMap::new();
            while let Some(row) = rows.next()? {
                let agent_id: String = row.get(0)?;
                out.insert(
                    agent_id,
                    LocalDirectAgentStats {
                        requests: row.get(1)?,
                        total_tokens: row.get(2)?,
                        last_ts: row.get(3)?,
                    },
                );
            }
            Ok(out)
        })
        .unwrap_or_default()
    }

    /// 按 request rowId 或请求 id 读取本机直连索引，用于详情页按需读取本地对话文件。
    pub fn local_direct_ref_by_request(
        &self,
        row_id: Option<i64>,
        id: Option<&str>,
    ) -> Option<LocalDirectRef> {
        let guard = self.guard();
        self.with_conn(&guard, |conn| {
            if let Some(rid) = row_id.filter(|v| *v > 0) {
                let req_id: Option<String> = conn
                    .query_row("SELECT id FROM requests WHERE row_id = ?1 LIMIT 1", params![rid], |row| row.get(0))
                    .ok();
                if let Some(req_id) = req_id.filter(|v| !v.is_empty()) {
                    return select_ref_by_request_id(conn, &req_id);
                }
            }
            if let Some(req_id) = id.map(str::trim).filter(|v| !v.is_empty()) {
                return select_ref_by_request_id(conn, req_id);
            }
            Ok(None)
        })
        .flatten()
    }

    /// 查询缺少思考等级的历史 Antigravity 本机直连请求，供后台原地补全使用。
    pub fn list_antigravity_missing_reasoning(&self) -> Vec<LocalDirectReasoningCandidate> {
        let guard = self.guard();
        self.with_conn(&guard, |conn| {
            let mut stmt = conn.prepare(
                "SELECT r.row_id, l.db_path, l.transcript_path, l.step_index, l.response_id, r.model \
                 FROM local_direct_refs l \
                 JOIN requests r ON r.id = l.request_id \
                 WHERE l.agent_id = 'antigravity' \
                   AND r.origin = ?1 \
                   AND r.client_reasoning = '' \
                   AND r.upstream_reasoning = '' \
                 ORDER BY r.row_id ASC",
            )?;
            let mut rows = stmt.query(params![ORIGIN_LOCAL_DIRECT])?;
            let mut out = Vec::new();
            while let Some(row) = rows.next()? {
                out.push(LocalDirectReasoningCandidate {
                    row_id: row.get(0)?,
                    db_path: row.get(1)?,
                    transcript_path: row.get(2)?,
                    step_index: row.get(3)?,
                    response_id: row.get(4)?,
                    model: row.get(5)?,
                });
            }
            Ok(out)
        })
        .unwrap_or_default()
    }

    /// 按主键 `row_id` 升序分批原地回填历史本机直连请求的思考等级（单批 500 行、批次间释放锁，避免长时间占用数据库）。
    pub fn apply_local_direct_reasoning_updates(&self, updates: &[(i64, String)]) -> usize {
        if updates.is_empty() {
            return 0;
        }
        const BATCH_SIZE: usize = 500;
        let mut sorted: Vec<(i64, &str)> = updates
            .iter()
            .filter_map(|(row_id, reasoning)| {
                let level = reasoning.trim();
                (*row_id > 0 && !level.is_empty()).then_some((*row_id, level))
            })
            .collect();
        if sorted.is_empty() {
            return 0;
        }
        sorted.sort_unstable_by_key(|(row_id, _)| *row_id);

        let mut total_updated = 0usize;
        for chunk in sorted.chunks(BATCH_SIZE) {
            let guard = self.guard();
            let batch_changed = self
                .with_conn_mut(&guard, |conn| {
                    let tx = conn.transaction()?;
                    let mut changed = 0usize;
                    {
                        let mut stmt = tx.prepare(
                            "UPDATE requests \
                             SET client_reasoning = CASE WHEN client_reasoning = '' THEN ?1 ELSE client_reasoning END, \
                                 upstream_reasoning = CASE WHEN upstream_reasoning = '' THEN ?1 ELSE upstream_reasoning END \
                             WHERE row_id = ?2 AND origin = ?3 \
                               AND (client_reasoning = '' OR upstream_reasoning = '')",
                        )?;
                        for &(row_id, level) in chunk {
                            changed += stmt.execute(params![level, row_id, ORIGIN_LOCAL_DIRECT])?;
                        }
                    }
                    tx.commit()?;
                    Ok(changed)
                })
                .unwrap_or(0);
            total_updated += batch_changed;
        }

        if total_updated > 0 {
            self.filters_cache.invalidate();
            crate::server::core::panel_events::notify_requests();
        }
        total_updated
    }

    /// 单事务批量写入本机直连请求、指针引用、日报聚合与扫描水位，自动双重去重。
    pub fn ingest_local_direct(
        &self,
        items: Vec<LocalDirectIngestItem>,
        file_updates: &[LocalDirectFileState],
    ) -> Option<LocalDirectIngestReport> {
        if items.is_empty() && file_updates.is_empty() {
            return Some(LocalDirectIngestReport::default());
        }
        // 在拿数据库锁前完成条目归一化与计价，避免与其它锁嵌套。
        let normalized: Vec<(RequestEntry, LocalDirectRef)> = items
            .into_iter()
            .map(|mut item| {
                item.entry.origin = ORIGIN_LOCAL_DIRECT.to_string();
                let mut entry = item.entry.normalize();
                entry.origin = ORIGIN_LOCAL_DIRECT.to_string();
                (entry, item.reference)
            })
            .collect();

        let outcome = {
            let guard = self.guard();
            self.with_conn_mut(&guard, |conn| {
                let tx = conn.transaction()?;
                let mut report = LocalDirectIngestReport::default();
                let mut updated_existing = 0usize;
                let mut seen_keys: HashSet<String> = HashSet::with_capacity(normalized.len());
                let mut dirty_days: BTreeMap<String, DailyEntry> = BTreeMap::new();

                for (entry, reference) in &normalized {
                    if reference.source_key.is_empty()
                        || entry.id.is_empty()
                        || !seen_keys.insert(reference.source_key.clone())
                    {
                        report.deduped_existing += 1;
                        continue;
                    }
                    let existing_request_id: Option<String> = {
                        let mut stmt = tx.prepare_cached(
                            "SELECT request_id FROM local_direct_refs WHERE source_key = ?1 OR request_id = ?2 LIMIT 1",
                        )?;
                        let mut rows = stmt.query(params![reference.source_key, entry.id])?;
                        match rows.next()? {
                            Some(row) => Some(row.get(0)?),
                            None => None,
                        }
                    };
                    if let Some(existing_id) = existing_request_id {
                        if !entry.client_reasoning.is_empty() || !entry.upstream_reasoning.is_empty() {
                            let mut upd = tx.prepare_cached(
                                "UPDATE requests \
                                 SET client_reasoning = CASE WHEN client_reasoning = '' THEN ?1 ELSE client_reasoning END, \
                                     upstream_reasoning = CASE WHEN upstream_reasoning = '' THEN ?2 ELSE upstream_reasoning END \
                                 WHERE id = ?3 AND origin = ?4 \
                                   AND (client_reasoning = '' OR upstream_reasoning = '')",
                            )?;
                            updated_existing += upd.execute(params![
                                entry.client_reasoning,
                                entry.upstream_reasoning,
                                existing_id,
                                ORIGIN_LOCAL_DIRECT,
                            ])?;
                        }
                        report.deduped_existing += 1;
                        continue;
                    }
                    if matches_existing_gateway(&tx, entry)? {
                        report.deduped_gateway += 1;
                        continue;
                    }

                    sql::insert_request(&tx, entry)?;
                    tx.execute(
                        "INSERT INTO local_direct_refs \
                         (request_id, agent_id, root_kind, source_key, conversation_id, \
                          conversation_title, workspace_uri, db_path, transcript_path, \
                          step_index, response_id, ts, created_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                        params![
                            reference.request_id,
                            reference.agent_id,
                            reference.root_kind,
                            reference.source_key,
                            reference.conversation_id,
                            reference.conversation_title,
                            reference.workspace_uri,
                            reference.db_path,
                            reference.transcript_path,
                            reference.step_index,
                            reference.response_id,
                            reference.ts,
                            reference.created_at,
                        ],
                    )?;

                    if !entry.is_test && entry.status != 0 {
                        let key = date_key(day_of(entry.ts));
                        let day = match dirty_days.get_mut(&key) {
                            Some(existing) => existing,
                            None => {
                                let loaded = daily::select_daily_row(&tx, &key)?
                                    .unwrap_or_else(|| DailyEntry::new(key.clone()));
                                dirty_days.entry(key).or_insert(loaded)
                            }
                        };
                        fold_into_daily(day, entry);
                    }
                    report.inserted += 1;
                }

                for day in dirty_days.values() {
                    daily::upsert_daily(&tx, day)?;
                }

                for file in file_updates {
                    tx.execute(
                        "INSERT INTO local_direct_files \
                         (file_path, agent_id, mtime_ms, size_bytes, last_step_idx, updated_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                         ON CONFLICT(file_path) DO UPDATE SET \
                           agent_id = excluded.agent_id, \
                           mtime_ms = excluded.mtime_ms, \
                           size_bytes = excluded.size_bytes, \
                           last_step_idx = MAX(local_direct_files.last_step_idx, excluded.last_step_idx), \
                           updated_at = excluded.updated_at",
                        params![
                            file.file_path,
                            file.agent_id,
                            file.mtime_ms,
                            file.size_bytes,
                            file.last_step_idx,
                            file.updated_at,
                        ],
                    )?;
                }

                tx.commit()?;
                Ok((report, updated_existing))
            })
        };

        if let Some((ref res, updated_existing)) = outcome {
            if res.inserted > 0 || updated_existing > 0 {
                self.filters_cache.invalidate();
                crate::server::core::panel_events::notify_requests();
            }
        }
        outcome.map(|(report, _)| report)
    }
}
