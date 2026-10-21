//! 云同步的**设置与状态**（kv 保留键 `cloudSync`）与配置读写。
//!
//! ── 为什么不放进 `config` 的顶层键 ───────────────────────────
//! 它含远端凭据与同步密码的信封，属于「其它零散状态」；更关键的是**它自己
//! 不能被云同步上传**（否则远端凭据会被传到远端，换机时还会被旧凭据覆盖）。
//! 放进 `RESERVED_KV_KEYS` 之后，快照的键筛选天然排除它 —— 这条安全性靠
//! 「键的类别」表达，而不是靠一份容易漏改的黑名单。
//!
//! ── 密码 / Secret 的存放 ────────────────────────────────────
//! 全部经 `crypto::seal`（`SecretScope::CloudSync`）加密后落库，明文只在
//! 「保存的那一刻」与「发起请求的那一刻」存在于内存。界面读设置时只回
//! `hasPassword` / `hasSecretKey`，绝不回显 —— 与 `core::update::token` 的
//! `status_json` 同一取向。
//!
//! ── 为什么同步密码是必填 ────────────────────────────────────
//! 它封装的是**账号加密密钥**（`account.key`）。允许留空就等于允许把账号
//! 密钥以可被任何拿到存储权限的人解开的形式上云；而本项目所有账号凭证都是
//! 用那把密钥加密的，泄露它的后果与直接泄露全部账号相同。

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::server::core::crypto::{self, SecretScope};
use crate::server::db::Db;

use super::manifest;

/// `kv` 表里的键名（保留键，见 `db::schema::RESERVED_KV_KEYS`）
pub(crate) const KV_KEY: &str = "cloudSync";

/// 服务商标识
pub(crate) const PROVIDER_WEBDAV: &str = "webdav";
pub(crate) const PROVIDER_S3: &str = "s3";

/// 默认远程根目录。与 cc-switch 的 `cc-switch-sync` 同一思路：给一个**不会被
/// 别的程序占用**的名字，用户不改也不会撞上同目录下其它应用的文件。
pub(crate) const DEFAULT_REMOTE_ROOT: &str = "agent2api-sync";
/// 默认档案名（多套配置分开存放时改它）
pub(crate) const DEFAULT_PROFILE: &str = "default";

/// 同步密码最短长度。8 位是底线：它派生出加密账号密钥的对称密钥，
/// 太短会让「云盘泄露 + 弱密码」变成可离线爆破的组合。
pub(crate) const MIN_PASSWORD_LEN: usize = 8;

/// WebDAV 设置（界面上那一组输入框）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WebDavConfig {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub username: String,
    /// `enc1:` 信封（见模块头）。空串 = 未设置
    #[serde(default)]
    pub password: String,
}

/// S3 兼容存储设置
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct S3Config {
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub bucket: String,
    #[serde(default)]
    pub access_key_id: String,
    /// `enc1:` 信封。空串 = 未设置
    #[serde(default)]
    pub secret_access_key: String,
    /// 自定义端点（`minio.example.com:9000`）；空 = AWS 官方
    #[serde(default)]
    pub endpoint: String,
}

/// 同步状态（服务端维护，界面只读）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyncStatus {
    /// 上次成功同步的时刻（毫秒）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync_at: Option<i64>,
    /// 上次成功的动作：`upload` / `download`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_action: Option<String>,
    /// 上次成功的快照 id
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_snapshot_id: Option<String>,
    /// 上次上传时本地快照的哈希（自动同步用它判断「内容没变」）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_local_hash: Option<String>,
    /// 上次上传时账号密钥的指纹（本机密钥换了就必须重传）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_key_fingerprint: Option<String>,
    /// 上次失败的原因
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// 失败来源：`manual` / `auto`。界面据此决定是否常驻展示（自动同步失败
    /// 用户没盯着，必须留痕；手动失败当场就有 toast）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_source: Option<String>,
}

/// 整份云同步设置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CloudSyncSettings {
    /// 当前生效的服务商：`webdav` / `s3`。两套配置同时保留，切换不丢已填内容
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default)]
    pub webdav: WebDavConfig,
    #[serde(default)]
    pub s3: S3Config,
    /// 同步密码的 `enc1:` 信封。空串 = 未设置
    #[serde(default)]
    pub sync_password: String,
    /// 总开关。关着时手动上传 / 下载也会被拒（与 cc-switch 的 `enabled` 同义）
    #[serde(default)]
    pub enabled: bool,
    /// 本地数据变化后自动上传
    #[serde(default)]
    pub auto_sync: bool,
    #[serde(default = "default_remote_root")]
    pub remote_root: String,
    #[serde(default = "default_profile")]
    pub profile: String,
    #[serde(default)]
    pub status: SyncStatus,
}

