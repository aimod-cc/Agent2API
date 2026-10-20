//! 云同步：把业务数据快照传到 WebDAV / S3 兼容存储，或从远端取回。
//!
//! ```text
//! GET  /api/cloud-sync             设置 + 状态
//! PUT  /api/cloud-sync             保存设置
//! POST /api/cloud-sync/test        测试连接（顺带建目录）
//! GET  /api/cloud-sync/remote      远端快照信息（二次确认弹窗用）
//! POST /api/cloud-sync/upload      上传
//! POST /api/cloud-sync/download    下载并应用
//! POST /api/cloud-sync/import-key  用同步密码解出并安装远端账号密钥
//! ```
//!
//! ── 为什么分成这几块文件 ────────────────────────────────────
//!   `settings`   设置与状态（kv 保留键 `cloudSync`）
//!   `manifest`   远端「这一份是什么」的元数据与兼容判定
//!   `snapshot`   业务数据的取 / 装（唯一的数据范围判定）
//!   `keyfile`    同步密码 → 账号密钥的封装
//!   `webdav` / `s3`  传输原语
//!   `transport`  传输抽象（协议层不认识具体服务商）
//!   `auto_sync`  变更触发 + 防抖
//!
//! ── 为什么全局一把同步锁 ────────────────────────────────────
//! 上传与下载都会读写同一份远端数据，两个并发操作的结果取决于谁后完成 ——
//! 用户点两下按钮就可能得到「远端是我刚传的、本机是旧机器的」这种最难解释的
//! 状态。与 cc-switch 的 `sync_mutex` 同一取向：所有传输共用一把锁，操作排队。
//! 锁是 `tokio::sync::Mutex`（跨 await 持有），不能换成 std 的。
//!
//! ── 下载会重装哪些内存态 ────────────────────────────────────
//! 与整库恢复同一清单（见 `db::backup` 模块头），逐项核实：
//!   · `config::reload()` —— 配置快照必须重读，否则本次运行还是旧配置；
//!   · `usage_records::reload_facts()` —— 余额事实表是选路依据；
//!   · `crypto::reset_cache(Accounts)` —— **仅当密钥文件被替换**（见 `keyfile`）；
//!   · 面板登录态、桌面设置、任务排期**不在同步范围**，因此不需要重装。

pub(crate) mod auto_sync;
pub(crate) mod keyfile;
pub(crate) mod manifest;
pub(crate) mod s3;
pub(crate) mod settings;
pub(crate) mod snapshot;
pub(crate) mod transport;
pub(crate) mod webdav;

use std::sync::OnceLock;

use serde_json::{json, Value};

use crate::server::core::crypto::{self, SecretScope};
use crate::server::db::Db;
use crate::server::logging;

use manifest::{MAX_ARTIFACT_BYTES, REMOTE_KEY_FILE, REMOTE_MANIFEST, REMOTE_SNAPSHOT};
use transport::Transport;

/// 装库句柄（`bootstrap` 里调用一次；设置读写都在 `settings` 子模块里）。
pub(crate) fn install(db: Option<Db>) {
    settings::install(db);
}

/// 全局同步锁（上传 / 下载 / 自动同步共用）
pub(crate) fn sync_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// sha256 → 小写十六进制（manifest / 快照 / 签名共用一份实现）
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};
    let digest = Sha256::digest(bytes);
    let mut text = String::with_capacity(64);
    for byte in digest.iter() {
        use std::fmt::Write as _;
        let _ = write!(text, "{byte:02x}");
    }
    text
}

// ─── 设置 ─────────────────────────────────────────────────────

/// GET /api/cloud-sync 的数据
pub(crate) fn settings_json() -> Value {
    settings::load().public_json()
}

/// 保存设置。`input` 里空密码 = 沿用旧值（见 `settings::apply_save`）。
pub(crate) fn save_settings(input: settings::SaveInput) -> Result<Value, String> {
    let saved = settings::apply_save(input)?;
    logging::log(
        "[CloudSync]",
        &format!(
            "云同步设置已保存：{}，远端 {}{}",
            if saved.provider == settings::PROVIDER_S3 {
                "S3 兼容存储"
            } else {
                "WebDAV"
            },
            manifest::remote_dir_display(&saved.remote_root, &saved.profile),
            if saved.enabled { "（已启用）" } else { "（未启用）" }
        ),
    );
    Ok(saved.public_json())
}

/// 读设置并要求「已启用」。手动操作也受总开关约束 —— 与 cc-switch 同义：
/// 开关关着意味着「这台机器不参与云同步」，不该因为用户点了一下按钮就绕过。
///
/// 只校验远端凭据（`validate_remote`）：同步密码在**下载 / 预览**路径上允许为空，
/// 理由见 `settings::validate_remote` 的注释（新机器第一次下载时本机还没有密码）。
/// 真正需要密码的是上传（见 `upload_locked`），那里单独报错。
fn require_enabled() -> Result<settings::CloudSyncSettings, String> {
    let settings = settings::load();
    if !settings.enabled {
        return Err("云同步未启用，请先在设置里打开开关".to_string());
    }
    settings.validate_remote()?;
    Ok(settings)
}

