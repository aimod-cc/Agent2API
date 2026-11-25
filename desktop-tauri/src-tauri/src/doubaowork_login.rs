use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, WebviewWindow};

const DOMAINS: &[&str] = &["doubao.com", "feishu.cn", "larksuite.com"];
const CONTEXT_SCRIPT: &str = r#"(() => {
  if (location.origin !== 'https://www.doubao.com') return null;
  const script = document.getElementById('agent2api-doubao-login-context');
  const value = script && script.textContent;
  if (value && value.length > 8192) return null;
  try {
    const context = value ? JSON.parse(value) : {};
    if (!context.webId) {
      try {
        const saved = JSON.parse(localStorage.getItem('samantha_web_web_id') || '{}');
        if (typeof saved.web_id === 'string' && saved.web_id.length <= 1024) context.webId = saved.web_id;
      } catch (_) {}
    }
    return { ...context, userAgent: navigator.userAgent };
  } catch (_) { return null; }
})()"#;

pub(crate) fn capture_script() -> &'static str {
    r#"(() => {
      if (location.origin !== 'https://www.doubao.com') return;
      const save = raw => {
        try {
          const url = new URL(raw, location.href);
          if (url.origin !== location.origin || !/^\/(?:chat|alice|account|im)(?:\/|$)/.test(url.pathname)) return;
          const fields = { device_id: 'deviceId', web_id: 'webId', fp: 'fingerprint', tea_uuid: 'teaUuid', web_tab_id: 'webTabId',
            pc_version: 'clientVersion', doubao_pc_version: 'clientVersion', channel: 'channel', chromium_version: 'chromiumVersion' };
          const id = 'agent2api-doubao-login-context';
          let node = document.getElementById(id);
          let stored = {};
          if (node) { try { stored = JSON.parse(node.textContent || '{}'); } catch (_) {} }
          for (const [key, name] of Object.entries(fields)) {
            const value = url.searchParams.get(key);
            if (value && value.length <= 1024 && !/[\r\n;]/.test(value)) stored[name] = value;
          }
          // 初始化脚本可能早于documentElement；内存保留当前官方请求，DOM就绪后再投影。
          if (!Object.keys(stored).length) return;
          const persist = () => {
            if (!document.documentElement) return;
            let target = document.getElementById(id);
            if (!target) { target = document.createElement('script'); target.id = id; target.type = 'application/json'; document.documentElement.appendChild(target); }
            let latest = {};
            try { latest = JSON.parse(target.textContent || '{}'); } catch (_) {}
            target.textContent = JSON.stringify({ ...latest, ...stored });
          };
          if (document.documentElement) {
            if (!node) { node = document.createElement('script'); node.id = id; node.type = 'application/json'; document.documentElement.appendChild(node); }
            node.textContent = JSON.stringify(stored);
          } else document.addEventListener('DOMContentLoaded', persist, { once: true });
        } catch (_) {}
      };
      const originalFetch = window.fetch;
      window.fetch = function(input, ...args) {
        save(typeof input === 'string' ? input : input && input.url);
        return Reflect.apply(originalFetch, this, [input, ...args]);
      };
      const originalOpen = XMLHttpRequest.prototype.open;
      XMLHttpRequest.prototype.open = function(method, url, ...args) {
        save(url);
        return Reflect.apply(originalOpen, this, [method, url, ...args]);
      };
    })()"#
}

async fn page_context(window: &WebviewWindow) -> Result<Value, String> {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    window.eval_with_callback(CONTEXT_SCRIPT, move |result| {
        let _ = sender.try_send(result);
    }).map_err(|_| "读取豆包工作登录窗口设备信息失败")?;
    let result = tokio::time::timeout(Duration::from_secs(5), receiver.recv()).await
        .map_err(|_| "读取豆包工作登录窗口设备信息超时")?
        .ok_or("豆包工作登录窗口已关闭")?;
    serde_json::from_str(&result).map_err(|_| "豆包工作登录窗口设备信息无效".into())
}

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
    if !window.url().ok().is_some_and(|url| allowed(&url) && url.host_str() == Some("www.doubao.com")) {
        return Ok(None);
    }
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
    if rows.len() > 512 { return Err("豆包工作登录窗口 Cookie 数量异常".into()); }
    if !rows.iter().any(|cookie| {
        matches!(cookie["name"].as_str(), Some("sessionid" | "sessionid_ss"))
            && cookie["value"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
    }) {
        return Ok(None);
    }
    let is_numeric_id = |text: &str| {
        !text.is_empty()
            && text.len() <= 32
            && text.bytes().all(|byte| byte.is_ascii_digit())
            && text.bytes().any(|byte| byte != b'0')
    };
    let mut value = json!({"cookies":rows});
    if let Ok(captured) = page_context(window).await {
        for key in ["deviceId", "webId", "fingerprint", "teaUuid", "webTabId", "userAgent", "clientVersion", "channel", "chromiumVersion"] {
            if let Some(text) = captured[key].as_str().filter(|text| !text.is_empty() && text.len() <= 1024
                && text.bytes().all(|byte| (32..=126).contains(&byte)) && !text.contains(';')) {
                if key == "deviceId" && !is_numeric_id(text) {
                    continue;
                }
                value[key] = json!(text);
            }
        }
    }
    if value["deviceId"].is_null() {
        if let Some(device) = value["cookies"].as_array().into_iter().flatten()
            .find(|cookie| cookie["name"] == "device_id")
            .and_then(|cookie| cookie["value"].as_str())
            .filter(|device| is_numeric_id(device))
            .map(str::to_string) {
            value["deviceId"] = json!(device);
        }
    }
    if value["deviceId"].is_null() {
        if let Some(web_id) = value["webId"].as_str().filter(|id| is_numeric_id(id)).map(str::to_string) {
            value["deviceId"] = json!(web_id);
        }
    }
    Ok(Some(value.to_string()))
}
