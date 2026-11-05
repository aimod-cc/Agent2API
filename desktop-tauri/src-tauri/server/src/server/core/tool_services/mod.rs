//! 工具服务（「工具服务」页）：网络搜索与视觉分析的配置存储与服务清单。
//!
//! ── 这是什么 ────────────────────────────────────────────────
//! 在「Agent 配置」（本机客户端接管）与「网络代理」之间新增的一层：对外提供
//! 两种可选能力，客户端可以按两种可管理协议接入 ——
//!   · Skill：打包成 Agent Skill，由面板一次性安装到本机各客户端的 skills 目录；
//!   · MCP：Streamable HTTP 远程服务，在客户端 MCP 配置里填地址即可。
//! 两种协议共享同一份上游配置，开关与执行超时按「服务 × 协议」各自独立。
//!
//! 网络搜索另有一个固定的 REST 接口（`/api/web-search/*`）：它是接口而不是
//! 可管理协议，不进服务清单、不单独开关，说明写在「文档」页，运行时见
//! `api::tool_runtime`。
//!
//! ── 配置存在哪 ──────────────────────────────────────────────
//! 整份配置存在网关配置的 `toolServices` 键下（kv 表一行），形状见
//! [`ToolConfig`]。为什么不单开表：这是低频读写的设置，跟着 `config` 走天然
//! 享受「整库备份 / 云同步 / 手改配置」三条既有通道，也不必为一张表加 schema
//! 版本。写入口只有 [`save`]，内部走 `config::update_raw_field` 的串行读改写。
//!
//! ── 密钥 ────────────────────────────────────────────────────
//! 四家搜索提供商的 API Key 与账号凭证同级：落盘前用 `crypto`（AES-256-GCM，
//! `enc1:` 信封）加密，读取路径只在真正发请求时解密。管理 API 只回
//! `configured` 与 `key_hint`，不回明文。
//!
//! ── 视觉分析的模型选择 ──────────────────────────────────────
//! 配置的是「提供商 + 上游模型 + 思考等级 + 内部调用 Key」。运行时用内部 Key
//! 打本进程网关的 `/v1/chat/completions`，协议转换、重试、故障转移与请求日志
//! 全部复用网关管线（与 OmniProxy 的视觉服务同一做法）。

mod account_search;
mod bridge;
mod install;
mod mcp;
mod normalize;
mod providers;
mod search;
mod skill_package;
mod vision;

pub use bridge::VISION_BRIDGE_SCRIPT;
pub use install::{
    install_mcp, install_skill, mcp_targets, skill_targets, uninstall_mcp, uninstall_skill,
    InstallOutcome,
};
pub use mcp::{dispatch as mcp_dispatch, McpRequestContext, McpServerKind, McpSpec};
pub use account_search::test_provider;
pub use search::{batch_items, run_fetch, run_search, SearchError, SearchErrorCode};
pub use skill_package::{
    build_skill_files, is_skill_variant, recommended_skill_variant, skill_variants, SkillFile,
    SkillVariant, WEB_SEARCH_SKILL_NAME, WEB_SEARCH_SKILL_VERSION,
};
pub use vision::{
    clear_vision_config, is_reasoning_level, run_vision, save_vision_config, test_vision,
    vision_config, vision_key_options, VisionConfig, VISION_MAX_IMAGES, VISION_MAX_PROMPT_LENGTH,
    VISION_SERVICE_NAME, VISION_SERVICE_VERSION,
};

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::server::config;
use crate::server::core::account_store::AccountStore;
use crate::server::core::crypto::{self, SecretScope};

/// 配置在网关配置里的键名（kv 表一行）
pub const KEY_TOOL_SERVICES: &str = "toolServices";

/// 固定服务 id（顺序即界面展示顺序）
pub const SERVICE_IDS: [&str; 2] = [WEB_SEARCH_SERVICE_ID, VISION_SERVICE_ID];
pub const WEB_SEARCH_SERVICE_ID: &str = "web-search";
pub const VISION_SERVICE_ID: &str = "vision";

/// 接入协议（顺序即界面展示顺序）。
///
/// API 不在这里：它是固定的对外工具接口（`/api/web-search/*`），不参与
/// 「工具服务」页的协议开关，只在「文档」页说明；运行时仍由
/// [`endpoint`] 与 `tool_runtime` 提供。
pub const PROTOCOLS: [&str; 2] = ["skill", "mcp"];

