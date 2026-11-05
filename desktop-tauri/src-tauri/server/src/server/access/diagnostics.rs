//! 面板鉴权拒绝的可读诊断，只描述本次请求，不把「没带续期凭证」当成整条会话失效。

use axum::http::{HeaderMap, Method};
use serde_json::json;

use crate::server::logging;

struct Diagnosis {
    level: &'static str,
    reason: &'static str,
    summary: &'static str,
    detail: &'static str,
    suggestion: &'static str,
}

fn diagnose(headers: &HeaderMap, live_refresh: bool) -> Diagnosis {
    if !super::presents_credentials(headers) {
        return Diagnosis {
            level: "debug",
            reason: "credentials_missing",
            summary: "面板尚未登录，本次请求未携带登录凭证",
            detail: "本次请求没有可供验证的面板会话或 API Key，通常是打开页面时尚未登录。",
            suggestion: "需要使用面板时先完成登录；登录前的状态探测无需处理。",
        };
    }
    if live_refresh {
        return Diagnosis {
            level: "debug",
            reason: "refresh_available",
            summary: "面板访问凭证未通过验证，可自动续期",
            detail: "本次访问凭证未通过验证，但随请求提供的续期凭证仍有效。临时凭证到期时可能出现此过程，并不表示登录已经失效。",
            suggestion: "客户端可自动续期并重试；若随后恢复正常，无需操作。",
        };
    }
    // 只有单独携带 Key、没有面板会话候选时，才能明确把原因归到 Key。
    if headers.contains_key("x-api-key")
        && super::access_candidates(headers).is_empty()
        && super::refresh_candidates(headers).is_empty()
    {
        return Diagnosis {
            level: "warn",
            reason: "api_key_rejected",
            summary: "管理请求的 API Key 未通过验证",
            detail: "本次请求仅提供了 API Key，服务端未认可该 Key，也没有可用的面板登录凭证。",
            suggestion: "请登录管理面板，或检查调用方填写的 Key 是否正确、启用且属于当前网关。不要因此重新添加模型提供商账号。",
        };
    }
    if super::refresh_candidates(headers).is_empty() {
        return Diagnosis {
            level: "warn",
            reason: "refresh_not_presented",
            summary: "面板访问凭证未通过验证，本次请求未携带续期凭证",
            detail: "服务端未认可本次访问凭证，且本次请求没有提供续期凭证。这不代表客户端没有续期能力：桌面端会由本机重新获取凭证，网页端也可能只在续期时另行发送。",
            suggestion: "若页面随后恢复正常，无需操作；若持续出现，桌面端可关闭后重新打开，网页端请重新登录。仍有问题时检查是否访问了另一台网关，或中转入口未完整传递登录信息。",
        };
    }
    Diagnosis {
        level: "warn",
        reason: "refresh_not_accepted",
        summary: "面板访问凭证未通过验证，本次请求未匹配到有效续期凭证",
        detail: "本次请求提供了凭证，但未匹配到当前可用的访问及续期记录。可能是凭证到期、已经换新或撤销，也可能来自另一个网关；仅凭这次请求无法确认是哪一种，也不能确认后续是否恢复。",
        suggestion: "客户端可尝试恢复登录状态；若页面不能自动恢复，请重新登录。若反复出现，关闭长期闲置的旧标签页，并检查访问地址及中转入口。",
    }
}

fn operation(method: &Method, path: &str) -> &'static str {
    match (method.as_str(), path) {
        ("GET", "/api/update/policy") => "读取更新策略",
        ("GET", "/api/session") => "读取面板状态",
        _ => "访问管理接口",
    }
}

/// 调用方已确认管理请求未通过鉴权；仍由原入口返回 401，不改变任何准入判定。
/// `live_refresh` 与响应是否清除 cookie 共用同一次读数。
pub(crate) fn log_rejection(headers: &HeaderMap, method: &Method, path: &str, live_refresh: bool) {
    let diagnosis = diagnose(headers, live_refresh);
    let action = operation(method, path);
    let impact = if method == Method::GET && path == "/api/update/policy" {
        "本次未能读取更新策略，请求尚未进入更新处理；这条日志不表示更新服务器故障，也不代表模型提供商账号失效。"
    } else {
        "本次管理请求在身份验证阶段被拒绝，未执行后续操作；这条日志本身不代表模型提供商账号失效，也不代表整个网关停止工作。"
    };
    let summary = format!("{action}：{}", diagnosis.summary);
    let detail = format!(
        "原因：{}\n影响：{impact}\n建议：{}",
        diagnosis.detail, diagnosis.suggestion,
    );
    let message = format!("[Security] {summary}（{method} {path}，HTTP 401）");
    // 控制台与日志库各一份，不再用 log() 根据「失败」等字样猜级别。
    logging::console_line("[Security]", &format!("{summary}（{method} {path}，HTTP 401）\n{detail}"));
    let credential_headers: Vec<&str> = ["x-panel-token", "x-panel-refresh", "authorization", "x-api-key"]
        .into_iter()
        .filter(|name| headers.contains_key(*name))
        .collect();
    logging::log_event(
        diagnosis.level,
        "auth",
        &message,
        Some(json!({
            "component": "Security",
            "summary": summary,
            "detail": detail,
            "action": action,
            "status": 401,
            "reason": diagnosis.reason,
            "httpMethod": method.as_str(),
            "path": path,
            "accessFingerprints": super::access_fingerprints(headers),
            "cookieNames": super::cookie_names(headers),
            "credentialHeaders": credential_headers,
            "liveRefreshPresented": live_refresh,
        })),
    );
}
