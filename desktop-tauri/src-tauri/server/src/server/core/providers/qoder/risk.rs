//! Qoder 设备风控身份（`Cosy-MachineToken` / `Cosy-MachineCode` /
//! `Cosy-MachineType`）——国际版签到的关键依赖。
//!
//! ── 为什么需要它（CreditDaddy 的实测结论，issue #140）──────────
//! 国际版服务端**只对带风控头的请求**下发「每天领 100 Credits」的活动
//! （campaign）：不带时 `GET /sash/api/v1/me/campaigns` 里永远只有
//! `VIEW_DETAILS` 促销 —— 这正是本家旧注释「国际版没有签到计划」的真正
//! 原因（缺头，不是没活动）。中国版不需要风控头就能领，两个分支见
//! `checkin.rs`。
//!
//! ── 三个值从哪来 ────────────────────────────────────────────
//! 由 Qoder 的 **UMID 程序**（`runtime-info`）在本机生成：
//!   - Qoder 客户端自带：`<安装目录>/resources/umid/runtime-info(.exe)`；
//!   - 官方 qodercli 解压：`~/.qoder/.bin/umid-<平台>-<架构>-*/runtime-info`
//!     （国内版 `~/.qoder-cn/...`）—— 本文件发现顺序的第二优先级；
//!   - 网关数据目录：Linux/Docker 上前两个都不存在，`install_component`
//!     从官方 qodercli 的 npm 包里提取一份存到 `{config_dir}/qoder-umid/`。
//!
//! 调用方式照抄参考实现：`runtime-info <env>`（Windows / macOS 另带
//! `--account-stdin` 并把 `{"account":"<uid>"}` 写进 stdin），第一行输出
//! JSON `{machineToken, machineCode, machineType}`。`env` 是环境编号：
//! 客户端组件国际版 3 / 中国版 0；qodercli 组件国际版 4（SINGAPORE）/
//! 中国版 0。**同variant组件缺失时回落另一 variant 的**（CreditDaddy 的
//! `riskRunner` 同款：协议一致，只有 env 编号不同）。
//!
//! ── 缓存 ────────────────────────────────────────────────────
//! 客户端每 60±5 分钟轮换一次身份，这里保守缓存 50 分钟（按 region + uid）。
//! 进程内缓存足够：身份只在本机使用，重启重新生成一次没有副作用。
//!
//! ── panic=abort ─────────────────────────────────────────────
//! 零 unwrap/expect/panic；子进程输出不信任（按行解析 + 长度上限）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine;
use sha2::Digest;

use crate::server::errors::GatewayError;
use crate::server::logging;

use super::endpoints::Region;
use super::machine;

/// 身份缓存时长（客户端 60±5 分钟轮换，保守取 50 分钟）
const RISK_TTL_MS: i64 = 50 * 60 * 1000;
/// 单次组件调用超时（与客户端一致）
const RISK_TIMEOUT_MS: u64 = 25_000;
/// stdout 大小上限（正常输出就一行 JSON，超限视为组件异常）
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
/// 客户端版本读不到时的兜底（CreditDaddy 同值）
const DEFAULT_CLIENT_VERSION: &str = "0.2.5";

/// 一次生成的设备风控身份三件套
#[derive(Clone)]
pub struct RiskIdentity {
    pub machine_token: String,
    pub machine_code: String,
    pub machine_type: String,
}

/// 一个可调用的 UMID 组件
pub(crate) struct Component {
    pub(crate) exe: PathBuf,
    /// 环境编号（国际版 3/4、中国版 0，见模块头）
    pub(crate) env: u32,
    /// 来源标签（日志与状态接口用）：app / cli / data
    pub(crate) source: &'static str,
}

// ─── 组件发现 ───────────────────────────────────────────────

/// 用户主目录（与 `autoclaw::credentials::home_dir` 同口径）
fn home_dir() -> PathBuf {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn exists(path: &Path) -> bool {
    path.is_file()
}

/// 安装目录下的 resources 候选（Windows）：0.3+ 启动器把实际版本放在
/// `.qoder-versions\<版本>\resources`（新版本在前），顶层 resources 可能
/// 只是首装残留（CreditDaddy `resourceDirsIn` 同款）。
fn windows_resource_dirs(install: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(install.join(".qoder-versions")) {
        let mut versions: Vec<(String, PathBuf)> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.split('.').all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit())))
            })
            .map(|path| {
                let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("").to_string();
                (name, path)
            })
            .collect();
        versions.sort_by(|a, b| b.0.cmp(&a.0));
        out.extend(versions.into_iter().map(|(_, path)| path.join("resources")));
    }
    out.push(install.join("resources"));
    out
}

