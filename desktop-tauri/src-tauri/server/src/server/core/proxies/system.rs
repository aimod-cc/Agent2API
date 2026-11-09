use base64::Engine;
use hyper_util::client::proxy::matcher::{Intercept, Matcher};
use serde_json::{json, Value};

pub(crate) fn snapshot() -> Result<Value, String> {
    // 只读取通用 HTTPS 出口，不访问探测地址；导入后是普通自定义代理，不是动态系统规则。
    let target = axum::http::Uri::from_static("https://proxy-detection.invalid/");
    let Some(proxy) = Matcher::from_system().intercept(&target) else {
        return Ok(json!({ "proxy": null }));
    };
    let uri = proxy.uri();
    let (protocol, default_port) = match uri.scheme_str() {
        Some("http") => ("http", 80),
        Some("socks5" | "socks5h") => ("socks5", 1080),
        _ => return Err("系统代理协议暂不支持自动填入，请手动配置 HTTP 或 SOCKS5 代理".into()),
    };
    let host = uri.host().filter(|host| !host.is_empty())
        .ok_or("系统代理地址无效，请手动填写")?;
    let port = uri.port_u16().unwrap_or(default_port);
    if port == 0 {
        return Err("系统代理地址无效，请手动填写".into());
    }
    let (username, password) = credentials(&proxy)?;
    let config = super::normalize_account_proxy(&json!({
        "source": "custom", "protocol": protocol, "host": host, "port": port,
        "username": username, "password": password,
    })).map_err(|_| "系统代理地址无效，请手动填写".to_string())?;
    Ok(json!({ "proxy": config }))
}

pub(crate) fn upstream() -> Result<super::ResolvedProxy, String> {
    let value = snapshot()?;
    let config = value.get("proxy").filter(|value| !value.is_null())
        .ok_or("已启用非直连代理，但网关所在设备未配置可用的系统代理")?;
    match super::resolve_account_proxy(Some(config)) {
        Some(super::ProxyResolution::Resolved(proxy)) => Ok(proxy),
        _ => Err("系统代理地址无效，无法连接非直连代理".to_string()),
    }
}

fn credentials(proxy: &Intercept) -> Result<(String, String), String> {
    if let Some((username, password)) = proxy.raw_auth() {
        return Ok((username.to_string(), password.to_string()));
    }
    let Some(header) = proxy.basic_auth() else {
        return Ok((String::new(), String::new()));
    };
    let invalid = || "系统代理认证信息无法读取，请手动填写".to_string();
    let encoded = header.to_str().ok().and_then(|value| value.strip_prefix("Basic "))
        .ok_or_else(invalid)?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_| invalid())?;
    let text = String::from_utf8(bytes).map_err(|_| invalid())?;
    let (username, password) = text.split_once(':').ok_or_else(invalid)?;
    Ok((username.to_string(), password.to_string()))
}
