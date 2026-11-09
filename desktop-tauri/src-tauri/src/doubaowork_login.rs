use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, WebviewWindow};

const DOMAINS: &[&str] = &["doubao.com", "feishu.cn", "larksuite.com"];

pub fn allowed(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url.host_str().is_some_and(|host| {
            DOMAINS
                .iter()
                .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
        })
}

pub async fn start(app: &AppHandle, mode: &str, proxy: Option<Value>) -> Result<Value, String> {
    if mode == "external" {
        return Err("豆包工作网页登录只支持内嵌窗口；系统浏览器登录后请粘贴完整 Cookie。".into());
    }
    let started = crate::login::start_provider_login("doubaowork", proxy).await?;
    let flow = started["state"]
        .as_str()
        .ok_or("后端未返回登录状态")?
        .to_string();
    let url = started["authUrl"]
        .as_str()
        .ok_or("后端未返回登录链接")?
        .to_string();
    let parsed = url::Url::parse(&url).map_err(|_| "豆包工作登录链接无效")?;
    if !allowed(&parsed) || parsed.host_str() != Some("www.doubao.com") {
        return Err("豆包工作登录链接不是官方站点".into());
    }
    let result =
        crate::login::run_cookie_login(app, "doubaowork", &url, "登录豆包工作账号", &flow).await;
    if result.as_ref().map(|value| value["ok"].as_bool()) != Ok(Some(true)) {
        let _ = crate::gateway::call(
            "POST",
            "/api/session/login/cancel",
            Some(&json!({"state":flow})),
        )
        .await;
    }
    result
}

pub async fn cookie(window: &WebviewWindow) -> Result<Option<String>, String> {
    let owned = window.clone();
    let cookies = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::task::spawn_blocking(move || owned.cookies()),
    )
    .await
    .map_err(|_| "读取豆包工作登录窗口 Cookie 超时")?
    .map_err(|_| "读取豆包工作 Cookie 的后台任务失败")?
    .map_err(|_| "读取豆包工作登录窗口 Cookie 失败")?;
    let now = chrono::Utc::now().timestamp();
    let rows: Vec<Value> = cookies.into_iter().filter_map(|cookie| {
        let domain = cookie.domain()?.trim_start_matches('.');
        if !matches!(domain, "doubao.com" | "www.doubao.com") || cookie.path().unwrap_or("/") != "/" { return None; }
        let expires = cookie.expires_datetime().map(|expiry| expiry.unix_timestamp());
        if expires.is_some_and(|expires| expires <= now) { return None; }
        Some(json!({"name":cookie.name(),"value":cookie.value(),"domain":format!(".{domain}"),"path":"/","expires":expires}))
    }).collect();
    if !rows.iter().any(|cookie| {
        matches!(cookie["name"].as_str(), Some("sessionid" | "sessionid_ss"))
            && cookie["value"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
    }) {
        return Ok(None);
    }
    Ok(Some(json!({"cookies":rows}).to_string()))
}