/// Windows 的 Qoder 客户端安装目录候选（CreditDaddy `candidateResourceDirs`）
fn windows_install_dirs(install_name: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        out.push(PathBuf::from(&local).join("Programs").join(install_name));
    }
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        out.push(PathBuf::from(&program_files).join(install_name));
    }
    if let Some(program_files_x86) = std::env::var_os("ProgramFiles(x86)") {
        out.push(PathBuf::from(&program_files_x86).join(install_name));
    }
    out
}

/// 在一个 resources 目录里找 UMID 组件
fn runtime_info_in(resources: &Path) -> Option<PathBuf> {
    let exe = if cfg!(windows) { "runtime-info.exe" } else { "runtime-info" };
    let path = resources.join("umid").join(exe);
    exists(&path).then_some(path)
}

/// 客户端（app 来源）的组件：先找本 variant 的安装目录，找不到再看另一
/// variant —— 协议一致只有 env 编号不同（CreditDaddy `apps.find || apps[0]`）。
fn app_component(region: Region) -> Option<Component> {
    let env = if region == Region::Global { 3 } else { 0 };
    // (installName, macOS app 名)：本 variant 在前
    let variants: &[(&str, &str)] = if region == Region::Global {
        &[("Qoder", "Qoder"), ("Qoder CN", "Qoder CN")]
    } else {
        &[("Qoder CN", "Qoder CN"), ("Qoder", "Qoder")]
    };
    for (install_name, mac_name) in variants {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if cfg!(windows) {
            for install in windows_install_dirs(install_name) {
                candidates.extend(windows_resource_dirs(&install));
            }
        } else if cfg!(target_os = "macos") {
            let home = home_dir();
            candidates.push(PathBuf::from("/Applications").join(format!("{mac_name}.app")).join("Contents/Resources"));
            candidates.push(home.join("Applications").join(format!("{mac_name}.app")).join("Contents/Resources"));
        } else {
            let slug = install_name.to_lowercase().replace(' ', "-");
            candidates.push(PathBuf::from("/opt").join(install_name).join("resources"));
            candidates.push(PathBuf::from("/opt").join(&slug).join("resources"));
            candidates.push(PathBuf::from("/usr/lib").join(&slug).join("resources"));
            candidates.push(PathBuf::from("/snap").join(&slug).join("current/resources"));
        }
        if let Some(exe) = candidates.iter().find_map(|resources| runtime_info_in(resources)) {
            return Some(Component { exe, env, source: "app" });
        }
    }
    None
}

/// qodercli 解压出的组件（cli 来源）：`~/.qoder/.bin/umid-*/runtime-info`
/// （国内版 `~/.qoder-cn`），本 variant 优先、缺失回落另一 variant。
/// env 用 qodercli 的取值：国际版 4（SINGAPORE）/ 中国版 0。
///
/// 注意布局与客户端不同：qodercli 把 runtime-info 直接解在 `umid-<平台>-<哈希>/`
/// 目录下（旁边还有 sgsdk.dll），**不再套一层 `umid/`** —— 客户端那层目录来自
/// `resources/umid/`，两边不能共用同一个查找函数。
fn cli_component(region: Region) -> Option<Component> {
    let env = if region == Region::Global { 4 } else { 0 };
    let homes: &[&str] = if region == Region::Global {
        &[".qoder", ".qoder-cn"]
    } else {
        &[".qoder-cn", ".qoder"]
    };
    let exe_name = if cfg!(windows) { "runtime-info.exe" } else { "runtime-info" };
    let home = home_dir();
    for dir in homes {
        let bin = home.join(dir).join(".bin");
        let Ok(entries) = std::fs::read_dir(&bin) else { continue };
        let mut dirs: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_dir()
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("umid-"))
            })
            .collect();
        dirs.sort();
        if let Some(exe) = dirs.into_iter().map(|dir| dir.join(exe_name)).find(|path| exists(path)) {
            return Some(Component { exe, env, source: "cli" });
        }
    }
    None
}

