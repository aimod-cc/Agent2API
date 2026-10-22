//! 运行环境探测（设置页「版本信息」的数据源）。
//!
//! 这里只回答「这台机器 / 这个进程是什么」这一层事实，**不碰** `ServerState`
//! （账号数、模型数、监听端口这些由 `api::system_info` 组装）：core 不认识
//! axum，也不该为了拼一个展示页去反向依赖服务状态。
//!
//! ── 为什么环境判定要在后端做 ────────────────────────────────
//! 界面侧的 `workbuddyDesktop.platform` 只说明**编译目标**（windows / macos /
//! linux / web），它区分不了「桌面端进程」与「Windows 服务端进程」——两者
//! 在 Windows 上编译出的是同一套 `platform: 'windows'`。真正可靠的判据是
//! **进程自己**：桌面壳启动时会置 `update::set_desktop_shell(true)`，
//! headless 二进制带 `--service` 时置本模块的 service 标志。所以「形态」这
//! 一栏由后端给，前端只补充「当前界面在哪」（应用窗口 / 浏览器 / 手机）。
//!
//! 一个容易踩的坑：桌面端开了「同时开放网页管理面板」后，别人用浏览器访问
//! 那个面板时 `platform` 是 `'web'`，但后端进程仍是桌面端。只看前端会把它
//! 误报成 Docker 面板 —— 这正是环境判定必须落在后端的原因。
//!
//! ── 容器与 systemd 怎么认 ──────────────────────────────────
//!   · 容器：`/.dockerenv` 存在，或 `/proc/1/cgroup` 里出现 docker /
//!     kubepods / containerd / lxc。两条并用是因为前者在部分运行时里不存在
//!     （Podman、containerd 直跑），后者在 cgroup v2 下内容可能很干净。
//!     显式设置 `AGENT2API_DEPLOY=docker` 时直接采信（自建镜像最稳的口径）。
//!   · systemd：`INVOCATION_ID` 由 systemd 为每次启动注入，前台手跑没有。
//!   · Windows 服务：只能靠启动参数 `--service`，由 bin 在解析后调
//!     [`set_service_mode`] 置位（运行期没有别的可观察痕迹）。
//!
//! ── 路径脱敏 ────────────────────────────────────────────────
//! 「隐藏用户名」是给**截图分享**用的：把主目录前缀换成 `~`，其余原样保留
//! （`C:\Users\Bob\.agent2api` → `~\.agent2api`）。放在后端做而不是前端，
//! 是为了让「复制全部信息」拿到的也是脱敏后的文本 —— 若在前端脱敏，剪贴板
//! 那条路很容易漏掉，用户以为隐藏了、实际复制出去的是完整路径。
//!
//! Windows 路径比较要**忽略大小写**：同一个目录可能以 `C:\Users\Bob` 或
//! `c:\users\bob` 的形式出现（环境变量与 `current_exe` 的来源不同）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// 进程启动时刻（毫秒 Unix 时间戳），由 `ServerState::bootstrap` 记录。
///
/// 不用 `OnceLock::get_or_init(now)` 的懒初始化：那样「运行时长」会从
/// **第一次打开设置页**开始算，而不是从进程启动开始算 —— 一个重启了三天的
/// 进程会显示「刚启动」，正好把排障最想知道的那个信息说反。
static STARTED_AT: OnceLock<i64> = OnceLock::new();

/// 是否由 SCM（Windows 服务控制管理器）拉起（`--service`）。
static SERVICE_MODE: AtomicBool = AtomicBool::new(false);

/// 记录进程启动时刻（幂等；只在 bootstrap 里调一次）。
pub fn record_start() {
    let _ = STARTED_AT.set(now_ms());
}

/// 标记「本进程由 SCM 托管」（headless 二进制解析到 `--service` 时调）。
pub fn set_service_mode(on: bool) {
    SERVICE_MODE.store(on, Ordering::Relaxed);
}

/// 进程启动时刻（毫秒）；尚未记录时返回 0。
pub fn started_at_ms() -> i64 {
    STARTED_AT.get().copied().unwrap_or(0)
}

