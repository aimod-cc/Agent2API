//! 整库备份 / 恢复的四条端点（设置页「整库备份 / 恢复」）。
//!
//! ```text
//! POST /api/backup/export    { filePath, includeLogs }  库 → SQL 文本写到该路径（桌面壳）
//! POST /api/backup/import    { filePath }               SQL 文件校验后整体覆盖主库（桌面壳）
//! GET  /api/backup/download  ?includeLogs=0|1           库 → SQL 文本作为响应体（网页端）
//! POST /api/backup/upload    （请求体 = SQL 文本）        校验后整体覆盖主库（网页端）
//! ```
//!
//! ── 为什么有两对端点 ────────────────────────────────────────
//! 桌面壳与网关同机，文件对话框给出路径，后端直读直写最省事（理由见下）；
//! 网页端（含 Docker / LAN 面板）只有浏览器，文件必须过 HTTP —— 浏览器
//! 只会「下载一个附件」和「上传一个文件」，所以补了 download / upload 一对。
//! 两对端点共用 `db::backup` 的同一条 dump / 校验 / 覆盖链路，产物格式
//! 完全一致，备份文件在两端可以互导。
//!
//! ── 为什么由后端直接读写用户选的路径（而不是文件内容过 HTTP）────
//! 导出文件可能上百 MB（日志与调试报文都在库里），经壳 → HTTP JSON 信封
//! 再落盘等于白白多两次全量拷贝与一段峰值内存；本网关与壳同机同用户，
//! 直接按路径读写即可（cc-switch 的 `export_config_to_file` 同款形态）。
//! 文件路径来自桌面壳的系统对话框，本端点只做基本校验（绝对路径 + `.sql`）。
//! 网页端没有文件对话框，走下面那对 download / upload —— 内容过 HTTP 是
//! 浏览器唯一的办法，此时用原始 body（不是 JSON 信封）省掉一次转义与解码。
//!
//! ── 挂 protected 的理由 ────────────────────────────────────
//! 导出会把全部账号凭证与面板管理员凭据写进用户指定路径；导入会整体替换
//! 数据库。两者都是 /api/config 级别的敏感动作。
//!
//! 慢操作（dump / 校验 / 覆盖）在 `spawn_blocking` 里跑，不占 async worker；
//! 重入靠 `db::backup::RESTORE_MUTEX` 串行（第二个恢复排队等待，行为可预期）。

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;

use crate::server::db::backup;
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::logging;
use crate::server::ServerState;

/// 网页端上传整库备份的请求体上限：2GB。
///
/// 管理面默认 32MB（见 `http::MAX_BODY_SIZE`），整库备份动辄上百 MB，必须
/// 单独放宽 —— 该路由在 `http.rs` 里用独立的 `DefaultBodyLimit` 覆盖。
/// 上限仍然要有：没有它，一个畸形请求就能把容器磁盘写满。
pub const MAX_BACKUP_UPLOAD: usize = 2 * 1024 * 1024 * 1024;

/// 校验壳侧传来的文件路径：必须绝对路径且 `.sql` 结尾（导出与导入同一口径
/// —— 只认自家格式，对话框之外的路径拼改在这里收口）。
fn check_path(raw: &str) -> Result<std::path::PathBuf, Response> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(errors::management_error(400, "缺少文件路径"));
    }
    let path = std::path::PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err(errors::management_error(400, "文件路径必须是绝对路径"));
    }
    if path.extension().and_then(|e| e.to_str()) != Some("sql") {
        return Err(errors::management_error(400, "备份文件必须是 .sql 文件"));
    }
    Ok(path)
}

/// POST /api/backup/export —— 整库导出到指定路径。
pub async fn export_backup(State(state): State<ServerState>, body: axum::body::Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(file_path) = payload.get("filePath").and_then(serde_json::Value::as_str) else {
        return errors::management_error(400, "缺少 filePath");
    };
    let target = match check_path(file_path) {
        Ok(path) => path,
        Err(response) => return response,
    };
    let include_logs = !matches!(payload.get("includeLogs"), Some(serde_json::Value::Bool(false)));

    let Some(db) = state.db().cloned() else {
        return errors::management_error(500, "数据库不可用，无法导出（详见启动日志）");
    };
    let result = tokio::task::spawn_blocking(move || backup::export_to_file(&db, &target, include_logs))
        .await;
    match result {
        Ok(Ok(data)) => ok_json(data),
        Ok(Err(message)) => errors::management_error(500, message),
        Err(error) => errors::management_error(500, format!("导出任务执行失败: {error}")),
    }
}

/// POST /api/backup/import —— 从指定路径恢复整库（替换语义）。
pub async fn import_backup(State(state): State<ServerState>, body: axum::body::Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(file_path) = payload.get("filePath").and_then(serde_json::Value::as_str) else {
        return errors::management_error(400, "缺少 filePath");
    };
    let source = match check_path(file_path) {
        Ok(path) => path,
        Err(response) => return response,
    };

    let Some(db) = state.db().cloned() else {
        return errors::management_error(500, "数据库不可用，无法恢复（详见启动日志）");
    };
    let config_dir = state.config_dir.clone();
    let result =
        tokio::task::spawn_blocking(move || backup::import_from_file(&db, &source, &config_dir))
            .await;
    match result {
        Ok(Ok(data)) => ok_json(data),
        Ok(Err(message)) => errors::management_error(500, message),
        Err(error) => errors::management_error(500, format!("恢复任务执行失败: {error}")),
    }
}

