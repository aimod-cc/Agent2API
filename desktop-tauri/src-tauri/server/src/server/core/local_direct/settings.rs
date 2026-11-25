use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::server::access;

/// `kv` 表中的本机直连采集配置键（已登记在 `db::schema::RESERVED_KV_KEYS`）。
pub const KV_KEY: &str = "localDirectAgents";

pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 30;
pub const MIN_POLL_INTERVAL_SECS: u64 = 10;
pub const MAX_POLL_INTERVAL_SECS: u64 = 3600;
const MAX_CUSTOM_PATHS_PER_AGENT: usize = 32;

fn default_true() -> bool {
    true
}

fn default_interval() -> u64 {
    DEFAULT_POLL_INTERVAL_SECS
}

fn default_agents() -> BTreeMap<String, LocalDirectAgentConfig> {
    let mut map = BTreeMap::new();
    map.insert(
        "antigravity".to_string(),
        LocalDirectAgentConfig {
            enabled: true,
            custom_paths: Vec::new(),
        },
    );
    map
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalDirectAgentConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub custom_paths: Vec<String>,
}

impl Default for LocalDirectAgentConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            custom_paths: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalDirectSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_interval")]
    pub poll_interval_secs: u64,
    #[serde(default = "default_agents")]
    pub agents: BTreeMap<String, LocalDirectAgentConfig>,
}

impl Default for LocalDirectSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_secs: DEFAULT_POLL_INTERVAL_SECS,
            agents: default_agents(),
        }
    }
}

impl LocalDirectSettings {
    pub fn agent_enabled(&self, agent_id: &str) -> bool {
        self.enabled
            && self
                .agents
                .get(agent_id)
                .map(|cfg| cfg.enabled)
                .unwrap_or(false)
    }

    pub fn custom_paths_for(&self, agent_id: &str) -> &[String] {
        self.agents
            .get(agent_id)
            .map(|cfg| cfg.custom_paths.as_slice())
            .unwrap_or(&[])
    }
}

fn normalize_custom_path(raw: &str) -> Result<Option<String>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().any(char::is_control) {
        return Err("自定义路径不能包含换行等控制字符".to_string());
    }
    let trimmed_sep = if trimmed.len() > 3 {
        trimmed.trim_end_matches(['/', '\\'])
    } else {
        trimmed
    };
    Ok(Some(trimmed_sep.to_string()))
}

fn dedup_paths(paths: Vec<String>) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for raw in paths {
        let Some(clean) = normalize_custom_path(&raw)? else {
            continue;
        };
        if out.len() >= MAX_CUSTOM_PATHS_PER_AGENT {
            return Err(format!("每个 Agent 最多支持 {MAX_CUSTOM_PATHS_PER_AGENT} 个自定义目录"));
        }
        #[cfg(windows)]
        let key = clean.to_ascii_lowercase();
        #[cfg(not(windows))]
        let key = clean.clone();
        if seen.insert(key) {
            out.push(clean);
        }
    }
    Ok(out)
}

pub fn normalize_settings(mut settings: LocalDirectSettings) -> Result<LocalDirectSettings, String> {
    settings.poll_interval_secs = settings
        .poll_interval_secs
        .clamp(MIN_POLL_INTERVAL_SECS, MAX_POLL_INTERVAL_SECS);

    let known_ids = super::registry::known_agent_ids();
    let mut cleaned = BTreeMap::new();
    for id in known_ids {
        let mut entry = settings.agents.remove(*id).unwrap_or_default();
        entry.custom_paths = dedup_paths(entry.custom_paths)?;
        cleaned.insert((*id).to_string(), entry);
    }
    settings.agents = cleaned;
    Ok(settings)
}

pub fn load() -> LocalDirectSettings {
    let Some(text) = access::kv_get(KV_KEY) else {
        return LocalDirectSettings::default();
    };
    let parsed: LocalDirectSettings = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(_) => return LocalDirectSettings::default(),
    };
    normalize_settings(parsed).unwrap_or_default()
}

pub fn save(settings: &LocalDirectSettings) -> Result<LocalDirectSettings, String> {
    let normalized = normalize_settings(settings.clone())?;
    let json = serde_json::to_string(&normalized)
        .map_err(|err| format!("本机直连配置序列化失败: {err}"))?;
    access::kv_put(KV_KEY, &json);
    Ok(normalized)
}

/// 将前端部分更新 JSON 合并进当前配置并持久化。
pub fn update_from_json(payload: &Value) -> Result<LocalDirectSettings, String> {
    let obj = payload
        .as_object()
        .ok_or_else(|| "配置内容必须是 JSON 对象".to_string())?;
    let mut current = load();
    if let Some(enabled) = obj.get("enabled").and_then(Value::as_bool) {
        current.enabled = enabled;
    }
    if let Some(secs) = obj.get("pollIntervalSecs").and_then(Value::as_u64) {
        current.poll_interval_secs = secs;
    }
    if let Some(agents) = obj.get("agents").and_then(Value::as_object) {
        for (id, raw_cfg) in agents {
            if !super::registry::known_agent_ids().contains(&id.as_str()) {
                return Err(format!("不支持的本机直连 Agent: {id}"));
            }
            let Some(cfg_obj) = raw_cfg.as_object() else {
                return Err(format!("Agent {id} 的配置必须是对象"));
            };
            let entry = current.agents.entry(id.clone()).or_default();
            if let Some(enabled) = cfg_obj.get("enabled").and_then(Value::as_bool) {
                entry.enabled = enabled;
            }
            if let Some(paths_val) = cfg_obj.get("customPaths") {
                let paths_arr = paths_val
                    .as_array()
                    .ok_or_else(|| format!("Agent {id} 的自定义路径必须是数组"))?;
                let mut next = Vec::with_capacity(paths_arr.len());
                for item in paths_arr {
                    let s = item
                        .as_str()
                        .ok_or_else(|| "自定义路径必须是字符串".to_string())?;
                    next.push(s.to_string());
                }
                entry.custom_paths = next;
            }
        }
    }
    save(&current)
}
