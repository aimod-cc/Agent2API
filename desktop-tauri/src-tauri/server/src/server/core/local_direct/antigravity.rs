use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use chrono::DateTime;
use serde::Serialize;

use crate::server::core::client::catalog;
use crate::server::request_stats::{
    ApiKeyIdentity, LocalDirectFileState, LocalDirectIngestItem, LocalDirectRef, NewRequestEntry,
    RequestSource,
};

use super::antigravity_account::{detect_root_email, resolve_account_identity};
use super::protobuf::{
    decode_workspace_uri, parse_gen_metadata, parse_step_metadata, strip_gemini_deployment_suffix,
};
use super::sqlite_reader::{file_fingerprint, open_readonly_with_snapshot};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredRoot {
    pub path: String,
    pub kind: String,
    pub kind_label: String,
    pub custom: bool,
    pub exists: bool,
}

#[derive(Clone, Debug, Default)]
struct ConversationSummaryMeta {
    title: String,
    workspace_uri: String,
}

struct ExtractedStep {
    step_index: i64,
    model_raw: String,
    model_norm: String,
    began_ms: Option<i64>,
    first_ms: Option<i64>,
    ended_ms: Option<i64>,
    uncached_input: i64,
    output: i64,
    cache_write: i64,
    cache_read: i64,
    response_id: String,
}

pub struct ScanBatch {
    pub items: Vec<LocalDirectIngestItem>,
    pub file_updates: Vec<LocalDirectFileState>,
}

fn kind_display_label(kind: &str) -> &'static str {
    match kind {
        "desktop" => "Antigravity 桌面版",
        "insiders" => "Antigravity Insiders",
        "cli" => "Antigravity CLI (agy)",
        "t3_acp" => "T3 Code ACP",
        "wsl" => "WSL 目录",
        _ => "自定义目录",
    }
}

fn user_home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

fn normalize_root_candidate(input: &Path) -> PathBuf {
    if let Some(name) = input.file_name().and_then(|s| s.to_str()) {
        if name.eq_ignore_ascii_case("conversations") || name.eq_ignore_ascii_case("brain") {
            if let Some(parent) = input.parent() {
                return parent.to_path_buf();
            }
        }
    }
    input.to_path_buf()
}

