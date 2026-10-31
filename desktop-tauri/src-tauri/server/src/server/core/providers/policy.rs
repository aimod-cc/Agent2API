//! 插件可选策略只表达通用动作，不让上游业务码进入编排层。

use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChatPolicy {
    pub repair_native_tools: bool,
    pub stable_conversation: bool,
    pub network: Option<NetworkPolicy>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NetworkPolicy {
    pub trusted_origins: Vec<String>,
    pub backup_origin: String,
    pub retry_statuses: Vec<u16>,
    pub address_statuses: Vec<u16>,
    pub http_dns: Option<HttpDnsPolicy>,
    pub fallback_addresses: Vec<String>,
    pub max_attempts: usize,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HttpDnsPolicy {
    pub token: String,
    pub bootstrap_ips: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ErrorPolicy {
    pub stop: bool,
    pub account_cooldown_until: Option<i64>,
    pub suspend_account: bool,
    pub reason: String,
}

impl ChatPolicy {
    pub fn from_value(value: &serde_json::Value) -> Self {
        serde_json::from_value(value.clone()).unwrap_or_default()
    }
}

impl ErrorPolicy {
    pub fn from_value(value: serde_json::Value) -> Self {
        serde_json::from_value(value).unwrap_or_default()
    }
}