/// 密钥型来源同时支持搜索与抓取；账号型来源只支持搜索，默认不选中。
pub const API_KEY_PROVIDERS: [&str; 4] = ["tavily", "querit", "firecrawl", "ollama"];
pub const ACCOUNT_PROVIDERS: [&str; 3] = ["workbuddy", "workbuddy-intl", "kiro"];
pub const SEARCH_PROVIDERS: [&str; 7] = ["tavily", "querit", "firecrawl", "ollama", "workbuddy", "workbuddy-intl", "kiro"];

/// 服务固定元数据（文案由前端按 id 映射，后端只回数据）
pub const WEB_SEARCH_PACKAGE: &str = WEB_SEARCH_SKILL_NAME;
pub const VISION_PACKAGE: &str = VISION_SERVICE_NAME;
pub const WEB_SEARCH_VERSION: &str = WEB_SEARCH_SKILL_VERSION;
pub const VISION_VERSION: &str = VISION_SERVICE_VERSION;

pub const CAPABILITY_COUNTS: [(&str, u32); 2] = [
    (WEB_SEARCH_SERVICE_ID, 2),
    (VISION_SERVICE_ID, 1),
];

/// 工具执行超时（秒）：同一服务各协议共用；服务端强制上限，装 MCP 时写进客户端
pub const TIMEOUT_MIN_SECONDS: u64 = 5;
pub const TIMEOUT_MAX_SECONDS: u64 = 600;
pub const DEFAULT_TIMEOUTS: [(&str, u64); 2] = [(WEB_SEARCH_SERVICE_ID, 30), (VISION_SERVICE_ID, 60)];

/// 端点路径（写死：改动会影响已接入的客户端）。
///
/// `"api"` 不进 [`PROTOCOLS`]，但这里保留映射：REST 工具接口是固定入口，
/// 由 `tool_runtime` 直接使用，不是可管理的「服务 × 协议」。
pub fn endpoint(service: &str, protocol: &str) -> &'static str {
    match (service, protocol) {
        (WEB_SEARCH_SERVICE_ID, "skill") => "/skill/web-search",
        (WEB_SEARCH_SERVICE_ID, "mcp") => "/mcp/web-search",
        (WEB_SEARCH_SERVICE_ID, "api") => "/api/web-search",
        (VISION_SERVICE_ID, "mcp") => "/mcp/vision",
        _ => "",
    }
}

pub fn is_service(value: &str) -> bool {
    SERVICE_IDS.contains(&value)
}

pub fn is_protocol(value: &str) -> bool {
    PROTOCOLS.contains(&value)
}

/// 该服务支持的协议（保持 PROTOCOLS 顺序）
pub fn service_protocols(service: &str) -> Vec<&'static str> {
    PROTOCOLS
        .iter()
        .copied()
        .filter(|protocol| !endpoint(service, protocol).is_empty())
        .collect()
}

pub fn capability_count(service: &str) -> u32 {
    CAPABILITY_COUNTS
        .iter()
        .find(|(id, _)| *id == service)
        .map(|(_, count)| *count)
        .unwrap_or(0)
}

pub fn package_name(service: &str) -> &'static str {
    match service {
        WEB_SEARCH_SERVICE_ID => WEB_SEARCH_PACKAGE,
        VISION_SERVICE_ID => VISION_PACKAGE,
        _ => "",
    }
}

pub fn version(service: &str) -> &'static str {
    match service {
        WEB_SEARCH_SERVICE_ID => WEB_SEARCH_VERSION,
        VISION_SERVICE_ID => VISION_VERSION,
        _ => "",
    }
}

