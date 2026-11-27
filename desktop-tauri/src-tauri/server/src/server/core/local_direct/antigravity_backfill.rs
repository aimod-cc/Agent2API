//! 为已入库的 Antigravity 本机直连历史记录补全思考等级（只读原文件、不重插已删除记录）。

use std::collections::HashMap;
use std::path::Path;

use crate::server::request_stats::{LocalDirectReasoningCandidate, RequestStats};

use super::antigravity::{split_transcript_model_and_reasoning, transcript_row_explicit_reasoning};
use super::protobuf::{parse_gen_metadata, parse_step_metadata};
use super::sqlite_reader::open_readonly_with_snapshot;

struct DbReasoningMap {
    by_response_id: HashMap<String, String>,
    by_step_index: HashMap<i64, String>,
}

fn load_db_reasonings(db_path: &Path) -> Option<DbReasoningMap> {
    if !db_path.is_file() {
        return None;
    }
    let parsed = open_readonly_with_snapshot(db_path, |conn| {
        let mut gen_reasonings: HashMap<i64, String> = HashMap::new();
        let mut step_to_gen: HashMap<i64, i64> = HashMap::new();

        if let Ok(mut stmt) = conn.prepare("SELECT idx, data FROM gen_metadata ORDER BY idx ASC") {
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let gen_idx: i64 = row.get(0)?;
                let data: Vec<u8> = row.get(1)?;
                let parsed = parse_gen_metadata(gen_idx, &data);
                if parsed.model.is_some() || parsed.reasoning.is_some() {
                    gen_reasonings.insert(gen_idx, parsed.reasoning.unwrap_or_default());
                }
                for (step_idx, g_idx) in parsed.step_indices {
                    step_to_gen.insert(step_idx, g_idx);
                }
            }
        }

        let mut raw_steps = Vec::new();
        if let Ok(mut stmt) = conn.prepare("SELECT idx, metadata FROM steps ORDER BY idx ASC") {
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let step_idx: i64 = row.get(0)?;
                let data: Vec<u8> = row.get(1)?;
                raw_steps.push((step_idx, parse_step_metadata(&data)));
            }
        }
        Ok((gen_reasonings, step_to_gen, raw_steps))
    })
    .ok()?;

    let (gen_reasonings, step_to_gen, raw_steps) = parsed;
    if gen_reasonings.is_empty() {
        return None;
    }

    let mut fallback_entries: Vec<(i64, &String)> =
        gen_reasonings.iter().map(|(k, v)| (*k, v)).collect();
    fallback_entries.sort_by_key(|(k, _)| *k);

    let resolve_reasoning = |gen_idx: i64| -> String {
        if let Some(level) = gen_reasonings.get(&gen_idx) {
            return level.clone();
        }
        if let Some((_, level)) = fallback_entries.iter().rfind(|(idx, _)| *idx <= gen_idx) {
            return (*level).clone();
        }
        if let Some((_, level)) = fallback_entries.last() {
            return (*level).clone();
        }
        String::new()
    };

    let mut by_response_id: HashMap<String, String> = HashMap::new();
    let mut by_step_index: HashMap<i64, String> = HashMap::new();

    for (step_idx, meta) in raw_steps {
        let gen_idx = step_to_gen
            .get(&step_idx)
            .copied()
            .or(meta.gen_idx)
            .unwrap_or(0);
        let level = resolve_reasoning(gen_idx);
        if level.is_empty() {
            continue;
        }
        by_step_index.entry(step_idx).or_insert_with(|| level.clone());
        if !meta.response_id.is_empty() {
            by_response_id
                .entry(meta.response_id)
                .or_insert(level);
        }
    }

    Some(DbReasoningMap {
        by_response_id,
        by_step_index,
    })
}

