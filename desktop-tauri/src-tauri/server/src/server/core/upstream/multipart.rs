//! 插件 multipart 信封只承载内联文件；此层不读本地路径、不拉取外部 URL。

use base64::Engine;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct File {
    field: String,
    filename: String,
    content_type: String,
    base64: String,
}

fn safe_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-[]".contains(&byte))
}

pub(super) fn request_form(payload: &str) -> Result<reqwest::multipart::Form, String> {
    const MAX_BYTES: usize = 14 * 1024 * 1024;
    let body: Value =
        serde_json::from_str(payload).map_err(|_| "插件上传信封不是合法 JSON".to_string())?;
    let multipart = body
        .get("multipart")
        .filter(|value| value.is_object())
        .ok_or_else(|| "插件上传计划缺少 multipart 信封".to_string())?;
    let fields = multipart
        .get("fields")
        .and_then(Value::as_object)
        .filter(|fields| fields.len() <= 64)
        .ok_or_else(|| "插件上传字段无效".to_string())?;
    let files: Vec<File> =
        serde_json::from_value(multipart.get("files").cloned().unwrap_or(Value::Null))
            .map_err(|_| "插件上传文件列表无效".to_string())?;
    if files.is_empty() || files.len() > 16 {
        return Err("插件上传需要 1 至 16 个内联文件".to_string());
    }
    let mut total = 0usize;
    let mut form = reqwest::multipart::Form::new();
    for (name, value) in fields {
        if !safe_token(name) {
            return Err("插件上传字段名无效".to_string());
        }
        let text = match value {
            Value::Null => continue,
            Value::String(text) => text.clone(),
            Value::Number(_) | Value::Bool(_) => value.to_string(),
            _ => return Err("插件上传字段必须是标量值".to_string()),
        };
        total = total.saturating_add(text.len());
        if total > MAX_BYTES {
            return Err("插件上传总大小超过 14 MiB".to_string());
        }
        form = form.text(name.clone(), text);
    }
    for file in files {
        if !safe_token(&file.field)
            || !safe_token(&file.filename)
            || file.base64.len() > MAX_BYTES * 4 / 3 + 4
        {
            return Err("插件上传文件名或大小无效".to_string());
        }
        let data = base64::engine::general_purpose::STANDARD
            .decode(&file.base64)
            .map_err(|_| "插件上传文件不是合法 base64".to_string())?;
        total = total.saturating_add(data.len());
        if data.is_empty() || total > MAX_BYTES {
            return Err("插件上传文件为空或总大小超过 14 MiB".to_string());
        }
        let part = reqwest::multipart::Part::bytes(data)
            .file_name(file.filename)
            .mime_str(&file.content_type)
            .map_err(|_| "插件上传文件类型无效".to_string())?;
        form = form.part(file.field, part);
    }
    Ok(form)
}