fn default_provider() -> String {
    PROVIDER_WEBDAV.to_string()
}

fn default_remote_root() -> String {
    DEFAULT_REMOTE_ROOT.to_string()
}

fn default_profile() -> String {
    DEFAULT_PROFILE.to_string()
}

impl Default for CloudSyncSettings {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            webdav: WebDavConfig::default(),
            s3: S3Config::default(),
            sync_password: String::new(),
            enabled: false,
            auto_sync: false,
            remote_root: default_remote_root(),
            profile: default_profile(),
            status: SyncStatus::default(),
        }
    }
}

impl CloudSyncSettings {
    /// 收口空白字段并补默认值（保存时调用）
    pub(crate) fn normalize(&mut self) {
        if self.provider != PROVIDER_S3 {
            self.provider = PROVIDER_WEBDAV.to_string();
        }
        self.webdav.base_url = self.webdav.base_url.trim().to_string();
        self.webdav.username = self.webdav.username.trim().to_string();
        self.s3.region = self.s3.region.trim().to_string();
        self.s3.bucket = self.s3.bucket.trim().to_string();
        self.s3.access_key_id = self.s3.access_key_id.trim().to_string();
        self.s3.endpoint = self.s3.endpoint.trim().to_string();
        self.remote_root = self.remote_root.trim().to_string();
        self.profile = self.profile.trim().to_string();
        if self.remote_root.is_empty() {
            self.remote_root = default_remote_root();
        }
        if self.profile.is_empty() {
            self.profile = default_profile();
        }
    }

    /// 保存前的校验：只校验**当前服务商**的必填项。
    ///
    /// 另一家的配置可能只填了一半（用户切过去看过又切回来），不该因此拦住保存 ——
    /// 但它也不会被用到，因为 `provider` 只有一个值。
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.sync_password.is_empty() {
            return Err("请设置同步密码（用于加密上云的账号密钥）".to_string());
        }
        self.validate_remote()
    }

    /// 只校验「能连上远端」所需的部分（不含同步密码）。
    ///
    /// ── 为什么下载 / 远端预览不能用完整校验 ────────────────────
    /// 新机器第一次下载时，本机**还没有**同步密码（它本来就不存在云端，
    /// 设计如此）。完整校验会把「预览远端」和「下载」在用户输入密码之前就
    /// 挡掉，而输入密码那一步（`import-key`）本身又需要先连上远端读密钥文件 ——
    /// 于是整个换机流程死锁。所以这两条路径只要求远端地址 / 凭据齐全。
    pub(crate) fn validate_remote(&self) -> Result<(), String> {
        manifest::remote_dir_segments(&self.remote_root, &self.profile)?;
        match self.provider.as_str() {
            PROVIDER_S3 => {
                if self.s3.bucket.is_empty() {
                    return Err("S3 存储桶不能为空".to_string());
                }
                if self.s3.region.is_empty() {
                    return Err("S3 区域不能为空（Cloudflare R2 填 auto）".to_string());
                }
                if self.s3.access_key_id.is_empty() {
                    return Err("S3 Access Key ID 不能为空".to_string());
                }
                if self.s3.secret_access_key.is_empty() {
                    return Err("S3 Secret Access Key 不能为空".to_string());
                }
            }
            _ => {
                if self.webdav.base_url.is_empty() {
                    return Err("WebDAV 地址不能为空".to_string());
                }
                if self.webdav.username.is_empty() {
                    return Err("WebDAV 用户名不能为空".to_string());
                }
            }
        }
        Ok(())
    }

    /// 界面形态：密码 / Secret 只给「有没有」，绝不回显。
    pub(crate) fn public_json(&self) -> Value {
        json!({
            "provider": self.provider,
            "webdav": {
                "baseUrl": self.webdav.base_url,
                "username": self.webdav.username,
                "hasPassword": !self.webdav.password.is_empty(),
            },
            "s3": {
                "region": self.s3.region,
                "bucket": self.s3.bucket,
                "accessKeyId": self.s3.access_key_id,
                "endpoint": self.s3.endpoint,
                "hasSecretKey": !self.s3.secret_access_key.is_empty(),
            },
            "hasSyncPassword": !self.sync_password.is_empty(),
            "enabled": self.enabled,
            "autoSync": self.auto_sync,
            "remoteRoot": self.remote_root,
            "profile": self.profile,
            "status": self.status,
            "remotePath": manifest::remote_dir_display(&self.remote_root, &self.profile),
            "keyFingerprint": crypto::key_fingerprint(SecretScope::Accounts),
        })
    }

    /// 解出同步密码明文（未设置 → `None`）
    pub(crate) fn sync_password_plain(&self) -> Result<Option<String>, String> {
        if self.sync_password.is_empty() {
            return Ok(None);
        }
        let bytes = crypto::open(&self.sync_password, SecretScope::CloudSync)?;
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| "同步密码不是合法 UTF-8 文本".to_string())
    }

    /// 解出 WebDAV 密码明文（未设置 → 空串）
    pub(crate) fn webdav_password_plain(&self) -> Result<String, String> {
        if self.webdav.password.is_empty() {
            return Ok(String::new());
        }
        let bytes = crypto::open(&self.webdav.password, SecretScope::CloudSync)?;
        String::from_utf8(bytes).map_err(|_| "WebDAV 密码不是合法 UTF-8 文本".to_string())
    }

    /// 解出 S3 Secret 明文（未设置 → 空串）
    pub(crate) fn s3_secret_plain(&self) -> Result<String, String> {
        if self.s3.secret_access_key.is_empty() {
            return Ok(String::new());
        }
        let bytes = crypto::open(&self.s3.secret_access_key, SecretScope::CloudSync)?;
        String::from_utf8(bytes).map_err(|_| "S3 Secret Access Key 不是合法 UTF-8 文本".to_string())
    }
}