/// 网关数据目录里安装的组件（data 来源，Linux/Docker 的
/// `install_component` 落地路径）。env 与 cli 来源一致。
fn data_component() -> Option<Component> {
    let exe = crate::server::config::config_dir().join("qoder-umid").join("runtime-info");
    exists(&exe).then(|| Component { exe, env: 4, source: "data" })
}

/// 组件发现（按 app → cli → data 的顺序，同 variant 优先）。
pub(crate) fn find_component(region: Region) -> Option<Component> {
    app_component(region).or_else(|| cli_component(region)).or_else(|| data_component())
}

/// 本机是否能生成风控身份（界面提示用）
pub fn available(region: Region) -> bool {
    find_component(region).is_some()
}

// ─── 身份的辅助字段（请求头用）──────────────────────────────

/// `Cosy-MachineOS`：与客户端一致的 `{架构}_{平台}` 形态（x86_64_win32 等）
pub fn machine_os() -> String {
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "unknown"
    };
    let platform = if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    };
    format!("{arch}_{platform}")
}

/// `Cosy-MachineHostname`：可打印 ASCII，超长截断并附短哈希
/// （CreditDaddy `machineHostname` 同款清洗）。
pub fn machine_hostname() -> Option<String> {
    let raw = hostname();
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let short_hash = |text: &str| {
        let digest = sha2::Sha256::digest(text.as_bytes());
        digest.iter().take(4).map(|byte| format!("{byte:02x}")).collect::<String>()
    };
    let clip = |text: &str| -> String {
        if text.chars().count() <= 96 {
            return text.to_string();
        }
        let hash = short_hash(text);
        let head: String = text.chars().take(96 - 8 - 1).collect();
        let head = head.trim_end_matches(['-', ' ']).to_string();
        if head.is_empty() { format!("unknown-{hash}") } else { format!("{head}-{hash}") }
    };
    let printable = raw.chars().all(|ch| ('\x21'..='\x7e').contains(&ch) || ch == ' ')
        && raw.starts_with(|ch: char| ('\x21'..='\x7e').contains(&ch))
        && raw.ends_with(|ch: char| ('\x21'..='\x7e').contains(&ch));
    if printable {
        return Some(clip(raw));
    }
    let hash = short_hash(raw);
    let cleaned: String = raw
        .chars()
        .map(|ch| if ('\x21'..='\x7e').contains(&ch) { ch } else { '-' })
        .collect();
    let cleaned = cleaned.split('-').filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-");
    Some(clip(&if cleaned.is_empty() { format!("unknown-{hash}") } else { format!("{cleaned}-{hash}") }))
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default()
}

/// `Cosy-Version`：客户端版本（从已发现客户端的 build-manifest.json 读；
/// 只有 cli / data 组件时读不到，回落常量 —— 上游只拿它做画像参考）。
fn client_version(region: Region) -> String {
    let versions = if region == Region::Global {
        ["Qoder", "Qoder CN"]
    } else {
        ["Qoder CN", "Qoder"]
    };
    for name in versions {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if cfg!(windows) {
            candidates.extend(windows_install_dirs(name).iter().flat_map(|install| windows_resource_dirs(install)));
        } else if cfg!(target_os = "macos") {
            candidates.push(PathBuf::from("/Applications").join(format!("{name}.app")).join("Contents/Resources"));
        } else {
            candidates.push(PathBuf::from("/opt").join(name).join("resources"));
        }
        for resources in candidates {
            let manifest = resources.join("build-manifest.json");
            let Ok(text) = std::fs::read_to_string(&manifest) else { continue };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
            if let Some(version) = value.get("productVersion").and_then(|value| value.as_str()) {
                let version = version.trim();
                if !version.is_empty() {
                    return version.to_string();
                }
            }
        }
    }
    DEFAULT_CLIENT_VERSION.to_string()
}

// ─── 组件调用 ───────────────────────────────────────────────