// ─── 测试连接 ─────────────────────────────────────────────────

/// 测试连接：**不看**总开关（还没配好就打开开关是正常的配置顺序），
/// 但要求当前服务商的必填项齐全。
pub(crate) async fn test_connection(input: settings::SaveInput) -> Result<Value, String> {
    // 用「当前已保存的密文 + 这次输入的明文」拼一份临时设置来测：
    // 用户改了密码但还没保存时，测的应该是新密码。
    let mut probe = settings::load();
    probe.provider = input.provider;
    probe.webdav.base_url = input.webdav_base_url;
    probe.webdav.username = input.webdav_username;
    if !input.webdav_password.is_empty() {
        probe.webdav.password = crypto::seal(
            input.webdav_password.as_bytes(),
            SecretScope::CloudSync,
        )?;
    }
    probe.s3.region = input.s3_region;
    probe.s3.bucket = input.s3_bucket;
    probe.s3.access_key_id = input.s3_access_key_id;
    if !input.s3_secret_access_key.is_empty() {
        probe.s3.secret_access_key = crypto::seal(
            input.s3_secret_access_key.as_bytes(),
            SecretScope::CloudSync,
        )?;
    }
    probe.s3.endpoint = input.s3_endpoint;
    probe.remote_root = input.remote_root;
    probe.profile = input.profile;
    probe.normalize();

    // 只校验当前服务商的必填项：临时填一个占位同步密码，跳过「密码未设置」
    // 这一条（测试连接不需要密码，那个密码只用于封装账号密钥）。
    let mut validate_target = probe.clone();
    if validate_target.sync_password.is_empty() {
        validate_target.sync_password = "placeholder".to_string();
    }
    validate_target.validate()?;

    let transport = Transport::from_settings(&probe)?;
    transport.test_connection().await?;
    Ok(json!({
        "success": true,
        "remotePath": transport.display_path(),
    }))
}

// ─── 远端信息 ─────────────────────────────────────────────────

/// 远端快照摘要（不含数据本身）
pub(crate) async fn remote_info() -> Result<Value, String> {
    let settings = settings::load();
    // 远端预览只要求地址 / 凭据齐全（新机器还没有同步密码，见 validate_remote）
    settings.validate_remote()?;
    let transport = Transport::from_settings(&settings)?;
    let Some((bytes, manifest)) = transport.get_manifest().await? else {
        return Ok(json!({
            "empty": true,
            "remotePath": transport.display_path(),
        }));
    };
    let compatible = manifest::validate_compat(&manifest).is_ok();
    let incompatible_reason = manifest::validate_compat(&manifest).err();
    let local_fingerprint = crypto::key_fingerprint(SecretScope::Accounts);
    Ok(json!({
        "empty": false,
        "remotePath": transport.display_path(),
        "manifestBytes": bytes.len(),
        "format": manifest.format,
        "version": manifest.version,
        "appVersion": manifest.app_version,
        "schemaVersion": manifest.schema_version,
        "deviceName": manifest.device_name,
        "createdAt": manifest.created_at,
        "snapshotId": manifest.snapshot_id,
        "artifacts": manifest.artifacts.keys().collect::<Vec<_>>(),
        "compatible": compatible,
        "incompatibleReason": incompatible_reason,
        "keyFingerprint": manifest.key_fingerprint,
        "localKeyFingerprint": local_fingerprint,
        // 远端带了密钥封装、且与本机密钥不一致（或本机还没有密钥）→
        // 下载前必须先输入同步密码导入密钥
        "keyRequired": manifest.key_fingerprint.is_some()
            && manifest.key_fingerprint != local_fingerprint,
        "hasKeyArtifact": manifest.artifacts.contains_key(REMOTE_KEY_FILE),
    }))
}

// ─── 上传 ─────────────────────────────────────────────────────

/// 上传：构建快照 → 封装密钥 → 传 artifact → 最后传 manifest。
pub(crate) async fn upload(db: &Db) -> Result<Value, String> {
    let _guard = sync_lock().lock().await;
    let settings = require_enabled()?;
    upload_locked(db, &settings).await
}