// ─── 进程级库句柄 ─────────────────────────────────────────────

static DB: OnceLock<Option<Db>> = OnceLock::new();

/// 装库句柄（`bootstrap` 里调用一次，与其它 kv 状态模块同形）
pub(crate) fn install(db: Option<Db>) {
    let _ = DB.set(db);
}

fn database() -> Option<&'static Db> {
    DB.get().and_then(Option::as_ref)
}

// ─── 读写 ─────────────────────────────────────────────────────

/// 读整份设置。键不存在 / 解析失败 / 库不可用一律回落默认值 ——
/// 与 `settings::load` 的容错取向一致（读不出来 = 还没配过）。
pub(crate) fn load() -> CloudSyncSettings {
    let Some(db) = database() else {
        return CloudSyncSettings::default();
    };
    let text = db
        .with(|conn| {
            conn.query_row(
                "SELECT value FROM kv WHERE key = ?1",
                rusqlite::params![KV_KEY],
                |row| row.get::<_, String>(0),
            )
            .ok()
        })
        .flatten();
    let Some(text) = text else {
        return CloudSyncSettings::default();
    };
    let mut settings: CloudSyncSettings = serde_json::from_str(&text).unwrap_or_default();
    // 老版本 / 手改数据里的空路径要收口，否则远端路径会算成 `/v1/default`
    settings.normalize();
    settings
}

/// 整份写回（增删改都走它）。返回落库结果，调用方在锁外记日志。
pub(crate) fn save(settings: &CloudSyncSettings) -> Result<(), String> {
    let db = database().ok_or_else(|| "数据库不可用，云同步设置未保存".to_string())?;
    let text = serde_json::to_string(settings)
        .map_err(|error| format!("云同步设置序列化失败: {error}"))?;
    db.with(|conn| {
        conn.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![KV_KEY, text],
        )
        .map(|_| ())
        .map_err(|error| format!("保存云同步设置失败: {error}"))
    })
    .ok_or_else(|| "数据库不可用，云同步设置未保存".to_string())?
}

/// 只更新状态字段（成功后 / 失败后调用）。读-改-写整份。
pub(crate) fn update_status(patch: impl FnOnce(&mut SyncStatus)) {
    let mut settings = load();
    patch(&mut settings.status);
    if let Err(error) = save(&settings) {
        // 状态写不进去不该让同步本身报错（与 cc-switch 的 best-effort 同义），
        // 但必须留痕：下一次打开设置看不到「上次同步时间」时要知道原因。
        crate::server::logging::log("[CloudSync]", &format!("⚠️ 同步状态写入失败: {error}"));
    }
}