/// 跑一次 runtime-info，解析首行 JSON。
///
/// Windows / macOS 带 `--account-stdin` 并把 `{"account":"<uid>"}` 写进 stdin
/// （与客户端一致）；Linux 只传 env。stdout 限制 1 MiB，整段调用限时 25 秒。
async fn run_component(component: &Component, uid: &str) -> Result<RiskIdentity, String> {
    use tokio::io::AsyncWriteExt;

    let mut command = tokio::process::Command::new(&component.exe);
    let with_stdin = cfg!(windows) || cfg!(target_os = "macos");
    if with_stdin {
        command.args([component.env.to_string(), "--account-stdin".to_string()]);
    } else {
        command.arg(component.env.to_string());
    }
    command.stdin(if with_stdin { std::process::Stdio::piped() } else { std::process::Stdio::null() });
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::null());
    // Windows 上隐藏控制台窗口（桌面端用户不该看到一个闪过的黑框）
    #[cfg(windows)]
    {
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW — tokio Command 同名扩展方法
    }

    let mut child = command.spawn().map_err(|error| format!("UMID 组件启动失败：{error}"))?;
    if with_stdin {
        if let Some(mut stdin) = child.stdin.take() {
            let payload = format!("{}\n", serde_json::json!({ "account": uid }));
            let _ = stdin.write_all(payload.as_bytes()).await;
            let _ = stdin.flush().await;
            // drop 关闭 stdin，组件读到 EOF 才会输出
        }
    }
    let output = tokio::time::timeout(Duration::from_millis(RISK_TIMEOUT_MS), child.wait_with_output())
        .await
        .map_err(|_| format!("UMID 组件调用超时（{RISK_TIMEOUT_MS} 秒）"))?
        .map_err(|error| format!("UMID 组件执行失败：{error}"))?;
    if output.stdout.len() > MAX_OUTPUT_BYTES {
        return Err("UMID 组件输出过大".to_string());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().next().unwrap_or("").trim();
    let parsed: serde_json::Value =
        serde_json::from_str(line).map_err(|_| format!("UMID 组件输出不是 JSON：{}", truncate(line, 120)))?;
    let pick = |key: &str| -> Result<String, String> {
        let value = parsed.get(key).and_then(|value| value.as_str()).unwrap_or("").trim().to_string();
        if value.is_empty() || value.len() > 4096 {
            return Err(format!("UMID 组件输出缺少 {key}"));
        }
        Ok(value)
    };
    Ok(RiskIdentity {
        machine_token: pick("machineToken")?,
        machine_code: pick("machineCode")?,
        machine_type: pick("machineType")?,
    })
}

