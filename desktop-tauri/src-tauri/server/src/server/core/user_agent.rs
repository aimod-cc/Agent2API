//! 账号设置只覆盖 HTTP 请求头，不改凭证或请求体里的客户端身份字段。

use std::future::Future;

use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use serde_json::Value;

use crate::server::core::{custom_providers, plugin_host, providers};

pub(crate) fn builtin_agent_plugin(provider: &str) -> bool {
    plugin_host::provider_pack(provider).is_some_and(|pack| {
        pack == "agent2api-providers"
            || pack.strip_prefix("agent2api-wasm-") == Some(provider)
                && plugin_host::provider_category(provider) == plugin_host::ProviderCategory::Agent
    })
}

pub(crate) fn custom_overridable(provider: &Value) -> bool {
    let preset = provider.get("presetKey").and_then(Value::as_str).unwrap_or("");
    if matches!(preset, "sub2api" | "newapi") { return true; }
    if matches!(preset, "opencode-zen" | "clinepass") { return false; }
    let emulated = provider.get("clientEmulation").and_then(Value::as_str)
        == Some(custom_providers::CLIENT_EMULATION_OPENCODE);
    if emulated && !custom_providers::is_opencode_go_endpoint(
        provider.get("baseUrl").and_then(Value::as_str).unwrap_or(""),
    ) { return false; }
    // 旧 Cline Pass 模板没有 presetKey，但仍保存了成套客户端身份头。
    !provider.get("headers").and_then(Value::as_object).is_some_and(|headers| {
        headers.iter().any(|(name, value)| name.eq_ignore_ascii_case("x-client-type")
            && value.as_str() == Some("cline-sdk"))
            && headers.iter().any(|(name, value)| name.eq_ignore_ascii_case("user-agent")
                && value.as_str().is_some_and(|value| value.starts_with("Cline/")))
    })
}

pub(crate) fn provider_overridable(provider: &str) -> bool {
    if provider.starts_with(custom_providers::ID_PREFIX) {
        return custom_providers::get(provider).is_some_and(|record| custom_overridable(&record));
    }
    providers::is_executable_provider(provider)
        && providers::adapter::adapter_by_id(provider)
            .is_some_and(|adapter| adapter.supports_account_user_agent())
}

pub(crate) fn override_value<'a>(provider: &str, value: &'a str) -> &'a str {
    if provider_overridable(provider) { value.trim() } else { "" }
}

pub(crate) fn normalize_override(
    provider: &str,
    value: &Value,
) -> Result<String, crate::server::core::account_store::AccountStoreError> {
    if !provider_overridable(provider) { return Ok(String::new()); }
    crate::server::core::client::catalog::normalize_user_agent_override(value)
}

pub(crate) fn task_value<'a>(provider: &str, value: &'a str) -> &'a str {
    if provider_overridable(provider) { return value.trim(); }
    match provider {
        // 固定任务链沿用 MonkeyCode 适配器的身份，不借用账号的可编辑覆盖值。
        "monkeycode" | "monkeycode-intl" => "monkeycode-local-proxy",
        _ => "",
    }
}

tokio::task_local! {
    static CURRENT: String;
}

pub(crate) async fn scope<F: Future>(value: &str, work: F) -> F::Output {
    CURRENT.scope(value.trim().to_string(), work).await
}

pub(crate) fn apply(headers: &mut Vec<(String, String)>, value: &str) {
    let value = value.trim();
    if value.is_empty() { return; }
    headers.retain(|(name, _)| !name.eq_ignore_ascii_case("user-agent"));
    headers.push(("User-Agent".to_string(), value.to_string()));
}

pub(crate) trait AccountRequest: Sized {
    fn with_user_agent(self, value: &str) -> Self;
    fn with_account_user_agent(self) -> Self {
        let value = CURRENT.try_with(Clone::clone).unwrap_or_default();
        self.with_user_agent(&value)
    }
}

impl AccountRequest for reqwest::RequestBuilder {
    fn with_user_agent(self, value: &str) -> Self {
        let value = value.trim();
        if value.is_empty() { return self; }
        match HeaderValue::from_str(value) {
            Ok(value) => {
                let mut headers = HeaderMap::new();
                headers.insert(USER_AGENT, value);
                self.headers(headers)
            }
            Err(_) => self.header(USER_AGENT, value),
        }
    }
}