fn canonical_dedup_key(path: &Path) -> String {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = resolved.to_string_lossy().to_string();
    #[cfg(windows)]
    {
        text.trim_start_matches(r"\\?\")
            .trim_end_matches(['\\', '/'])
            .to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        text.trim_end_matches('/').to_string()
    }
}

/// 探测本机所有 Antigravity 数据目录，并合并用户自定义路径。
pub fn discover_roots(custom_paths: &[String]) -> Vec<DiscoveredRoot> {
    let mut candidates: Vec<(PathBuf, String, bool)> = Vec::new();
    if let Some(home) = user_home_dir() {
        let gemini = home.join(".gemini");
        candidates.push((gemini.join("antigravity"), "desktop".to_string(), false));
        candidates.push((gemini.join("antigravity-insiders"), "insiders".to_string(), false));
        candidates.push((gemini.join("antigravity-cli"), "cli".to_string(), false));

        let t3_home = std::env::var_os("T3CODE_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".t3"));
        for sub in ["userdata", "dev"] {
            let base = t3_home.join(sub).join("providers").join("antigravity");
            if let Ok(entries) = std::fs::read_dir(base) {
                for entry in entries.flatten() {
                    let acp = entry.path().join("antigravity-acp");
                    if acp.is_dir() {
                        candidates.push((acp, "t3_acp".to_string(), false));
                    }
                }
            }
        }
    }

    #[cfg(windows)]
    discover_windows_wsl_roots(&mut candidates);

    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for (path, kind, custom) in candidates {
        if !path.is_dir() {
            continue;
        }
        let key = canonical_dedup_key(&path);
        if seen.insert(key) {
            out.push(DiscoveredRoot {
                path: path.to_string_lossy().to_string(),
                kind_label: kind_display_label(&kind).to_string(),
                kind,
                custom,
                exists: true,
            });
        }
    }

    for raw in custom_paths {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let path = normalize_root_candidate(Path::new(trimmed));
        let exists = path.is_dir();
        let key = canonical_dedup_key(&path);
        if !seen.insert(key) {
            continue;
        }
        out.push(DiscoveredRoot {
            path: path.to_string_lossy().to_string(),
            kind_label: kind_display_label("custom").to_string(),
            kind: "custom".to_string(),
            custom: true,
            exists,
        });
    }

    out
}

#[cfg(windows)]
fn discover_windows_wsl_roots(candidates: &mut Vec<(PathBuf, String, bool)>) {
    // 仅在 WSL UNC 根实际可达时扫描常见的 WSL 发行版 home 目录，避免触发阻塞或卡顿。
    for unc_root in [r"\\wsl.localhost", r"\\wsl$"] {
        if let Ok(distros) = std::fs::read_dir(unc_root) {
            for distro in distros.flatten().take(8) {
                let home_dir = distro.path().join("home");
                if let Ok(users) = std::fs::read_dir(home_dir) {
                    for user in users.flatten().take(6) {
                        let gemini = user.path().join(".gemini");
                        for folder in ["antigravity", "antigravity-insiders", "antigravity-cli"] {
                            let target = gemini.join(folder);
                            if target.is_dir() {
                                candidates.push((target, "wsl".to_string(), false));
                            }
                        }
                    }
                }
            }
            break;
        }
    }
}

fn load_conversation_summaries(root: &Path) -> HashMap<String, ConversationSummaryMeta> {
    let summary_db = root.join("conversation_summaries.db");
    if !summary_db.is_file() {
        return HashMap::new();
    }
    open_readonly_with_snapshot(&summary_db, |conn| {
        let mut map = HashMap::new();
        let query_full = "SELECT conversation_id, title, preview, workspace_uris FROM conversation_summaries";
        let query_lite = "SELECT conversation_id, title, '' AS preview, workspace_uris FROM conversation_summaries";
        let mut stmt = match conn.prepare(query_full) {
            Ok(s) => s,
            Err(_) => conn.prepare(query_lite)?,
        };
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let conv_id: String = row.get::<_, Option<String>>(0)?.unwrap_or_default();
            if conv_id.trim().is_empty() {
                continue;
            }
            let title_raw: String = row.get::<_, Option<String>>(1)?.unwrap_or_default();
            let preview_raw: String = row.get::<_, Option<String>>(2)?.unwrap_or_default();
            let ws_raw: String = row.get::<_, Option<String>>(3)?.unwrap_or_default();
            let title = if !title_raw.trim().is_empty() {
                title_raw.trim().to_string()
            } else {
                preview_raw.trim().chars().take(80).collect()
            };
            map.insert(
                conv_id,
                ConversationSummaryMeta {
                    title,
                    workspace_uri: decode_workspace_uri(&ws_raw),
                },
            );
        }
        Ok(map)
    })
    .unwrap_or_default()
}

fn find_transcript_path(root: &Path, conv_id: &str) -> Option<PathBuf> {
    let logs = root.join("brain").join(conv_id).join(".system_generated").join("logs");
    let full = logs.join("transcript_full.jsonl");
    if full.is_file() {
        return Some(full);
    }
    let compact = logs.join("transcript.jsonl");
    if compact.is_file() {
        return Some(compact);
    }
    None
}

fn stable_request_id(source_key: &str) -> String {
    let bytes = source_key.as_bytes();
    let mut h1: u64 = 0xcbf29ce484222325;
    let mut h2: u64 = 0x84222325cbf29ce4;
    for (idx, &b) in bytes.iter().enumerate() {
        h1 ^= u64::from(b);
        h1 = h1.wrapping_mul(0x100000001b3);
        h2 ^= u64::from(b).wrapping_add((idx as u64).wrapping_mul(31));
        h2 = h2.wrapping_mul(0x9e3779b97f4a7c15).rotate_left(13);
    }
    format!("ld-ag-{h1:016x}{h2:016x}")
}

fn build_ingest_item(
    root: &DiscoveredRoot,
    conv_id: &str,
    summary: Option<&ConversationSummaryMeta>,
    db_path: Option<&Path>,
    transcript_path: Option<&Path>,
    detected_email: Option<&str>,
    account_names: &HashMap<String, String>,
    step: ExtractedStep,
) -> LocalDirectIngestItem {
    let now = crate::server::logging::now_ms();
    let ts = step
        .ended_ms
        .filter(|v| *v > 0)
        .or(step.began_ms.filter(|v| *v > 0))
        .unwrap_or(now);
    let duration_ms = match (step.began_ms, step.ended_ms) {
        (Some(b), Some(e)) if e >= b => e - b,
        _ => 0,
    };
    let first_response_ms = match (step.began_ms, step.first_ms) {
        (Some(b), Some(f)) if f >= b => Some(f - b),
        _ => None,
    };
    let prompt_tokens = step
        .uncached_input
        .saturating_add(step.cache_read)
        .saturating_add(step.cache_write)
        .max(0);
    let completion_tokens = step.output.max(0);
    let total_tokens = prompt_tokens.saturating_add(completion_tokens);

    let model = if step.model_norm.is_empty() {
        "antigravity".to_string()
    } else {
        step.model_norm
    };
    let client_model = if step.model_raw.is_empty() {
        model.clone()
    } else {
        step.model_raw
    };

    let source_key = if !step.response_id.is_empty() {
        format!("antigravity:{}:{}:r:{}", root.kind, conv_id, step.response_id)
    } else {
        format!("antigravity:{}:{}:s:{}", root.kind, conv_id, step.step_index)
    };
    let request_id = stable_request_id(&source_key);
    let (account_id, account_name) =
        resolve_account_identity(&root.kind, detected_email, account_names, ts, now);

    let mut entry = NewRequestEntry::new(model.clone(), 200);
    entry.id = request_id.clone();
    entry.ts = Some(ts);
    entry.duration_ms = duration_ms;
    entry.first_response_ms = first_response_ms;
    entry.provider = Some("antigravity".to_string());
    entry.account_id = account_id;
    entry.account_name = account_name;
    entry.client_model = client_model;
    entry.upstream_model = model;
    entry.prompt_tokens = prompt_tokens;
    entry.completion_tokens = completion_tokens;
    entry.total_tokens = total_tokens;
    entry.cache_read_tokens = step.cache_read.max(0);
    entry.cache_creation_tokens = step.cache_write.max(0);
    entry.origin = "local_direct".to_string();
    entry.source = RequestSource {
        client: "Antigravity".to_string(),
        api_key: ApiKeyIdentity {
            id: String::new(),
            name: String::new(),
            source: "local_direct".to_string(),
        },
        evidence: None,
        behavior_client: String::new(),
    };

    let reference = LocalDirectRef {
        request_id,
        agent_id: "antigravity".to_string(),
        root_kind: root.kind.clone(),
        source_key,
        conversation_id: conv_id.to_string(),
        conversation_title: summary.map(|s| s.title.clone()).unwrap_or_default(),
        workspace_uri: summary.map(|s| s.workspace_uri.clone()).unwrap_or_default(),
        db_path: db_path.map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
        transcript_path: transcript_path
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        step_index: step.step_index,
        response_id: step.response_id,
        ts,
        created_at: now,
    };

    LocalDirectIngestItem { entry, reference }
}

fn scan_conversation_db(
    root: &DiscoveredRoot,
    db_path: &Path,
    conv_id: &str,
    summary: Option<&ConversationSummaryMeta>,
    transcript_path: Option<&Path>,
    detected_email: Option<&str>,
    account_names: &HashMap<String, String>,
    prev_state: Option<&LocalDirectFileState>,
    force_rescan: bool,
) -> (Vec<LocalDirectIngestItem>, Option<LocalDirectFileState>, bool) {
    let Some((mtime_ms, size_bytes)) = file_fingerprint(db_path) else {
        return (Vec::new(), None, false);
    };
    let file_key = db_path.to_string_lossy().to_string();
    if !force_rescan {
        if let Some(prev) = prev_state {
            if prev.mtime_ms == mtime_ms && prev.size_bytes == size_bytes {
                return (Vec::new(), None, true);
            }
        }
    }

    let parsed = open_readonly_with_snapshot(db_path, |conn| {
        let mut gen_models: HashMap<i64, String> = HashMap::new();
        let mut step_to_gen: HashMap<i64, i64> = HashMap::new();

        if let Ok(mut stmt) = conn.prepare("SELECT idx, data FROM gen_metadata ORDER BY idx ASC") {
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let gen_idx: i64 = row.get(0)?;
                let data: Vec<u8> = row.get(1)?;
                let parsed = parse_gen_metadata(gen_idx, &data);
                if let Some(model) = parsed.model {
                    gen_models.insert(gen_idx, model);
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
        Ok((gen_models, step_to_gen, raw_steps))
    });

    let Ok((gen_models, step_to_gen, raw_steps)) = parsed else {
        return (Vec::new(), None, false);
    };

    let mut fallback_models: Vec<(i64, &String)> = gen_models.iter().map(|(k, v)| (*k, v)).collect();
    fallback_models.sort_by_key(|(k, _)| *k);

    let resolve_model = |gen_idx: i64| -> (String, String) {
        if let Some(raw) = gen_models.get(&gen_idx) {
            return (raw.clone(), strip_gemini_deployment_suffix(raw));
        }
        if let Some((_, raw)) = fallback_models.iter().rfind(|(idx, _)| *idx <= gen_idx) {
            return ((*raw).clone(), strip_gemini_deployment_suffix(raw));
        }
        if let Some((_, raw)) = fallback_models.last() {
            return ((*raw).clone(), strip_gemini_deployment_suffix(raw));
        }
        (String::new(), String::new())
    };

    // 按 response_id 合并同一次模型调用拆分出的多条 step；无 response_id 则保留原步。
    let mut collapsed: Vec<ExtractedStep> = Vec::new();
    let mut resp_positions: HashMap<String, usize> = HashMap::new();
    let mut saw_any_usage = false;

    for (step_idx, meta) in raw_steps {
        let has_usage = meta.uncached_input > 0
            || meta.output > 0
            || meta.cache_read > 0
            || meta.cache_write > 0
            || !meta.response_id.is_empty();
        if has_usage {
            saw_any_usage = true;
        } else {
            continue;
        }

        let gen_idx = step_to_gen
            .get(&step_idx)
            .copied()
            .or(meta.gen_idx)
            .unwrap_or(0);
        let (model_raw, model_norm) = resolve_model(gen_idx);

        if !meta.response_id.is_empty() {
            if let Some(&pos) = resp_positions.get(&meta.response_id) {
                let existing = &mut collapsed[pos];
                existing.began_ms = match (existing.began_ms, meta.began_ms) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
                existing.first_ms = match (existing.first_ms, meta.first_ms) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
                existing.ended_ms = match (existing.ended_ms, meta.ended_ms) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                };
                existing.uncached_input = existing.uncached_input.max(meta.uncached_input);
                existing.output = existing.output.max(meta.output);
                existing.cache_write = existing.cache_write.max(meta.cache_write);
                existing.cache_read = existing.cache_read.max(meta.cache_read);
                if existing.model_norm.is_empty() && !model_norm.is_empty() {
                    existing.model_raw = model_raw;
                    existing.model_norm = model_norm;
                }
                continue;
            }
            resp_positions.insert(meta.response_id.clone(), collapsed.len());
        }

        collapsed.push(ExtractedStep {
            step_index: step_idx,
            model_raw,
            model_norm,
            began_ms: meta.began_ms,
            first_ms: meta.first_ms,
            ended_ms: meta.ended_ms,
            uncached_input: meta.uncached_input,
            output: meta.output,
            cache_write: meta.cache_write,
            cache_read: meta.cache_read,
            response_id: meta.response_id,
        });
    }

    let last_done_watermark = prev_state.map(|s| s.last_step_idx).unwrap_or(-1);
    let mut new_last_step_idx = if force_rescan { -1 } else { last_done_watermark };
    let mut blocked_by_in_progress = false;
    let mut items = Vec::new();

    for step in collapsed {
        // 仍在生成中的记录暂不入库、也不推进完成水位，待下一轮补齐。
        let in_progress = step.ended_ms.is_none()
            && step.output == 0
            && step.uncached_input == 0
            && step.cache_read == 0;
        if in_progress {
            blocked_by_in_progress = true;
            continue;
        }
        if !force_rescan && last_done_watermark >= 0 && step.step_index <= last_done_watermark {
            continue;
        }
        if !blocked_by_in_progress && step.step_index > new_last_step_idx {
            new_last_step_idx = step.step_index;
        }
        items.push(build_ingest_item(
            root,
            conv_id,
            summary,
            Some(db_path),
            transcript_path,
            detected_email,
            account_names,
            step,
        ));
    }

    let file_state = if !blocked_by_in_progress || new_last_step_idx >= 0 {
        Some(LocalDirectFileState {
            file_path: file_key,
            agent_id: "antigravity".to_string(),
            mtime_ms,
            size_bytes,
            last_step_idx: new_last_step_idx,
            updated_at: crate::server::logging::now_ms(),
        })
    } else {
        None
    };

    (items, file_state, saw_any_usage)
}

fn parse_rfc3339_ms(raw: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(raw.trim())
        .ok()
        .map(|dt| dt.timestamp_millis())
}

fn scan_transcript_fallback(
    root: &DiscoveredRoot,
    conv_id: &str,
    summary: Option<&ConversationSummaryMeta>,
    db_path: Option<&Path>,
    transcript_path: &Path,
    detected_email: Option<&str>,
    account_names: &HashMap<String, String>,
    prev_state: Option<&LocalDirectFileState>,
    force_rescan: bool,
) -> (Vec<LocalDirectIngestItem>, Option<LocalDirectFileState>) {
    let Some((mtime_ms, size_bytes)) = file_fingerprint(transcript_path) else {
        return (Vec::new(), None);
    };
    let file_key = transcript_path.to_string_lossy().to_string();
    if !force_rescan {
        if let Some(prev) = prev_state {
            if prev.mtime_ms == mtime_ms && prev.size_bytes == size_bytes {
                return (Vec::new(), None);
            }
        }
    }

    let Ok(content) = std::fs::read_to_string(transcript_path) else {
        return (Vec::new(), None);
    };
    let last_done = if force_rescan {
        -1
    } else {
        prev_state.map(|s| s.last_step_idx).unwrap_or(-1)
    };

    // 若同会话 DB 里已有 gen_metadata 模型，顺手读出最新模型名给 agy JSONL 复用。
    let db_model = db_path
        .and_then(|p| {
            open_readonly_with_snapshot(p, |conn| {
                let mut stmt = conn.prepare("SELECT idx, data FROM gen_metadata ORDER BY idx DESC LIMIT 1")?;
                let mut rows = stmt.query([])?;
                if let Some(row) = rows.next()? {
                    let gen_idx: i64 = row.get(0)?;
                    let data: Vec<u8> = row.get(1)?;
                    return Ok(parse_gen_metadata(gen_idx, &data).model);
                }
                Ok(None)
            })
            .ok()
            .flatten()
        })
        .unwrap_or_default();
    let norm_db_model = strip_gemini_deployment_suffix(&db_model);

    let mut new_last = last_done;
    let mut items = Vec::new();
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
        let uncached = row
            .get("input_tokens")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
            .max(0);
        let cache_read = row
            .get("cache_read_tokens")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
            .max(0);
        let output = row
            .get("output_tokens")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
            .max(0);
        if uncached == 0 && cache_read == 0 && output == 0 {
            continue;
        }
        let step_index = row
            .get("step_index")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or((line_idx + 1) as i64);
        if last_done >= 0 && step_index <= last_done {
            continue;
        }
        if step_index > new_last {
            new_last = step_index;
        }
        let row_model = row
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);
        let (model_raw, model_norm) = match row_model {
            Some(m) => (m.clone(), strip_gemini_deployment_suffix(&m)),
            None => (db_model.clone(), norm_db_model.clone()),
        };
        let ts = row
            .get("created_at")
            .and_then(serde_json::Value::as_str)
            .and_then(parse_rfc3339_ms);

        items.push(build_ingest_item(
            root,
            conv_id,
            summary,
            db_path,
            Some(transcript_path),
            detected_email,
            account_names,
            ExtractedStep {
                step_index,
                model_raw,
                model_norm,
                began_ms: ts,
                first_ms: None,
                ended_ms: ts,
                uncached_input: uncached,
                output,
                cache_write: 0,
                cache_read,
                response_id: String::new(),
            },
        ));
    }

    let state = LocalDirectFileState {
        file_path: file_key,
        agent_id: "antigravity".to_string(),
        mtime_ms,
        size_bytes,
        last_step_idx: new_last,
        updated_at: crate::server::logging::now_ms(),
    };
    (items, Some(state))
}

