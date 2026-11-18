use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
pub async fn pick_desktop_client(app: AppHandle, default_path: Option<String>) -> Result<Value, String> {
    if app.get_webview_window(crate::MAIN_WINDOW_LABEL).is_none() { return Err("本机客户端选择仅用于桌面主窗口".into()); }
    tauri::async_runtime::spawn_blocking(move || {
        let mut picker = app.dialog().file().set_title("选择客户端程序").add_filter("客户端程序", &["exe"]);
        if let Some(path) = default_path.as_deref().map(std::path::Path::new).and_then(|path| path.parent()).filter(|path| path.is_dir()) {
            picker = picker.set_directory(path);
        }
        let Some(file) = picker.blocking_pick_file() else { return Ok(json!({ "canceled": true })); };
        let path = file.into_path().map_err(|_| "所选客户端程序路径无效")?;
        Ok(json!({ "canceled": false, "path": path.display().to_string() }))
    }).await.map_err(|_| "客户端程序选择未完成".to_string())?
}
