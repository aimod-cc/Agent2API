//! 插件管理接口。在线安装启动后台任务；状态读取不触发外网请求。

use axum::body::Bytes;
use axum::response::Response;
use base64::Engine;
use serde_json::{json, Value};

use crate::server::core::plugin_host;
use crate::server::core::plugin_host::update::MAX_PACK_BYTES;
use crate::server::errors::management_error;
use crate::server::http::{ok_json, parse_body};

const MAX_PACK_BASE64_BYTES: usize = (MAX_PACK_BYTES + 2) / 3 * 4;
// 仅插件上传路由使用：96 MiB ZIP 的 base64 为 128 MiB，另留有限的 JSON 开销。
pub(crate) const MAX_PACK_UPLOAD_BYTES: usize = MAX_PACK_BASE64_BYTES + 1024 * 1024;

pub async fn list() -> Response {
    let mut result = plugin_host::management_snapshot();
    result["target"] = json!(plugin_host::host_target());
    result["protocol"] = json!(agent2api_provider_protocol::PROTOCOL_VERSION);
    result["maxPackBytes"] = json!(MAX_PACK_BYTES);
    result["pluginsDir"] = json!(plugin_host::plugins_dir_display());
    result["catalog"] = plugin_host::catalog::snapshot();
    result["updates"] = plugin_host::catalog::updates()["updates"].clone();
    result["operations"] = plugin_host::operations::snapshot()["operations"].clone();
    ok_json(result)
}

pub async fn settings() -> Response {
    ok_json(json!(plugin_host::settings::current()))
}

pub async fn save_settings(body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(error) => return management_error(400, error.message),
    };
    match plugin_host::settings::configure(&payload) {
        Ok(result) => ok_json(result),
        Err(error) => management_error(400, error),
    }
}

pub async fn progress() -> Response {
    let mut result = plugin_host::operations::snapshot();
    result["maxPackBytes"] = json!(MAX_PACK_BYTES);
    ok_json(result)
}

pub async fn install(body: Bytes) -> Response {
    let bytes = match pack_bytes(&body) {
        Ok(bytes) => bytes,
        Err(error) => return management_error(400, error),
    };
    if bytes.is_empty() || bytes.len() > MAX_PACK_BYTES {
        return management_error(400, pack_size_error());
    }
    let prepared = match tokio::task::spawn_blocking(move || {
        plugin_host::prepare_pack(&bytes, None, None, false)
    })
    .await
    {
        Ok(Ok(prepared)) => prepared,
        Ok(Err(error)) => return management_error(400, error),
        Err(error) => return management_error(500, format!("插件校验任务中断：{error}")),
    };
    match plugin_host::operations::install_prepared(prepared).await {
        Ok(result) => ok_json(result),
        Err(error) => management_error(400, format!("安装失败：{error}")),
    }
}

async fn change_pack(id: &str, change: plugin_host::operations::PackChange) -> Response {
    match plugin_host::operations::change(id.to_string(), change).await {
        Ok(result) => ok_json(result),
        Err(error) => management_error(error.status_code, error.message),
    }
}

pub async fn uninstall(body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(error) => return management_error(400, error.message),
    };
    let id = payload["id"].as_str().unwrap_or("").trim();
    if id.is_empty() {
        return management_error(400, "缺少插件包 id");
    }
    change_pack(id, plugin_host::operations::PackChange::Uninstall).await
}

pub async fn set_enabled(body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(error) => return management_error(400, error.message),
    };
    let id = payload["id"].as_str().unwrap_or("").trim();
    let Some(enabled) = payload["enabled"].as_bool() else {
        return management_error(400, "缺少 enabled 布尔值");
    };
    if id.is_empty() {
        return management_error(400, "缺少插件包 id");
    }
    change_pack(id, plugin_host::operations::PackChange::Enabled(enabled)).await
}