/// 保存请求：界面传来的形态 → 落库形态。
///
/// 密码 / Secret 的**沿用语义**：请求里 `hasPassword` 为真且 `password` 为空串
/// = 用户没动过这一栏，保留库里那份；`password` 非空 = 换新（重新加密）。
/// 这一条与 cc-switch 的 `passwordTouched` 同一手法，但方向反过来 ——
/// 那边是前端决定要不要发空串，这里是后端按「有没有新值」判断，前端少一个状态。
pub(crate) struct SaveInput {
    pub provider: String,
    pub webdav_base_url: String,
    pub webdav_username: String,
    pub webdav_password: String,
    pub s3_region: String,
    pub s3_bucket: String,
    pub s3_access_key_id: String,
    pub s3_secret_access_key: String,
    pub s3_endpoint: String,
    pub sync_password: String,
    pub enabled: bool,
    pub auto_sync: bool,
    pub remote_root: String,
    pub profile: String,
}

/// 应用一次保存请求，返回落库后的设置。
pub(crate) fn apply_save(input: SaveInput) -> Result<CloudSyncSettings, String> {
    let mut settings = load();
    settings.provider = input.provider;
    settings.webdav.base_url = input.webdav_base_url;
    settings.webdav.username = input.webdav_username;
    // 空串 = 沿用旧值；非空 = 换新（加密后落库）
    if !input.webdav_password.is_empty() {
        settings.webdav.password = crypto::seal(
            input.webdav_password.as_bytes(),
            SecretScope::CloudSync,
        )?;
    }
    settings.s3.region = input.s3_region;
    settings.s3.bucket = input.s3_bucket;
    settings.s3.access_key_id = input.s3_access_key_id;
    if !input.s3_secret_access_key.is_empty() {
        settings.s3.secret_access_key = crypto::seal(
            input.s3_secret_access_key.as_bytes(),
            SecretScope::CloudSync,
        )?;
    }
    settings.s3.endpoint = input.s3_endpoint;
    if !input.sync_password.is_empty() {
        if input.sync_password.chars().count() < MIN_PASSWORD_LEN {
            return Err(format!("同步密码至少需要 {MIN_PASSWORD_LEN} 位"));
        }
        settings.sync_password =
            crypto::seal(input.sync_password.as_bytes(), SecretScope::CloudSync)?;
    }
    settings.enabled = input.enabled;
    settings.auto_sync = input.auto_sync && input.enabled;
    settings.remote_root = input.remote_root;
    settings.profile = input.profile;
    settings.normalize();
    settings.validate()?;
    save(&settings)?;
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_paths_are_stable() {
        let settings = CloudSyncSettings::default();
        assert_eq!(settings.remote_root, "agent2api-sync");
        assert_eq!(settings.profile, "default");
        assert_eq!(settings.provider, PROVIDER_WEBDAV);
    }

    #[test]
    fn normalize_restores_blank_paths() {
        let mut settings = CloudSyncSettings {
            remote_root: "  ".to_string(),
            profile: "".to_string(),
            ..CloudSyncSettings::default()
        };
        settings.normalize();
        assert_eq!(settings.remote_root, DEFAULT_REMOTE_ROOT);
        assert_eq!(settings.profile, DEFAULT_PROFILE);
    }

    #[test]
    fn validate_rejects_blank_sync_password() {
        let mut settings = CloudSyncSettings {
            sync_password: String::new(),
            webdav: WebDavConfig {
                base_url: "https://dav.example.com/dav".to_string(),
                username: "u".to_string(),
                password: "x".to_string(),
            },
            ..CloudSyncSettings::default()
        };
        settings.normalize();
        assert!(settings.validate().is_err());
    }

    #[test]
    fn validate_only_checks_active_provider() {
        let mut settings = CloudSyncSettings {
            provider: PROVIDER_WEBDAV.to_string(),
            sync_password: "sealed".to_string(),
            webdav: WebDavConfig {
                base_url: "https://dav.example.com/dav".to_string(),
                username: "u".to_string(),
                password: "x".to_string(),
            },
            ..CloudSyncSettings::default()
        };
        settings.normalize();
        // S3 那半边全空也不该拦住 WebDAV 的保存
        assert!(settings.validate().is_ok());
    }
}
