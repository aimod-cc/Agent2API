use std::time::Duration;

use agent2api_server::server::core::providers::monkeycode::Region;
use serde_json::{json, Value};
use tauri::{AppHandle, WebviewWindow};

const SESSION_COOKIE: &str = "monkeycode_ai_session";

fn site_host(region: Region) -> &'static str {
    match region {
        Region::Cn => "monkeycode-ai.com",
        Region::Intl => "monkeycode-ai.net",
    }
}

pub fn allowed(url: &url::Url, region: Region) -> bool {
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    if host == site_host(region) {
        return true;
    }
    match region {
        Region::Cn => host == "baizhi.cloud" || host.ends_with(".baizhi.cloud"),
        Region::Intl => matches!(host, "github.com" | "accounts.google.com"),
    }
}

pub async fn start(app: &AppHandle, region: Region, mode: &str) -> Result<Value, String> {
    if mode == "external" {
        return Err(
            "MonkeyCode 网页登录需要桌面内嵌窗口；系统浏览器登录后请使用「填写凭证」粘贴 session。"
                .into(),
        );
    }
    let started = crate::gateway::call(
        "POST",
        "/api/session/login/start",
        Some(&json!({"provider": region.provider_id()})),
    )
    .await?;
    let flow = started["state"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or("后端未返回登录状态")?;
    let result = async {
        let auth_url = started["authUrl"].as_str().ok_or("后端未返回登录链接")?;
        let url = url::Url::parse(auth_url).map_err(|_| "MonkeyCode 登录链接无效")?;
        if !allowed(&url, region) || url.host_str() != Some(site_host(region)) {
            return Err("MonkeyCode 登录链接与所选站点不符".into());
        }
        crate::login::run_cookie_login(
            app,
            region.provider_id(),
            auth_url,
            &format!("登录 MonkeyCode {}账号", region.label()),
            flow,
        )
        .await
    }
    .await;
    if result.as_ref().map(|value| value["ok"].as_bool()) != Ok(Some(true)) {
        let _ = crate::gateway::call(
            "POST",
            "/api/session/login/cancel",
            Some(&json!({"state": flow})),
        )
        .await;
    }
    result
}

pub async fn credentials(window: &WebviewWindow, region: Region) -> Result<Option<String>, String> {
    let url = window
        .url()
        .map_err(|_| "读取 MonkeyCode 登录窗口地址失败")?;
    if !allowed(&url, region) || url.host_str() != Some(site_host(region)) {
        return Ok(None);
    }
    let owned = window.clone();
    // HttpOnly 凭证只能由窗口宿主读取；Windows 上不能在消息线程同步等待 Cookie。
    let cookies = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::task::spawn_blocking(move || owned.cookies()),
    )
    .await
    .map_err(|_| "读取 MonkeyCode 登录凭证超时")?
    .map_err(|_| "读取 MonkeyCode 登录凭证的后台任务失败")?
    .map_err(|_| "读取 MonkeyCode 登录凭证失败")?;
    let now = chrono::Utc::now().timestamp();
    let session = cookies.iter().find(|cookie| {
        cookie.name() == SESSION_COOKIE
            && cookie
                .domain()
                .is_some_and(|domain| domain.trim_start_matches('.') == site_host(region))
            && cookie.path().unwrap_or("/") == "/"
            && !cookie.value().is_empty()
            && !cookie
                .expires_datetime()
                .is_some_and(|expiry| expiry.unix_timestamp() <= now)
    });
    Ok(session.map(|cookie| {
        json!({
            "session": cookie.value(),
            "expiresAt": cookie.expires_datetime().map(|expiry| expiry.unix_timestamp() * 1000),
        })
        .to_string()
    }))
}