/// 扫描指定发现根目录集合中的增量请求与文件水位（不带账号备注映射的快捷入口，供测试复用）。
pub fn scan_discovered_roots(
    roots: Vec<DiscoveredRoot>,
    file_states: &HashMap<String, LocalDirectFileState>,
    force_rescan: bool,
) -> ScanBatch {
    let empty_names = HashMap::new();
    scan_discovered_roots_with_accounts(roots, file_states, force_rescan, &empty_names)
}

/// 扫描指定发现根目录集合中的增量请求与文件水位，并结合本机账号与面板备注映射补全具体账号。
pub fn scan_discovered_roots_with_accounts(
    roots: Vec<DiscoveredRoot>,
    file_states: &HashMap<String, LocalDirectFileState>,
    force_rescan: bool,
    account_names: &HashMap<String, String>,
) -> ScanBatch {
    // 确保 Antigravity 客户端名已在内建目录注册（无副作用引用）。
    let _ = catalog::CLIENTS;
    let mut batch = ScanBatch {
        items: Vec::new(),
        file_updates: Vec::new(),
    };

    for root in roots.into_iter().filter(|r| r.exists) {
        let root_path = Path::new(&root.path);
        let summaries = load_conversation_summaries(root_path);
        let detected_email = detect_root_email(&root);
        let mut handled_convs = HashSet::new();

        let conv_dir = root_path.join("conversations");
        if let Ok(entries) = std::fs::read_dir(&conv_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("db") {
                    continue;
                }
                let Some(conv_id) = path.file_stem().and_then(|s| s.to_str()).map(ToString::to_string) else {
                    continue;
                };
                handled_convs.insert(conv_id.clone());
                let summary = summaries.get(&conv_id);
                let transcript = find_transcript_path(root_path, &conv_id);
                let prev = file_states.get(&path.to_string_lossy().to_string());

                let (mut items, file_update, saw_usage_in_db) = scan_conversation_db(
                    &root,
                    &path,
                    &conv_id,
                    summary,
                    transcript.as_deref(),
                    detected_email.as_deref(),
                    account_names,
                    prev,
                    force_rescan,
                );
                batch.items.append(&mut items);
                if let Some(update) = file_update {
                    batch.file_updates.push(update);
                }

                // 若该对话 `.db` 尚未写 steps.usage（典型如 agy CLI），尝试从 transcript_full.jsonl 回落采集。
                if !saw_usage_in_db {
                    if let Some(tp) = transcript.as_deref() {
                        let tp_prev = file_states.get(&tp.to_string_lossy().to_string());
                        let (mut t_items, t_update) = scan_transcript_fallback(
                            &root,
                            &conv_id,
                            summary,
                            Some(&path),
                            tp,
                            detected_email.as_deref(),
                            account_names,
                            tp_prev,
                            force_rescan,
                        );
                        batch.items.append(&mut t_items);
                        if let Some(update) = t_update {
                            batch.file_updates.push(update);
                        }
                    }
                }
            }
        }

        // 处理仅存在 brain/<conv_id>/.system_generated/logs/transcript*.jsonl 的对话目录。
        let brain_dir = root_path.join("brain");
        if let Ok(entries) = std::fs::read_dir(&brain_dir) {
            for entry in entries.flatten() {
                let conv_path = entry.path();
                if !conv_path.is_dir() {
                    continue;
                }
                let Some(conv_id) = conv_path.file_name().and_then(|s| s.to_str()).map(ToString::to_string) else {
                    continue;
                };
                if handled_convs.contains(&conv_id) {
                    continue;
                }
                let Some(tp) = find_transcript_path(root_path, &conv_id) else {
                    continue;
                };
                let summary = summaries.get(&conv_id);
                let tp_prev = file_states.get(&tp.to_string_lossy().to_string());
                let (mut t_items, t_update) = scan_transcript_fallback(
                    &root,
                    &conv_id,
                    summary,
                    None,
                    &tp,
                    detected_email.as_deref(),
                    account_names,
                    tp_prev,
                    force_rescan,
                );
                batch.items.append(&mut t_items);
                if let Some(update) = t_update {
                    batch.file_updates.push(update);
                }
            }
        }
    }

    batch
}

/// 扫描全部已发现 Antigravity 根目录（含自动发现目录与自定义目录）的增量请求与文件水位。
pub fn scan_all(
    custom_paths: &[String],
    file_states: &HashMap<String, LocalDirectFileState>,
    force_rescan: bool,
    account_names: &HashMap<String, String>,
) -> ScanBatch {
    let roots = discover_roots(custom_paths);
    scan_discovered_roots_with_accounts(roots, file_states, force_rescan, account_names)
}
