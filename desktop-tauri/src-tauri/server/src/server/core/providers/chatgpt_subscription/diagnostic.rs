use std::collections::BTreeMap;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use crate::server::core::login::LoginDiagnostic;
use crate::server::errors::GatewayError;

use super::{error, http};

const MAX_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_DISPLAY_BYTES: usize = 16 * 1024;
const REDACTED: &str = "[redacted]";
const SAFE_HEADERS: &[&str] = &[
    "content-type", "x-request-id", "request-id", "cf-ray", "cf-mitigated", "server", "date",
    "retry-after",
];

fn bounded(text: &str, limit: usize) -> (String, bool) {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    (text[..end].to_string(), end < text.len())
}

fn sensitive(name: &str) -> bool {
    let name: String = name.chars().filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase).collect();
    name.contains("token") || name.contains("secret") || name.contains("password")
        || name.contains("credential") || name.contains("cookie") || name.contains("authorization")
        || name.contains("privatekey") || name.contains("apikey") || name.contains("session")
        || matches!(name.as_str(), "codeverifier" | "verifier" | "codechallenge" | "state" | "nonce"
            | "clientid" | "hostid" | "extagenthostid" | "email" | "sub" | "subject" | "authcode")
}

fn scrub(text: &str, secrets: &[&str], redacted: &mut bool) -> String {
    let mut text = text.chars().filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\r' | '\t')).collect::<String>();
    for secret in secrets.iter().filter(|value| !value.is_empty()) {
        let encoded: String = url::form_urlencoded::byte_serialize(secret.as_bytes()).collect();
        let escaped = serde_json::to_string(secret).unwrap_or_default();
        for value in [secret.to_string(), encoded, escaped.trim_matches('"').to_string()] {
            if text.contains(&value) {
                text = text.replace(&value, REDACTED);
                *redacted = true;
            }
        }
    }
    static FILTERS: OnceLock<Result<Vec<Regex>, regex::Error>> = OnceLock::new();
    let filters = FILTERS.get_or_init(|| [
        r#"(?i)\b(?:bearer|basic)\s+[A-Za-z0-9_./+=-]+"#,
        r#"\beyJ[A-Za-z0-9_-]+(?:\.[A-Za-z0-9_-]+){1,2}\b"#,
        r#"(?i)\b(?:sk-|gh[pousr]_)[A-Za-z0-9_-]+"#,
        r#"(?i)\b(?:ac|rt)\.[0-9]+\.[A-Za-z0-9_.-]+"#,
        r#"\b[A-Za-z0-9_+=/-]{40,}(?:\.[A-Za-z0-9_+=/-]+)*\b"#,
        r#"(?i)[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}"#,
        r#"(?i)(?:https?|wss?)://[^\s<>\"']+"#,
        r#"(?is)[\"']?[\w-]*(?:token|secret|password|passwd|cookie|credential|authorization|api[_-]?key|code[_-]?verifier|state|nonce)[\w-]*[\"']?\s*[:=]\s*(?:\"(?:\\.|[^\"\\])*\"|'(?:\\.|[^'\\])*'|[^\s,;}<]+)"#,
    ].into_iter().map(Regex::new).collect());
    let Ok(filters) = filters else {
        *redacted = true;
        return REDACTED.to_string();
    };
    for filter in filters {
        if filter.is_match(&text) {
            text = filter.replace_all(&text, REDACTED).into_owned();
            *redacted = true;
        }
    }
    text
}

fn scrub_json(value: &mut Value, secrets: &[&str], code: &str, redacted: &mut bool, depth: usize) {
    if depth > 32 {
        *value = Value::String(REDACTED.into());
        *redacted = true;
        return;
    }
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if sensitive(key) || key.eq_ignore_ascii_case("code") && value.as_str().is_some_and(|value| value != code) {
                    *value = Value::String(REDACTED.into());
                    *redacted = true;
                } else {
                    scrub_json(value, secrets, code, redacted, depth + 1);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                scrub_json(item, secrets, code, redacted, depth + 1);
            }
        }
        Value::String(text) => {
            if !code.is_empty() && text == code {
                return;
            }
            if let Ok(mut nested) = serde_json::from_str::<Value>(text) {
                if nested.is_object() || nested.is_array() {
                    scrub_json(&mut nested, secrets, code, redacted, depth + 1);
                    *text = nested.to_string();
                    return;
                }
            }
            *text = scrub(text, secrets, redacted);
        }
        _ => {}
    }
}

