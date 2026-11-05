//! 软件更新：共享 GitHub 访问策略、版本检查与经过校验的安装包下载。

mod check;
mod download;
pub mod capabilities;
pub mod systemd;
pub mod windows_service;
mod service_task;
#[cfg(any(target_os = "linux", windows))]
#[path = "systemd/trust.rs"]
mod service_trust;
pub(crate) mod source;
mod source_state;
mod selection;
mod probe;
mod integrity;
// 更新源索引：从索引仓库的 `app.json` 读「构建产物放在哪个仓库」。
// 与插件商店共用信任根仓库、两个文件（见模块头）。
mod index;
// 公网出网的候选出口实现（`fetch_public`）也被 `core::model_meta`（models.dev
// 元数据刷新）复用：那条链路回答的是「这台机器的公网怎么出去」，与访问哪个
// 公网站点无关，为它复制一份候选解析只会多一处会漂移的知识。因此对本 crate
// 可见，对外仍不暴露。
pub(crate) mod client;
// 容器内自更新：headless 入口注册重启钩子（`set_restart_hook`）要用它，
// 所以对本 crate 之外也要可见（桌面形态不注册，该模块的函数在非 Linux 上
// 一律返回「不支持」）。
pub mod selfupdate;
mod token;
mod version;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value};

use crate::server::{config, logging};

pub use version::{
    api_base, asset_suffixes, assert_downloadable, compare_versions, custom_api_base, desktop_shell,
    installer_kind, installer_suffixes, is_prerelease_version, parse_version, pick_android_apk,
    pick_asset_by_name, pick_installer, safe_file_name, server_bundle_name, set_desktop_shell,
    UpdateError, DEFAULT_REPO, GITHUB_API, MAX_INSTALLER_BYTES, SOURCE_REPO,
};
pub use integrity::download_plan;
/// 「更新设置」弹窗的 GitHub 令牌接口（api::update 走这两条；加解密本体
/// 只在 token 模块内部，见它的模块头）
pub use token::{set_token, status_json};

/// 请求 GitHub 的 UA（Node 版字面量，随项目改名同步）
const USER_AGENT: &str = "agent2api-local-proxy";

/// 强制更新策略独立于用户的普通版本检查开关，每五分钟刷新一次。
const INDEX_RETRY_MS: i64 = 5 * 60 * 1000;

/// 本应用的当前版本号（编译期取自 Cargo.toml，发布流程与 tauri.conf.json
/// 同步更新）。壳的 `checkUpdate` 命令用的是运行时 package_info —— 两者常态
/// 一致；后端定时检查（scheduled_tasks）拿不到 Tauri 句柄，用这一份自足。
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

// ─── 下载任务状态 ───────────────────────────────────────────

/// 下载任务（对应 Node 版闭包里的 `task` 对象）。
#[derive(Clone, Debug)]
struct Task {
    active: bool,
    done: bool,
    canceled: bool,
    error: Option<String>,
    filename: String,
    /// 落盘路径；取消或失败后置 None（与 Node 的 `current.path = null` 一致）
    path: Option<String>,
    received: u64,
    total: u64,
    selecting: bool,
    source: Option<String>,
}

impl Task {
    fn to_json(&self) -> Value {
        let mut map = Map::new();
        map.insert("active".to_string(), Value::Bool(self.active));
        map.insert("done".to_string(), Value::Bool(self.done));
        map.insert("canceled".to_string(), Value::Bool(self.canceled));
        map.insert(
            "error".to_string(),
            self.error.clone().map(Value::String).unwrap_or(Value::Null),
        );
        map.insert("filename".to_string(), Value::String(self.filename.clone()));
        map.insert(
            "path".to_string(),
            self.path.clone().map(Value::String).unwrap_or(Value::Null),
        );
        map.insert("received".to_string(), Value::from(self.received));
        map.insert("total".to_string(), Value::from(self.total));
        map.insert("selecting".to_string(), Value::Bool(self.selecting));
        map.insert("source".to_string(), json!(self.source));
        // percent：总量未知时为 0；已知时向上取整到 100 封顶
        // （Node: `task.total > 0 ? Math.min(100, Math.round(received/total*100)) : 0`）
        let percent = if self.total > 0 {
            let raw = (self.received as f64 / self.total as f64 * 100.0).round() as i64;
            raw.clamp(0, 100)
        } else {
            0
        };
        map.insert("percent".to_string(), Value::from(percent));
        Value::Object(map)
    }
}