/// 上传本体（调用方已持锁）。自动同步也走这里，避免两处顺序漂移。
pub(crate) async fn upload_locked(
    db: &Db,
    settings: &settings::CloudSyncSettings,
) -> Result<Value, String> {
    let transport = Transport::from_settings(settings)?;
    transport.test_connection().await?;

    let snapshot = snapshot::build(db)?;
    let snapshot_bytes = serde_json::to_vec(&snapshot)
        .map_err(|error| format!("序列化同步快照失败: {error}"))?;
    if snapshot_bytes.len() as u64 > MAX_ARTIFACT_BYTES {
        return Err(format!(
            "本机业务数据超过同步上限（{} 字节 > {} 字节），请先清理签到 / 余额记录",
            snapshot_bytes.len(),
            MAX_ARTIFACT_BYTES
        ));
    }

    // 账号密钥：本机还没有密钥（从未保存过账号）时不传
    let key_bytes = crypto::existing_key_bytes(SecretScope::Accounts);
    let password = settings.sync_password_plain()?.ok_or_else(|| {
        "未设置同步密码，无法加密上云的账号密钥".to_string()
    })?;
    let key_envelope = match key_bytes.as_ref() {
        Some(bytes) => Some(keyfile::seal_key(bytes, &password)?),
        None => None,
    };
    let key_fingerprint = key_bytes
        .as_ref()
        .map(|bytes| crypto::fingerprint_of(bytes));

    let manifest = snapshot::build_manifest(
        &snapshot_bytes,
        key_envelope.as_deref(),
        key_fingerprint.clone(),
    );
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| format!("序列化 manifest 失败: {error}"))?;

    // 顺序：artifact 先、manifest 最后（见 manifest 模块头）
    transport
        .put(REMOTE_SNAPSHOT, snapshot_bytes.clone(), "application/json")
        .await?;
    if let Some(envelope) = key_envelope {
        transport
            .put(REMOTE_KEY_FILE, envelope, "application/json")
            .await?;
    }
    transport
        .put(REMOTE_MANIFEST, manifest_bytes, "application/json")
        .await?;

    let content_hash = snapshot.content_hash();
    let snapshot_id = manifest.snapshot_id.clone();
    settings::update_status(|status| {
        status.last_sync_at = Some(logging::now_ms());
        status.last_action = Some("upload".to_string());
        status.last_snapshot_id = Some(snapshot_id.clone());
        status.last_local_hash = Some(content_hash.clone());
        status.last_key_fingerprint = key_fingerprint.clone();
        status.last_error = None;
        status.last_error_source = None;
    });
    logging::log(
        "[CloudSync]",
        &format!(
            "☁️ 云同步上传完成：{}（{} 字节，账号 {} 条）",
            transport.display_path(),
            snapshot_bytes.len(),
            snapshot.tables.get("accounts").map(Vec::len).unwrap_or(0)
        ),
    );

    Ok(json!({
        "status": "uploaded",
        "remotePath": transport.display_path(),
        "bytes": snapshot_bytes.len(),
        "snapshotId": snapshot_id,
        "rows": snapshot.row_counts(),
    }))
}

// ─── 下载 ─────────────────────────────────────────────────────

/// 下载：校验 manifest → 拉快照 → 校验 → 安全备份 → 应用 → 重装内存态。
pub(crate) async fn download(db: &Db, config_dir: &std::path::Path) -> Result<Value, String> {
    let _guard = sync_lock().lock().await;
    let settings = require_enabled()?;
    download_locked(db, config_dir, &settings).await
}

