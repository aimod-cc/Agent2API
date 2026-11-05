//! 账号型搜索的宿主接线：配置只存账号引用，上游协议与凭证全部留在插件。
//! 可选动作沿用 admin.action，不升级基础握手；能力探测不发上游请求。

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::normalize::{clip, SourceDraft, MAX_CONTENT_LENGTH, MAX_TITLE_LENGTH};
use super::{ProviderConfig, SearchError, SearchErrorCode};
use crate::server::core::account_store::AccountStore;
use crate::server::core::plugin_host;
use crate::server::core::providers::adapter::ProviderAdapter;

const CAPABILITIES: &str = "web-search.capabilities";
const SEARCH: &str = "web-search.search";

fn configuration(message: &str) -> SearchError {
    SearchError::new(SearchErrorCode::Configuration, message)
}

/// 公开账号视图只投影 id/name/enabled，不能把整条账号或插件 secret 送给前端。
fn account_options(store: &AccountStore, provider: &str) -> Vec<Value> {
    store.list_visible_accounts().get("accounts").and_then(Value::as_array)
        .into_iter().flatten()
        .filter(|account| account.get("provider").and_then(Value::as_str) == Some(provider))
        .filter_map(|account| {
            let id = account.get("id")?.as_str()?;
            let name = account.get("name").and_then(Value::as_str).filter(|name| !name.is_empty()).unwrap_or(id);
            Some(json!({ "id": id, "name": name, "enabled": account.get("enabled").and_then(Value::as_bool).unwrap_or(true) }))
        }).collect()
}

pub(super) fn validate_account(store: &AccountStore, provider: &str, account_id: &str) -> Result<(), SearchError> {
    if account_id.is_empty() { return Err(configuration("请先选择搜索使用的账号")); }
    let snapshot = store.list_accounts();
    let account = snapshot.get("accounts").and_then(Value::as_array).into_iter().flatten()
        .find(|account| account.get("id").and_then(Value::as_str) == Some(account_id))
        .ok_or_else(|| configuration("已绑定的搜索账号不存在，请重新选择"))?;
    if account.get("provider").and_then(Value::as_str) != Some(provider) {
        return Err(configuration("搜索账号不属于该提供商，请重新选择"));
    }
    if account.get("enabled").and_then(Value::as_bool) == Some(false) {
        return Err(configuration("已绑定的搜索账号已停用"));
    }
    Ok(())
}

async fn capability(
    store: &AccountStore,
    provider: &str,
) -> Result<(Arc<dyn ProviderAdapter>, usize), SearchError> {
    let adapter = plugin_host::lookup(provider)
        .ok_or_else(|| configuration("提供商插件未安装或未启用"))?;
    let metadata = tokio::time::timeout(Duration::from_secs(5),
        adapter.admin_action(store, "", CAPABILITIES, &json!({}))).await
        .map_err(|_| SearchError::new(SearchErrorCode::ProviderUnavailable, "提供商插件暂不可用"))?
        .map_err(|error| {
            if error.status_code == 501 || error.code.as_deref() == Some("unsupported") {
                configuration("当前提供商插件不支持账号搜索，请更新插件")
            } else {
                SearchError::new(SearchErrorCode::ProviderUnavailable, "无法确认插件搜索能力，请更新或重启提供商插件")
            }
        })?;
    if metadata.get("version").and_then(Value::as_u64) != Some(1)
        || metadata.get("search").and_then(Value::as_bool) != Some(true) {
        return Err(configuration("当前提供商插件不支持账号搜索，请更新插件"));
    }
    let max_chars = metadata.get("max_query_chars").and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok()).filter(|value| (1..=16000).contains(value))
        .ok_or_else(|| configuration("插件搜索能力声明不完整，请更新插件"))?;
    Ok((adapter, max_chars))
}

pub(super) async fn provider_row(store: &AccountStore, provider: &str, entry: ProviderConfig) -> Value {
    let options = account_options(store, provider);
    let account_name = options.iter().find(|account| account["id"] == entry.account_id)
        .and_then(|account| account.get("name")).and_then(Value::as_str).unwrap_or("");
    let support = capability(store, provider).await;
    let account = validate_account(store, provider, &entry.account_id);
    let error = support.as_ref().err().or_else(|| account.as_ref().err());
    let configured = support.is_ok() && account.is_ok();
    json!({
        "provider": provider,
        "display_name": super::provider_display_name(provider),
        "auth_type": "account", "supports_search": true, "supports_fetch": false,
        "available": support.is_ok(), "configured": configured, "enabled": entry.enabled,
        "account_id": entry.account_id, "account_name": account_name, "accounts": options,
        "max_query_chars": support.as_ref().ok().map(|(_, chars)| chars),
        "unavailable_reason": error.map(|error| error.message.as_str()), "key_hint": Value::Null,
        "updated_at": if entry.updated_at > 0 { json!(entry.updated_at) } else { Value::Null },
    })
}

#[derive(Clone)]
pub(super) enum SearchSource {
    Key { provider: String, key: String },
    Account { provider: String, account_id: String, adapter: Arc<dyn ProviderAdapter>, max_chars: usize },
}

impl SearchSource {
    pub(super) fn provider(&self) -> &str {
        match self { Self::Key { provider, .. } | Self::Account { provider, .. } => provider }
    }

