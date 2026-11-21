//! 统一网页登录回调结果页（零脚本、零外链、严格 CSP）。
//!
//! 所有直接呈现给浏览器的 OAuth / 网页登录回调（网关 HTTP 路由与各路本机回环监听）
//! 共用同一套居中卡片排版与自适应浅/深色样式，并按当前登录结果区分三种状态：
//! - `Success`：已完成授权与账号保存，可安全关闭页面；
//! - `Accepted`：回调已接收并转交后台/插件继续处理，需返回面板等待最终结果；
//! - `Error`：回调被拒绝、参数无效、流程过期或换取凭证失败。

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use super::LoginCallbackSubmission;
use crate::server::core::branding::DEFAULT_NAME;
use crate::server::core::providers;
use crate::server::errors::GatewayError;

const CSP_HEADER: &str = "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'";

#[derive(Clone, Copy)]
enum CallbackKind {
    Success,
    Accepted,
    Error,
}

impl CallbackKind {
    fn tone_class(self) -> &'static str {
        match self {
            Self::Success => "tone-success",
            Self::Accepted => "tone-info",
            Self::Error => "tone-error",
        }
    }

    fn icon_svg(self) -> &'static str {
        match self {
            Self::Success => {
                "<svg width=\"22\" height=\"22\" viewBox=\"0 0 24 24\" fill=\"none\" \
                 stroke=\"currentColor\" stroke-width=\"2.3\" stroke-linecap=\"round\" \
                 stroke-linejoin=\"round\" aria-hidden=\"true\"><path d=\"M20 6 9 17l-5-5\"/></svg>"
            }
            Self::Accepted => {
                "<svg width=\"22\" height=\"22\" viewBox=\"0 0 24 24\" fill=\"none\" \
                 stroke=\"currentColor\" stroke-width=\"2.1\" stroke-linecap=\"round\" \
                 stroke-linejoin=\"round\" aria-hidden=\"true\">\
                 <path d=\"M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8\"/>\
                 <path d=\"M3 3v5h5\"/>\
                 <path d=\"M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16\"/>\
                 <path d=\"M16 16h5v5\"/></svg>"
            }
            Self::Error => {
                "<svg width=\"22\" height=\"22\" viewBox=\"0 0 24 24\" fill=\"none\" \
                 stroke=\"currentColor\" stroke-width=\"2.2\" stroke-linecap=\"round\" \
                 stroke-linejoin=\"round\" aria-hidden=\"true\">\
                 <circle cx=\"12\" cy=\"12\" r=\"9\"/>\
                 <path d=\"M12 8v5\"/><path d=\"M12 16h.01\"/></svg>"
            }
        }
    }
}

struct CallbackPage<'a> {
    kind: CallbackKind,
    provider: Option<&'a str>,
    title: &'a str,
    desc: &'a str,
    hint: &'a str,
}

fn success_page(provider: Option<&str>) -> CallbackPage<'_> {
    CallbackPage {
        kind: CallbackKind::Success,
        provider,
        title: "登录成功",
        desc: "已完成授权并将账号保存到网关，您可以关闭此页面并返回面板继续使用。",
        hint: "账号已就绪 · 可安全关闭当前标签页",
    }
}

fn accepted_page(provider: Option<&str>) -> CallbackPage<'_> {
    CallbackPage {
        kind: CallbackKind::Accepted,
        provider,
        title: "授权回调已接收",
        desc: "正在后台为您核验凭证并完成保存，请返回 Agent2API 面板查看最终结果。",
        hint: "登录仍在面板中完成 · 此页面可先关闭",
    }
}

/// 返回「登录成功」的 Axum HTML 响应。
pub(crate) fn success_response(provider: Option<&str>) -> Response {
    axum_response(200, &success_page(provider))
}

/// 返回「登录流程已过期」的 Axum HTML 响应。
pub(crate) fn expired_response(provider: Option<&str>) -> Response {
    axum_response(
        404,
        &CallbackPage {
            kind: CallbackKind::Error,
            provider,
            title: "登录流程已过期",
            desc: "当前登录链接已失效或已使用，请返回 Agent2API 面板重新发起网页登录。",
            hint: "状态：登录流程不存在或已过期",
        },
    )
}

/// 将 `submit_login_callback` 的结果统一渲染为 Axum HTML 响应。
pub(crate) fn submission_response(
    result: Result<LoginCallbackSubmission, GatewayError>,
    provider: Option<&str>,
) -> Response {
    match result {
        Ok(LoginCallbackSubmission::Completed(_)) => success_response(provider),
        Ok(LoginCallbackSubmission::Accepted) => axum_response(200, &accepted_page(provider)),
        Ok(LoginCallbackSubmission::Retry(message)) => {
            let hint = format_reason_hint(&message);
            axum_response(
                400,
                &CallbackPage {
                    kind: CallbackKind::Error,
                    provider,
                    title: "登录未完成",
                    desc: "本次授权未能完成，请返回 Agent2API 面板查看提示后重试。",
                    hint: &hint,
                },
            )
        }
        Ok(LoginCallbackSubmission::ContinueTo(_)) => axum_response(
            400,
            &CallbackPage {
                kind: CallbackKind::Error,
                provider,
                title: "登录未完成",
                desc: "登录回调仍需继续，请返回 Agent2API 面板重新发起网页登录。",
                hint: "状态：当前页面无法自动完成下一步授权",
            },
        ),
        Err(error) => error_response(error.status_code, provider, &error.message),
    }
}

