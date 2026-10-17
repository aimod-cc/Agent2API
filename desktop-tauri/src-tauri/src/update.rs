//! 软件更新中「壳」这一侧要做的事。
//!
//! 检测新版本、下载安装包都放在后端（壳侧 reqwest 为省 TLS 依赖
//! 关掉了默认特性，发不出 GitHub 的 HTTPS 请求）。这里只负责三件
//! 壳才能做的事：
//!   1. 报出当前应用版本（后端不知道自己被哪个壳打包，比较必须有此值）
//!   2. 校验后端下载好的安装包路径确实落在受控目录内
//!   3. 启动安装包，并按需要退出本程序（覆盖安装前要先让出文件占用）
//!
//! 路径校验不是多余的：`run_installer` 会执行一个可执行文件，
//! 若把路径当信任输入，等于给渲染层开了任意程序执行的入口。
//!
//! ── 平台差异（后缀与启动方式）────────────────────────────────
//! 安装包后缀按平台分派（见 [`allowed_installer_suffixes`]）：Windows 是 NSIS 的
//! `.exe`，macOS 是 `.dmg`，Linux 桌面端是 `.deb` / `.AppImage`。启动方式也完全
//! 不同 —— Windows 要 `runas` 提权，macOS 是「挂载磁盘映像 → 让用户把 app 拖进
//! 应用程序」，Linux 则区分 deb（交给系统包管理器）与 AppImage（换文件即升级）。

use std::path::{Path, PathBuf};

use crate::gateway;

/// 本平台安装包的**全部**合法后缀（小写，**不含点**），按优先级排列。
///
/// 与后端 `update::version` 的同类函数是**同一口径的两份实现**：后端用它挑
/// Release 资产，这里用它校验「要运行的这个文件是不是安装包」。两边都按
/// `cfg!(target_os)` 编译期取值，所以不会出现不一致。
///
/// 单值与多值的差别只来自 Linux：桌面端同时提供 deb 与 AppImage 两种产物，
/// 用户装的是哪种只有他自己知道，所以两种后缀都得放行（挑资产时 deb 优先，
/// 见后端 `pick_installer`）。Windows / macOS 各只有一个候选。
pub const fn allowed_installer_suffixes() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &["dmg"]
    } else if cfg!(target_os = "windows") {
        &["exe"]
    } else {
        // 小写比对：AppImage 的资产名后缀是 `.AppImage`
        &["deb", "appimage"]
    }
}

/// 下载目录：与后端 `createUpdateManager` 的 downloadDir 一致
pub fn download_dir() -> PathBuf {
    gateway::config_dir().join("updates")
}

/// 规范化路径，消除 `..` 与重复分隔符后再比较前缀。
/// 目标不存在时 canonicalize 会失败，因此只对存在的父目录做规范化。
fn normalize(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return path.canonicalize().ok();
    }
    let parent = path.parent()?;
    let name = path.file_name()?;
    Some(parent.canonicalize().ok()?.join(name))
}

/// 校验待运行的安装包：必须存在、后缀属于本平台、且位于下载目录内。
///
/// 三者缺一不可 —— 少了「目录内」这一条，渲染层就能传入任意路径
/// 让本进程替它执行；少了后缀检查，则可能被用来启动脚本类文件。
pub fn verify_installer(raw: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(raw.trim());
    if raw.trim().is_empty() {
        return Err("安装包路径为空".to_string());
    }
    if !path.is_absolute() {
        return Err("安装包路径必须是绝对路径".to_string());
    }

    let suffix = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let allowed = allowed_installer_suffixes();
    if !allowed.contains(&suffix.as_str()) {
        // 列出本平台允许的全部后缀：Linux 上有两种（deb / AppImage），
        // 只说一个会让拿另一种产物的用户以为自己的包不被支持
        let list = allowed.iter().map(|s| format!(".{s}")).collect::<Vec<_>>().join(" / ");
        return Err(format!("只允许运行 {list} 安装包"));
    }

    let target = normalize(&path).ok_or_else(|| "无法解析安装包路径".to_string())?;
    let root = normalize(&download_dir())
        .ok_or_else(|| "无法解析下载目录".to_string())?;
    if !target.starts_with(&root) {
        return Err("安装包不在受控的下载目录内，已拒绝执行".to_string());
    }
    if !target.is_file() {
        return Err("安装包不存在或不是文件".to_string());
    }
    Ok(target)
}

