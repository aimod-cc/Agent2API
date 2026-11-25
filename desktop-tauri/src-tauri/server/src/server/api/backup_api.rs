//! 整库备份的桌面路径接口与网页流式下载/上传接口。
//! 密码只经请求体传递；所有入口共享同一份加密包与恢复实现。

use axum::body::{Body, Bytes};
use axum::extract::{FromRequest, Multipart, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use serde::Deserialize;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use zeroize::Zeroizing;

use crate::server::db::backup;
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

pub const MAX_BACKUP_UPLOAD: usize = backup::MAX_BACKUP_BYTES as usize + 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportOptions {
    #[serde(default = "include_logs_default")]
    include_logs: bool,
    password: String,
}

fn include_logs_default() -> bool {
    true
}

fn check_path(raw: &str, importing: bool) -> Result<std::path::PathBuf, Response> {
    let path = std::path::PathBuf::from(raw.trim());
    if !path.is_absolute() {
        return Err(errors::management_error(400, "文件路径必须是绝对路径"));
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if !extension.eq_ignore_ascii_case(backup::BACKUP_EXTENSION)
        && !(importing && extension.eq_ignore_ascii_case("sql"))
    {
        return Err(errors::management_error(
            400,
            if importing {
                "请选择 .a2ab 加密备份或旧 .sql 备份"
            } else {
                "完整备份必须保存为 .a2ab 文件"
            },
        ));
    }
    Ok(path)
}

pub async fn export_backup(State(state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let target = match payload.get("filePath").and_then(Value::as_str) {
        Some(path) => match check_path(path, false) {
            Ok(path) => path,
            Err(response) => return response,
        },
        None => return errors::management_error(400, "缺少 filePath"),
    };
    let options: ExportOptions = match serde_json::from_value(payload) {
        Ok(options) => options,
        Err(_) => return errors::management_error(400, "请设置备份密码后导出完整备份"),
    };
    let password = Zeroizing::new(options.password);
    let Some(db) = state.db().cloned() else {
        return errors::management_error(500, "数据库不可用，无法导出");
    };
    let result = tokio::task::spawn_blocking(move || {
        backup::export_to_file(&db, &target, options.include_logs, &password)
    })
    .await;
    task_response(result, "导出")
}

pub async fn import_backup(State(state): State<ServerState>, body: Bytes) -> Response {
    let mut payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let source = match payload.get("filePath").and_then(Value::as_str) {
        Some(path) => match check_path(path, true) {
            Ok(path) => path,
            Err(response) => return response,
        },
        None => return errors::management_error(400, "缺少 filePath"),
    };
    let password = Zeroizing::new(
        payload
            .get_mut("password")
            .map(Value::take)
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default(),
    );
    let Some(db) = state.db().cloned() else {
        return errors::management_error(500, "数据库不可用，无法恢复");
    };
    let db_after = db.clone();
    let result = tokio::task::spawn_blocking(move || {
        backup::import_from_file(&db, &source, &state.config_dir, &password)
    })
    .await;
    if matches!(&result, Ok(Ok(_))) {
        let _suppress = crate::server::core::cloud_sync::auto_sync::SuppressionGuard::new();
        if let Err(error) =
            crate::server::core::plugin_host::script::reconcile_after_restore(&db_after).await
        {
            logging::log_with_level(
                "[Storage]",
                &format!("⚠️ 恢复后同步脚本插件身份映射失败：{error}"),
                "warn",
            );
        }
    }
    task_response(result, "恢复")
}

pub async fn restore_progress() -> Response {
    ok_json(backup::restore_progress())
}

pub async fn legacy_download_backup() -> Response {
    errors::management_error(400, "完整备份已改为密码加密包，请更新界面并设置备份密码")
}

pub async fn download_backup(State(state): State<ServerState>, body: Bytes) -> Response {
    let options: ExportOptions = match serde_json::from_slice(&body) {
        Ok(options) => options,
        Err(_) => return errors::management_error(400, "请设置备份密码后导出完整备份"),
    };
    let Some(db) = state.db().cloned() else {
        return errors::management_error(500, "数据库不可用，无法导出");
    };
    let password = Zeroizing::new(options.password);
    let result = tokio::task::spawn_blocking(move || {
        let mut file = backup::transfer_file(&state.config_dir)?;
        let data =
            backup::export_to_writer(&db, file.as_file_mut(), options.include_logs, &password)?;
        file.as_file()
            .sync_all()
            .map_err(|error| format!("同步备份传输文件失败: {error}"))?;
        Ok::<_, String>((file, data))
    })
    .await;
    let (file, data) = match result {
        Ok(Ok(result)) => result,
        Ok(Err(message)) => return errors::management_error(500, message),
        Err(error) => return errors::management_error(500, format!("导出任务执行失败: {error}")),
    };
    let mut reader = match file.reopen() {
        Ok(reader) => reader,
        Err(error) => {
            return errors::management_error(500, format!("打开备份传输文件失败: {error}"))
        }
    };
    if let Err(error) = std::io::Seek::rewind(&mut reader) {
        return errors::management_error(500, format!("读取备份传输文件失败: {error}"));
    }
    let reader = tokio::fs::File::from_std(reader);
    // 响应流持有临时文件；客户端断开时也释放，不留下携带密钥的备份副本。
    let stream = ReaderStream::new(reader).map(move |chunk| {
        let _keep = &file;
        chunk
    });
    let filename = format!(
        "agent2api-backup-{}.{}",
        file_stamp(),
        backup::BACKUP_EXTENSION
    );
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::PRAGMA, "no-cache")
        .header("x-content-type-options", "nosniff")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    insert_header(
        &mut response,
        header::CONTENT_DISPOSITION.as_str(),
        &format!("attachment; filename=\"{filename}\""),
    );
    insert_header(
        &mut response,
        "x-backup-tables",
        &data["tables"].to_string(),
    );
    insert_header(
        &mut response,
        "x-backup-keys",
        &data["includedKeys"].to_string(),
    );
    insert_header(
        &mut response,
        header::CONTENT_LENGTH.as_str(),
        &data["bytes"].to_string(),
    );
    logging::log("[Storage]", "📤 加密整库备份完成（网页端下载）");
    response
}

pub async fn upload_backup(
    State(state): State<ServerState>,
    request: axum::extract::Request,
) -> Response {
    let Some(db) = state.db().cloned() else {
        return errors::management_error(500, "数据库不可用，无法恢复");
    };
    let file = match backup::transfer_file(&state.config_dir) {
        Ok(file) => file,
        Err(message) => return errors::management_error(500, message),
    };
    let writer = match file.reopen() {
        Ok(file) => file,
        Err(error) => {
            return errors::management_error(500, format!("打开上传暂存文件失败: {error}"))
        }
    };
    let mut writer = tokio::fs::File::from_std(writer);
    let multipart = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("multipart/form-data"));
    let password = if multipart {
        match write_multipart(request, &state, &mut writer).await {
            Ok(password) => password,
            Err(response) => {
                drop(writer);
                return response;
            }
        }
    } else {
        if let Err(message) = write_raw(request, &mut writer).await {
            drop(writer);
            return upload_error(message);
        }
        Zeroizing::new(String::new())
    };
    if let Err(error) = writer.flush().await {
        drop(writer);
        return errors::management_error(500, format!("写入上传暂存文件失败: {error}"));
    }
    drop(writer);
    let db_after = db.clone();
    let result = tokio::task::spawn_blocking(move || {
        backup::import_from_file(&db, file.path(), &state.config_dir, &password)
    })
    .await;
    if matches!(&result, Ok(Ok(_))) {
        let _suppress = crate::server::core::cloud_sync::auto_sync::SuppressionGuard::new();
        if let Err(error) =
            crate::server::core::plugin_host::script::reconcile_after_restore(&db_after).await
        {
            logging::log_with_level(
                "[Storage]",
                &format!("⚠️ 恢复后同步脚本插件身份映射失败：{error}"),
                "warn",
            );
        }
    }
    task_response(result, "恢复")
}

async fn write_multipart(
    request: axum::extract::Request,
    state: &ServerState,
    writer: &mut tokio::fs::File,
) -> Result<Zeroizing<String>, Response> {
    let mut multipart = Multipart::from_request(request, state)
        .await
        .map_err(|error| upload_error(error.to_string()))?;
    let mut password = Zeroizing::new(String::new());
    let mut has_password = false;
    let mut has_file = false;
    let mut bytes = 0u64;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|error| upload_error(error.to_string()))?
    {
        match field.name() {
            Some("password") if !has_password => {
                let mut value = Zeroizing::new(Vec::new());
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|error| upload_error(error.to_string()))?
                {
                    if value.len() + chunk.len() > 1024 {
                        return Err(errors::management_error(400, "备份密码超过长度上限"));
                    }
                    value.extend_from_slice(&chunk);
                }
                *password = String::from_utf8(value.to_vec())
                    .map_err(|_| errors::management_error(400, "备份密码不是合法文本"))?;
                has_password = true;
            }
            Some("backup") if !has_file => {
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|error| upload_error(error.to_string()))?
                {
                    bytes += chunk.len() as u64;
                    if bytes > backup::MAX_BACKUP_BYTES {
                        return Err(upload_error("备份文件超过 2GB 上限".into()));
                    }
                    writer
                        .write_all(&chunk)
                        .await
                        .map_err(|error| upload_error(error.to_string()))?;
                }
                has_file = true;
            }
            _ => return Err(errors::management_error(400, "备份上传包含未知或重复字段")),
        }
    }
    if !has_file || bytes == 0 {
        return Err(errors::management_error(400, "未上传备份文件"));
    }
    Ok(password)
}

