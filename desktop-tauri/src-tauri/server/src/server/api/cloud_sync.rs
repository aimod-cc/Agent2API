//! 云同步的管理接口（设置页「数据 → 云同步」）。
//!
//! ```text
//! GET  /api/cloud-sync              设置 + 状态（密码 / Secret 只回 hasXxx）
//! PUT  /api/cloud-sync              保存设置（空密码 = 沿用）
//! POST /api/cloud-sync/test         测试连接（顺带建远端目录）
//! GET  /api/cloud-sync/remote       远端快照摘要（二次确认弹窗用）
//! POST /api/cloud-sync/upload       上传
//! POST /api/cloud-sync/download     下载并应用
//! POST /api/cloud-sync/import-key   { password } 解密并安装远端账号密钥
//! ```
//!
//! ── 为什么全挂 protected ────────────────────────────────────
//! 上传会把全部账号密文与网关 Key 送到远端；下载会整体替换本机业务数据；
//! 保存会写入远端凭据。三者都是 `/api/config` 级别的敏感动作。`GET` 虽然只
//! 回 `hasXxx`，但它同时透出远端路径与同步状态，与 `/api/storage` 同级。
//!
//! ── 为什么下载 / 上传要单独的壳命令 ──────────────────────────
//! 壳到网关的通用请求超时是 60 秒（`gateway::REQUEST_TIMEOUT_MS`，面向管理
//! API 的小请求）。云同步要打外网传几 MB，60 秒不够 —— 所以 `bridge.rs` 给这
//! 两条命令单独放宽到 300 秒（见 `commands::cloud_sync_upload`）。

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};

use crate::server::core::cloud_sync::{self, manifest, settings};
use crate::server::errors;
use crate::server::http::{ok_json, parse_body};
use crate::server::ServerState;

/// GET /api/cloud-sync
pub async fn get_settings(State(_state): State<ServerState>) -> Response {
    ok_json(cloud_sync::settings_json())
}

/// PUT /api/cloud-sync
///
/// 请求体是**整份表单**（与 cc-switch 的 `webdavSyncSaveSettings` 同形）。
/// 密码类字段传空串 = 沿用库里那份；非空 = 换新。
pub async fn put_settings(State(_state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let input = match parse_save_input(&payload) {
        Ok(input) => input,
        Err(message) => return errors::management_error(400, message),
    };
    match cloud_sync::save_settings(input) {
        Ok(data) => ok_json(data),
        Err(message) => errors::management_error(400, message),
    }
}

/// POST /api/cloud-sync/test
pub async fn post_test(State(_state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let input = match parse_save_input(&payload) {
        Ok(input) => input,
        Err(message) => return errors::management_error(400, message),
    };
    match cloud_sync::test_connection(input).await {
        Ok(data) => ok_json(data),
        Err(message) => errors::management_error(502, message),
    }
}

/// GET /api/cloud-sync/remote
pub async fn get_remote(State(_state): State<ServerState>) -> Response {
    match cloud_sync::remote_info().await {
        Ok(data) => ok_json(data),
        // 配置不全（地址 / 用户名没填）是 400，网络 / 远端错误是 502 ——
        // 分开的理由：前者用户要改设置，后者只需要重试或换线路
        Err(message) => {
            if is_config_error(&message) {
                errors::management_error(400, message)
            } else {
                errors::management_error(502, message)
            }
        }
    }
}

/// POST /api/cloud-sync/upload
pub async fn post_upload(State(state): State<ServerState>) -> Response {
    let Some(db) = state.db().cloned() else {
        return errors::management_error(500, "数据库不可用，无法上传同步数据");
    };
    match cloud_sync::upload(&db).await {
        Ok(data) => ok_json(data),
        Err(message) => {
            cloud_sync::record_failure(&message, "manual");
            if is_config_error(&message) {
                errors::management_error(400, message)
            } else {
                errors::management_error(502, message)
            }
        }
    }
}

/// POST /api/cloud-sync/download
pub async fn post_download(State(state): State<ServerState>) -> Response {
    let Some(db) = state.db().cloned() else {
        return errors::management_error(500, "数据库不可用，无法下载同步数据");
    };
    let config_dir = state.config_dir.clone();
    match cloud_sync::download(&db, &config_dir).await {
        Ok(data) => ok_json(data),
        Err(message) => {
            // 「需要先导入密钥」不是失败：它是下载流程的中间步骤，不该写进
            // 「上次同步失败」的状态里（否则用户导入密钥后还会看到一条红字）
            if message == cloud_sync::KEY_REQUIRED {
                return errors::management_error(409, message);
            }
            cloud_sync::record_failure(&message, "manual");
            if is_config_error(&message) {
                errors::management_error(400, message)
            } else {
                errors::management_error(502, message)
            }
        }
    }
}

/// POST /api/cloud-sync/import-key
pub async fn post_import_key(State(_state): State<ServerState>, body: Bytes) -> Response {
    let payload = match parse_body(&body) {
        Ok(value) => value,
        Err(error) => return errors::management_error(400, error.message),
    };
    let Some(password) = payload.get("password").and_then(Value::as_str) else {
        return errors::management_error(400, "缺少 password");
    };
    match cloud_sync::import_key(password).await {
        Ok(data) => ok_json(data),
        Err(message) => errors::management_error(400, message),
    }
}

/// 这个错误是不是「配置问题」（用户要改设置）而不是「远端问题」（重试即可）。
///
/// 按文案判断不优雅，但本模块的对外契约是 `Result<_, String>`（与其它 API
/// 模块一致），为它引入一个错误枚举会让 `settings` / `transport` 全部改签名。
/// 判据只看几条明确的配置类文案，宁可把少数情况归成 502（重试无害）。
fn is_config_error(message: &str) -> bool {
    message.contains("不能为空")
        || message.contains("未启用")
        || message.contains("请先设置同步密码")
        || message.contains("未设置同步密码")
        || message.contains("至少需要")
        || message.contains("只支持 http/https")
        || message.contains("地址无效")
        || message.contains("不能包含 ..")
}

/// 请求体 → `SaveInput`（缺字段按空串，与前端「整份表单」的语义一致）
fn parse_save_input(payload: &Value) -> Result<settings::SaveInput, String> {
    let Some(object) = payload.as_object() else {
        return Err("请求体必须是 JSON 对象".to_string());
    };
    let text = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let flag = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    Ok(settings::SaveInput {
        provider: {
            let provider = text("provider");
            if provider.is_empty() {
                settings::PROVIDER_WEBDAV.to_string()
            } else {
                provider
            }
        },
        webdav_base_url: text("webdavBaseUrl"),
        webdav_username: text("webdavUsername"),
        webdav_password: text("webdavPassword"),
        s3_region: text("s3Region"),
        s3_bucket: text("s3Bucket"),
        s3_access_key_id: text("s3AccessKeyId"),
        s3_secret_access_key: text("s3SecretAccessKey"),
        s3_endpoint: text("s3Endpoint"),
        sync_password: text("syncPassword"),
        enabled: flag("enabled"),
        auto_sync: flag("autoSync"),
        remote_root: text("remoteRoot"),
        profile: text("profile"),
    })
}

/// 供排障用：一句「现在配到哪了」
#[allow(dead_code)]
pub fn describe() -> Value {
    let settings = settings::load();
    json!({
        "provider": settings.provider,
        "remotePath": manifest::remote_dir_display(&settings.remote_root, &settings.profile),
        "enabled": settings.enabled,
        "autoSync": settings.auto_sync,
        "lastSyncAt": settings.status.last_sync_at,
    })
}