/// 已运行秒数；启动时刻未知时返回 0（界面显示「—」而不是一个假数字）。
pub fn uptime_seconds() -> i64 {
    let started = started_at_ms();
    if started <= 0 {
        return 0;
    }
    ((now_ms() - started) / 1000).max(0)
}

/// 运行形态：`desktop` / `windows-service` / `docker` / `systemd` / `foreground`。
///
/// 判定顺序有讲究：桌面壳优先（它同时可能跑在容器里做开发，但用户看到的
/// 是桌面应用）；其次是 SCM（Windows 服务不可能是容器）；再是容器；最后
/// systemd 与前台。
pub fn deployment() -> &'static str {
    if crate::server::core::update::desktop_shell() {
        return "desktop";
    }
    if SERVICE_MODE.load(Ordering::Relaxed) {
        return "windows-service";
    }
    if in_container() {
        return "docker";
    }
    if std::env::var("INVOCATION_ID")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
    {
        return "systemd";
    }
    "foreground"
}

/// 是否跑在容器里（见模块头的判据说明）。
pub fn in_container() -> bool {
    if std::env::var("AGENT2API_DEPLOY")
        .map(|value| value.trim().eq_ignore_ascii_case("docker"))
        .unwrap_or(false)
    {
        return true;
    }
    if Path::new("/.dockerenv").exists() {
        return true;
    }
    // cgroup v1 与 v2 都可能命中；只做关键字匹配，不解析结构
    if let Ok(text) = std::fs::read_to_string("/proc/1/cgroup") {
        let lower = text.to_ascii_lowercase();
        if lower.contains("docker")
            || lower.contains("kubepods")
            || lower.contains("containerd")
            || lower.contains("lxc")
        {
            return true;
        }
    }
    false
}

/// 编译目标系统（`std::env::consts::OS` 原值：windows / macos / linux）。
pub fn os() -> &'static str {
    std::env::consts::OS
}

/// 架构的展示写法：`x86_64` → `x64`、`aarch64` → `arm64`，其余原样。
///
/// 与更新资产命名（`agent2api-server-linux-amd64`）同一套词汇，用户截图
/// 报问题时能直接与 Release 文件名对上。
pub fn arch_label() -> String {
    match std::env::consts::ARCH {
        "x86_64" => "x64".to_string(),
        "aarch64" => "arm64".to_string(),
        "x86" => "x86".to_string(),
        other => other.to_string(),
    }
}

/// 系统版本的可读描述（如 `Windows 11 Pro 24H2 (build 26200)` / `Ubuntu 24.04 LTS`）。
///
/// 拿不到时返回空串 —— 界面显示「—」，不编造。
pub fn os_version() -> String {
    #[cfg(windows)]
    {
        windows_version()
    }
    #[cfg(target_os = "linux")]
    {
        linux_version()
    }
    #[cfg(target_os = "macos")]
    {
        macos_version()
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        String::new()
    }
}

/// 用户主目录（脱敏时用来识别前缀）；拿不到返回空 PathBuf。
pub fn home_dir() -> PathBuf {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()
        .map(|value| PathBuf::from(value.trim()))
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_default()
}

/// 把主目录前缀替换成 `~`（见模块头「路径脱敏」）。
pub fn mask_path(path: &Path) -> String {
    let text = path.to_string_lossy().to_string();
    let home = home_dir();
    if home.as_os_str().is_empty() {
        return text;
    }
    mask_text(&text, &home.to_string_lossy())
}

