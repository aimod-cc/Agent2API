use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, WebviewWindow};

const TOKEN_SCRIPT: &str = r#"(() => {
  if (location.origin !== 'https://chat.deepseek.com') return null;
  try {
    const raw = localStorage.getItem('userToken');
    if (!raw || raw.length > 32768) return null;
    const token = JSON.parse(raw).value;
    return typeof token === 'string' && token.length > 0 && token.length <= 16384
      && !/[\r\n]/.test(token) ? token : null;
  } catch (_) { return null; }
})()"#;

pub fn allowed(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(url.host_str(), Some("chat.deepseek.com" | "accounts.google.com" | "appleid.apple.com"))
}

fn window_proxy(value: Option<&Value>) -> Result<Option<url::Url>, String> {
    use crate::server::core::proxies::{normalize_account_proxy, resolve_account_proxy, ResolvedProxy};
    let Some(value) = value.filter(|value| !value.is_null()) else { return Ok(None) };
    let config = normalize_account_proxy(value).map_err(|_| "DeepSeek 登录代理配置无效")?;
    let Some(config) = config else { return Ok(None) };
    if config["source"].as_str() == Some("direct") { return Ok(None) }
    let resolution = resolve_account_proxy(Some(&config)).ok_or("DeepSeek 登录代理无法解析")?;
    if resolution.error().is_some() { return Err("DeepSeek 登录代理不可用，请重新选择".into()) }
    let proxy = ResolvedProxy::from_json(&resolution.to_json()).map_err(|_| "DeepSeek 登录代理出口无效")?
        .ok_or("DeepSeek 登录代理出口无效")?;
    if !proxy.username.is_empty() || !proxy.password.is_empty() {
        return Err("DeepSeek 登录窗口暂不支持需要用户名密码的代理，请选择无认证代理或粘贴令牌。".into());
    }
    let scheme = match proxy.protocol.as_str() {
        "http" => "http", "socks5" => "socks5", _ => return Err("DeepSeek 登录代理协议无效".into()),
    };
    let mut url = url::Url::parse(&format!("{scheme}://localhost")).map_err(|_| "DeepSeek 登录代理地址无效")?;
    url.set_host(Some(&proxy.host)).map_err(|_| "DeepSeek 登录代理主机无效")?;
    url.set_port(proxy.port).map_err(|_| "DeepSeek 登录代理端口无效")?;
    Ok(Some(url))
}

pub async fn start(app: &AppHandle, options: &Value) -> Result<Value, String> {
    if options.get("methodIndex").and_then(Value::as_u64) != Some(1) {
        return Err("DeepSeek 网页登录方式索引无效".into());
    }
    crate::startup::wait().await?;
    let proxy = window_proxy(options.get("proxy"))?;
    let started = crate::gateway::call("POST", "/api/session/login/start", Some(&json!({
        "provider": "deepseek-web", "methodIndex": 1,
        "inputs": options.get("inputs").cloned().unwrap_or_else(|| json!({})),
        "proxy": options.get("proxy").cloned().unwrap_or(Value::Null),
    }))).await?;
    let flow = started["state"].as_str().filter(|value| !value.is_empty())
        .ok_or("后端未返回 DeepSeek 登录状态")?.to_string();
    let auth_url = started["authUrl"].as_str().unwrap_or("");
    let valid = url::Url::parse(auth_url).ok().is_some_and(|url| {
        allowed(&url) && url.host_str() == Some("chat.deepseek.com")
    });
    let result = if valid {
        crate::login::run_cookie_login_with_proxy(app, "deepseek-web", auth_url, "登录 DeepSeek 账号", &flow, proxy).await
    } else {
        Err("DeepSeek 登录地址不是官方站点".into())
    };
    if result.as_ref().map(|value| value["ok"].as_bool()) != Ok(Some(true)) {
        let _ = crate::gateway::call("POST", "/api/session/login/cancel", Some(&json!({ "state": flow }))).await;
    }
    result
}

pub async fn credentials(window: &WebviewWindow) -> Result<Option<String>, String> {
    if !window.url().ok().is_some_and(|url| allowed(&url) && url.host_str() == Some("chat.deepseek.com")) {
        return Ok(None);
    }
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    window.eval_with_callback(TOKEN_SCRIPT, move |result| {
        let _ = sender.try_send(result);
    }).map_err(|_| "读取 DeepSeek 网页登录态失败")?;
    let result = tokio::time::timeout(Duration::from_secs(5), receiver.recv()).await
        .map_err(|_| "读取 DeepSeek 网页登录态超时")?
        .ok_or("DeepSeek 网页登录态读取已结束")?;
    let value: Value = serde_json::from_str(&result).map_err(|_| "DeepSeek 网页登录态格式无效")?;
    let Some(token) = value.as_str().filter(|token| !token.is_empty() && token.len() <= 16384 && !token.contains(['\r', '\n'])) else {
        return Ok(None);
    };
    let owned = window.clone();
    let cookies = tokio::time::timeout(Duration::from_secs(10), tokio::task::spawn_blocking(move || owned.cookies()))
        .await.map_err(|_| "读取 DeepSeek Cookie 超时")?
        .map_err(|_| "读取 DeepSeek Cookie 的后台任务失败")?
        .map_err(|_| "读取 DeepSeek Cookie 失败")?;
    let now = chrono::Utc::now().timestamp();
    let rows: Vec<String> = cookies.into_iter().filter_map(|cookie| {
        let domain = cookie.domain()?.trim_start_matches('.');
        if !matches!(domain, "deepseek.com" | "chat.deepseek.com") || cookie.path().unwrap_or("/") != "/" {
            return None;
        }
        if cookie.expires_datetime().is_some_and(|expiry| expiry.unix_timestamp() <= now) {
            return None;
        }
        let name = cookie.name();
        let value = cookie.value();
        if name.is_empty() || name.contains(['\r', '\n', ';', '=']) || value.contains(['\r', '\n', ';']) {
            return None;
        }
        Some(format!("{name}={value}"))
    }).collect();
    let cookie = rows.join("; ");
    if cookie.len() > 32768 {
        return Err("DeepSeek 网页 Cookie 过大".into());
    }
    Ok(Some(json!({ "accessToken": token, "cookie": cookie }).to_string()))
}
