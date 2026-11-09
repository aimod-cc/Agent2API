use agent2api_provider_protocol as proto;
use serde_json::Value;
use super::{context::Context, resources::Resources};
use super::super::script::RequestBody;

fn name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-[]".contains(&byte))
}

pub(super) fn body(context: &Context, params: &Value) -> Result<Option<RequestBody>, proto::WireError> {
    let resource = params.get("body_resource").filter(|value| !value.is_null());
    let multipart = params.get("multipart").filter(|value| !value.is_null());
    if resource.is_some() as u8 + multipart.is_some() as u8 + params.get("body").is_some_and(|value| !value.is_null()) as u8 > 1 {
        return Err(super::error("请求正文不能同时使用多种传输形式"));
    }
    if let Some(resource) = resource {
        context.permission("resources.v1")?;
        return Ok(Some(RequestBody::Bytes(context.blobs.raw(resource.as_str().ok_or_else(super::errors::protocol)?)?)));
    }
    let Some(multipart) = multipart else { return Ok(None) };
    context.permission("resources.v1")?;
    Ok(Some(RequestBody::Multipart(form(&context.blobs, multipart)?)))
}

fn form(resources: &Resources, multipart: &Value) -> Result<reqwest::multipart::Form, proto::WireError> {
    let fields = multipart["fields"].as_object().filter(|fields| fields.len() <= 64).ok_or_else(super::errors::protocol)?;
    let files = multipart["files"].as_array().filter(|files| files.len() <= 16).ok_or_else(super::errors::protocol)?;
    let mut total = 0;
    let mut form = reqwest::multipart::Form::new();
    for (key, value) in fields {
        if !name(key) || !value.is_string() { return Err(super::errors::protocol()); }
        let text = value.as_str().unwrap_or("");
        total += text.len();
        if total > 1024 * 1024 { return Err(super::error("上传文本字段过大")); }
        form = form.text(key.clone(), text.to_string());
    }
    for file in files {
        let field = file["field"].as_str().unwrap_or("");
        let filename = file["filename"].as_str().unwrap_or("");
        if !name(field) || !name(filename) { return Err(super::errors::protocol()); }
        let bytes = resources.raw(file["resource"].as_str().ok_or_else(super::errors::protocol)?)?;
        total += bytes.len();
        if total > 96 * 1024 * 1024 { return Err(super::error("上传总量超过资源限制")); }
        let part = reqwest::multipart::Part::stream(reqwest::Body::from(bytes))
            .file_name(filename.to_string()).mime_str(file["content_type"].as_str().unwrap_or("application/octet-stream"))
            .map_err(|_| super::error("上传文件类型无效"))?;
        form = form.part(field.to_string(), part);
    }
    Ok(form)
}