fn truncate(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

// ─── 缓存与对外入口 ─────────────────────────────────────────

struct CacheEntry {
    value: RiskIdentity,
    at: i64,
}

static CACHE: Mutex<Option<HashMap<String, CacheEntry>>> = Mutex::new(None);

/// 取某账号的风控身份（带 50 分钟缓存）。组件不可用或生成失败返回 None，
/// 失败原因只进 verbose 日志 —— 签到链路会给出「缺组件」的用户文案。
pub async fn identity(region: Region, uid: &str) -> Option<RiskIdentity> {
    if uid.is_empty() {
        return None;
    }
    let key = format!("{}:{uid}", region.id());
    if let Ok(guard) = CACHE.lock() {
        if let Some(entry) = guard.as_ref().and_then(|map| map.get(&key)) {
            if logging::now_ms() - entry.at < RISK_TTL_MS {
                return Some(entry.value.clone());
            }
        }
    }
    let component = find_component(region)?;
    match run_component(&component, uid).await {
        Ok(value) => {
            if let Ok(mut guard) = CACHE.lock() {
                guard.get_or_insert_with(HashMap::new).insert(key, CacheEntry { value: value.clone(), at: logging::now_ms() });
            }
            logging::verbose("[Qoder]", &format!("设备风控身份已生成（来源 {}）", component.source));
            Some(value)
        }
        Err(error) => {
            logging::verbose("[Qoder]", &format!("设备风控身份生成失败：{error}"));
            None
        }
    }
}

/// 签到头增强：在基础头（`sash_headers`）上叠加客户端身份与设备风控身份。
///
/// 返回 `(增强后的头, 风控不可用的原因)` —— 原因只用于日志与签到结果文案，
/// 头里缺风控三件套时上游不下发积分活动（这就是它的表现，不抛错）。
pub async fn augment_headers(
    mut headers: Vec<(String, String)>,
    region: Region,
    uid: &str,
) -> (Vec<(String, String)>, Option<String>) {
    headers.push(("Cosy-Version".to_string(), client_version(region)));
    headers.push(("Cosy-MachineOS".to_string(), machine_os()));
    // 与 COSY 签名头同一个机器标识（`machine::machine_id`：优先复用
    // 客户端/qodercli 落盘的 id，否则在网关数据目录持久化一个）
    match machine::machine_id() {
        Ok(id) => headers.push(("Cosy-MachineId".to_string(), id)),
        Err(_) => {}
    }
    if let Some(host) = machine_hostname() {
        headers.push(("Cosy-MachineHostname".to_string(), host));
    }
    match identity(region, uid).await {
        Some(risk) => {
            headers.push(("Cosy-MachineToken".to_string(), risk.machine_token));
            headers.push(("Cosy-MachineCode".to_string(), risk.machine_code));
            headers.push(("Cosy-MachineType".to_string(), risk.machine_type));
            (headers, None)
        }
        None => {
            let why = if find_component(region).is_none() {
                "本机没有 Qoder 客户端 / qodercli 的 UMID 组件".to_string()
            } else {
                "UMID 组件生成身份失败".to_string()
            };
            (headers, Some(why))
        }
    }
}

// ─── 状态与安装（Linux/Docker）──────────────────────────────

/// 组件状态摘要（状态接口用）：来源 / 是否可用 / 平台是否支持安装。
pub fn status(region: Region) -> serde_json::Value {
    let component = find_component(region);
    serde_json::json!({
        "available": component.is_some(),
        "source": component.as_ref().map(|component| component.source),
        "installSupported": install_supported(),
        "installing": installing(),
    })
}

static INSTALLING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn installing() -> bool {
    INSTALLING.load(std::sync::atomic::Ordering::Relaxed)
}

/// 安装只支持 Linux：npm 包里内嵌的是 Linux ELF（CreditDaddy 的结论），
/// Windows / macOS 用户有客户端与 qodercli 两条现成路径，不需要下载。
pub fn install_supported() -> bool {
    cfg!(target_os = "linux")
}

/// 从官方 qodercli 的 npm 包提取本机架构的 UMID 组件。
///
/// 照抄 CreditDaddy `qoderUmid.js` 的链路：npmmirror 优先加速、npmjs 是
/// integrity 的**权威来源**（镜像被污染时它自己的校验形同虚设）；tar 解析
/// 手写（按名字取一个普通文件）；ELF 按 `e_machine` 挑本机架构（x64=62 /
/// arm64=183）；装好先试跑一次确认能出身份，再原子落地。
pub async fn install_component() -> Result<serde_json::Value, GatewayError> {
    if !install_supported() {
        return Err(GatewayError::with_status(400, "当前平台不需要或不支持下载安装 UMID 组件（Windows / macOS 请安装 Qoder 客户端或 qodercli）"));
    }
    if INSTALLING.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return Err(GatewayError::with_status(409, "UMID 组件正在安装中，请稍候"));
    }
    let result = install_component_inner().await;
    INSTALLING.store(false, std::sync::atomic::Ordering::Relaxed);
    result
}

const NPM_PACKAGE: &str = "@qoder-ai/qodercli";
const NPM_PACKAGE_ESCAPED: &str = "@qoder-ai%2fqodercli";
const NPM_MIRRORS: [&str; 2] = ["https://registry.npmmirror.com", "https://registry.npmjs.org"];
const NPM_AUTHORITY: &str = "https://registry.npmjs.org";
const BUNDLE_ENTRY: &str = "package/bundle/qodercli.js";
const DOWNLOAD_TIMEOUT_MS: u64 = 180_000;
const META_TIMEOUT_MS: u64 = 15_000;

