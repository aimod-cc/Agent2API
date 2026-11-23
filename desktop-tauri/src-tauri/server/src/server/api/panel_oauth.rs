//! 管理面板 OAuth 第三方登录与账号绑定接口（`/api/panel/oauth/*`）。

use std::collections::HashMap;
use std::net::SocketAddr;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, Query};
use axum::http::{
    header::{CACHE_CONTROL, CONTENT_TYPE, LOCATION, SET_COOKIE},
    HeaderMap, HeaderValue, StatusCode,
};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::server::core::panel_oauth::{
    self,
    flow::{self, OauthMode, OauthProvider},
    PanelOauthSettings,
};
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;

fn infer_request_origin(headers: &HeaderMap, query_origin: Option<&str>) -> String {
    if let Some(origin) = query_origin.map(str::trim).filter(|s| !s.is_empty()) {
        if origin.starts_with("http://") || origin.starts_with("https://") {
            return origin.trim_end_matches('/').to_string();
        }
    }
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .filter(|s| matches!(*s, "http" | "https"))
        .unwrap_or("http");
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get("host"))
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .unwrap_or("127.0.0.1:3065");
    format!("{proto}://{host}")
}

fn url_encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for &byte in value.as_bytes() {
        let ch = byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~') {
            out.push(ch);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn redirect_to(location: &str, cookie: Option<&str>) -> Response {
    let mut resp = (StatusCode::FOUND, "").into_response();
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(val) = HeaderValue::from_str(location) {
        resp.headers_mut().insert(LOCATION, val);
    }
    if let Some(cookie_str) = cookie {
        if let Ok(val) = HeaderValue::from_str(cookie_str) {
            resp.headers_mut().append(SET_COOKIE, val);
        }
    }
    resp
}

fn html_popup_result(success: bool, title: &str, detail: &str) -> Response {
    let color = if success { "#22c55e" } else { "#ef4444" };
    let icon = if success { "✓" } else { "✕" };
    let escaped_title = title
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let escaped_detail = detail
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let html = format!(
        r#"<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>{escaped_title}</title>
  <style>
    body {{
      margin: 0; min-height: 100vh; display: flex; align-items: center; justify-content: center;
      background: #0f1115; color: #e5e7eb;
      font: 14px/1.6 system-ui, -apple-system, "Segoe UI", "Microsoft YaHei", sans-serif;
    }}
    .card {{
      background: #181b22; border: 1px solid #262a34; border-radius: 12px;
      padding: 28px 32px; max-width: 380px; width: calc(100% - 40px); text-align: center;
      box-shadow: 0 12px 32px rgba(0,0,0,0.45);
    }}
    .badge {{
      width: 44px; height: 44px; border-radius: 999px; margin: 0 auto 14px;
      display: flex; align-items: center; justify-content: center;
      font-size: 22px; font-weight: bold; color: #fff; background: {color};
    }}
    h1 {{ font-size: 17px; margin: 0 0 8px; }}
    p {{ font-size: 13px; color: #9ca3af; margin: 0; word-break: break-word; }}
  </style>
</head>
<body>
  <div class="card">
    <div class="badge">{icon}</div>
    <h1>{escaped_title}</h1>
    <p>{escaped_detail}</p>
  </div>
  <script>
    (function() {{
      try {{
        if (window.opener && !window.opener.closed) {{
          window.opener.postMessage({{ type: 'agent2api-oauth-done', success: {success} }}, '*');
        }}
      }} catch (e) {{}}
      if ({success}) {{
        setTimeout(function() {{ try {{ window.close(); }} catch (e) {{}} }}, 1200);
      }}
    }})();
  </script>
</body>
</html>"#
    );
    let mut resp = (StatusCode::OK, html).into_response();
    resp.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

/// `GET /api/panel/oauth/settings`（需鉴权）
pub async fn get_oauth_settings() -> Response {
    ok_json(PanelOauthSettings::current().management_json())
}

/// `PUT /api/panel/oauth/settings`（需鉴权）
pub async fn put_oauth_settings(body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    let settings = match panel_oauth::save(object) {
        Ok(settings) => settings,
        Err((status, message)) => return errors::management_error(status, message),
    };
    logging::log(
        "[Security]",
        &format!(
            "第三方登录设置已更新：Google={}，GitHub={}",
            if settings.google.enabled { "开启" } else { "关闭" },
            if settings.github.enabled { "开启" } else { "关闭" },
        ),
    );
    ok_json(settings.management_json())
}

/// `POST /api/panel/oauth/bind-url`（需鉴权：在设置页点击「跳转绑定当前账号」）
pub async fn post_oauth_bind_url(headers: HeaderMap, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    // 若前端携带了当前未保存的 Client ID / Secret 草稿，先一并落盘
    if let Some(patch) = object.get("patch").and_then(|v| v.as_object()) {
        if let Err((status, message)) = panel_oauth::save(patch) {
            return errors::management_error(status, message);
        }
    }
    let provider_raw = object
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let Some(provider) = OauthProvider::parse(provider_raw) else {
        return errors::management_error(400, "请选择 google 或 github");
    };
    let origin = infer_request_origin(&headers, object.get("origin").and_then(|v| v.as_str()));
    match flow::start_flow(provider, OauthMode::Bind, true, "", &origin) {
        Ok((state, authorize_url)) => ok_json(json!({
            "state": state,
            "authorizeUrl": authorize_url,
        })),
        Err(msg) => errors::management_error(400, msg),
    }
}

/// `POST /api/panel/oauth/start-url`（免鉴权：供桌面端 / 移动端弹出系统浏览器并轮询登录结果）
pub async fn post_oauth_start_url(headers: HeaderMap, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(object) = payload.as_object() else {
        return errors::management_error(400, "请求体必须是 JSON 对象");
    };
    let provider_raw = object
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let Some(provider) = OauthProvider::parse(provider_raw) else {
        return errors::management_error(400, "请选择 google 或 github");
    };
    let next = object.get("next").and_then(|v| v.as_str()).unwrap_or("");
    let origin = infer_request_origin(&headers, object.get("origin").and_then(|v| v.as_str()));
    match flow::start_flow(provider, OauthMode::Login, true, next, &origin) {
        Ok((state, authorize_url)) => ok_json(json!({
            "state": state,
            "authorizeUrl": authorize_url,
        })),
        Err(msg) => errors::management_error(400, msg),
    }
}

/// `GET /api/panel/oauth/start`（免鉴权：网页端同标签页跳转登录）
pub async fn get_oauth_start(
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let provider_raw = query.get("provider").map(String::as_str).unwrap_or("");
    let next = query.get("next").map(String::as_str).unwrap_or("");
    let popup = query
        .get("popup")
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    let next_suffix = if next == "/m" { "&next=%2Fm" } else { "" };

    let Some(provider) = OauthProvider::parse(provider_raw) else {
        return redirect_to(
            &format!("/login?oauth_error={}{next_suffix}", url_encode_query("不支持的第三方登录提供商")),
            None,
        );
    };
    let origin = infer_request_origin(&headers, query.get("origin").map(String::as_str));
    match flow::start_flow(provider, OauthMode::Login, popup, next, &origin) {
        Ok((_, authorize_url)) => redirect_to(&authorize_url, None),
        Err(msg) => {
            if popup {
                html_popup_result(false, "无法发起授权", &msg)
            } else {
                redirect_to(
                    &format!("/login?oauth_error={}{next_suffix}", url_encode_query(&msg)),
                    None,
                )
            }
        }
    }
}

/// `GET /api/panel/oauth/poll?state=...`（免鉴权：查询弹窗 / 外部浏览器授权状态）
pub async fn get_oauth_poll(Query(query): Query<HashMap<String, String>>) -> Response {
    let state = query.get("state").map(String::as_str).unwrap_or("").trim();
    if state.is_empty() {
        return errors::management_error(400, "缺少 state 参数");
    }
    ok_json(flow::poll_state(state))
}

/// `GET /api/panel/oauth/callback`（免鉴权：Google / GitHub 授权回调）
pub async fn get_oauth_callback(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let state = query.get("state").map(String::as_str).unwrap_or("").trim();
    let code = query.get("code").map(String::as_str);
    let error = query
        .get("error_description")
        .or_else(|| query.get("error"))
        .map(String::as_str);
    let ip = addr.ip().to_string();
    let result = flow::complete_callback(state, code, error, &ip).await;
    let next_suffix = if result.next == "/m" { "&next=%2Fm" } else { "" };

    if result.mode == OauthMode::Bind {
        return match result.error {
            None => html_popup_result(
                true,
                &format!("{} 账号绑定成功", result.provider_label),
                &format!(
                    "已绑定账号：{}，此窗口即将自动关闭。",
                    result.identity.as_deref().unwrap_or("已授权账号")
                ),
            ),
            Some(err) => html_popup_result(
                false,
                &format!("{} 账号绑定失败", result.provider_label),
                &err,
            ),
        };
    }

    if result.popup {
        return match result.error {
            None => html_popup_result(
                true,
                &format!("{} 登录成功", result.provider_label),
                "授权已完成，请返回 Agent2API 窗口，此页面即将自动关闭。",
            ),
            Some(err) => html_popup_result(
                false,
                &format!("{} 登录失败", result.provider_label),
                &err,
            ),
        };
    }

    match (result.session, result.ticket, result.error) {
        (Some(session), Some(ticket), None) => redirect_to(
            &format!("/login?oauth_ticket={}{next_suffix}", url_encode_query(&ticket)),
            Some(&session.session_cookie),
        ),
        (_, _, Some(err)) => redirect_to(
            &format!("/login?oauth_error={}{next_suffix}", url_encode_query(&err)),
            None,
        ),
        _ => redirect_to(
            &format!("/login?oauth_error={}{next_suffix}", url_encode_query("登录验证未完成")),
            None,
        ),
    }
}

/// `POST /api/panel/oauth/exchange`（免鉴权：用一次性票据换取会话 Cookie 与双令牌）
pub async fn post_oauth_exchange(body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let ticket = payload
        .get("ticket")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if ticket.is_empty() {
        return errors::management_error(400, "缺少登录票据");
    }
    let Some(session) = flow::take_ticket(ticket) else {
        return errors::management_error(401, "登录票据已过期或已使用，请重新登录");
    };
    let mut response = ok_json(json!({
        "loggedIn": true,
        "accessToken": session.access_token,
        "refreshToken": session.refresh_token,
        "expiresIn": session.expires_in,
    }));
    if let Ok(value) = HeaderValue::from_str(&session.session_cookie) {
        response.headers_mut().append(SET_COOKIE, value);
    }
    response
}