/// 管理器内部状态：下载任务、取消标志、最近一次拉到的 Release、仓库名。
struct Inner {
    task: Option<Task>,
    /// 与任务一一对应的取消标志（Node 用 AbortController，Rust 用原子标志）
    cancel_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// 最近一次拉到的 Release（内存缓存）
    latest: Option<Value>,
    repository: String,
    /// `repository` 来自 `WORKBUDDY_UPDATE_REPO` 环境变量（显式覆盖）。
    ///
    /// 为真时**不读索引**：用户的覆盖绝不能被索引悄悄改回官方仓库。
    repository_explicit: bool,
    /// `repository` 是从索引里解析出来的（`true`）还是内置默认值（`false`）。
    /// 只用于日志口径。
    repository_from_index: bool,
    /// 上次尝试解析索引的时刻（毫秒，0 = 还没试过）。与 [`INDEX_RETRY_MS`]
    /// 一起做节流。
    index_checked_at: i64,
    app_index: index::AppIndex,
    download_dir: PathBuf,
    /// 容器内更新已应用、但进程还没重启（只活在内存里，重启即自然清零）。
    ///
    /// 挡「同一版重复安装」：新版本已就位、只差重启时不再重复替换文件。
    pending_restart: bool,
    service_update_active: bool,
}

/// 更新管理器句柄：内部一把锁 + Clone（与 auto_checkin / models 同构）。
#[derive(Clone)]
pub struct UpdateManager {
    inner: Arc<Mutex<Inner>>,
}