async fn install_component_inner() -> Result<serde_json::Value, GatewayError> {
    let fail = |message: String| GatewayError::with_status(502, message);

    // ① 权威 integrity（npmjs；拿不到时退回镜像自带的，与旧行为一致）
    let authority = npm_meta(NPM_AUTHORITY).await.ok();

    // ② 逐个 registry 下载
    let mut errors: Vec<String> = Vec::new();
    for registry in NPM_MIRRORS {
        let meta = match npm_meta(registry).await {
            Ok(meta) => meta,
            Err(error) => {
                errors.push(format!("{registry}：{error}"));
                continue;
            }
        };
        let version = meta.version.clone().unwrap_or_default();
        let Some((tarball, integrity)) = meta.dist else {
            errors.push(format!("{registry}：元数据缺少 tarball / integrity"));
            continue;
        };
        // 镜像版本与 npmjs 不一致时没有权威校验值可比，跳过镜像直接用官方源
        if registry != NPM_AUTHORITY {
            if let Some(authority) = authority.as_ref() {
                let authority_version = authority.version.as_deref().unwrap_or("");
                if !authority_version.is_empty() && authority_version != version {
                    errors.push(format!("{registry}：版本（{version}）与 npmjs（{authority_version}）不一致"));
                    continue;
                }
                if let Some((_, authority_integrity)) = &authority.dist {
                    if &integrity != authority_integrity {
                        errors.push(format!("{registry}：integrity 与 npmjs 不一致"));
                        continue;
                    }
                }
            }
        }
        logging::log("[Qoder]", &format!("正在下载 {NPM_PACKAGE}@{version}（{registry}）"));
        let body = match http_get(&tarball, DOWNLOAD_TIMEOUT_MS).await {
            Ok(body) => body,
            Err(error) => {
                errors.push(format!("{registry}：下载失败 {error}"));
                continue;
            }
        };
        if let Some(expected) = strip_integrity_prefix(&integrity) {
            let digest = sha2::Sha512::digest(&body);
            let actual = base64::engine::general_purpose::STANDARD.encode(digest);
            if actual != expected {
                errors.push(format!("{registry}：下载内容与 npm integrity 不一致"));
                continue;
            }
        }
        // ③ gunzip + tar：取 bundle/qodercli.js
        let tarball = match flate2::read::GzDecoder::new(&body[..]).bytes_to_vec() {
            Ok(tar) => tar,
            Err(error) => {
                errors.push(format!("{registry}：解压失败 {error}"));
                continue;
            }
        };
        let Some(bundle) = tar_entry(&tarball, BUNDLE_ENTRY) else {
            errors.push(format!("{registry}：包里没有 {BUNDLE_ENTRY}（qodercli 结构可能变化）"));
            continue;
        };
        // ④ 挑本机架构的内嵌 ELF
        let machine = if cfg!(target_arch = "x86_64") { 62u16 } else if cfg!(target_arch = "aarch64") { 183u16 } else { 0 };
        if machine == 0 {
            return Err(fail("当前 CPU 架构不受支持".to_string()));
        }
        let bundle = String::from_utf8_lossy(&bundle).to_string();
        let Some(elf) = extract_elf(&bundle, machine) else {
            errors.push(format!("{registry}：qodercli {version} 里没有本机架构的 UMID 程序"));
            continue;
        };
        // ⑤ 落地 + 试跑 + 原子替换
        let dir = crate::server::config::config_dir().join("qoder-umid");
        if let Err(error) = std::fs::create_dir_all(&dir) {
            return Err(fail(format!("创建组件目录失败: {error}")));
        }
        let final_path = dir.join("runtime-info");
        let tmp_path = dir.join(format!("runtime-info.{}.tmp", std::process::id()));
        if let Err(error) = std::fs::write(&tmp_path, &elf) {
            return Err(fail(format!("写入组件失败: {error}")));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o755));
        }
        let component = Component { exe: tmp_path.clone(), env: 4, source: "data" };
        if let Err(error) = run_component(&component, "install-test").await {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(fail(format!("组件试运行失败：{error}")));
        }
        if let Err(error) = std::fs::rename(&tmp_path, &final_path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(fail(format!("组件落地失败: {error}")));
        }
        let manifest = serde_json::json!({
            "version": version,
            "registry": registry,
            "installedAt": logging::now_ms(),
        });
        let _ = std::fs::write(dir.join("manifest.json"), serde_json::to_string_pretty(&manifest).unwrap_or_default());
        logging::log("[Qoder]", &format!("✅ 设备风控组件已安装：qodercli {version}"));
        return Ok(manifest);
    }
    Err(fail(format!("设备身份组件安装失败 —— {}", errors.join("；"))))
}

struct NpmMeta {
    version: Option<String>,
    /// (tarball, integrity)
    dist: Option<(String, String)>,
}

async fn npm_meta(registry: &str) -> Result<NpmMeta, String> {
    let url = format!("{registry}/{NPM_PACKAGE_ESCAPED}/latest");
    let body = http_get(&url, META_TIMEOUT_MS).await?;
    let value: serde_json::Value = serde_json::from_slice(&body).map_err(|error| format!("元数据不是 JSON: {error}"))?;
    Ok(NpmMeta {
        version: value.get("version").and_then(|value| value.as_str()).map(str::to_string),
        dist: value.get("dist").and_then(|dist| {
            let tarball = dist.get("tarball").and_then(|value| value.as_str())?.trim().to_string();
            let integrity = dist.get("integrity").and_then(|value| value.as_str())?.trim().to_string();
            (!tarball.is_empty() && !integrity.is_empty()).then_some((tarball, integrity))
        }),
    })
}

