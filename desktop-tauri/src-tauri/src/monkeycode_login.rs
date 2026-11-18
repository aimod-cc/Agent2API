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

pub async fn start(app: &AppHandle, region: Region, mode: &str, proxy: Option<Value>) -> Result<Value, String> {
    if mode == "external" {
        return Err(
            "MonkeyCode 网页登录需要桌面内嵌窗口；系统浏览器登录后请使用「填写凭证」粘贴 session。"
                .into(),
        );
    }
    let started = crate::login::start_provider_login(region.provider_id(), proxy).await?;
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
    let cookies = crate::login::read_window_cookies(window).await?;
    if cookies.len() > 2048 {
        return Err("MonkeyCode 登录窗口 Cookie 数量超过读取上限".into());
    }
    let now = chrono::Utc::now().timestamp();
    let mut captured: Option<(&str, Option<i64>)> = None;
    for cookie in &cookies {
        if cookie.name() != SESSION_COOKIE
            || !cookie.domain().is_some_and(|domain| {
                domain.trim_start_matches('.').eq_ignore_ascii_case(site_host(region))
            })
            || cookie.path().unwrap_or("/") != "/"
            || cookie.value().is_empty()
            || cookie.expires_datetime().is_some_and(|expiry| expiry.unix_timestamp() <= now)
        {
            continue;
        }
        let value = cookie.value();
        if value.len() > 8192 || !value.bytes().all(|byte| {
            matches!(byte, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e)
        }) {
            return Err("MonkeyCode 登录窗口的 Session 格式无效".into());
        }
        let expiry = cookie.expires_datetime().and_then(|expiry| {
            expiry.unix_timestamp().checked_mul(1000)
        });
        if captured.as_ref().is_some_and(|(previous, _)| *previous != value) {
            return Err("MonkeyCode 登录窗口出现不同 Session，请重新发起登录".into());
        }
        captured = Some((value, expiry));
    }
    // 读取后再次核对本家地址；不从第三方 SSO 页面交付凭据。
    let current = window.url().map_err(|_| "读取 MonkeyCode 登录窗口地址失败")?;
    if !allowed(&current, region) || current.host_str() != Some(site_host(region)) {
        return Ok(None);
    }
    Ok(captured.map(|(session, expiry)| {
        json!({ "session": session, "expiresAt": expiry }).to_string()
    }))
}