/// 将失败原因渲染为统一 Axum HTML 响应。
pub(crate) fn error_response(status_code: i32, provider: Option<&str>, reason: &str) -> Response {
    let title = if matches!(status_code, 404 | 410) {
        "登录流程已过期"
    } else {
        "登录未完成"
    };
    let hint = format_reason_hint(reason);
    axum_response(
        status_code,
        &CallbackPage {
            kind: CallbackKind::Error,
            provider,
            title,
            desc: "本次授权未能完成，请返回 Agent2API 面板重试。",
            hint: &hint,
        },
    )
}

/// 为本机回环回调监听器生成完整的 HTTP/1.1 HTML 响应报文（已接收 / 未接受）。
pub(crate) fn raw_receipt_response(
    accepted: bool,
    provider: &str,
    rejected_reason: &str,
) -> String {
    if accepted {
        let body = render_html(&accepted_page(Some(provider)));
        return raw_http_html(200, "OK", &body);
    }
    let hint = format_reason_hint(rejected_reason);
    let body = render_html(&CallbackPage {
        kind: CallbackKind::Error,
        provider: Some(provider),
        title: "登录未完成",
        desc: "本次回调未通过校验，请返回 Agent2API 面板检查当前登录步骤。",
        hint: &hint,
    });
    raw_http_html(400, "Bad Request", &body)
}

fn axum_response(status_code: i32, page: &CallbackPage<'_>) -> Response {
    let status = u16::try_from(status_code)
        .ok()
        .and_then(|code| StatusCode::from_u16(code).ok())
        .unwrap_or(StatusCode::OK);
    let body = render_html(page);
    let mut response = (status, body).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP_HEADER));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response
}

fn raw_http_html(status_code: u16, reason: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status_code} {reason}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Referrer-Policy: no-referrer\r\n\
         Content-Security-Policy: {CSP_HEADER}\r\n\
         X-Content-Type-Options: nosniff\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    )
}

fn render_html(page: &CallbackPage<'_>) -> String {
    let brand_name = escape_html(DEFAULT_NAME);
    let title = escape_html(page.title);
    let desc = escape_html(page.desc);
    let hint = escape_html(page.hint);
    let tone = page.kind.tone_class();
    let icon = page.kind.icon_svg();
    let provider_html = resolve_provider_label(page.provider)
        .map(|label| {
            format!(
                "<span class=\"brand-sep\" aria-hidden=\"true\">·</span>\
                 <span class=\"brand-provider\">{}</span>",
                escape_html(&label)
            )
        })
        .unwrap_or_default();

    format!(
        "<!doctype html>\
<html lang=\"zh-CN\">\
<head>\
<meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<meta name=\"color-scheme\" content=\"light dark\">\
<title>{brand_name} · {title}</title>\
<style>{PAGE_CSS}</style>\
</head>\
<body>\
<main class=\"card {tone}\" role=\"main\">\
<div class=\"brand\">\
<svg width=\"22\" height=\"22\" viewBox=\"0 0 24 24\" aria-hidden=\"true\" class=\"brand-logo\">\
<rect width=\"24\" height=\"24\" rx=\"5.4\" fill=\"#007AFF\"/>\
<g fill=\"none\" stroke=\"#fff\" stroke-width=\"2.2\" stroke-linecap=\"round\" stroke-linejoin=\"round\">\
<path d=\"M5.23 9.24h10.71\"/>\
<path d=\"M15.94 6.72 18.77 9.24 15.94 11.76\"/>\
<path d=\"M18.77 14.76H8.06\"/>\
<path d=\"M8.06 12.24 5.23 14.76 8.06 17.28\"/>\
</g>\
</svg>\
<span class=\"brand-name\">{brand_name}</span>{provider_html}\
</div>\
<div class=\"icon\">{icon}</div>\
<h1>{title}</h1>\
<p class=\"desc\">{desc}</p>\
<div class=\"hint\"><span class=\"dot\" aria-hidden=\"true\"></span><span>{hint}</span></div>\
</main>\
</body>\
</html>"
    )
}