/// 字符串形态的脱敏（供 `to_string_lossy` 之后的路径复用）。
pub fn mask_text(text: &str, home: &str) -> String {
    // `\\?\C:\...` 这种扩展长度前缀会让 starts_with 失配，先剥掉再比
    let (prefix, body) = match text.strip_prefix(r"\\?\") {
        Some(rest) => (r"\\?\", rest),
        None => ("", text),
    };
    let home = home.trim_end_matches(['/', '\\']);
    if home.is_empty() {
        return text.to_string();
    }
    let body_matches = if cfg!(windows) {
        body.len() >= home.len() && body[..home.len()].eq_ignore_ascii_case(home)
    } else {
        body.starts_with(home)
    };
    if !body_matches {
        return text.to_string();
    }
    let rest = &body[home.len()..];
    // 必须落在分隔符边界上：`C:\Users\Bob2` 不能被 `C:\Users\Bob` 命中
    if !rest.is_empty() && !rest.starts_with(['/', '\\']) {
        return text.to_string();
    }
    let rest = rest.trim_start_matches(['/', '\\']);
    if rest.is_empty() {
        format!("{prefix}~")
    } else {
        format!("{prefix}~{}{rest}", std::path::MAIN_SEPARATOR)
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

// ─── 各平台的系统版本 ───────────────────────────────────────

#[cfg(windows)]
fn windows_version() -> String {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 读一个 REG_SZ 值（不存在 / 读失败返回 None）。
    fn read(name: &str) -> Option<String> {
        let sub = wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
        let value = wide(name);
        let mut size = 0u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(sub.as_ptr()),
                PCWSTR(value.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut size),
            )
        };
        if status.is_err() || size == 0 {
            return None;
        }
        let mut buffer = vec![0u16; (size as usize) / 2 + 1];
        let mut got = size;
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(sub.as_ptr()),
                PCWSTR(value.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut got),
            )
        };
        if status.is_err() {
            return None;
        }
        let len = (got as usize) / 2;
        let text = String::from_utf16_lossy(&buffer[..len]);
        let text = text.trim_end_matches('\0').trim().to_string();
        (!text.is_empty()).then_some(text)
    }

    let build = read("CurrentBuildNumber").unwrap_or_default();
    let mut product = read("ProductName").unwrap_or_default();
    let display = read("DisplayVersion").unwrap_or_default();

    // 注册表里 Windows 11 的 ProductName 仍写着 "Windows 10 ..."（微软已知的
    // 兼容遗留）。用 build 号纠正：22000 起是 Windows 11。
    if let Ok(number) = build.parse::<u32>() {
        if number >= 22000 && product.starts_with("Windows 10") {
            product = product.replacen("Windows 10", "Windows 11", 1);
        }
    }

    let mut parts: Vec<String> = Vec::new();
    if !product.is_empty() {
        parts.push(product);
    }
    if !display.is_empty() {
        parts.push(display);
    }
    if !build.is_empty() {
        parts.push(format!("build {build}"));
    }
    parts.join(" ")
}

#[cfg(target_os = "linux")]
fn linux_version() -> String {
    let Ok(text) = std::fs::read_to_string("/etc/os-release") else {
        return String::new();
    };
    // 只取 PRETTY_NAME（`PRETTY_NAME="Ubuntu 24.04.1 LTS"`），没有就退 NAME + VERSION_ID
    let field = |key: &str| -> Option<String> {
        text.lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .map(|value| value.trim().trim_matches('"').to_string())
            .filter(|value| !value.is_empty())
    };
    if let Some(pretty) = field("PRETTY_NAME") {
        return pretty;
    }
    match (field("NAME"), field("VERSION_ID")) {
        (Some(name), Some(version)) => format!("{name} {version}"),
        (Some(name), None) => name,
        _ => String::new(),
    }
}

#[cfg(target_os = "macos")]
fn macos_version() -> String {
    // `/System/Library/CoreServices/SystemVersion.plist` 是纯文本 XML，取
    // ProductVersion 的 <string> 即可。不 spawn `sw_vers`：桌面壳里 spawn
    // 外部命令在部分终端环境会被拦（也与 Windows 那边避开命令行工具的取向一致）。
    let Ok(text) = std::fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist")
    else {
        return String::new();
    };
    let Some(index) = text.find("<key>ProductVersion</key>") else {
        return String::new();
    };
    let rest = &text[index..];
    let Some(start) = rest.find("<string>") else {
        return String::new();
    };
    let rest = &rest[start + "<string>".len()..];
    let Some(end) = rest.find("</string>") else {
        return String::new();
    };
    let version = rest[..end].trim();
    if version.is_empty() {
        String::new()
    } else {
        format!("macOS {version}")
    }
}