fn scrub_markup(text: &str, redacted: &mut bool) -> String {
    static FILTERS: OnceLock<Result<Vec<Regex>, regex::Error>> = OnceLock::new();
    let filters = FILTERS.get_or_init(|| [
        r#"(?is)<(?:script|style|template)\b[^>]*>.*?(?:</(?:script|style|template)\s*>|$)"#,
        r#"(?is)<!--.*?(?:-->|$)"#,
        r#"(?is)</?[A-Za-z][\w:-]*(?:\s+[^>]*|/?)>"#,
    ].into_iter().map(Regex::new).collect());
    let Ok(filters) = filters else {
        *redacted = true;
        return REDACTED.to_string();
    };
    let mut text = text.to_string();
    for (index, filter) in filters.iter().enumerate() {
        if filter.is_match(&text) {
            text = filter.replace_all(&text, if index == 2 { " " } else { REDACTED }).into_owned();
            *redacted = true;
        }
    }
    text
}

pub(super) async fn failure(
    mut response: reqwest::Response,
    stage: &str,
    secrets: &[&str],
) -> GatewayError {
    let status = response.status().as_u16();
    let url = crate::server::core::debug_traffic::redact_url(response.url().as_str());
    let mut redacted = false;
    let mut headers = BTreeMap::new();
    for name in SAFE_HEADERS {
        if let Some(value) = response.headers().get(*name).and_then(|value| value.to_str().ok()) {
            headers.insert((*name).to_string(), bounded(&scrub(value, secrets, &mut redacted), 512).0);
        }
    }
    redacted |= response.headers().keys().any(|name| !SAFE_HEADERS.contains(&name.as_str()));
    let challenged = headers.get("cf-mitigated").is_some_and(|value| value.eq_ignore_ascii_case("challenge"));
    let request_id = headers.get("x-request-id").or_else(|| headers.get("request-id")).cloned();
    let mut bytes = Vec::new();
    let mut truncated = false;
    let read = async {
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    let remaining = MAX_RESPONSE_BYTES.saturating_sub(bytes.len());
                    bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                    if chunk.len() > remaining {
                        truncated = true;
                        return Ok(());
                    }
                }
                Ok(None) => return Ok(()),
                Err(_) => return Err("response_read_failed"),
            }
        }
    };
    let body_note = match tokio::time::timeout(http::REQUEST_TIMEOUT, read).await {
        Ok(Ok(())) => None,
        Ok(Err(note)) => Some(note.to_string()),
        Err(_) => Some("response_read_timeout".to_string()),
    };
    let text = String::from_utf8_lossy(&bytes);
    let parsed = serde_json::from_str::<Value>(&text).ok();
    let candidate = parsed.as_ref().map(error::code).unwrap_or_default();
    let code = if candidate.split('_').all(|word| word.len() < 32)
        && !secrets.iter().any(|secret| *secret == candidate) {
        candidate
    } else {
        String::new()
    };
    let body = if let Some(mut value) = parsed {
        scrub_json(&mut value, secrets, &code, &mut redacted, 0);
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| REDACTED.to_string())
    } else if text.trim_start().starts_with(['{', '[']) || headers.get("content-type").is_some_and(|kind| {
        let kind = kind.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
        kind == "application/json" || kind.ends_with("+json") || kind.contains("javascript") || kind.contains("ecmascript")
    }) {
        redacted = true;
        REDACTED.to_string()
    } else {
        let text = scrub_markup(&text, &mut redacted);
        scrub(&text, secrets, &mut redacted)
    };
    let (body, clipped) = bounded(&body, MAX_DISPLAY_BYTES);
    let reason = if challenged {
        "OpenAI 返回了安全验证页面，请检查当前网络或联系 OpenAI 支持".to_string()
    } else if !code.is_empty() {
        error::message(status, &code)
    } else {
        "OpenAI 未接受本次请求，请查看原始响应确认原因".to_string()
    };
    let action = match stage {
        "token_refresh" => "更新 ChatGPT 登录凭证失败",
        "identity_keys" => "读取 OpenAI 身份验证公钥失败",
        _ => "换取 ChatGPT 登录凭证失败",
    };
    let mut error = GatewayError::with_status(i32::from(status), format!("{action}（HTTP {status}）：{reason}"));
    error.code = (!code.is_empty()).then_some(code.clone());
    error.login_diagnostic = Some(LoginDiagnostic {
        stage: stage.to_string(), method: if stage == "identity_keys" { "GET" } else { "POST" }.to_string(),
        url, status: Some(status), code: (!code.is_empty()).then_some(code), request_id,
        route: None, headers, body, truncated: truncated || clipped, redacted, body_note,
    });
    error
}
