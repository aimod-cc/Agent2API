//! HTTP 边界：鉴权/容量闸门之后遮盖正文，记账/调试记录之后仅向本次调用方还原。
use axum::{
    body::{to_bytes, Body, Bytes},
    extract::Request,
    http::{header, HeaderValue, Method},
    middleware::Next,
    response::Response,
};
use serde_json::Value;

use crate::server::{
    config::privacy_masking as config,
    core::privacy_masking::{
        stream,
        wire::{self, Format},
        Mask, Restore, Rules, Settings,
    },
    errors::{management_error, GatewayError},
    http::ok_json,
};

const MAX_BODY: usize = 32 * 1024 * 1024;

pub(crate) async fn get() -> Response {
    match config::snapshot() {
        Ok(Some(policy)) => match policy.as_ref() {
            Ok(rules) => settings_response(&rules.settings),
            Err(message) => management_error(503, *message),
        },
        Ok(None) => settings_response(&Settings::default()),
        Err(message) => management_error(503, message),
    }
}

fn settings_response(settings: &Settings) -> Response {
    match serde_json::to_value(settings) {
        Ok(value) => ok_json(value),
        Err(_) => management_error(500, "隐私脱敏设置序列化失败"),
    }
}

pub(crate) async fn put(body: Bytes) -> Response {
    if body.len() > 64 * 1024 {
        return management_error(413, "隐私脱敏设置过大");
    }
    let Ok(value @ Value::Object(_)) = serde_json::from_slice::<Value>(&body) else {
        return management_error(400, "隐私脱敏设置必须是 JSON 对象");
    };
    let Ok(settings) = serde_json::from_value::<Settings>(value) else {
        return management_error(
            400,
            "隐私脱敏设置必须是包含 secrets、personal、terms 的对象",
        );
    };
    match config::save(settings) {
        Ok(()) => get().await,
        Err(config::SaveError::Storage) => management_error(500, "隐私脱敏设置未能持久化"),
        Err(config::SaveError::Invalid(message)) => management_error(400, message),
    }
}

pub(crate) async fn protect(request: Request, next: Next) -> Response {
    if request.method() != Method::POST {
        return next.run(request).await;
    }
    let format = match request.uri().path() {
        "/v1/chat/completions" => Some(Format::Chat),
        "/v1/responses" => Some(Format::Responses),
        "/v1/messages" => Some(Format::Messages),
        "/v1/images/generations" | "/v1/images/edits" | "/mcp/vision" | "/mcp/image-generation" => {
            None
        }
        _ => return next.run(request).await,
    };
    let policy = match config::snapshot() {
        Ok(None) => return next.run(request).await,
        Ok(Some(policy)) => policy,
        Err(message) => return error(503, message),
    };
    let rules = match policy.as_ref() {
        Ok(rules) if rules.enabled() => rules,
        Ok(_) => return next.run(request).await,
        Err(message) => return error(503, message),
    };
    let Some(format) = format else {
        return error(400, "隐私脱敏已开启；此推理端点不支持本地脱敏，未转发");
    };
    if request
        .headers()
        .get(header::CONTENT_ENCODING)
        .is_some_and(|v| v.as_bytes() != b"identity")
    {
        return error(415, "隐私脱敏仅接受未压缩的 JSON 请求，未转发");
    }
    let (mut parts, body) = request.into_parts();
    let original = match to_bytes(body, MAX_BODY).await {
        Ok(body) => body,
        Err(_) => return error(413, "隐私脱敏无法读取请求或请求超过 32 MiB，未转发"),
    };
    let (body, restore) = match mask_body(rules, format, original) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if restore.is_some() {
        parts.headers.remove(header::CONTENT_LENGTH);
    }
    let upstream = next.run(Request::from_parts(parts, Body::from(body))).await;
    match restore {
        Some(restore) if upstream.status().is_success() => {
            restore_response(format, upstream, restore).await
        }
        _ => upstream,
    }
}

fn mask_body(
    rules: &Rules,
    format: Format,
    original: Bytes,
) -> Result<(Bytes, Option<Restore>), Response> {
    let Ok(mut parsed @ Value::Object(_)) = serde_json::from_slice::<Value>(&original) else {
        return Err(error(400, "隐私脱敏需要 JSON 对象，未转发"));
    };
    let mut mask = Mask::new(rules);
    wire::request(format, &mut parsed, &mut mask)
        .map_err(|_| error(503, "隐私脱敏处理失败，未转发"))?;
    let restore = mask.into_restore();
    if restore.is_none() {
        return Ok((original, None));
    }
    let bytes = serde_json::to_vec(&parsed).map_err(|_| error(503, "隐私脱敏处理失败，未转发"))?;
    if bytes.len() > MAX_BODY {
        return Err(error(413, "脱敏后的请求超过 32 MiB，未转发"));
    }
    Ok((Bytes::from(bytes), restore))
}

async fn restore_response(format: Format, response: Response, mut restore: Restore) -> Response {
    let (mut parts, body) = response.into_parts();
    let mime = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .unwrap_or("");
    let streaming = mime.eq_ignore_ascii_case("text/event-stream");
    if !streaming && !mime.eq_ignore_ascii_case("application/json") {
        return Response::from_parts(parts, body);
    }
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.remove(header::ETAG);
    parts
        .headers
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if streaming {
        return Response::from_parts(parts, stream::wrap(body, format, restore));
    }
    let payload = match to_bytes(body, MAX_BODY).await {
        Ok(bytes) => bytes,
        Err(_) => return error(502, "隐私脱敏响应超过限制"),
    };
    let Ok(mut json) = serde_json::from_slice::<Value>(&payload) else {
        return error(502, "隐私脱敏响应不是合法 JSON");
    };
    if wire::response(format, &mut json, &mut restore).is_err() {
        return error(502, "隐私脱敏响应处理失败");
    }
    match serde_json::to_vec(&json) {
        Ok(bytes) => Response::from_parts(parts, Body::from(bytes)),
        Err(_) => error(502, "隐私脱敏响应处理失败"),
    }
}

fn error(status: i32, message: &str) -> Response {
    GatewayError::with_status(status, message)
        .with_code("privacy_masking")
        .payload_response()
}