/// 下载本体（调用方已持锁）。
///
/// 下载期间用 `auto_sync` 的抑制守卫包住「应用 + 重装」这一段：替换数据会写
/// 三张表，若不抑制，自动同步会立刻把刚下载的内容再传一遍（白跑一趟，而且
/// 若远端是别人传的，这一传就把「我刚下载的」变成「我覆盖的」）。
pub(crate) async fn download_locked(
    db: &Db,
    config_dir: &std::path::Path,
    settings: &settings::CloudSyncSettings,
) -> Result<Value, String> {
    let transport = Transport::from_settings(settings)?;
    let Some((manifest_bytes, manifest)) = transport.get_manifest().await? else {
        return Err("远端还没有可下载的同步数据".to_string());
    };
    manifest::validate_compat(&manifest)?;

    // 密钥检查：远端带了密钥封装、且与本机不一致时必须先 import-key。
    // 这里**不**自动用本机保存的同步密码去解 —— 那个密码是「上传时用的」，
    // 远端这份可能来自另一台机器（用了不同的密码）。让用户显式输入，失败
    // 的提示才准确（「同步密码不正确」而不是「账号解不开」）。
    let local_fingerprint = crypto::key_fingerprint(SecretScope::Accounts);
    if manifest.key_fingerprint.is_some() && manifest.key_fingerprint != local_fingerprint {
        return Err(KEY_REQUIRED.to_string());
    }

    let snapshot_bytes = transport
        .get_artifact(REMOTE_SNAPSHOT, &manifest.artifacts)
        .await?;
    let snapshot: snapshot::Snapshot = serde_json::from_slice(&snapshot_bytes)
        .map_err(|error| format!("远端 snapshot.json 解析失败: {error}"))?;

    // 安全备份：用**当前**数据构建一份快照写到 backups/
    let safety = {
        let current = snapshot::build(db)?;
        snapshot::safety_backup(&current, config_dir)?
    };

    let applied = {
        let _suppress = auto_sync::SuppressionGuard::new();
        let applied = snapshot::apply(db, &snapshot)?;
        // 配置快照重读：下载改的就是 kv，不重读的话本次运行还是旧配置
        crate::server::config::reload();
        // 余额事实表重读：选路依赖它（账号可能被换掉了一半）
        crate::server::core::usage_records::reload_facts();
        applied
    };

    let manifest_hash = sha256_hex(&manifest_bytes);
    let snapshot_id = manifest.snapshot_id.clone();
    let device = manifest.device_name.clone();
    let created_at = manifest.created_at.clone();
    settings::update_status(|status| {
        status.last_sync_at = Some(logging::now_ms());
        status.last_action = Some("download".to_string());
        status.last_snapshot_id = Some(snapshot_id.clone());
        status.last_local_hash = Some(manifest_hash.clone());
        status.last_key_fingerprint = manifest.key_fingerprint.clone();
        status.last_error = None;
        status.last_error_source = None;
    });
    logging::log(
        "[CloudSync]",
        &format!(
            "☁️ 云同步下载完成：来自 {}（{}），替换前数据已备份到 backups/{}",
            if device.is_empty() { "未知设备" } else { &device },
            if created_at.is_empty() { "未知时间" } else { &created_at },
            safety
        ),
    );

    Ok(json!({
        "status": "downloaded",
        "remotePath": transport.display_path(),
        "deviceName": device,
        "createdAt": created_at,
        "snapshotId": snapshot_id,
        "applied": applied,
        "safetyBackup": safety,
    }))
}

/// 下载需要先导入密钥时返回的**哨兵错误**。
///
/// 用字符串哨兵而不是自定义错误类型：本模块的对外契约是 `Result<_, String>`
/// （与其它 API 模块一致），而界面要按它弹「输入同步密码」。哨兵值同时是
/// 错误文案，用户直接点下载时也能看懂。
pub(crate) const KEY_REQUIRED: &str = "远端快照使用了另一把账号加密密钥，请先输入同步密码导入密钥";

/// 用同步密码解出远端账号密钥并装到本机。
pub(crate) async fn import_key(password: &str) -> Result<Value, String> {
    if password.is_empty() {
        return Err("同步密码不能为空".to_string());
    }
    let _guard = sync_lock().lock().await;
    let settings = settings::load();
    // 只校验远端凭据：这一步的作用就是「新机器还没有同步密码」时先把密钥装进来
    settings.validate_remote()?;
    let transport = Transport::from_settings(&settings)?;
    let Some((_, manifest)) = transport.get_manifest().await? else {
        return Err("远端还没有可下载的同步数据".to_string());
    };
    if !manifest.artifacts.contains_key(REMOTE_KEY_FILE) {
        return Err("远端这份快照没有携带账号密钥（上传时本机还没有保存过账号）".to_string());
    }
    let envelope = transport
        .get_artifact(REMOTE_KEY_FILE, &manifest.artifacts)
        .await?;
    let key = keyfile::open_key(&envelope, password)?;
    let fingerprint = crypto::fingerprint_of(&key);
    if manifest.key_fingerprint.as_deref() != Some(fingerprint.as_str()) {
        return Err(
            "解出的密钥与远端 manifest 记录的指纹不一致，文件可能已损坏，请重新上传".to_string(),
        );
    }
    let backup = keyfile::install_key(&key)?;
    logging::log(
        "[CloudSync]",
        &format!(
            "🔑 已从云同步导入账号密钥（指纹 {fingerprint}{}）",
            backup
                .as_ref()
                .map(|name| format!("，原密钥已备份为 {name}"))
                .unwrap_or_default()
        ),
    );
    Ok(json!({
        "status": "imported",
        "keyFingerprint": fingerprint,
        "backup": backup,
    }))
}

/// 记录一次失败（手动 / 自动）。状态落库 + 事件日志。
///
/// 日志用 `warn` 级而不是 `error`：同步失败通常可重试（网络抖动、密码填错），
/// 把它当 error 会让日志页的错误徽标长期亮着，掩盖真正的服务故障。
pub(crate) fn record_failure(error: &str, source: &str) {
    settings::update_status(|status| {
        status.last_error = Some(error.to_string());
        status.last_error_source = Some(source.to_string());
    });
    logging::log_with_level(
        "[CloudSync]",
        &format!(
            "⚠️ 云同步{}失败：{error}",
            if source == "auto" { "（自动）" } else { "" }
        ),
        "warn",
    );
}
