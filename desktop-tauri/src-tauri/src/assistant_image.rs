use serde_json::{json, Value};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
pub async fn save_assistant_image(app: AppHandle, id: String) -> Result<Value, String> {
    let (bytes, mime) = agent2api_server::server::core::assistant_images::read(&id, None)
        .ok_or("助手图片不存在或已随会话删除")?;
    save_bytes(app, id, bytes, mime).await
}

#[tauri::command]
pub async fn save_tool_image(app: AppHandle, id: String) -> Result<Value, String> {
    if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("图片编号无效".into());
    }
    crate::startup::wait().await?;
    let client = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(60)).build().map_err(|error| error.to_string())?;
    let request = client.get(format!("http://127.0.0.1:{}/api/tools/image-generation/files/{id}", crate::gateway::proxy_port()));
    let mut response = crate::panel_session::attach(request)?.send().await.map_err(|error| error.to_string())?;
    if !response.status().is_success() { return Err(format!("下载图片失败（HTTP {}）", response.status().as_u16())); }
    let mime = response.headers().get("content-type").and_then(|value| value.to_str().ok()).unwrap_or("").to_string();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        if bytes.len().saturating_add(chunk.len()) > 32 * 1024 * 1024 { return Err("图片超过 32 MiB 上限".into()); }
        bytes.extend_from_slice(&chunk);
    }
    save_bytes(app, id, bytes, mime).await
}

async fn save_bytes(app: AppHandle, id: String, bytes: Vec<u8>, mime: String) -> Result<Value, String> {
    let extension = match mime.as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => return Err("不支持的助手图片类型".into()),
    };
    tauri::async_runtime::spawn_blocking(move || {
        let target = app.dialog().file().set_title("保存图片")
            .set_file_name(format!("image-{}.{extension}", &id[..8]))
            .add_filter("图片", &[extension]).blocking_save_file();
        let Some(target) = target else { return Ok(json!({ "canceled": true })); };
        let path = target.into_path().map_err(|error| format!("保存路径无效：{error}"))?;
        std::fs::write(&path, bytes).map_err(|error| format!("保存图片失败：{error}"))?;
        Ok(json!({ "saved": true, "path": path.display().to_string() }))
    }).await.map_err(|error| format!("保存图片任务失败：{error}"))?
}
