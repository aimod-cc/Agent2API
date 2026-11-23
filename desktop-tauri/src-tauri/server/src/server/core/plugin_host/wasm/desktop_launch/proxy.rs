use serde_json::{json, Value};

use crate::server::core::proxies::{
    is_rotate_config, normalize_host, resolve_account_proxy, ProxyResolution,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchProxy {
    pub url: String,
    pub label: String,
    pub address: String,
}

#[derive(Clone, Debug)]
pub struct LaunchProxyState {
    pub supported: bool,
    pub configured: bool,
    pub enabled: bool,
    pub available: bool,
    pub label: String,
    pub address: String,
    pub error: Option<String>,
    pub resolved: Option<LaunchProxy>,
}

enum EvaluatedProxy {
    NotConfigured,
    Supported(LaunchProxy),
    Unavailable { label: String, error: String },
}

impl LaunchProxyState {
    pub fn disabled() -> Self {
        Self {
            supported: false,
            configured: false,
            enabled: false,
            available: false,
            label: String::new(),
            address: String::new(),
            error: None,
            resolved: None,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "supported": self.supported,
            "configured": self.configured,
            "enabled": self.enabled,
            "available": self.available,
            "label": self.label,
            "address": self.address,
            "error": self.error,
        })
    }
}

fn safe_proxy_host(host: &str) -> Result<String, String> {
    let normalized = normalize_host(host).map_err(|_| "代理主机地址无效".to_string())?;
    if normalized.is_empty()
        || normalized.chars().any(|ch| {
            ch.is_whitespace() || ch.is_control() || matches!(ch, '"' | '\'' | '@' | '/' | '\\' | '?' | '#')
        })
    {
        return Err("代理主机地址包含不支持的字符".into());
    }
    Ok(normalized)
}

fn evaluate_raw(raw_proxy: &Value, refresh_clash: bool) -> EvaluatedProxy {
    if raw_proxy.is_null() {
        return EvaluatedProxy::NotConfigured;
    }
    if raw_proxy.get("source").and_then(Value::as_str) == Some("direct") {
        return EvaluatedProxy::NotConfigured;
    }
    if is_rotate_config(raw_proxy) {
        return EvaluatedProxy::Unavailable {
            label: "轮询代理".into(),
            error: "轮询代理不支持带入客户端启动，请关闭代理开关或改用固定 HTTP 代理".into(),
        };
    }
    if refresh_clash {
        let _ = crate::server::core::clash::clash_snapshot_with(true);
    }
    let Some(resolution) = resolve_account_proxy(Some(raw_proxy)) else {
        return EvaluatedProxy::NotConfigured;
    };
    match resolution {
        ProxyResolution::Failed(reason) => EvaluatedProxy::Unavailable {
            label: "解析失败".into(),
            error: reason,
        },
        ProxyResolution::Resolved(proxy) => {
            let label = if proxy.label.trim().is_empty() {
                "账号代理".to_string()
            } else {
                proxy.label.trim().to_string()
            };
            if proxy.non_direct {
                return EvaluatedProxy::Unavailable {
                    label,
                    error: "非直连链式代理不支持带入客户端启动，请关闭代理开关或改用普通 HTTP 代理".into(),
                };
            }
            if !proxy.username.is_empty() || !proxy.password.is_empty() {
                return EvaluatedProxy::Unavailable {
                    label,
                    error: "带账号密码的代理不支持带入客户端启动，请关闭代理开关或改用无认证 HTTP 代理".into(),
                };
            }
            if proxy.protocol != "http" {
                return EvaluatedProxy::Unavailable {
                    label,
                    error: "SOCKS5 代理不支持带入客户端启动，请关闭代理开关或改用 HTTP 代理".into(),
                };
            }
            let Some(port) = proxy.port.filter(|port| *port > 0) else {
                return EvaluatedProxy::Unavailable {
                    label,
                    error: "代理端口配置无效".into(),
                };
            };
            let host = match safe_proxy_host(&proxy.host) {
                Ok(host) => host,
                Err(error) => return EvaluatedProxy::Unavailable { label, error },
            };
            let address = if host.contains(':') {
                format!("[{host}]:{port}")
            } else {
                format!("{host}:{port}")
            };
            let url = format!("http://{address}");
            EvaluatedProxy::Supported(LaunchProxy { url, label, address })
        }
    }
}

pub fn prepare_state(
    supported: bool,
    raw_proxy: &Value,
    requested_use_proxy: Option<bool>,
) -> Result<LaunchProxyState, String> {
    if !supported {
        if requested_use_proxy == Some(true) {
            return Err("当前客户端不支持使用账号代理启动".into());
        }
        return Ok(LaunchProxyState::disabled());
    }
    let evaluated = evaluate_raw(raw_proxy, true);
    match evaluated {
        EvaluatedProxy::NotConfigured => Ok(LaunchProxyState {
            supported: true,
            configured: false,
            enabled: false,
            available: false,
            label: String::new(),
            address: String::new(),
            error: None,
            resolved: None,
        }),
        EvaluatedProxy::Supported(proxy) => {
            let enabled = requested_use_proxy.unwrap_or(true);
            Ok(LaunchProxyState {
                supported: true,
                configured: true,
                enabled,
                available: true,
                label: proxy.label.clone(),
                address: proxy.address.clone(),
                error: None,
                resolved: enabled.then_some(proxy),
            })
        }
        EvaluatedProxy::Unavailable { label, error } => {
            let enabled = requested_use_proxy.unwrap_or(true);
            Ok(LaunchProxyState {
                supported: true,
                configured: true,
                enabled,
                available: false,
                label,
                address: String::new(),
                error: enabled.then_some(error),
                resolved: None,
            })
        }
    }
}

pub fn reverify(raw_proxy: &Value, expected: &LaunchProxy) -> Result<(), String> {
    match evaluate_raw(raw_proxy, true) {
        EvaluatedProxy::Supported(current) if current.url == expected.url => Ok(()),
        EvaluatedProxy::Supported(_) | EvaluatedProxy::NotConfigured => {
            Err("账号代理配置已变化，请重新确认".into())
        }
        EvaluatedProxy::Unavailable { error, .. } => Err(error),
    }
}