fn resolve_provider_label(provider: Option<&str>) -> Option<String> {
    let raw = provider?.trim();
    if raw.is_empty() {
        return None;
    }
    let label = providers::label_of(raw);
    let trimmed = label.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn format_reason_hint(message: &str) -> String {
    let trimmed = message.trim();
    let stripped = trimmed
        .strip_prefix("登录失败：")
        .or_else(|| trimmed.strip_prefix("登录失败:"))
        .or_else(|| trimmed.strip_prefix("原因："))
        .or_else(|| trimmed.strip_prefix("原因:"))
        .unwrap_or(trimmed)
        .trim();
    if stripped.is_empty() {
        "请返回 Agent2API 面板查看详情并重试".to_string()
    } else {
        format!("原因：{stripped}")
    }
}

fn escape_html(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

const PAGE_CSS: &str = "\
*,*::before,*::after{box-sizing:border-box}\
html,body{margin:0;padding:0;min-height:100%}\
body{\
min-height:100vh;min-height:100dvh;\
display:flex;align-items:center;justify-content:center;padding:20px;\
font:14px/1.6 'Segoe UI','Microsoft YaHei','Microsoft YaHei UI',system-ui,-apple-system,sans-serif;\
color:#0F172A;\
background:radial-gradient(900px 380px at 50% -10%,rgba(0,122,255,.13),transparent 65%),#F6F7FB;\
-webkit-font-smoothing:antialiased\
}\
.card{\
width:100%;max-width:400px;padding:28px 28px 24px;border-radius:18px;text-align:center;\
background:#FFFFFF;border:1px solid rgba(15,23,42,.09);\
box-shadow:0 1px 3px rgba(15,23,42,.06),0 18px 40px -18px rgba(15,23,42,.18)\
}\
.brand{\
display:inline-flex;align-items:center;justify-content:center;flex-wrap:wrap;gap:7px;\
margin-bottom:18px;max-width:100%\
}\
.brand-logo{display:block;flex-shrink:0}\
.brand-name{font-size:14px;font-weight:600;letter-spacing:-.01em;color:#0F172A}\
.brand-sep{font-size:13px;color:#94A3B8}\
.brand-provider{font-size:13px;font-weight:500;color:#475569;word-break:break-word}\
.icon{\
width:46px;height:46px;margin:0 auto 14px;border-radius:14px;border:1px solid transparent;\
display:flex;align-items:center;justify-content:center\
}\
h1{margin:0 0 8px;font-size:18px;font-weight:600;line-height:1.35;color:#0F172A}\
.desc{margin:0 0 18px;font-size:13.5px;line-height:1.65;color:#475569;word-break:break-word}\
.hint{\
display:inline-flex;align-items:flex-start;justify-content:center;gap:8px;max-width:100%;\
padding:8px 12px;border-radius:12px;border:1px solid transparent;\
font-size:12px;line-height:1.45;text-align:left;word-break:break-word\
}\
.dot{width:6px;height:6px;margin-top:5.5px;border-radius:999px;flex-shrink:0}\
.tone-success .icon{background:#ECFDF5;border-color:#A7F3D0;color:#059669}\
.tone-success .hint{background:#F0FDF4;border-color:#BBF7D0;color:#15803D}\
.tone-success .dot{background:#10B981}\
.tone-info .icon{background:#EFF6FF;border-color:#BFDBFE;color:#2563EB}\
.tone-info .hint{background:#EFF6FF;border-color:#BFDBFE;color:#1D4ED8}\
.tone-info .dot{background:#3B82F6}\
.tone-error .icon{background:#FEF2F2;border-color:#FECACA;color:#DC2626}\
.tone-error .hint{background:#FEF2F2;border-color:#FECACA;color:#B91C1C}\
.tone-error .dot{background:#EF4444}\
@media(max-width:480px){\
body{padding:16px}\
.card{padding:24px 20px 20px;border-radius:16px}\
}\
@media(prefers-color-scheme:dark){\
body{\
color:#F8FAFC;\
background:radial-gradient(900px 380px at 50% -10%,rgba(37,99,235,.24),transparent 65%),#090A0D\
}\
.card{\
background:#17191E;border-color:rgba(255,255,255,.11);\
box-shadow:0 2px 10px rgba(0,0,0,.45),0 22px 48px -20px rgba(0,0,0,.85)\
}\
.brand-name,h1{color:#F8FAFC}\
.brand-sep{color:#64748B}\
.brand-provider,.desc{color:#A1A8B8}\
.tone-success .icon{background:rgba(16,185,129,.14);border-color:rgba(52,211,153,.30);color:#34D399}\
.tone-success .hint{background:rgba(16,185,129,.11);border-color:rgba(52,211,153,.26);color:#6EE7B7}\
.tone-success .dot{background:#34D399}\
.tone-info .icon{background:rgba(59,130,246,.16);border-color:rgba(96,165,250,.32);color:#60A5FA}\
.tone-info .hint{background:rgba(59,130,246,.12);border-color:rgba(96,165,250,.28);color:#93C5FD}\
.tone-info .dot{background:#60A5FA}\
.tone-error .icon{background:rgba(239,68,68,.15);border-color:rgba(248,113,113,.32);color:#F87171}\
.tone-error .hint{background:rgba(239,68,68,.12);border-color:rgba(248,113,113,.28);color:#FCA5A5}\
.tone-error .dot{background:#F87171}\
}";