// ── 网页端：内容过 HTTP ────────────────────────────────────────

/// `GET /api/backup/download?includeLogs=0|1` 的查询参数。
#[derive(Debug, Deserialize)]
pub struct DownloadQuery {
    /// `0` / `false` = 精简导出（不含日志与调试报文）。缺省按完整导出。
    #[serde(rename = "includeLogs")]
    include_logs: Option<String>,
}

impl DownloadQuery {
    /// 缺省与无法解析的值都按「包含日志」处理 —— 与桌面端点
    /// （`includeLogs` 非 `false` 即完整导出）保持同一默认值。
    fn include_logs(&self) -> bool {
        match self.include_logs.as_deref().map(str::trim) {
            Some("0") | Some("false") | Some("no") => false,
            _ => true,
        }
    }
}

/// GET /api/backup/download —— 整库导出，SQL 文本直接作为响应体。
///
/// 表计数放在 `X-Backup-Tables` 响应头（JSON 数组）：网页端桥用它拼出与
/// 桌面端一致的 `{ bytes, tables }` 返回值，设置页的成功提示与「恢复整库」
/// 结果展示都不必区分端别。响应头有大小限制，但这里只有十来个表名。
pub async fn download_backup(
    State(state): State<ServerState>,
    Query(query): Query<DownloadQuery>,
) -> Response {
    let Some(db) = state.db().cloned() else {
        return errors::management_error(500, "数据库不可用，无法导出（详见启动日志）");
    };
    let include_logs = query.include_logs();
    let result = tokio::task::spawn_blocking(move || backup::export_sql(&db, include_logs)).await;
    let (dump, tables) = match result {
        Ok(Ok(data)) => data,
        Ok(Err(message)) => return errors::management_error(500, message),
        Err(error) => {
            return errors::management_error(500, format!("导出任务执行失败: {error}"))
        }
    };

    logging::log(
        "[Storage]",
        &format!(
            "📤 整库导出完成（网页端下载）: {} 字节，{} 张表",
            dump.len(),
            tables.len()
        ),
    );

    let filename = format!("agent2api-backup-{}.sql", file_stamp());
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/sql; charset=utf-8")
        .body(Body::from(dump))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    if let Ok(value) = HeaderValue::from_str(&tables_json(&tables)) {
        response.headers_mut().insert("x-backup-tables", value);
    }
    response
}

/// 表计数 → 单行 JSON（放进响应头；序列化失败退回空数组，不影响导出本身）。
fn tables_json(tables: &[serde_json::Value]) -> String {
    serde_json::to_string(tables).unwrap_or_else(|_| "[]".to_string())
}

/// POST /api/backup/upload —— 请求体为 SQL 文本，校验后整体覆盖主库。
///
/// ── 为什么先落临时文件 ──────────────────────────────────────
/// `import_from_file` 的入参是路径（它自己按文件读、在暂存库上执行、再
/// 用 SQLite Backup 覆盖主库）。把整个请求体先攒进内存再写临时文件，等于
/// 为一份几百 MB 的备份多留一份常驻内存 —— 这里按流边收边写，内存占用
/// 只与单个 chunk 有关。临时文件与 `db::backup` 的暂存库放在同一处
/// （系统临时目录），由本函数负责删除（失败路径也删）。
pub async fn upload_backup(State(state): State<ServerState>, request: axum::extract::Request) -> Response {
    let Some(db) = state.db().cloned() else {
        return errors::management_error(500, "数据库不可用，无法恢复（详见启动日志）");
    };

    let target = std::env::temp_dir().join(format!(
        "agent2api-upload-{}-{}.sql",
        std::process::id(),
        logging::now_ms()
    ));
    if let Err(message) = write_body_to_file(request, &target).await {
        let _ = tokio::fs::remove_file(&target).await;
        // 413 是 body 超过 `MAX_BACKUP_UPLOAD` 时 axum 抛出的错误文案（英文），
        // 换成用户能照做的说明；其余原样上抛。
        return if message.contains("length limit") || message.contains("too large") {
            errors::management_error(413, "备份文件超过 2GB 上限，请改用精简导出（不含日志）")
        } else {
            errors::management_error(400, format!("读取上传内容失败: {message}"))
        };
    }

    let config_dir = state.config_dir.clone();
    let source = target.clone();
    let result =
        tokio::task::spawn_blocking(move || backup::import_from_file(&db, &source, &config_dir))
            .await;
    // 导入是同步读文件的，spawn_blocking 返回时文件已经读完，可以安全删除。
    let _ = tokio::fs::remove_file(&target).await;
    match result {
        Ok(Ok(data)) => ok_json(data),
        Ok(Err(message)) => errors::management_error(500, message),
        Err(error) => errors::management_error(500, format!("恢复任务执行失败: {error}")),
    }
}

/// 把请求体流式写到 `target`（不整段进内存）。
async fn write_body_to_file(
    request: axum::extract::Request,
    target: &std::path::Path,
) -> Result<(), String> {
    let mut file = tokio::fs::File::create(target)
        .await
        .map_err(|error| format!("创建临时文件失败: {error}"))?;
    let mut stream = request.into_body().into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| error.to_string())?;
        file.write_all(&chunk)
            .await
            .map_err(|error| format!("写入临时文件失败: {error}"))?;
    }
    file.flush()
        .await
        .map_err(|error| format!("写入临时文件失败: {error}"))?;
    Ok(())
}

/// 导出文件名的时间戳（本地时间，`20261019-153012` 形态，与云同步快照同款）。
fn file_stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}