/// 启动安装包。
///
/// Windows：`silent` 为 true 时带上 NSIS 的 `/S /R`（静默 + 装完重启）。
/// macOS：dmg 没有等价的静默参数，交给系统 `open` 挂载（用户随后自己把
/// app 拖进「应用程序」）—— 这也是 dmg 分发的常规交互，强行静默反而会让
/// 用户不知道安装包被挂到哪去了。
/// Linux：按包型分流，见下面两个 `cfg` 分支的说明。
///
/// 不等待安装结束：安装程序通常需要用户交互，阻塞在这里会让
/// 界面一直转圈。调用方随后自行决定是否退出程序。
pub fn launch_installer(path: &Path, silent: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        launch_installer_elevated(path, silent)
    }
    #[cfg(target_os = "macos")]
    {
        // dmg 由系统挂载：`open` 会交给 DiskImages 处理并弹出安装窗口。
        // silent 在这里没有对应语义（见函数说明），显式忽略而不是假装支持。
        let _ = silent;
        std::process::Command::new("open")
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("打开磁盘映像失败: {error}"))
    }
    #[cfg(target_os = "linux")]
    {
        launch_installer_linux(path, silent)
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let mut command = std::process::Command::new(path);
        if silent {
            command.arg("/S").arg("/R");
        }
        command
            .current_dir(path.parent().unwrap_or_else(|| Path::new(".")))
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("启动安装包失败: {error}"))
    }
}

/// Linux 桌面的安装包启动：deb 与 AppImage 是两条不同的路。
///
/// ── deb：必须交给系统包管理器 ────────────────────────────────
/// `.deb` 不能直接执行，它是 ar 归档。正确做法是交给桌面环境登记的处理程序
/// （`xdg-open` 会转给 GNOME Software / Discover 这类图形安装器，用户点两下
/// 装完）。**不走 `dpkg -i`**：那需要 root，而本进程是普通用户 —— 直接
/// `sudo` 会把「界面点了更新」变成一次静默提权，既不该由网关代劳，也没有
/// 终端可以输密码。
///
/// `silent` 对 deb 没有对应语义（装不装、什么时候装由用户在图形安装器里
/// 决定），显式忽略而不是假装支持。
///
/// ── AppImage：自己就能跑 ───────────────────────────────────
/// AppImage 是可执行的单文件：加上可执行位直接启动即为「升级」（它自带
/// 运行时，换文件就换了版本）。这里**只启动不替换**：真正的替换由用户或
/// 外层脚本决定（把新 AppImage 覆盖到旧文件上）；本进程此刻还占着旧的
/// 可执行文件，自己去覆盖自己会失败。
#[cfg(target_os = "linux")]
fn launch_installer_linux(path: &Path, silent: bool) -> Result<(), String> {
    let is_appimage = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.eq_ignore_ascii_case("appimage"))
        .unwrap_or(false);

    if is_appimage {
        // AppImage 需要可执行位才能真正启动（下载器落盘时默认没有）。
        // 失败就如实报错：一个不可执行的 AppImage 启动起来只会得到
        // Permission denied，不如在这里把话说清楚。
        if let Err(error) = make_executable(path) {
            return Err(format!("无法为 AppImage 添加可执行权限: {error}"));
        }
        let _ = silent;
        std::process::Command::new(path)
            .current_dir(path.parent().unwrap_or_else(|| Path::new(".")))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("启动 AppImage 失败: {error}"))
    } else {
        // deb：交给桌面环境登记的图形安装器（见函数说明为何不用 dpkg）
        let _ = silent;
        std::process::Command::new("xdg-open")
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|error| {
                format!("调用系统安装器失败（需要桌面环境提供 xdg-open）: {error}")
            })
    }
}

/// 给文件加可执行位（`chmod +x`）：AppImage 启动前的必要步骤。
///
/// 用 `std::os::unix::fs::PermissionsExt` 而不是 spawn `chmod`：少一个进程、
/// 少一次 PATH 查找，且失败原因直接是 io 错误（可读）。
#[cfg(target_os = "linux")]
fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(path)?;
    let mut permissions = metadata.permissions();
    // 在既有权限上补三档的可执行位（rw- → rwx），保留其余位不动
    permissions.set_mode(permissions.mode() | 0o111);
    std::fs::set_permissions(path, permissions)
}