/* ─── 配置形状 ─────────────────────────────────────────────── */

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderConfig {
    /// `enc1:` 信封；空串 = 未配置
    pub encrypted_key: String,
    pub key_hint: String,
    /// 账号型来源只保存账号引用，凭证仍由账号库与插件管理。
    pub account_id: String,
    pub enabled: bool,
    pub updated_at: i64,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self { encrypted_key: String::new(), key_hint: String::new(), account_id: String::new(), enabled: false, updated_at: 0 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Selection {
    pub search: Vec<String>,
    pub fetch: String,
}

impl Default for Selection {
    fn default() -> Self {
        Self { search: API_KEY_PROVIDERS.iter().map(|item| item.to_string()).collect(), fetch: "tavily".into() }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolConfig {
    /// service → protocol → 开关
    pub enabled: BTreeMap<String, BTreeMap<String, bool>>,
    /// service → 超时秒数
    pub timeouts: BTreeMap<String, u64>,
    /// provider → 密钥与启用状态
    pub providers: BTreeMap<String, ProviderConfig>,
    pub selection: Selection,
    pub vision: Option<VisionConfig>,
}

impl ToolConfig {
    /// 补齐缺省项（旧版本写入的配置 / 手工改过的 JSON 都走这一层）
    fn normalized(mut self) -> Self {
        for service in SERVICE_IDS {
            let entry = self.enabled.entry(service.to_string()).or_default();
            for protocol in service_protocols(service) {
                entry.entry(protocol.to_string()).or_insert(true);
            }
            let fallback = DEFAULT_TIMEOUTS
                .iter()
                .find(|(id, _)| *id == service)
                .map(|(_, value)| *value)
                .unwrap_or(30);
            self.timeouts.entry(service.to_string()).or_insert(fallback);
        }
        for provider in SEARCH_PROVIDERS {
            self.providers.entry(provider.to_string()).or_default();
        }
        self.selection.search.retain(|item| is_search_provider(item));
        if self.selection.search.is_empty() {
            self.selection.search = Selection::default().search;
        }
        if !is_fetch_provider(&self.selection.fetch) {
            self.selection.fetch = "tavily".into();
        }
        self
    }
}

pub fn load() -> ToolConfig {
    config::current()
        .raw()
        .get(KEY_TOOL_SERVICES)
        .map(parse_stored)
        .unwrap_or_default()
        .normalized()
}

fn parse_stored(value: &serde_json::Value) -> ToolConfig {
    if value.is_null() {
        return ToolConfig::default();
    }
    serde_json::from_value::<ToolConfig>(value.clone()).unwrap_or_default().normalized()
}

/// 在配置锁内做一次读改写：两个页面同时保存时不会互相覆盖（后写者基于前写者的结果）
fn update(mutate: impl FnOnce(&mut ToolConfig)) -> Result<ToolConfig, String> {
    let mut next: Option<ToolConfig> = None;
    let saved = crate::server::config::update_raw_field_with(KEY_TOOL_SERVICES, |value| {
        let mut current = parse_stored(value);
        mutate(&mut current);
        let current = current.normalized();
        if let Ok(serialized) = serde_json::to_value(&current) {
            *value = serialized;
            next = Some(current);
        }
    });
    match (saved, next) {
        (true, Some(config_value)) => Ok(config_value),
        _ => Err("工具服务配置写入失败".into()),
    }
}

/* ─── 开关与超时 ───────────────────────────────────────────── */

pub fn is_enabled(service: &str, protocol: &str) -> bool {
    if endpoint(service, protocol).is_empty() {
        return false;
    }
    // API 是固定的对外工具接口，不再是「工具服务」页里可开关的协议；
    // 保留 endpoint 与运行时路由，旧配置里的 api 开关一律不再生效。
    if protocol == "api" {
        return true;
    }
    load()
        .enabled
        .get(service)
        .and_then(|entry| entry.get(protocol))
        .copied()
        .unwrap_or(true)
}

pub fn set_enabled(service: &str, protocol: &str, enabled: bool) -> Result<Value, String> {
    if endpoint(service, protocol).is_empty() {
        return Err(format!("服务 {service} 不支持 {protocol} 协议"));
    }
    update(|config_value| {
        config_value
            .enabled
            .entry(service.to_string())
            .or_default()
            .insert(protocol.to_string(), enabled);
    })?;
    Ok(service_info(service, protocol))
}

pub fn timeout_seconds(service: &str) -> u64 {
    let value = load().timeouts.get(service).copied().unwrap_or(30);
    value.clamp(TIMEOUT_MIN_SECONDS, TIMEOUT_MAX_SECONDS)
}

pub fn set_timeout(service: &str, seconds: u64) -> Result<Value, String> {
    if !is_service(service) {
        return Err("未知的工具服务".into());
    }
    if !(TIMEOUT_MIN_SECONDS..=TIMEOUT_MAX_SECONDS).contains(&seconds) {
        return Err(format!(
            "超时必须是 {TIMEOUT_MIN_SECONDS}~{TIMEOUT_MAX_SECONDS} 秒之间的整数"
        ));
    }
    update(|config_value| {
        config_value.timeouts.insert(service.to_string(), seconds);
    })?;
    Ok(service_info(service, service_protocols(service)[0]))
}

/* ─── 服务清单 ─────────────────────────────────────────────── */

pub fn service_info(service: &str, protocol: &str) -> Value {
    let mut info = json!({
        "id": service,
        "protocol": protocol,
        "key": format!("{service}:{protocol}"),
        "enabled": is_enabled(service, protocol),
        "capability_count": capability_count(service),
        "endpoint": endpoint(service, protocol),
        "package_name": package_name(service),
        "version": version(service),
        "timeout_seconds": timeout_seconds(service),
    });
    if service == WEB_SEARCH_SERVICE_ID && protocol == "skill" {
        info["recommended_variant"] = json!(recommended_skill_variant());
        info["variants"] = serde_json::to_value(skill_variants()).unwrap_or(Value::Null);
    }
    info
}

pub fn list_services(protocol: Option<&str>) -> Vec<Value> {
    let mut rows = Vec::new();
    for service in SERVICE_IDS {
        for supported in service_protocols(service) {
            if protocol.is_none() || protocol == Some(supported) {
                rows.push(service_info(service, supported));
            }
        }
    }
    rows
}

/* ─── 搜索提供商 ───────────────────────────────────────────── */

pub fn is_search_provider(value: &str) -> bool {
    SEARCH_PROVIDERS.contains(&value)
}

pub fn is_account_provider(value: &str) -> bool {
    ACCOUNT_PROVIDERS.contains(&value)
}

pub fn is_fetch_provider(value: &str) -> bool {
    API_KEY_PROVIDERS.contains(&value)
}

pub fn provider_display_name(provider: &str) -> &'static str {
    match provider {
        "tavily" => "Tavily",
        "querit" => "Querit",
        "firecrawl" => "Firecrawl",
        "ollama" => "Ollama Web Search",
        "workbuddy" => "WorkBuddy 国内版",
        "workbuddy-intl" => "WorkBuddy 国际版",
        "kiro" => "Kiro",
        _ => "",
    }
}

pub fn key_hint(value: &str) -> String {
    if value.chars().count() <= 12 {
        return format!("{}••••", value.chars().take(4).collect::<String>());
    }
    let head: String = value.chars().take(7).collect();
    let tail: String = value.chars().skip(value.chars().count().saturating_sub(4)).collect();
    format!("{head}••••{tail}")
}

pub async fn list_providers(store: &AccountStore) -> Value {
    let config_value = load();
    let mut rows: Vec<Value> = API_KEY_PROVIDERS.iter().map(|provider| {
        let entry = config_value.providers.get(*provider).cloned().unwrap_or_default();
        json!({
            "provider": provider,
            "display_name": provider_display_name(provider),
            "auth_type": "api_key",
            "supports_search": true,
            "supports_fetch": true,
            "available": true,
            "configured": !entry.encrypted_key.is_empty(),
            "key_hint": if entry.key_hint.is_empty() { Value::Null } else { json!(entry.key_hint) },
            "enabled": entry.enabled,
            "updated_at": if entry.updated_at > 0 { json!(entry.updated_at) } else { Value::Null },
        })
    }).collect();
    let account_rows = futures::future::join_all(ACCOUNT_PROVIDERS.iter().map(|provider| {
        let entry = config_value.providers.get(*provider).cloned().unwrap_or_default();
        account_search::provider_row(store, provider, entry)
    })).await;
    rows.extend(account_rows);
    Value::Array(rows)
}

pub fn provider_selection() -> Value {
    let config_value = load();
    json!({
        "search_providers": config_value.selection.search,
        "fetch_provider": config_value.selection.fetch,
    })
}

pub fn update_provider(
    store: &AccountStore,
    provider: &str,
    api_key: Option<&str>,
    enabled: Option<bool>,
    account_id: Option<&str>,
) -> Result<Value, String> {
    if !is_search_provider(provider) {
        return Err("未知的搜索提供商".into());
    }
    if is_account_provider(provider) {
        if api_key.is_some_and(|key| !key.trim().is_empty()) {
            return Err("该搜索来源使用已有账号，不接受 API Key".into());
        }
        let account_id = account_id.map(str::trim);
        if let Some(id) = account_id.filter(|id| !id.is_empty()) {
            account_search::validate_account(store, provider, id).map_err(|error| error.message)?;
        }
        let config_value = update(|config_value| {
            let entry = config_value.providers.entry(provider.to_string()).or_default();
            if let Some(id) = account_id { entry.account_id = id.to_string(); }
            if let Some(enabled) = enabled { entry.enabled = enabled; }
            entry.updated_at = crate::server::logging::now_ms();
        })?;
        let entry = config_value.providers.get(provider).cloned().unwrap_or_default();
        return Ok(json!({ "success": true, "provider": provider, "account_id": entry.account_id }));
    }
    if account_id.is_some() {
        return Err("该搜索来源使用 API Key，不接受账号绑定".into());
    }
    let api_key = api_key.map(str::trim).filter(|value| !value.is_empty());
    let encrypted = match api_key {
        Some(key) => {
            let sealed = crypto::seal(key.as_bytes(), SecretScope::Accounts)
                .map_err(|_| "无法加密 API Key".to_string())?;
            Some((sealed, key_hint(key)))
        }
        None => None,
    };
    let config_value = update(|config_value| {
        let entry = config_value.providers.entry(provider.to_string()).or_default();
        if let Some((sealed, hint)) = encrypted {
            entry.encrypted_key = sealed;
            entry.key_hint = hint;
            entry.updated_at = crate::server::logging::now_ms();
        }
        if let Some(enabled) = enabled {
            entry.enabled = enabled;
        }
    })?;
    let entry = config_value.providers.get(provider).cloned().unwrap_or_default();
    Ok(json!({
        "success": true,
        "provider": provider,
        "configured": !entry.encrypted_key.is_empty(),
        "key_hint": if entry.key_hint.is_empty() { Value::Null } else { json!(entry.key_hint) },
    }))
}

pub fn clear_provider_key(provider: &str) -> bool {
    if !API_KEY_PROVIDERS.contains(&provider) {
        return false;
    }
    update(|config_value| {
        let entry = config_value.providers.entry(provider.to_string()).or_default();
        entry.encrypted_key.clear();
        entry.key_hint.clear();
        entry.updated_at = crate::server::logging::now_ms();
    })
    .is_ok()
}

/// 读取某家当前配置（含解密后的密钥；仅服务端内部使用，禁止出现在日志/响应中）
pub fn provider_key(provider: &str) -> Option<String> {
    decrypt_provider_key(load().providers.get(provider)?)
}

fn decrypt_provider_key(entry: &ProviderConfig) -> Option<String> {
    if entry.encrypted_key.is_empty() {
        return None;
    }
    let bytes = crypto::open(&entry.encrypted_key, SecretScope::Accounts).ok()?;
    String::from_utf8(bytes).ok().filter(|value| !value.is_empty())
}

pub fn configured_provider(provider: &str) -> bool {
    load()
        .providers
        .get(provider)
        .is_some_and(|entry| !entry.encrypted_key.is_empty() && entry.enabled)
}

/// 已启用且已配置密钥的提供商
pub fn ready_providers() -> Vec<&'static str> {
    let config_value = load();
    SEARCH_PROVIDERS
        .iter()
        .copied()
        .filter(|provider| {
            config_value
                .providers
                .get(*provider)
                .is_some_and(|entry| entry.enabled && !entry.encrypted_key.is_empty())
        })
        .collect()
}

/// 已选择但未启用或未配置密钥的提供商（展示名，供运行时返回可读配置错误）
pub fn missing_selected_providers() -> Vec<&'static str> {
    let selected = load().selection.search;
    let ready = ready_providers();
    selected
        .iter()
        .filter(|provider| !ready.contains(&provider.as_str()))
        .map(|provider| provider_display_name(provider))
        .filter(|name| !name.is_empty())
        .collect()
}

pub fn update_selection(
    search: Option<Vec<String>>,
    fetch: Option<String>,
) -> Result<Value, String> {
    let mut next_search = None;
    if let Some(items) = search {
        if items.is_empty() {
            return Err("批量网页搜索至少选择一个提供商".into());
        }
        if items.iter().any(|item| !is_search_provider(item)) {
            return Err("搜索提供商包含未知项".into());
        }
        let mut seen = Vec::new();
        for item in items {
            if !seen.contains(&item) {
                seen.push(item);
            }
        }
        next_search = Some(seen);
    }
    if let Some(provider) = fetch.as_deref() {
        if !is_fetch_provider(provider) {
            return Err("该提供商不支持网页抓取".into());
        }
    }
    update(|config_value| {
        if let Some(items) = next_search {
            config_value.selection.search = items;
        }
        if let Some(provider) = fetch {
            config_value.selection.fetch = provider;
        }
    })?;
    Ok(provider_selection())
}