pub async fn set_provider(body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(payload) => payload,
        Err(error) => return management_error(400, error.message),
    };
    let id = payload["id"].as_str().unwrap_or("").trim();
    let provider = payload["provider"].as_str().unwrap_or("").trim();
    let Some(enabled) = payload["enabled"].as_bool() else {
        return management_error(400, "缺少 enabled 布尔值");
    };
    if id.is_empty() || provider.is_empty() {
        return management_error(400, "缺少插件包 id 或提供商 id");
    }
    change_pack(id, plugin_host::operations::PackChange::Provider {
        id: provider.to_string(), enabled,
    }).await
}

pub async fn check_update() -> Response {
    match plugin_host::catalog::refresh(true).await {
        Ok(_) => ok_json(plugin_host::catalog::updates()),
        Err(error) => {
            let mut result = plugin_host::catalog::updates();
            result["error"] = json!(true);
            result["reason"] = json!(error);
            ok_json(result)
        }
    }
}

pub async fn run_update(body: Bytes) -> Response {
    start_operation(body, "update").await
}

pub async fn install_from_release(body: Bytes) -> Response {
    start_operation(body, "install").await
}

async fn start_operation(body: Bytes, action: &str) -> Response {
    let payload = if body.is_empty() {
        json!({})
    } else {
        match parse_body(&body) {
            Ok(payload) => payload,
            Err(error) => return management_error(400, error.message),
        }
    };
    let default_id = plugin_host::list_packs()
        .first()
        .map(|pack| pack.id.clone())
        .unwrap_or_else(|| "agent2api-providers".to_string());
    let id = payload["id"].as_str().unwrap_or(&default_id);
    let current = plugin_host::host_target();
    let target = payload["target"].as_str().unwrap_or(&current);
    match plugin_host::operations::start(id, action, target).await {
        Ok(operation) => ok_json(json!({ "operation": operation })),
        Err(error) => management_error(400, error),
    }
}

pub async fn release() -> Response {
    match plugin_host::catalog::refresh(true).await {
        Ok(result) => ok_json(result),
        Err(error) => {
            let mut result = plugin_host::catalog::snapshot();
            result["error"] = json!(true);
            result["reason"] = json!(error);
            ok_json(result)
        }
    }
}

fn pack_size_error() -> String {
    format!(
        "插件包必须为非空且不超过 {} MiB 的 zip",
        MAX_PACK_BYTES / (1024 * 1024)
    )
}

fn pack_bytes(body: &[u8]) -> Result<Vec<u8>, String> {
    if body.is_empty() {
        return Err("请求体是空的：请选择插件包 zip 后重试".to_string());
    }
    if body.starts_with(b"PK") {
        if body.len() > MAX_PACK_BYTES {
            return Err(pack_size_error());
        }
        return Ok(body.to_vec());
    }
    if body.len() > MAX_PACK_UPLOAD_BYTES {
        return Err(pack_size_error());
    }
    let payload = parse_body(body).map_err(|_| "请求体既不是 zip 也不是 JSON".to_string())?;
    let encoded = payload
        .get("packB64")
        .or_else(|| payload.get("fileB64"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "缺少 packB64".to_string())?;
    let encoded = encoded
        .split_once("base64,")
        .map(|(_, text)| text)
        .unwrap_or(encoded)
        .trim();
    // decode 会先为输出分配内存；按编码长度与 padding 预检，实际字节仍在解码后复核。
    if encoded.len() > MAX_PACK_BASE64_BYTES {
        return Err(pack_size_error());
    }
    let padding = encoded
        .bytes()
        .rev()
        .take_while(|byte| *byte == b'=')
        .take(2)
        .count();
    let estimated =
        (encoded.len() / 4 * 3 + encoded.len() % 4 * 3 / 4).saturating_sub(padding);
    if estimated > MAX_PACK_BYTES {
        return Err(pack_size_error());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| format!("插件包内容不是合法 base64：{error}"))?;
    if bytes.is_empty() || bytes.len() > MAX_PACK_BYTES {
        return Err(pack_size_error());
    }
    Ok(bytes)
}