async fn write_raw(
    request: axum::extract::Request,
    writer: &mut tokio::fs::File,
) -> Result<(), String> {
    let mut stream = request.into_body().into_data_stream();
    let mut bytes = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| error.to_string())?;
        bytes += chunk.len() as u64;
        if bytes > backup::MAX_BACKUP_BYTES {
            return Err("备份文件超过 2GB 上限".into());
        }
        writer
            .write_all(&chunk)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn task_response(
    result: Result<Result<Value, String>, tokio::task::JoinError>,
    action: &str,
) -> Response {
    match result {
        Ok(Ok(data)) => ok_json(data),
        Ok(Err(message)) => errors::management_error(500, message),
        Err(error) => errors::management_error(500, format!("{action}任务执行失败: {error}")),
    }
}

fn upload_error(message: String) -> Response {
    if message.contains("length limit") || message.contains("too large") || message.contains("2GB")
    {
        errors::management_error(413, "备份文件超过 2GB 上限，请改用精简导出（不含日志）")
    } else {
        errors::management_error(400, format!("读取上传内容失败: {message}"))
    }
}

fn insert_header(response: &mut Response, name: &str, value: &str) {
    if let (Ok(name), Ok(value)) = (
        name.parse::<header::HeaderName>(),
        HeaderValue::from_str(value),
    ) {
        response.headers_mut().insert(name, value);
    }
}

fn file_stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}
