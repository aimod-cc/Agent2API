use serde::Serialize;
use serde_json::{json, Value};

use crate::server::request_stats::RequestStats;

use super::antigravity;
use super::settings::LocalDirectSettings;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalDirectAdapterMeta {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub default_enabled: bool,
}

const ADAPTERS: &[LocalDirectAdapterMeta] = &[LocalDirectAdapterMeta {
    id: "antigravity",
    name: "Antigravity",
    description: "自动读取 Antigravity 桌面版、Insiders、agy CLI、T3 Code 与 WSL 中的历史对话用量，支持按需实时预览对话内容。",
    default_enabled: true,
}];

const KNOWN_IDS: &[&str] = &["antigravity"];

pub fn known_agent_ids() -> &'static [&'static str] {
    KNOWN_IDS
}

pub fn adapters() -> &'static [LocalDirectAdapterMeta] {
    ADAPTERS
}

pub fn agent_cards_json(
    settings: &LocalDirectSettings,
    stats: &RequestStats,
) -> Vec<Value> {
    let db_stats = stats.local_direct_agent_stats();
    ADAPTERS
        .iter()
        .map(|meta| {
            let cfg = settings.agents.get(meta.id).cloned().unwrap_or_default();
            let roots = match meta.id {
                "antigravity" => antigravity::discover_roots(&cfg.custom_paths),
                _ => Vec::new(),
            };
            let detected = roots.iter().any(|r| !r.custom && r.exists);
            let row_stats = db_stats.get(meta.id).cloned().unwrap_or_default();
            json!({
                "id": meta.id,
                "name": meta.name,
                "description": meta.description,
                "enabled": cfg.enabled,
                "detected": detected,
                "detectedRoots": roots,
                "customPaths": cfg.custom_paths,
                "requests": row_stats.requests,
                "totalTokens": row_stats.total_tokens,
                "lastTs": row_stats.last_ts,
            })
        })
        .collect()
}
