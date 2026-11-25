use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use crate::gateway;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountFile {
    id: String,
    name: String,
    file_name: String,
    content: Value,
}

#[derive(Deserialize)]
struct AccountFiles {
    total: usize,
    files: Vec<AccountFile>,
    failed: Vec<Value>,
}

#[tauri::command]
pub async fn export_account_files(app: AppHandle, selection: Value) -> Result<Value, String> {
    let data = gateway::call("POST", "/api/accounts/export-files", Some(&selection)).await?;
    let data: AccountFiles =
        serde_json::from_value(data).map_err(|_| "账号导出响应格式无效".to_string())?;
    if data.files.is_empty() {
        return Ok(json!({ "total": data.total, "count": 0, "failed": data.failed }));
    }
    let group = selection
        .get("provider")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    let target = if group {
        app.dialog()
            .file()
            .set_title(crate::native_ui::text(
                crate::native_ui::TextKey::ExportAccountGroup,
            ))
            .blocking_pick_folder()
    } else {
        app.dialog()
            .file()
            .set_title(crate::native_ui::text(
                crate::native_ui::TextKey::ExportAccounts,
            ))
            .set_file_name(&data.files[0].file_name)
            .add_filter("JSON", &["json"])
            .add_filter(
                crate::native_ui::text(crate::native_ui::TextKey::AllFiles),
                &["*"],
            )
            .blocking_save_file()
    };
    let Some(target) = target else {
        return Ok(json!({ "canceled": true }));
    };
    let path = target
        .into_path()
        .map_err(|error| format!("保存路径无效: {error}"))?;
    tauri::async_runtime::spawn_blocking(move || save_files(data, &path, group))
        .await
        .map_err(|error| format!("账号导出任务失败: {error}"))
}

fn save_files(data: AccountFiles, target: &Path, group: bool) -> Value {
    let mut failed = data.failed;
    let mut count = 0;
    for file in data.files {
        let result = serde_json::to_vec_pretty(&file.content)
            .map_err(|_| "账号文件序列化失败".to_string())
            .and_then(|bytes| {
                if group {
                    write_new_file(target, &file.file_name, &bytes).map(|_| ())
                } else {
                    std::fs::write(target, bytes)
                        .map_err(|error| format!("写入账号文件失败: {error}"))
                }
            });
        match result {
            Ok(()) => count += 1,
            Err(error) => failed.push(json!({ "id": file.id, "name": file.name, "error": error })),
        }
    }
    json!({
        "total": data.total,
        "count": count,
        "failed": failed,
        "file": target.to_string_lossy(),
        "saved": count > 0,
    })
}

fn write_new_file(directory: &Path, name: &str, content: &[u8]) -> Result<PathBuf, String> {
    let name_path = Path::new(name);
    let mut components = name_path.components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err("导出文件名无效".to_string());
    }
    let stem = name_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("account");
    for suffix in 1..=9999 {
        let name = if suffix == 1 {
            name.to_string()
        } else {
            format!("{stem}-{suffix}.json")
        };
        let path = directory.join(name);
        // 分组导出不覆盖用户已有文件，即使两个导出同时选择同一个目录。
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                write_content(&mut file, content)?;
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("创建账号文件失败: {error}")),
        }
    }
    Err("同名文件过多，无法分配新的导出文件名".to_string())
}

fn write_content(file: &mut File, content: &[u8]) -> Result<(), String> {
    file.write_all(content)
        .and_then(|_| file.flush())
        .map_err(|error| format!("写入账号文件失败: {error}"))
}
