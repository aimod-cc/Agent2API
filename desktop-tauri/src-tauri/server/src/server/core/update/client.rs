//! 公网请求共用更新出网代理；指定代理失效时不回退直连。

use std::time::Duration;

use serde_json::json;

use crate::server::config;
use crate::server::core::egress;
use crate::server::core::proxies::{
    resolve_account_proxy, ProxyResolution, ResolvedProxy, CLASH_MIXED_UID,
};
use crate::server::logging;

pub(crate) fn resolve_egress_candidates() -> Result<Vec<(String, Option<ResolvedProxy>)>, String> {
    if let Some(configured) = config::update_proxy() {
        match resolve_account_proxy(Some(&configured)) {
            Some(ProxyResolution::Resolved(proxy)) if proxy.port.is_some() => {
                let label = if proxy.label.is_empty() {
                    "指定的更新出口".to_string()
                } else {
                    proxy.label.clone()
                };
                return Ok(vec![(label, Some(proxy))]);
            }
            Some(ProxyResolution::Failed(reason)) => {
                return Err(format!("指定的更新出口不可用（{reason}）"));
            }
            _ => return Err("指定的更新出口没有可用端口".to_string()),
        }
    }
    let mut candidates = vec![("直连".to_string(), None)];
    if let Some(resolution) = resolve_account_proxy(Some(&json!({
        "source": "clash",
        "listenerUid": CLASH_MIXED_UID,
    }))) {
        if let Some(proxy) = resolution.resolved() {
            if proxy.port.is_some() {
                let label = if proxy.label.is_empty() {
                    "Clash 混合端口".to_string()
                } else {
                    proxy.label.clone()
                };
                candidates.push((label, Some(proxy.clone())));
            }
        } else if let Some(error) = resolution.error() {
            logging::verbose("[Update]", &format!("Clash 出口不可用（{error}），仅用直连"));
        }
    }
    Ok(candidates)
}

/// 普通公网资源只复用出口，不使用更新地址白名单或发送凭证。
pub async fn fetch_public(
    url: &str,
    headers: &[(String, String)],
    timeout_ms: Option<u64>,
) -> Result<reqwest::Response, String> {
    let host = url::Url::parse(url).ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string());
    let candidates = resolve_egress_candidates()?;
    let mut last_error = "未知错误".to_string();
    for (label, proxy) in candidates {
        let client = egress::client_for(proxy.as_ref());
        let mut builder = client.get(url);
        if let Some(timeout_ms) = timeout_ms {
            builder = builder.timeout(Duration::from_millis(timeout_ms));
        }
        for (name, value) in headers {
            if !name.eq_ignore_ascii_case("authorization") {
                builder = builder.header(name.as_str(), value.as_str());
            }
        }
        match builder.send().await {
            Ok(response) => return Ok(response),
            Err(error) => {
                last_error = egress::describe_error_detail(&error);
                logging::verbose("[Network]", &format!("经 {label} 访问 {host} 失败：{last_error}"));
            }
        }
    }
    Err(format!("无法连接 {host}：{last_error}"))
}