impl UpdateManager {
    /// 构造管理器。`directory` 是配置目录（下载落在 `{directory}/updates`）。
    ///
    /// 仓库解析顺序与 Node 版一致：参数 > `WORKBUDDY_UPDATE_REPO` > DEFAULT_REPO
    /// （`String(repo || env || DEFAULT).trim()`）。
    pub fn new(directory: PathBuf) -> Self {
        let explicit = std::env::var("WORKBUDDY_UPDATE_REPO")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let app_index = if explicit.is_some() { index::AppIndex::default() } else { index::cached() };
        let repository = explicit.clone().unwrap_or_else(|| app_index.repo.clone());
        Self {
            inner: Arc::new(Mutex::new(Inner {
                task: None,
                cancel_flag: None,
                latest: None,
                repository,
                repository_explicit: explicit.is_some(),
                repository_from_index: false,
                index_checked_at: 0,
                app_index,
                download_dir: directory.join("updates"),
                pending_restart: false,
                service_update_active: false,
            })),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// 仓库名（`owner/repo`）
    pub fn repository(&self) -> String {
        self.lock().repository.clone()
    }

    pub fn reset_source_preference(&self) {
        source::clear_preferences();
        self.lock().index_checked_at = 0;
    }

    /// 解析本次检查要用的仓库，必要时先读一次索引。
    ///
    /// ── 优先级 ──────────────────────────────────────────────────
    /// 1. `WORKBUDDY_UPDATE_REPO` 环境变量：显式覆盖，最高优先，**且不读索引**
    ///    —— 内网镜像 / 离线演练要能完全绕开官方源；
    /// 2. 索引里的 `app.repo`（见 [`index::resolve_app_repo`]）；
    /// 3. 内置 [`DEFAULT_REPO`]。
    ///
    /// ── 为什么带节流 ────────────────────────────────────────────
    /// 定时检查可能几分钟一轮，而索引是一份几乎不变的几百字节文件。每次都读
    /// 只是白打 raw.githubusercontent.com（顺带消耗它的限额）。所以读成功后
    /// [`INDEX_RETRY_MS`] 内不再读；**读失败**也一样等这么久再试 —— 失败多半
    /// 是网络不通，立刻重试只会连着失败。
    ///
    /// 注意节流只影响「多久读一次」，不影响「解析后的值」：一旦读到过，就用它
    /// 覆盖 `repository` 并一直用下去（进程重启后重新读）。
    async fn resolve_repository(&self) -> String {
        let (current, explicit) = { let guard = self.lock(); (guard.repository.clone(), guard.repository_explicit) };
        if explicit { return current; }
        let now = logging::now_ms();
        {
            let mut guard = self.lock();
            if guard.index_checked_at != 0 && now.saturating_sub(guard.index_checked_at) < INDEX_RETRY_MS { return current; }
            guard.index_checked_at = now;
        }
        let resolved = index::resolve().await;
        let repository = resolved.repo.clone();
        let changed = {
            let mut guard = self.lock();
            let changed = guard.repository != repository;
            guard.repository = repository.clone();
            guard.repository_from_index = true;
            guard.app_index = resolved;
            changed
        };
        if changed { logging::verbose("[Update]", &format!("更新源仓库改为 {repository}（来自索引）")); }
        repository
    }

    pub fn force_update(&self, current: &str) -> Option<Value> {
        let guard = self.lock();
        if guard.repository_explicit { return None; }
        guard.app_index.required(current)
    }

    pub async fn refresh_policy(&self, current: &str) -> Value {
        self.resolve_repository().await;
        json!({ "currentVersion": current, "forceUpdate": self.force_update(current), "repository": self.repository() })
    }

    /// 下载目录（`{config_dir}/updates`）—— 与壳侧 `update::download_dir()` 同源
    pub fn download_dir(&self) -> PathBuf {
        self.lock().download_dir.clone()
    }

    /// 组装 check 的响应（对应 buildCheckResult，字段逐个对齐）
    fn build_check_result(&self, current_version: &str) -> Value {
        let version = current_version.trim().to_string();
        let (latest, repository, download_dir, task, pending_restart) = {
            let guard = self.lock();
            (
                guard.latest.clone(),
                guard.repository.clone(),
                guard.download_dir.clone(),
                guard.task.clone(),
                guard.pending_restart,
            )
        };
        let channels = config::update_channels();
        let force_update = self.force_update(current_version);
        let latest = latest.filter(|release| {
            let prerelease = release["prerelease"] == true
                || version::is_prerelease_version(release["tag"].as_str().unwrap_or(""));
            force_update.is_some() || channels.accepts(prerelease)
        });
        let latest_tag = latest
            .as_ref()
            .and_then(|value| value.get("tag"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let comparison = if latest.is_some() && !version.is_empty() {
            compare_versions(latest_tag, &version)
        } else {
            None
        };
        // 只有带 url 的资产才算「可下载」
        let asset = latest
            .as_ref()
            .and_then(|value| value.get("asset"))
            .filter(|value| {
                value.get("url").and_then(Value::as_str).map(|url| !url.is_empty()).unwrap_or(false)
            })
            .cloned();
        // 容器内更新包与它的 .sha256 伴随文件（非 Linux、或该发布没挂这两个
        // 资产时都是 None —— 面板据此退回「手动升级命令」的提示）
        let downloadable = |value: Option<&Value>| -> Option<Value> {
            value
                .filter(|item| {
                    item.get("url").and_then(Value::as_str).map(|url| !url.is_empty()).unwrap_or(false)
                })
                .cloned()
        };
        let server_asset = downloadable(latest.as_ref().and_then(|value| value.get("serverBundle")));
        let server_checksum =
            downloadable(latest.as_ref().and_then(|value| value.get("serverChecksums")));
        let latest_text = |key: &str| -> Value {
            match latest.as_ref().and_then(|value| value.get(key)) {
                Some(Value::String(text)) if !text.is_empty() => Value::String(text.clone()),
                Some(Value::Bool(flag)) => Value::Bool(*flag),
                _ => Value::Null,
            }
        };
        // 「这一版有没有给本机的包」的诊断：release 那半边（资产总数 / 后缀清单 /
        // 有没有被更新源挡掉）来自 check.rs 的 release_json；两个 expected* 是
        // **本机事实**，在这里现补 —— 它们与服务端包无关地跟着平台走，不该混进
        // 那份按仓库落库的 Release 快照（换台机器读缓存会读到别人的后缀）。
        let probe = latest.as_ref().and_then(|value| value.get("assetProbe"));
        let probe_field = |key: &str| -> Value {
            probe.and_then(|value| value.get(key)).cloned().unwrap_or(Value::Null)
        };
        let asset_probe = json!({
            "assetsTotal": probe_field("assetsTotal"),
            "suffixes": probe_field("suffixes"),
            "filteredBySource": probe_field("filteredBySource"),
            // 桌面安装包要的后缀（容器 / headless 是空表）；容器内更新包要的完整文件名
            // （非 Linux 是 null）—— 界面按 selfUpdate 二选一，两条消息各有各的说法
            "expectedSuffixes": version::installer_suffixes(),
            "expectedName": version::server_bundle_name(),
        });
        let capability = capabilities::current();
        let systemd_task = (capability.deployment == "systemd").then(systemd::status).flatten();
        let service_task = if capability.deployment == "windows-service" {
            windows_service::status()
        } else { systemd_task.clone() };
        json!({
            "updateCapability": capability,
            "serviceTask": service_task,
            "systemdTask": systemd_task,
            "forceUpdate": force_update,
            "currentVersion": if version.is_empty() { Value::Null } else { Value::String(version) },
            "latestVersion": if latest_tag.is_empty() { Value::Null } else { Value::String(latest_tag.to_string()) },
            // 无法解析版本号时不谎报「有更新」，界面按 null 显示为「无法比较」
            "hasUpdate": match comparison {
                Some(order) => Value::Bool(order > 0),
                None => Value::Null,
            },
            "comparison": match comparison {
                Some(order) if order > 0 => Value::String("newer".to_string()),
                Some(order) if order < 0 => Value::String("older".to_string()),
                Some(_) => Value::String("same".to_string()),
                None => Value::Null,
            },
            "releaseName": latest_text("name"),
            "notes": latest_text("notes"),
            "publishedAt": latest_text("publishedAt"),
            "pageUrl": latest_text("pageUrl"),
            "prerelease": latest
                .as_ref()
                .and_then(|value| value.get("prerelease"))
                .and_then(Value::as_bool)
                == Some(true),
            "asset": asset.clone().unwrap_or(Value::Null),
            // ── 手机端（APK）────────────────────────────────────
            // 与 `asset` 并列：网关同时服务电脑和手机，两边各取所需。
            // 手机端拿这个 GitHub 下载地址自己去下（手机自己出网，不经网关中转）。
            // 该版本没挂 apk 时是 null，手机端据此藏起「立即更新」。
            "androidAsset": downloadable(latest.as_ref().and_then(|value| value.get("androidAsset")))
                .unwrap_or(Value::Null),
            // ── 容器内更新（Linux / Docker）──────────────────────
            // selfUpdate 说的是「这个部署能不能原地更新」（平台 + 环境开关），
            // serverAsset 说的是「这一版有没有可下的包」—— 两者分开，面板才能
            // 区分「本部署不支持」与「该版本没提供更新包」这两种不同的提示。
            "selfUpdate": selfupdate::supported(),
            "serverAsset": server_asset.unwrap_or(Value::Null),
            "serverChecksumUrl": server_checksum.unwrap_or(Value::Null),
            // 「有新版但没有可下载的包」的原因（界面拼在提示里，见 update-panel
            // 与 update-modal 的 assetMissingReason）
            "assetProbe": asset_probe,
            // 已应用、只差重启：界面据此把按钮换成「立即重启」，
            // 而不是再给一次注定 400 的「下载并更新」
            "pendingRestart": pending_restart,
            // 安装包所在的仓库（构建账号下），只用于「更新从哪来」的说明。
            // 界面的作者行 / Star 入口**不要**用它 —— 见下一行。
            "repository": repository,
            // 项目源码仓库：界面上的「关于作者」「收藏项目」由它拼链接。
            // 构建账号只是借来跑 Actions 的，主页上没有代码，不能拿来当项目地址。
            "sourceRepository": SOURCE_REPO,
            "downloadSupported": asset.is_some(),
            // 安装包形态（"nsis" / "dmg"）：界面据此决定文案 ——
            // Windows 要提示 UAC 提权、macOS 是挂载磁盘映像后手动拖进应用程序
            "installerKind": installer_kind(),
            "downloadDir": download_dir.to_string_lossy(),
            "task": task.map(|task| task.to_json()).unwrap_or(Value::Null),
        })
    }

    // ─── 容器内更新（Docker / Linux headless）────────────────────
    //
    // 与上面的「下载安装包」是两条路：那条给桌面端下安装器（下完由壳启动
    // 安装程序），这条给容器里的自己换文件（下完由本进程原地替换 + 重启）。
    // 两条入口都是 POST /api/update/{install,restart} 的唯一实现，
    // 细节与安全边界见 selfupdate 模块头。

    /// 应用已下载的更新包：校验 → 解包 → 原子替换二进制与 ui 目录。
    ///
    /// 下载目录里的文件按**固定文件名**取（`safe_file_name(包名)`），不接受
    /// 调用方传路径 —— 少一个「把任意文件当成更新包解包替换」的入口。
    pub async fn install_downloaded(&self, ui_dir: Option<&Path>) -> Result<Value, UpdateError> {
        if !selfupdate::supported() {
            return Err(UpdateError::with_status(
                400,
                "当前部署不支持容器内更新：需要在容器中运行并设置 AGENT2API_SELF_UPDATE=1",
            ));
        }
        // 下载还在跑的时候文件是半截的：宁可让人等，也不要拿半个包去替换
        if self.lock().task.as_ref().map(|task| task.active).unwrap_or(false) {
            return Err(UpdateError::with_status(400, "更新包还在下载中，请等下载完成后再安装"));
        }
        // 已经应用过一次、还没重启：只允许重启，不再重复替换文件。
        if self.lock().pending_restart {
            return Err(UpdateError::with_status(
                400,
                "本次更新已经应用，请先重启服务（面板上的重启，或宿主机 docker compose restart）",
            ));
        }
        let bundle = selfupdate::bundle_name().expect("supported 已判定平台与架构");
        // 从**内存里那份 release JSON** 取三个值：包地址、校验文件地址、版本号。
        // 键名是 check.rs 存进去的那套（`serverBundle` / `serverChecksums` / `tag`），
        // **不是** check 响应里给界面用的扁平字段（`serverAsset` / `serverChecksumUrl` /
        // `latestVersion`）—— 两套名字混用的后果是永远读空、永远报「没有提供校验文件」。
        let (bundle_url, checksum_url, version) = {
            let guard = self.lock();
            let latest = guard.latest.clone();
            let asset_url = |key: &str| -> String {
                latest
                    .as_ref()
                    .and_then(|value| value.get(key))
                    .and_then(|value| value.get("url"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            };
            let tag = latest
                .as_ref()
                .and_then(|value| value.get("tag"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            (asset_url("serverBundle"), asset_url("serverChecksums"), tag)
        };
        // 该发布没挂包（老版本 Release）：给界面一句能照做的提示，而不是含糊的失败
        if bundle_url.is_empty() {
            return Err(UpdateError::with_status(
                400,
                "本次发布没有提供容器内更新包，请在宿主机执行 docker compose pull && docker compose up -d 升级",
            ));
        }
        match compare_versions(&version, CURRENT_VERSION) {
            Some(order) if order < 0 => {
                return Err(UpdateError::with_status(400, "容器内更新不支持降级到较旧版本"));
            }
            None => {
                return Err(UpdateError::with_status(400, "更新包版本号无法比较，已放弃安装"));
            }
            _ => {}
        }
        if checksum_url.is_empty() && !source::is_resource(&bundle_url) {
            return Err(UpdateError::with_status(
                400,
                "本次发布没有提供校验文件（.sha256），已放弃安装",
            ));
        }
        let archive = self.download_dir().join(safe_file_name(&bundle));
        if !archive.is_file() {
            return Err(UpdateError::with_status(400, "更新包尚未下载，请先点击「下载并更新」"));
        }
        let expected = match integrity::trusted_digest(&bundle_url).await {
            Some((hash, _)) => hash,
            None if !checksum_url.is_empty() => selfupdate::fetch_checksum(&checksum_url).await?,
            None => return Err(UpdateError::new("未取得官方校验值，已放弃安装")),
        };
        let actual = selfupdate::file_sha256(&archive)?;
        if expected != actual {
            logging::log(
                "[Update]",
                &format!("❌ 更新包 sha256 校验失败（期望 {expected}，实际 {actual}）"),
            );
            return Err(UpdateError::new(
                "更新包校验失败（sha256 不匹配），已放弃安装；可删掉该文件后重新下载",
            ));
        }
        let exe = selfupdate::current_exe_path()?;
        let outcome = selfupdate::apply(&archive, &exe, ui_dir)?;
        self.lock().pending_restart = true;
        logging::log(
            "[Update]",
            &format!(
                "✅ 已在容器内替换程序{}（v{version}），重启后生效",
                if outcome.ui_updated { "与界面文件" } else { "" }
            ),
        );
        Ok(json!({
            "applied": true,
            "version": version,
            "needRestart": true,
            "uiUpdated": outcome.ui_updated,
            "archive": archive.to_string_lossy(),
        }))
    }

    /// 请求重启进程（容器由 restart 策略拉起；见 selfupdate 的重启钩子）。
    pub fn request_restart(&self) -> Result<Value, UpdateError> {
        if !selfupdate::supported() {
            return Err(UpdateError::with_status(
                400,
                "当前部署不支持在网页端重启：请在宿主机执行 docker compose restart",
            ));
        }
        if !selfupdate::request_restart() {
            return Err(UpdateError::with_status(
                400,
                "重启通道未就绪：本进程不是由 headless 容器入口启动的",
            ));
        }
        Ok(json!({ "restarting": true }))
    }
}

// ─── 进程级句柄 ─────────────────────────────────────────────

/// 进程级更新管理器（与 config / logging 同一模式）：
/// bootstrap 时装入一次，路由层与服务启动路径共用同一实例。
static GLOBAL: std::sync::OnceLock<UpdateManager> = std::sync::OnceLock::new();

/// 更新检查的排期按仓库隔离，缓存里的版本比较仍使用当前二进制版本。
pub fn global_check_key() -> String {
    GLOBAL.get().map(UpdateManager::check_key)
        .unwrap_or_else(|| format!("updateCheck:{DEFAULT_REPO}"))
}

pub fn required_update() -> Option<Value> {
    GLOBAL.get().and_then(|manager| manager.force_update(CURRENT_VERSION))
}

/// 初始化进程级管理器（幂等）
pub fn init_global(manager: UpdateManager) -> UpdateManager {
    let plugins_before = crate::server::core::plugin_host::settings::current();
    if config::sync_installed_update_channel(CURRENT_VERSION) {
        manager.invalidate_channel();
        crate::server::core::plugin_host::settings::notify_changed(plugins_before);
    }
    if GLOBAL.set(manager.clone()).is_ok() {
        let updater = manager.clone();
        // 必须走 `crate::spawn_task`：本函数由 `ServerState::bootstrap` 调用，
        // 而 headless（容器 / 命令行）的 bootstrap 跑在**建 Tokio 运行时之前**
        // 的主线程上 —— 裸 `tokio::spawn` 在那里直接 panic，release 是
        // panic=abort，进程当场退出，restart 策略把它拉起来又是同一次崩溃
        // （v3.3.0-beta.5 容器内更新后无限重启的根因）。
        crate::spawn_task(async move {
            loop {
                let _ = updater.refresh_policy(CURRENT_VERSION).await;
                tokio::time::sleep(std::time::Duration::from_millis(INDEX_RETRY_MS as u64)).await;
            }
        });
    }
    manager
}
