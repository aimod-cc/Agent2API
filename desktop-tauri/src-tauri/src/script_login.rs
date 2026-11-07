use serde_json::Value;
use tauri::WebviewWindow;

pub(crate) fn captures_wps(provider: &str, mode: &str, started: &Value) -> bool {
    provider == "wps-lingxi" && mode == "embedded"
        && started.get("cookieCapture").and_then(Value::as_str) == Some("wps-lingxi")
}

pub(crate) fn loopback_targets(started: &Value) -> Result<Vec<url::Url>, String> {
    let Some(values) = started.get("loopbackTargets").filter(|value| !value.is_null()) else { return Ok(Vec::new()) };
    let values = values.as_array().filter(|values| values.len() <= 8).ok_or("登录本机回调列表无效")?;
    values.iter().map(|value| {
        let text = value.as_str().ok_or("登录本机回调地址无效")?;
        let url = url::Url::parse(text).map_err(|_| "登录本机回调地址无效")?;
        if url.scheme() != "http" || !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
            || url.port().is_none() || !url.username().is_empty() || url.password().is_some()
            || url.query().is_some() || url.fragment().is_some()
        {
            return Err("登录回调不属于已声明的本机接收端".into());
        }
        Ok(url)
    }).collect()
}

pub(crate) async fn wps_cookie(window: &WebviewWindow) -> Result<Option<String>, String> {
    let current = window.url().map_err(|_| "无法读取 WPS 登录窗口地址")?;
    let allowed = |host: &str| ["wps.cn", "kdocs.cn"].iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")));
    let Some(host) = current.host_str().filter(|host| current.scheme() == "https"
        && current.port_or_known_default() == Some(443) && matches!(*host, "lingxi.wps.cn" | "lingxi.kdocs.cn"))
    else { return Ok(None) };
    let cookies = crate::login::read_window_cookies(window).await?;
    let now = chrono::Utc::now().timestamp();
    let mut selected = std::collections::BTreeMap::new();
    for cookie in cookies {
        let Some(domain) = cookie.domain().map(|domain| domain.trim_start_matches('.')) else { continue };
        if !allowed(domain) || !(host == domain || host.ends_with(&format!(".{domain}")))
            || !matches!(cookie.name(), "wps_sid" | "uid" | "copilot_server_branch")
            || cookie.value().is_empty() || cookie.path().unwrap_or("/") != "/"
            || cookie.expires_datetime().is_some_and(|expiry| expiry.unix_timestamp() <= now)
            || cookie.value().chars().any(|ch| ch.is_control() || ch == ';')
        {
            continue;
        }
        selected.insert(cookie.name().to_string(), cookie.value().to_string());
    }
    if !selected.get("wps_sid").is_some_and(|sid| sid.len() >= 16) { return Ok(None); }
    Ok(Some(selected.into_iter().map(|(name, value)| format!("{name}={value}")).collect::<Vec<_>>().join("; ")))
}
