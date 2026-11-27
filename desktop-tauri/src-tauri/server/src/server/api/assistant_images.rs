//! 图片生成与编辑必须携带网关 Key；只向模型返回附件 ID，图片字节不进入工具文本或对话上下文。

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use serde_json::{json, Value};

use crate::server::config::{self, PermissionMode};
use crate::server::core::providers::images::ImageOperation;
use crate::server::core::{assistant_images as assets, assistant_sessions};
use crate::server::errors::GatewayError;
use crate::server::http::ok_json;
use crate::server::{logging, ServerState};

use super::images::{self, ImageExecution};

pub async fn generations(State(state): State<ServerState>, headers: HeaderMap, body: Bytes) -> Response {
    respond(run(&state, &headers, &body, ImageOperation::Generate).await)
}

pub async fn edits(State(state): State<ServerState>, headers: HeaderMap, body: Bytes) -> Response {
    respond(run(&state, &headers, &body, ImageOperation::Edit).await)
}

fn respond(result: Result<Value, GatewayError>) -> Response {
    match result {
        Ok(data) => ok_json(data),
        Err(error) => (
            error.http_status(),
            axum::Json(json!({
                "success": false, "error": error.message, "code": error.code,
            })),
        )
            .into_response(),
    }
}

async fn run(
    state: &ServerState,
    headers: &HeaderMap,
    bytes: &[u8],
    operation: ImageOperation,
) -> Result<Value, GatewayError> {
    let access = super::model_access::authenticate_call(headers, super::model_access::ASSISTANT_IMAGES)?;
    let _usage_lease = access.usage_lease;
    let config = config::assistant_config();
    if !config.tool_enabled(operation.tool()) {
        return Err(GatewayError::with_status(
            403,
            format!("工具「{}」未获授权", operation.tool()),
        ));
    }
    let payload: Value = serde_json::from_slice(bytes)
        .map_err(|_| GatewayError::bad_request("图片工具请求必须是 JSON"))?;
    if config.mode == PermissionMode::Confirm
        && payload.get("confirmed").and_then(Value::as_bool) != Some(true)
    {
        return Err(GatewayError::with_status(
            403,
            "图片工具会消耗上游额度，请先确认执行",
        ));
    }
    let session = payload
        .get("session")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty() && id.len() <= 128)
        .ok_or_else(|| GatewayError::bad_request("图片工具缺少有效的会话 ID"))?;
    if !assistant_sessions::exists(session) {
        return Err(GatewayError::bad_request(
            "请先保存当前助手会话，再调用图片工具",
        ));
    }
    assets::available()?;
    let mut request = payload
        .get("request")
        .filter(|request| request.is_object())
        .cloned()
        .ok_or_else(|| GatewayError::bad_request("图片工具缺少 request 对象"))?;
    if let Some(images) = request.get_mut("images").and_then(Value::as_array_mut) {
        for image in images {
            if let Some(id) = image
                .get("image_url")
                .and_then(Value::as_str)
                .and_then(|url| url.strip_prefix("assistant-image:"))
                .map(str::to_string)
            {
                let (bytes, mime) = assets::read(&id, Some(session))
                    .ok_or_else(|| GatewayError::bad_request("参考图片不存在，或不属于当前会话"))?;
                image["image_url"] = json!(format!(
                    "data:{mime};base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(bytes)
                ));
            }
        }
    }
    // 统一先取得图片字节再存助手附件，不把短期上游签名链接当成历史记录。
    request["response_format"] = json!("b64_json");
    let model = request.get("model").cloned().unwrap_or(Value::Null);
    let result = images::execute(
        state,
        request,
        ImageExecution {
            operation,
            headers: HeaderMap::new(),
            scope: access.scope,
            key_source: access.key_source,
            source: access.source,
            started_at: logging::now_ms(),
            is_assistant: true,
        },
    )
    .await?;
    let items = result
        .get("data")
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
        .ok_or_else(|| GatewayError::with_status(502, "图片工具没有返回图片"))?;
    let mut files = Vec::with_capacity(items.len());
    let mut metadata = Vec::with_capacity(items.len());
    for item in items {
        let encoded = item
            .get("b64_json")
            .and_then(Value::as_str)
            .ok_or_else(|| GatewayError::with_status(502, "图片工具响应缺少图片字节"))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| GatewayError::with_status(502, "图片工具响应编码无效"))?;
        let (bytes, mime, width, height) = tokio::task::spawn_blocking(move || {
            let mime = super::image_input::mime_of(&bytes)?.to_string();
            let (width, height) = super::image_input::inspect_bytes(&bytes)?;
            Ok::<_, GatewayError>((bytes, mime, width, height))
        })
        .await
        .map_err(|_| GatewayError::new("图片已生成，但附件处理失败，请勿自动重发"))??;
        metadata.push(json!({ "mime": mime, "width": width, "height": height, "revised_prompt": item.get("revised_prompt") }));
        files.push((bytes, mime));
    }
    let ids = assets::store(session, files)?;
    for (item, id) in metadata.iter_mut().zip(ids) {
        item["id"] = json!(id);
        item["url"] = json!(format!("/api/assistant/images/files/{id}"));
    }
    let mut output = json!({ "created": result.get("created"), "model": model, "data": metadata });
    for field in [
        "usage",
        "points_consumed",
        "size",
        "quality",
        "output_format",
    ] {
        if let Some(value) = result.get(field) {
            output[field] = value.clone();
        }
    }
    Ok(output)
}

pub(super) fn preserve_tool_references(request: &mut Value) {
    let Some(messages) = request.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let Some(parts) = message.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        let count = parts
            .iter()
            .filter(|part| {
                matches!(
                    part.get("type").and_then(Value::as_str),
                    Some("image_url" | "input_image")
                )
            })
            .count();
        if count == 0 {
            continue;
        }
        parts.retain(|part| {
            !matches!(
                part.get("type").and_then(Value::as_str),
                Some("image_url" | "input_image")
            )
        });
        parts.push(json!({ "type": "text", "text": format!("本条消息附有 {count} 张参考图，当前对话模型不读取图片；图片编辑工具可以通过 attachment:1 至 attachment:{count} 使用这些图片。") }));
    }
}

pub async fn download(State(_state): State<ServerState>, Path(id): Path<String>) -> Response {
    let Some((bytes, mime)) = assets::read(&id, None) else {
        return GatewayError::with_status(404, "助手图片不存在或已随会话删除").payload_response();
    };
    let content_type = match HeaderValue::from_str(&mime) {
        Ok(value) => value,
        Err(_) => return GatewayError::new("助手图片类型无效").payload_response(),
    };
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, content_type);
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    (StatusCode::OK, headers, bytes).into_response()
}
