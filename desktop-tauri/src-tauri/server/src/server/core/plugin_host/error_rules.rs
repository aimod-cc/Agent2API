//! 解析并缓存插件在握手阶段申报的「错误识别映射」与「默认错误处理例外」。
//!
//! 只消费受信任的插件握手 JSON，并对条数、状态码、业务码与冷却边界做严格裁剪；
//! 单个插件声明格式有误时仅跳过无效条目，不影响插件正常装载。

use std::collections::{BTreeMap, HashSet};

use serde::Deserialize;
use serde_json::Value;

use crate::server::config::error_policy::{
    CooldownMode, CooldownScope, DeclaredErrorPolicy, ErrorAction, ErrorKind, ErrorRule,
    PluginErrorClassification, PluginErrorRule,
};

const MAX_RULES_PER_PROVIDER: usize = 32;
const MAX_CLASSIFICATIONS_PER_PROVIDER: usize = 64;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawClassification {
    kind: ErrorKind,
    #[serde(default)]
    statuses: Vec<u16>,
    #[serde(default)]
    codes: Vec<String>,
    #[serde(default)]
    scope: Option<CooldownScope>,
    #[serde(default, alias = "hard_stop")]
    hard_stop: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRule {
    id: String,
    name: String,
    #[serde(default)]
    reason: String,
    kind: ErrorKind,
    #[serde(default)]
    statuses: Vec<u16>,
    #[serde(default)]
    code: String,
    #[serde(default)]
    model: String,
    #[serde(default = "yes")]
    enabled: bool,
    action: ErrorAction,
    cooldown: CooldownMode,
    #[serde(default = "default_fallback", alias = "fallback_seconds")]
    fallback_seconds: u64,
    #[serde(default)]
    exponential: bool,
    #[serde(default = "default_cap", alias = "fallback_cap_seconds")]
    fallback_cap_seconds: u64,
    #[serde(default = "default_scope")]
    scope: CooldownScope,
}

fn yes() -> bool { true }
fn default_fallback() -> u64 { 60 }
fn default_cap() -> u64 { 1800 }
fn default_scope() -> CooldownScope { CooldownScope::Auto }

fn clean_text(raw: &str, max_chars: usize) -> String {
    raw.trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .take(max_chars)
        .collect()
}

fn canonical_id(provider: &str, local_id: &str) -> Option<String> {
    let local: String = local_id
        .trim()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        .take(64)
        .collect();
    if local.is_empty() {
        return None;
    }
    let full = format!("plugin:{provider}:{local}");
    (full.len() <= 128).then_some(full)
}

fn parse_provider(provider: &str, item: &Value) -> DeclaredErrorPolicy {
    let mut out = DeclaredErrorPolicy::default();
    if let Some(items) = item
        .get("error_classifications")
        .or_else(|| item.get("errorClassifications"))
        .and_then(Value::as_array)
    {
        for raw in items.iter().take(MAX_CLASSIFICATIONS_PER_PROVIDER) {
            let Ok(entry) = serde_json::from_value::<RawClassification>(raw.clone()) else { continue };
            let statuses: Vec<u16> = entry.statuses.into_iter().filter(|s| (400..600).contains(s)).take(32).collect();
            let codes: Vec<String> = entry
                .codes
                .into_iter()
                .map(|c| clean_text(&c, 128))
                .filter(|c| !c.is_empty())
                .take(64)
                .collect();
            if statuses.is_empty() && codes.is_empty() {
                continue;
            }
            out.classifications.push(PluginErrorClassification {
                kind: entry.kind,
                statuses,
                codes,
                scope: entry.scope,
                hard_stop: entry.hard_stop,
            });
        }
    }
    if let Some(items) = item
        .get("error_rules")
        .or_else(|| item.get("errorRules"))
        .and_then(Value::as_array)
    {
        let mut seen = HashSet::new();
        for raw in items.iter().take(MAX_RULES_PER_PROVIDER) {
            let Ok(entry) = serde_json::from_value::<RawRule>(raw.clone()) else { continue };
            let local_id = clean_text(&entry.id, 64);
            let Some(full_id) = canonical_id(provider, &local_id) else { continue };
            if !seen.insert(full_id.clone()) {
                continue;
            }
            let name = clean_text(&entry.name, 100);
            if name.is_empty() {
                continue;
            }
            let action = if entry.kind != ErrorKind::Auth && entry.action == ErrorAction::RefreshThenSwitch {
                ErrorAction::SwitchAccount
            } else {
                entry.action
            };
            let fallback_seconds = entry.fallback_seconds.clamp(1, 86_400);
            let fallback_cap_seconds = entry.fallback_cap_seconds.clamp(fallback_seconds, 86_400);
            let statuses: Vec<u16> = entry.statuses.into_iter().filter(|s| (400..600).contains(s)).take(50).collect();
            let code = clean_text(&entry.code, 256);
            let model = clean_text(&entry.model, 256);
            let reason = clean_text(&entry.reason, 240);
            out.rules.push(PluginErrorRule {
                rule: ErrorRule {
                    id: full_id,
                    name,
                    kind: entry.kind,
                    statuses,
                    code,
                    provider: provider.to_string(),
                    model,
                    enabled: entry.enabled,
                    action,
                    cooldown: entry.cooldown,
                    fallback_seconds,
                    exponential: entry.exponential,
                    fallback_cap_seconds,
                    scope: entry.scope,
                },
                plugin_rule_id: local_id,
                reason,
            });
        }
    }
    out
}

pub(super) fn parse(value: &Value) -> BTreeMap<String, DeclaredErrorPolicy> {
    let mut map = BTreeMap::new();
    let Some(items) = value.as_array() else { return map };
    for item in items {
        let Some(id) = item["id"].as_str().map(str::trim).filter(|id| !id.is_empty() && id.len() <= 128) else {
            continue;
        };
        let declared = parse_provider(id, item);
        if !declared.rules.is_empty() || !declared.classifications.is_empty() {
            map.insert(id.to_string(), declared);
        }
    }
    map
}

pub(crate) fn for_provider(provider: &str) -> DeclaredErrorPolicy {
    let Some(host) = super::host_for_provider(provider) else {
        return DeclaredErrorPolicy::default();
    };
    host.error_rules
        .lock()
        .ok()
        .and_then(|map| map.get(provider).cloned())
        .unwrap_or_default()
}

pub(crate) fn all_provider_rules() -> Vec<PluginErrorRule> {
    let mut out = Vec::new();
    for id in super::installed_ids() {
        let policy = for_provider(&id);
        out.extend(policy.rules);
    }
    out
}