/// 把 `canonicalize` 得到的路径还原成 shell 能接受的普通形式。
///
/// `verify_installer` 返回的是 `canonicalize` 的结果，而 Windows 上的
/// `canonicalize` 一律带 `\\?\` verbatim 前缀（UNC 则是 `\\?\UNC\...`）。
/// shell 的 API 不认这种形态：实测 `ShellExecuteW` 对 `\\?\C:\...\x.exe`
/// 直接返回 SE_ERR_FNF(2)，而 `Command::spawn`（CreateProcess）本来能吃下它 ——
/// 换成 ShellExecute 之后这一步是必需的，否则每次自动更新都会「找不到安装包」。
#[cfg(windows)]
fn shell_path(path: &Path) -> String {
    let text = path.to_string_lossy().to_string();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) => rest.to_string(),
        None => text,
    }
}

/// Windows 上的安装包启动：必须走 `ShellExecuteW` + `runas` 提权。
///
/// ── 为什么不能用 `Command::spawn`（提权后必然踩到）────────────────
/// perMachine 的安装包带 requireAdministrator 清单。`Command::spawn` 内部是
/// CreateProcess，而 CreateProcess **不会触发 UAC**：它直接失败并返回
/// `ERROR_ELEVATION_REQUIRED (740)`。只有 ShellExecute 会走 shell 的提权路径
/// 弹出 UAC 确认框，因此这里是唯一可行的调用方式。
///
/// 声明手法与 `login.rs` 的 `open_in_browser` 相同（不新增依赖）。
/// `SW_SHOWNORMAL` 让 UAC 确认后安装向导正常显示在用户面前。
#[cfg(windows)]
fn launch_installer_elevated(path: &Path, silent: bool) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    const SW_SHOWNORMAL: i32 = 1;
    /// 用户在 UAC 弹窗点了「否」：ShellExecuteW 此时返回 SE_ERR_ACCESSDENIED(5)
    /// （`ShellExecuteEx` 则是 GetLastError 给 ERROR_CANCELLED(1223)，两者含义
    /// 相同 —— 都表示「提权请求没被批准」）。
    const SE_ERR_ACCESSDENIED: isize = 5;
    const ERROR_CANCELLED: isize = 1223;

    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            hwnd: *mut std::ffi::c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show_cmd: i32,
        ) -> *mut std::ffi::c_void;
    }

    let to_wide = |text: &str| -> Vec<u16> {
        std::ffi::OsStr::new(text).encode_wide().chain(std::iter::once(0)).collect()
    };
    // runas = 提权启动；安装包自身带 requireAdministrator 清单
    let operation = to_wide("runas");
    let file = to_wide(&shell_path(path));
    // /S 静默，/R 装完自动重启（与改动前的行为一致）
    let parameters = if silent { to_wide("/S /R") } else { to_wide("") };
    let directory = path
        .parent()
        .map(|dir| to_wide(&shell_path(dir)))
        .unwrap_or_else(|| to_wide(""));

    // ShellExecuteW 的返回值 <= 32 表示失败（这是 Win32 的历史约定）
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            parameters.as_ptr(),
            directory.as_ptr(),
            SW_SHOWNORMAL,
        )
    };
    let code = result as isize;
    if code > 32 {
        return Ok(());
    }
    // 5 / 1223 都是「提权没被批准」：用户在 UAC 弹窗点了「否」，或系统的
    // 提权策略不允许（标准账户且无人可输管理员密码）。给一句能照做的提示，
    // 而不是一个光秃秃的错误码 —— 这是新安装模式下最常见的一种失败。
    if code == SE_ERR_ACCESSDENIED || code == ERROR_CANCELLED {
        return Err(
            "安装已取消：没有获得管理员权限（UAC 确认被拒绝）。新版本安装在 Program Files，\
             安装程序需要管理员权限；可重新点击「安装并重启」，并在系统弹窗中选择「是」"
                .to_string(),
        );
    }
    Err(format!("启动安装包失败: ShellExecute 返回 {code}"))
}
