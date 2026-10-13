//! 整库备份 / 恢复的两条端点（设置页「整库备份 / 恢复」）。
//!
//! ```text
//! POST /api/backup/export  { filePath, includeLogs }  库 → SQL 文本写到该路径
//! POST /api/backup/import  { filePath }               SQL 文件校验后整体覆盖主库
//! ```
//!
//! ── 为什么由后端直接读写用户选的路径（而不是文件内容过 HTTP）────
//! 导出文件可能上百 MB（日志与调试报文都在库里），经壳 → HTTP JSON 信封
//! 再落盘等于白白多两次全量拷贝与一段峰值内存；本网关与壳同机同用户，
//! 直接按路径读写即可（cc-switch 的 `export_config_to_file` 同款形态）。
//! 文件路径来自桌面壳的系统对话框，本端点只做基本校验（绝对路径 + `.sql`）。
//! 网页端（LAN 面板）没有文件对话框，继续用 `/api/accounts/*` 的旧 JSON
//! 导入导出 —— 整库恢复本就是桌面场景。
//!
//! ── 挂 protected 的理由 ────────────────────────────────────
//! 导出会把全部账号凭证与面板管理员凭据写进用户指定路径；导入会整体替换
//! 数据库。两者都是 /api/config 级别的敏感动作。
//!
//! 慢操作（dump / 校验 / 覆盖）在 `spawn_blocking` 里跑，不占 async worker；
//! 重入靠 `db::backup::RESTORE_MUTEX` 串行（第二个恢复排队等待，行为可预期）。

use axum::extract::State;
use axum::response::Response;

use crate::server::db::backup;
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::ServerState;

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
