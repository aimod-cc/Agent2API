use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use std::time::Duration;

use serde_json::json;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use super::{login_is_current, login_window_label, DynamicNavigation};
use crate::{gateway, login_profile::LoginProfile, panel_session};

#[tauri::command]
pub async fn open_managed_login(app: AppHandle, flow: String, interaction: String) -> Result<(), String> {
    if flow.is_empty() || interaction.is_empty() || flow.len() > 256 || interaction.len() > 256
        || !interaction.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        || !login_is_current(&app, &flow) { return Err("登录步骤已失效".into()); }
    let revision = panel_session::revision();
    let path = format!("/api/session/login/interaction?state={}", url::form_urlencoded::byte_serialize(flow.as_bytes()).collect::<String>());
    let result = gateway::call_without_auth_replay("GET", &path, None).await?;
    let step = &result["interaction"];
    if !login_is_current(&app, &flow) || panel_session::revision() != revision
        || step["type"] != "managed" || step["flow"] != flow || step["id"] != interaction
        || step["status"] != "ready" || step["browser"]["browserMode"] != "embedded"
        || step["remainingMs"].as_u64().unwrap_or(0) == 0 { return Err("登录步骤已失效".into()); }
    let provider = step["provider"].as_str().ok_or("登录提供商无效")?;
    let raw = step["browser"]["url"].as_str().ok_or("登录地址无效")?;
    let auth = url::Url::parse(raw).map_err(|_| "登录地址无效")?;
    if auth.scheme() != "https" || !auth.username().is_empty() || auth.password().is_some() { return Err("登录地址无效".into()); }
    let hosts: Vec<&str> = match provider {
        "trae" => vec![
            "trae.cn",
            "trae.com.cn",
            "doubao.com",
            "bytedance.com",
            "byteimg.com",
            "bytetos.com",
            "bytegoofy.com",
            "ibytedapm.com",
            "zijieapi.com",
            "snssdk.com",
            "toutiao.com",
            "pstatp.com",
            "yhgfb-cn-static.com",
            "volces.com",
            "volcengine.com",
            "volccdn.com",
            "feishu.cn",
            "larksuite.com",
            "larkoffice.com",
            "douyin.com",
            "iesdouyin.com",
            "juejin.cn",
            "github.com",
            "accounts.google.com",
            "google.com",
            "apple.com",
            "open.weixin.qq.com",
            "weixin.qq.com",
        ],
        "kiro" => vec!["kiro.dev", "amazonaws.com", "awsapps.com", "amazon.com", "aws.amazon.com", "signin.aws", "accounts.google.com", "github.com"],
        "autoclaw-intl" => vec![
            "accounts.google.com",
            "google.com",
            "gstatic.com",
            "googleusercontent.com",
            "accounts.youtube.com",
            "z.ai",
            "bigmodel.cn",
            "zhipuai.cn",
            "chatglm.cn",
            "autoglm.ai",
            "aliyun.com",
            "alicdn.com",
            "aliyuncs.com",
            "github.com",
            "apple.com",
        ],
        "catpaw" => vec!["catpaw.meituan.com", "meituan.com", "sankuai.com", "nocode.cn", "dianping.com"],
        _ => return Err("此提供商未接通托管内嵌窗口".into()),
    };
    let mut policy = DynamicNavigation::new(provider, &auth, &json!({"allowedHosts":hosts}))?;
    if provider == "kiro" {
        if let Some(target) = policy.callbacks.first().cloned() {
            for path in ["/signin/callback", "/oauth/callback"] {
                let mut target = target.clone(); target.set_path(path); target.set_query(None); policy.callbacks.push(target);
            }
        }
    }
    if !policy.allows(&auth) { return Err("登录地址不在官方来源中".into()); }
    let label = format!("{}-{}", login_window_label(&flow), &interaction[..interaction.len().min(24)]);
    if let Some(window) = app.get_webview_window(&label) { let _ = window.set_focus(); return Ok(()); }
    let profile = LoginProfile::new()?;
    let nav_policy = policy.clone();
    let nav_app = app.clone();
    let nav_flow = flow.clone();
    let popup_app = app.clone();
    let popup_flow = flow.clone();
    let popup_label = label.clone();
    let window = WebviewWindowBuilder::new(&app, label.clone(), WebviewUrl::External(auth))
        .data_directory(profile.path().to_path_buf()).general_autofill_enabled(false)
        .title(format!("登录 {} 账号", crate::server::core::providers::label_of(provider)))
        .inner_size(1100.0, 820.0).min_inner_size(760.0, 560.0).center()
        .on_navigation(move |url| {
            panel_session::revision() == revision && login_is_current(&nav_app, &nav_flow)
                && nav_policy.allows(url)
        })
        .on_new_window(move |url, _| {
            if panel_session::revision() != revision || !login_is_current(&popup_app, &popup_flow) {
                return tauri::webview::NewWindowResponse::Deny;
            }
            if policy.allows(&url) {
                let handle = popup_app.clone();
                let label = popup_label.clone();
                tauri::async_runtime::spawn(async move {
                    if let Some(window) = handle.get_webview_window(&label) { let _ = window.navigate(url); }
                });
            }
            tauri::webview::NewWindowResponse::Deny
        }).build().map_err(|_| "打开内嵌登录窗口失败")?;
    let closed = Arc::new(AtomicBool::new(false));
    let user_closed = closed.clone();
    let close_flow = flow.clone();
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::CloseRequested { .. }) && !user_closed.swap(true, Ordering::AcqRel)
            && panel_session::revision() == revision {
            let body = json!({"state":close_flow});
            tauri::async_runtime::spawn(async move {
                let _ = gateway::call_without_auth_replay("POST", "/api/session/login/cancel", Some(&body)).await;
            });
        }
    });
    let monitor_flow = step["flow"].as_str().ok_or("登录步骤无效")?.to_string();
    let monitor_step = step["id"].as_str().ok_or("登录步骤无效")?.to_string();
    let remaining = step["remainingMs"].as_u64().unwrap_or(0).min(900_000);
    tauri::async_runtime::spawn(async move {
        let _profile = profile;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(remaining);
        loop {
            if !login_is_current(&app, &monitor_flow) || panel_session::revision() != revision
                || tokio::time::Instant::now() >= deadline || app.get_webview_window(&label).is_none() { break; }
            match gateway::call_without_auth_replay("GET", &path, None).await {
                Ok(value) if value["interaction"]["id"] == monitor_step => {},
                _ => break,
            }
            tokio::time::sleep(Duration::from_millis(750)).await;
        }
        closed.store(true, Ordering::Release);
        let _ = window.close();
    });
    Ok(())
}