    pub(super) async fn search(&self, store: &AccountStore, query: &str, timeout: u64) -> Result<Vec<SourceDraft>, SearchError> {
        match self {
            Self::Key { provider, key } => super::providers::search(provider, key, query, timeout).await,
            Self::Account { provider, account_id, adapter, max_chars } => {
                if query.chars().count() > *max_chars {
                    return Err(SearchError::new(SearchErrorCode::InvalidInput, format!("该搜索来源最多接受 {max_chars} 个字符")));
                }
                validate_account(store, provider, account_id)?;
                let payload = json!({ "query": query, "timeout_seconds": timeout });
                let result = tokio::time::timeout(Duration::from_secs(timeout.saturating_add(2)),
                    adapter.admin_action(store, account_id, SEARCH, &payload)).await
                    .map_err(|_| SearchError::new(SearchErrorCode::TimedOut, "搜索请求超时"))?
                    .map_err(search_failure)?;
                source_rows(provider, &result)
            }
        }
    }
}

pub(super) async fn prepare(store: &AccountStore, provider: &str, entry: &ProviderConfig) -> Result<SearchSource, SearchError> {
    if super::is_account_provider(provider) {
        validate_account(store, provider, &entry.account_id)?;
        let (adapter, max_chars) = capability(store, provider).await?;
        Ok(SearchSource::Account { provider: provider.to_string(), account_id: entry.account_id.clone(), adapter, max_chars })
    } else {
        let key = super::decrypt_provider_key(entry).ok_or_else(|| configuration("提供商尚未配置有效密钥"))?;
        Ok(SearchSource::Key { provider: provider.to_string(), key })
    }
}

fn search_failure(error: crate::server::errors::GatewayError) -> SearchError {
    let (code, message) = match error.code.as_deref() {
        Some("invalid_input") => (SearchErrorCode::InvalidInput, "搜索参数不符合该来源的要求"),
        Some("configuration") => (SearchErrorCode::Configuration, "账号搜索配置不完整，请重新选择账号或更新插件"),
        _ if error.status_code == 401 => (SearchErrorCode::InvalidProviderKey, "搜索账号登录态已失效，请重新登录"),
        _ if matches!(error.status_code, 402 | 429) => (SearchErrorCode::RateLimited, "搜索额度已用完或触发限流"),
        _ if error.status_code == 504 => (SearchErrorCode::TimedOut, "搜索请求超时"),
        _ if error.status_code == 501 => (SearchErrorCode::Configuration, "当前提供商插件不支持账号搜索，请更新插件"),
        _ if error.status_code == 403 => (SearchErrorCode::ProviderUnavailable, "搜索服务拒绝了该账号，请检查账号权限"),
        _ => (SearchErrorCode::ProviderUnavailable, "账号搜索暂不可用，请检查账号及插件状态"),
    };
    // 插件错误可能包含认证或代理内部信息，公开层只回稳定错误说明。
    SearchError::new(code, message)
}

fn source_rows(provider: &str, value: &Value) -> Result<Vec<SourceDraft>, SearchError> {
    let rows = value.get("sources").and_then(Value::as_array)
        .ok_or_else(|| SearchError::new(SearchErrorCode::ProviderUnavailable, "插件返回的搜索来源格式不正确"))?;
    Ok(rows.iter().take(50).filter_map(|row| {
        let url = row.get("url").and_then(Value::as_str)?.trim();
        let parsed = url::Url::parse(url).ok()?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none()
            || !parsed.username().is_empty() || parsed.password().is_some() { return None; }
        let title = row.get("title").and_then(Value::as_str).unwrap_or(url).trim();
        let content = row.get("content").and_then(Value::as_str).unwrap_or("").trim();
        let date = row.get("publishedDate").and_then(Value::as_str).map(str::trim)
            .filter(|value| !value.is_empty()).map(|value| clip(value, 100));
        Some(SourceDraft {
            url: parsed.to_string(), title: clip(if title.is_empty() { url } else { title }, MAX_TITLE_LENGTH),
            content: clip(content, MAX_CONTENT_LENGTH), published_date: date,
            providers: vec![super::provider_display_name(provider).to_string()],
        })
    }).collect())
}

pub async fn test_provider(store: &AccountStore, provider: &str) -> Value {
    if !super::is_account_provider(provider) {
        return super::providers::test_provider(provider).await;
    }
    let started = crate::server::logging::now_ms();
    let entry = super::load().providers.get(provider).cloned().unwrap_or_default();
    let result = match prepare(store, provider, &entry).await {
        Ok(source) => source.search(store, "Agent2API connectivity check", super::timeout_seconds(super::WEB_SEARCH_SERVICE_ID)).await,
        Err(error) => Err(error),
    };
    let duration = crate::server::logging::now_ms() - started;
    match result {
        Ok(sources) if !sources.is_empty() => json!({ "success": true, "provider": provider, "result_count": sources.len(), "duration_ms": duration }),
        Ok(_) => json!({ "success": false, "provider": provider, "code": "no_results", "error": "搜索提供商没有返回来源", "duration_ms": duration }),
        Err(error) => json!({ "success": false, "provider": provider, "code": error.code.as_str(), "error": error.message, "duration_ms": duration }),
    }
}
