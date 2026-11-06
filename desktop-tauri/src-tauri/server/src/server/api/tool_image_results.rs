use std::io::Cursor;

use axum::{http::{header, HeaderValue}, response::{IntoResponse, Response}};
use base64::Engine;
use serde_json::{json, Value};

use crate::server::{core::{blocking, tool_services::image_jobs::{self, ImageFile, ImageJob}}, db::Db, errors::GatewayError};

pub(super) fn job_result(job: &ImageJob, base: &str, management: bool) -> Value {
    let mut result = job.result.clone().unwrap_or_else(|| json!({}));
    if !management {
        if let Some(object) = result.as_object_mut() { object.remove("actual_account_id"); object.remove("actual_account_name"); }
    }
    result["job_id"] = json!(job.id);
    result["request_id"] = json!(job.request_id);
    result["status"] = json!(job.state);
    result["operation"] = json!(job.operation);
    result["model"] = json!(job.model);
    result["provider"] = json!(job.provider);
    result["created_at"] = json!(job.created_at);
    result["updated_at"] = json!(job.updated_at);
    result["expires_at"] = json!(job.expires_at);
    result["deduplicated_until"] = json!(job.expires_at + 7 * 24 * 60 * 60_000);
    result["error"] = json!(job.error);
    if management { result["account_id"] = json!(job.account_id); }
    result["poll_after_seconds"] = if job.state == "running" { json!(5) } else { Value::Null };
    result["guidance"] = json!(match job.state.as_str() {
        "running" => "Generation is in progress. Call get_image_result with this job_id; do not submit again.",
        "succeeded" => "Images are ready. Download using the same gateway API key before expires_at. HTTP mode has not saved any files on the client machine.",
        "expired" => "The result has expired or was cleared. It has not been regenerated. Ask the user before creating a new job.",
        "uncertain" => "The upstream may have consumed credits. The result is uncertain; do not automatically generate again.",
        _ => "Generation did not complete. No automatic resubmission was made; ask the user before starting a new job.",
    });
    if let Some(images) = result.get_mut("images").and_then(Value::as_array_mut) {
        for image in images {
            let id = image["id"].as_str().unwrap_or("").to_string();
            let prefix = if management { "/api/tools/image-generation/files" } else { "/mcp/image-generation/files" };
            image["url"] = json!(format!("{}{prefix}/{id}", base.trim_end_matches('/')));
            image["ref"] = json!(format!("image:{id}"));
            image["requires_auth"] = json!(true);
        }
    }
    result
}

pub(super) async fn mcp_result(db: &Db, job: &ImageJob, base: &str, preview: bool) -> Result<Value, GatewayError> {
    let mut payload = job_result(job, base, false);
    let mut preview_content = None;
    if preview && job.state == "succeeded" {
        if let Some(id) = payload.pointer("/images/0/id").and_then(Value::as_str).map(str::to_string) {
            let db = db.clone();
            let result = blocking::run(move || {
                let (_, file) = image_jobs::read_file(&db, &id)?;
                thumbnail(&file)
            }).await;
            match result {
                Ok(Ok(data)) => preview_content = Some(json!({ "type": "image", "data": data, "mimeType": "image/png" })),
                _ => payload["preview_notice"] = json!("Preview unavailable; download the original image. Do not generate again."),
            }
        }
    }
    let mut content = vec![json!({ "type": "text", "text": serde_json::to_string(&payload).unwrap_or_default() })];
    if let Some(image) = preview_content { content.push(image); }
    Ok(json!({ "resultType": "complete", "content": content, "structuredContent": payload,
        "isError": !matches!(job.state.as_str(), "running" | "succeeded") }))
}

fn thumbnail(file: &ImageFile) -> Result<String, GatewayError> {
    let mut reader = image::ImageReader::new(Cursor::new(&file.bytes)).with_guessed_format()
        .map_err(|_| GatewayError::new("图片预览格式无效"))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|_| GatewayError::new("图片预览解码失败"))?;
    let mut bytes = Cursor::new(Vec::new());
    decoded.thumbnail(512, 512).write_to(&mut bytes, image::ImageFormat::Png)
        .map_err(|_| GatewayError::new("图片预览编码失败"))?;
    if bytes.get_ref().len() > 2 * 1024 * 1024 { return Err(GatewayError::new("图片预览过大")); }
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes.into_inner()))
}

pub(super) fn file_response(file: ImageFile) -> Response {
    let extension = match file.mime.as_str() { "image/jpeg" => "jpg", "image/webp" => "webp", _ => "png" };
    let mut response = file.bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(&file.mime).unwrap_or(HeaderValue::from_static("application/octet-stream")));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"image-{}.{extension}\"", file.id)) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    response
}