fn load_transcript_reasonings(transcript_path: &Path) -> HashMap<i64, String> {
    let mut out = HashMap::new();
    let Ok(content) = std::fs::read_to_string(transcript_path) else {
        return out;
    };
    for (line_idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(row) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        let kind = row.get("type").and_then(serde_json::Value::as_str).unwrap_or("");
        let source = row.get("source").and_then(serde_json::Value::as_str).unwrap_or("");
        if !kind.contains("PLANNER_RESPONSE") && source != "assistant" {
            continue;
        }
        let step_index = row
            .get("step_index")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or((line_idx + 1) as i64);
        let from_model = row
            .get("model")
            .and_then(serde_json::Value::as_str)
            .and_then(|raw| split_transcript_model_and_reasoning(raw).2);
        let level = from_model.or_else(|| transcript_row_explicit_reasoning(&row));
        if let Some(level) = level {
            out.entry(step_index).or_insert(level);
        }
    }
    out
}

/// 按原始对话 `.db` → `transcript*.jsonl` → 已存模型名后缀的顺序，批量解析历史直连记录的思考等级。
pub fn resolve_missing_reasonings(
    candidates: &[LocalDirectReasoningCandidate],
) -> Vec<(i64, String)> {
    if candidates.is_empty() {
        return Vec::new();
    }

    let mut resolved: Vec<Option<String>> = vec![None; candidates.len()];

    // 1. 按 db_path 分组，每个 `.db` 文件只做一次只读快照解析。
    let mut by_db: HashMap<&str, Vec<usize>> = HashMap::new();
    for (idx, item) in candidates.iter().enumerate() {
        let path = item.db_path.trim();
        if !path.is_empty() {
            by_db.entry(path).or_default().push(idx);
        }
    }
    for (db_path, indices) in by_db {
        let Some(map) = load_db_reasonings(Path::new(db_path)) else {
            continue;
        };
        for idx in indices {
            let item = &candidates[idx];
            let hit = (!item.response_id.trim().is_empty())
                .then(|| map.by_response_id.get(item.response_id.trim()))
                .flatten()
                .or_else(|| map.by_step_index.get(&item.step_index));
            if let Some(level) = hit.filter(|v| !v.is_empty()) {
                resolved[idx] = Some(level.clone());
            }
        }
    }

    // 2. 仍未命中的条目回落到对应 transcript*.jsonl（如 agy CLI 或无 steps.usage 的对话）。
    let mut by_transcript: HashMap<&str, Vec<usize>> = HashMap::new();
    for (idx, item) in candidates.iter().enumerate() {
        if resolved[idx].is_some() {
            continue;
        }
        let path = item.transcript_path.trim();
        if !path.is_empty() {
            by_transcript.entry(path).or_default().push(idx);
        }
    }
    for (transcript_path, indices) in by_transcript {
        let map = load_transcript_reasonings(Path::new(transcript_path));
        if map.is_empty() {
            continue;
        }
        for idx in indices {
            if let Some(level) = map.get(&candidates[idx].step_index) {
                resolved[idx] = Some(level.clone());
            }
        }
    }

    // 3. 本地原文件已被清理时，从已存模型名自身携带的 `(XHigh)` / `-high` 后缀兜底提取。
    let mut updates = Vec::new();
    for (idx, item) in candidates.iter().enumerate() {
        let level = resolved[idx]
            .take()
            .or_else(|| split_transcript_model_and_reasoning(&item.model).2);
        if let Some(level) = level.filter(|v| !v.is_empty()) {
            updates.push((item.row_id, level));
        }
    }
    updates
}

/// 为当前数据库中缺少思考等级的 Antigravity 历史本机直连请求做一次原地补全。
pub fn backfill_antigravity_reasoning(stats: &RequestStats) -> usize {
    let candidates = stats.list_antigravity_missing_reasoning();
    if candidates.is_empty() {
        return 0;
    }
    let updates = resolve_missing_reasonings(&candidates);
    if updates.is_empty() {
        return 0;
    }
    stats.apply_local_direct_reasoning_updates(&updates)
}