async fn http_get(url: &str, timeout_ms: u64) -> Result<Vec<u8>, String> {
    let client = crate::server::core::egress::client_for(None);
    let response = client
        .get(url)
        .timeout(Duration::from_millis(timeout_ms))
        .send()
        .await
        .map_err(|error| crate::server::core::egress::describe_error_detail(&error))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status}"));
    }
    response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|error| crate::server::core::egress::describe_error_detail(&error))
}

/// `sha512-<base64>` → base64 部分（前缀不匹配返回 None = 不校验）
fn strip_integrity_prefix(integrity: &str) -> Option<String> {
    integrity.strip_prefix("sha512-").map(str::to_string).filter(|value| !value.is_empty())
}

/// GzDecoder 的小包装（`bytes_to_vec` 不存在，读全量兜个底）
trait BytesToVec {
    fn bytes_to_vec(self) -> Result<Vec<u8>, String>;
}

impl<R: std::io::Read> BytesToVec for flate2::read::GzDecoder<R> {
    fn bytes_to_vec(mut self) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut self, &mut out).map_err(|error| error.to_string())?;
        Ok(out)
    }
}

/// 最小 tar 解析：按名字取一个普通文件的内容（CreditDaddy `tarEntry` 同款，
/// 处理 GNU longname 与 pax 扩展头的 path 覆盖）。
fn tar_entry(tar: &[u8], wanted: &str) -> Option<Vec<u8>> {
    let cstr = |bytes: &[u8]| -> String {
        let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[..end]).to_string()
    };
    let mut offset = 0usize;
    let mut long_name: Option<String> = None;
    while offset + 512 <= tar.len() {
        let header = &tar[offset..offset + 512];
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let size = usize::from_str_radix(cstr(&header[124..136]).trim(), 8).unwrap_or(0);
        let entry_type = header[156] as char;
        let prefix = cstr(&header[345..500]);
        let mut name = long_name.clone().unwrap_or_else(|| {
            if prefix.is_empty() { cstr(&header[..100]) } else { format!("{prefix}/{}", cstr(&header[..100])) }
        });
        long_name = None;
        let body_start = offset + 512;
        let body_end = body_start.saturating_add(size);
        if body_end > tar.len() {
            break;
        }
        offset = body_start + size.div_ceil(512) * 512;
        match entry_type {
            'L' => {
                name = String::from_utf8_lossy(&tar[body_start..body_end]).trim_end_matches('\0').to_string();
            }
            'x' => {
                let body = String::from_utf8_lossy(&tar[body_start..body_end]);
                if let Some(value) = body.lines().find_map(|line| line.strip_prefix("path=").map(|value| value.trim().to_string())) {
                    long_name = Some(value);
                }
            }
            '0' | '\0' => {
                if name == wanted {
                    return Some(tar[body_start..body_end].to_vec());
                }
            }
            _ => {}
        }
    }
    None
}

/// 在 qodercli bundle 文本里找内嵌的 base64 ELF，按 `e_machine` 挑本机架构
/// （CreditDaddy `extractElf` 同款：`"f0VMRgIB` 是 `\x7fELF` + 64 位 + 小端
/// 的 base64 开头）。
fn extract_elf(bundle: &str, machine: u16) -> Option<Vec<u8>> {
    const MARK: &str = "\"f0VMRgIB";
    let mut from = 0usize;
    while let Some(relative) = bundle[from..].find(MARK) {
        let start = from + relative; // 指向开头的引号
        let Some(end) = bundle[start + 1..].find('"').map(|value| start + 1 + value) else {
            break;
        };
        let encoded = &bundle[start + 1..end];
        if let Ok(head) = base64::engine::general_purpose::STANDARD.decode(&encoded[..encoded.len().min(4096).min(encoded.len())]) {
            if head.len() >= 20 {
                let value = u16::from_le_bytes([head[18], head[19]]);
                if value == machine {
                    return base64::engine::general_purpose::STANDARD.decode(encoded).ok();
                }
            }
        }
        from = end + 1;
    }
    None
}
