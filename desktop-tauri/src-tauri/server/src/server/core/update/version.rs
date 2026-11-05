//! 软件更新：版本比较、域名白名单、资产挑选、文件名安全化（纯函数）。
//!
//! 对照 Node 版 src/workbuddy-update.mjs 的版本/校验部分逐条移植。
//! 与 `mod.rs`（网络与状态机）拆开，是因为这里全是**纯函数**：
//! 改动原因不同（这里随「版本号与资产命名约定」变，mod.rs 随「HTTP 与文件」变），
//! 也方便单测式排查时直接调用。
//!
//! ── 一处有意的分歧：版本号带预发布段 ──────────────────────────
//! Node 版只比三段数字、后缀一律忽略；这里按 semver 继续比 `-` 之后的预发布段
//! （见 [`compare_versions`]），外加 [`pick_listed_release`]（从 Release 列表里
//! 挑版本最高的一版）—— 两者都是「加入 beta 版」这条通道的必要条件：通道打开
//! 后 `3.1.0-beta.1` 与 `3.1.0-beta.2` 必须能分出先后，正式通道的行为则一字不变
//! （`/releases/latest` 本来就不会返回预发布标签）。

use serde_json::Value;

use crate::server::config::UpdateChannels;

/// 默认仓库：安装包实际发布的地方（独立构建账号下的仓库）。
///
/// ── 为什么不是主仓库 ────────────────────────────────────────
/// 构建已搬到专用账号（见 `Agent2Builder/agent2api-releases` 的
/// `release.yml`）：七个平台 job 跑一次是重负载，留在主仓库做 tag 触发容易
/// 被判定为 Actions 滥用。因此**主仓库不再发带安装包的 Release** —— 这个
/// 常量若仍指向主仓库，更新检查会永远「已是最新」（不报错，只是静默失效）。
///
/// ── 这个值是可以被远程改掉的 ────────────────────────────────
/// 它只是**兜底**。正常情况下实际用的是索引里的 `app.repo`
/// （见 `core::update::index`）：构建仓库换了，改索引文件即可，不必发版。
/// 这里保留一个可用值，是为了「索引读不到也还能更新」。
///
/// `WORKBUDDY_UPDATE_REPO` 环境变量仍可覆盖（最高优先级，且不读索引）。
pub const DEFAULT_REPO: &str = "Agent2Builder/agent2api-releases";

/// 项目源码仓库（`owner/repo`）。
///
/// ── 为什么不能复用 [`DEFAULT_REPO`] ──────────────────────────
/// 界面上的「关于作者 / 收藏项目」两个入口指的是**项目**，不是安装包存放处。
/// 构建账号（`Agent2Builder`）只是借来跑 Actions 的，它的主页没有代码、
/// 没有 issue、Star 也没有意义 —— 拿 `repository` 现拼链接会把人领错地方。
///
/// 这个值不随索引变：索引回答的是「产物在哪」，这里回答的是「项目在哪」，
/// 两者只有「换源码仓库」时才需要一起改（换构建仓库不该动它）。
///
/// 网页端 / 手机端拿到的 `sourceRepository` 就是它（见 `build_check_result`）。
pub const SOURCE_REPO: &str = "aimod-cc/Agent2API";

/// GitHub API 根（Node 版 GITHUB_API）—— [`api_base`] 的默认值
pub const GITHUB_API: &str = "https://api.github.com";

/// 自建更新源的地址（`WORKBUDDY_UPDATE_API_BASE`，空串/非法值当没配）。
///
/// 用途是「不连 GitHub 也能更新」：内网镜像站、离线演练都走这条。配置后
/// **只管更新检查与下载两件事**（账号转发、模型清单一概不受影响）：
///   · 检查更新打 `{base}/repos/{repo}/releases/latest`，返回体与 GitHub 同形；
///     「加入 beta 版」打开时改打 `{base}/repos/{repo}/releases?per_page=…`
///     （取的是**数组**，元素同形）—— 自建源要支持 beta 通道就得提供这个列表接口；
///   · 下载白名单额外放行 `base` 的 host（见 [`assert_downloadable`]）——
///     这是**显式配置才生效**的口子，没配就还是只有 GitHub 那几个域名。
pub fn custom_api_base() -> Option<String> {
    std::env::var("WORKBUDDY_UPDATE_API_BASE")
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

/// 更新源 API 根：配了 `WORKBUDDY_UPDATE_API_BASE` 就用它，否则是 GitHub。
pub fn api_base() -> String {
    custom_api_base().unwrap_or_else(|| GITHUB_API.to_string())
}

/// 允许下载的域名：Release 资产实际会落在 github.com 或 *.githubusercontent.com
/// （Node 版 ALLOWED_HOSTS）
pub const ALLOWED_HOSTS: &[&str] = &[
    "github.com",
    "www.github.com",
    "objects.githubusercontent.com",
    "codeload.github.com",
];

/// 允许下载的域名后缀（Node 版 ALLOWED_HOST_SUFFIX）
pub const ALLOWED_HOST_SUFFIX: &str = ".githubusercontent.com";

/// 安装包体积上限：只用于拦明显异常的响应，避免把磁盘写满（Node 版 600MB）
pub const MAX_INSTALLER_BYTES: u64 = 600 * 1024 * 1024;

/// 更新模块的错误（对应 Node 版 UpdateError，默认 502）。
///
/// `status_code` 会原样变成 HTTP 状态码（400 参数/域名非法、502 网络与 GitHub 侧）。
#[derive(Clone, Debug)]
pub struct UpdateError {
    pub message: String,
    pub status_code: i32,
}

impl UpdateError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), status_code: 502 }
    }

    pub fn with_status(status_code: i32, message: impl Into<String>) -> Self {
        Self { message: message.into(), status_code }
    }

    /// 转成统一的网关错误。更新路由的失败 body 走 OpenAI 风格
    /// （这些写法的错误在 Node 里都被最外层大 try 的 errorPayload 兜住）
    pub fn to_gateway_error(&self) -> crate::server::errors::GatewayError {
        crate::server::errors::GatewayError::with_status(self.status_code, self.message.clone())
    }
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

// ─── 版本比较 ───────────────────────────────────────────────

/// 解析版本号为三段数字。
///
/// 对应 Node 版 parseVersion：先去首尾空白、剥掉可选的 `v` 前缀，
/// 再要求以 `d+.d+.d+` **开头**（后面的预发布/构建元数据不管）。
/// 非语义化版本返回 None（此时不做「有新版本」判断，只展示原文）。
pub fn parse_version(text: &str) -> Option<[u64; 3]> {
    split_version(text).map(|(core, _)| core)
}

/// 这个标签是不是**预发布版**（`-beta.1` 这类 semver 预发布段）？
///
/// ── 判据只用版本号，不读 Release 的 `prerelease` 标记 ──────────
/// 本仓库「tag 带 `-` ↔ 标 pre-release」是同一条约定（见
/// `.agents/skills/release-flow/SKILL.md` 的发版流程），
/// 而版本比较本来就看标签 —— 两处若各读一个来源，漏标 pre-release 时「取最高
/// 版本」与「是不是 beta」就会自相矛盾（它参与比较的方式与它被分类的方式不一致）。
/// 认不出来的标签一律按正式版处理，与 [`split_version`] 的口径一致。
pub fn is_prerelease_version(text: &str) -> bool {
    split_version(text)
        .map(|(_, prerelease)| prerelease.is_some())
        .unwrap_or(false)
}

/// 拆版本号：`(三段数字, 预发布段)`。数字三段与 [`parse_version`] 同一口径
/// （剥 `v` 前缀、只要求前缀匹配），预发布段是 `-` 之后那串（不含 `-`）。
///
/// ── 只有 `-` 才算预发布段 ────────────────────────────────────
/// `3.1.0.1` / `3.1.0_beta` 这类非 semver 写法一律当「没有预发布段」处理，
/// 与改动前「后缀一律忽略」的行为一致：它们本来就不该出现在 Release 标签里，
/// 真出现时按正式版对待（继续按语义化比较）比按预发布对待更安全。
/// 构建元数据（`+` 之后）按 semver 不参与优先级，这里直接截掉。
fn split_version(text: &str) -> Option<([u64; 3], Option<&str>)> {
    let trimmed = text.trim();
    let body = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    // `^(\d+)\.(\d+)\.(\d+)`：只要求前缀匹配，多余部分（-beta.1 等）交给下面拆
    let mut parts = [0u64; 3];
    let mut rest = body;
    for (index, slot) in parts.iter_mut().enumerate() {
        let digits_end = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits_end == 0 {
            return None;
        }
        // 只用 ASCII 数字：JS 的 \d 在非 u 标志下就是 [0-9]
        *slot = rest[..digits_end].parse().ok()?;
        rest = &rest[digits_end..];
        if index < 2 {
            rest = rest.strip_prefix('.')?;
        }
    }
    let head = rest.split('+').next().unwrap_or(rest);
    let prerelease = head.strip_prefix('-').filter(|tail| !tail.is_empty());
    Some((parts, prerelease))
}

/// 预发布段的先后（semver 第 11 条）：返回值口径同 [`compare_versions`]。
///
/// 两侧都是 `-` 之后的原文（`beta.1` / `rc.2` 这类），按 `.` 拆成标识符逐个比：
///   · 两边都是数字 → 按数值比（`beta.2` > `beta.10` 的坑正在这里）；
///   · 数字 < 非数字（`alpha.1` < `alpha.beta`）；
///   · 都是非数字 → 按 ASCII 字典序；
///   · 前缀全相同 → 标识符少的更小（`1.0.0-alpha` < `1.0.0-alpha.1`）。
fn compare_prerelease(left: &str, right: &str) -> i32 {
    let mut left_parts = left.split('.');
    let mut right_parts = right.split('.');
    loop {
        match (left_parts.next(), right_parts.next()) {
            (None, None) => return 0,
            (None, Some(_)) => return -1,
            (Some(_), None) => return 1,
            (Some(a), Some(b)) => {
                // `u64` 解析成功即「数字标识符」：带前导零的写法（01）按 semver
                // 本来就非法，这里当数字比也不吃亏
                let order = match (a.parse::<u64>(), b.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => std::cmp::Ordering::Less,
                    (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
                    (Err(_), Err(_)) => a.cmp(b),
                };
                if order != std::cmp::Ordering::Equal {
                    return if order == std::cmp::Ordering::Less { -1 } else { 1 };
                }
            }
        }
    }
}

/// 语义化比较：a > b 返回 1，相等 0，a < b 返回 -1；无法解析时 None。
///
/// ── 为什么必须比预发布段 ─────────────────────────────────────
/// 「加入 beta 版」通道会把 `3.1.0-beta.1` 与 `3.1.0-beta.2` 一起摆到比较里：
/// 只看核心三段的话这两个是「同一版」，beta 用户装完 beta.1 就再也收不到
/// beta.2（定时任务每轮都判「无更新」）。按 semver：带预发布段的比同核心号的
/// 正式版小（`1.0.0-beta.1` < `1.0.0`），于是「beta.1 → beta.2 → 正式版」
/// 这条链每一跳都算「有新版本」，正式版发布时 beta 用户也能自动接上。
pub fn compare_versions(a: &str, b: &str) -> Option<i32> {
    let (left, left_pre) = split_version(a)?;
    let (right, right_pre) = split_version(b)?;
    for index in 0..3 {
        if left[index] > right[index] {
            return Some(1);
        }
        if left[index] < right[index] {
            return Some(-1);
        }
    }
    Some(match (left_pre, right_pre) {
        (None, None) => 0,
        // 核心号相同：没有预发布段的那侧更大（`3.1.0` > `3.1.0-beta.1`）
        (None, Some(_)) => 1,
        (Some(_), None) => -1,
        (Some(left), Some(right)) => compare_prerelease(left, right),
    })
}

/// 从 Release 列表中挑选已关注通道里语义化版本最高的一版。
///
/// 草稿和非语义化标签不参与；分类同时考虑预发布标记和版本后缀。
/// 与当前版本的比较由调用方负责，列表发布时间不决定版本先后。
pub fn pick_listed_release(payload: &Value, channels: UpdateChannels) -> Option<&Value> {
    let list = payload.as_array()?;
    let mut best: Option<&Value> = None;
    for item in list {
        if item.get("draft").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let tag = item.get("tag_name").and_then(Value::as_str).unwrap_or("");
        if parse_version(tag).is_none() {
            continue;
        }
        let prerelease = item.get("prerelease").and_then(Value::as_bool) == Some(true)
            || is_prerelease_version(tag);
        if !channels.accepts(prerelease) {
            continue;
        }
        let take = match best {
            None => true,
            Some(current) => {
                let current_tag = current.get("tag_name").and_then(Value::as_str).unwrap_or("");
                compare_versions(tag, current_tag) == Some(1)
            }
        };
        if take {
            best = Some(item);
        }
    }
    best
}

// ─── URL 与资产 ─────────────────────────────────────────────

/// 域名白名单校验：只允许 https 且落在 GitHub 资产域名下（对应 assertDownloadable）。
///
/// Node 用 `new URL(url)` 解析：非法 URL 抛 400「下载地址不是合法 URL」，
/// 非 https 抛 400「下载地址必须是 https」，域名不在列表抛 400
/// 「下载地址域名不在允许列表内: <host>」。
pub fn assert_downloadable(url: &str) -> Result<url::Url, UpdateError> {
    let parsed = url::Url::parse(url)
        .map_err(|_| UpdateError::with_status(400, "下载地址不是合法 URL"))?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(UpdateError::with_status(400, "下载地址不能包含凭证"));
    }
    let host = parsed.host_str().unwrap_or("").to_lowercase();

    // ── 自建更新源（WORKBUDDY_UPDATE_API_BASE）───────────────
    // 放行「显式配置的那个源」本身：host 相同或它的子域。scheme 跟着配置走 ——
    // 配 http:// 就允许 http 下载（本地演练 / 内网镜像常见形态），配 https://
    // 仍然强制 https。公共 GitHub 那条路不受影响，照旧强制 https + 白名单。
    //
    // 代价说清楚：把源配成 http 等于放弃传输层保护，完整性只剩 sha256 这一道
    // （而校验文件也来自同一个源）。**演练与内网可信网络**用它没问题，公网部署
    // 请配 https。
    if let Some(base) = custom_api_base() {
        if let Ok(parsed_base) = url::Url::parse(&base) {
            let base_host = parsed_base.host_str().unwrap_or("").to_lowercase();
            let same_host = !base_host.is_empty()
                && (host == base_host || host.ends_with(&format!(".{base_host}")));
            if same_host {
                if parsed_base.scheme() == "https" && parsed.scheme() != "https" {
                    return Err(UpdateError::with_status(400, "下载地址必须是 https"));
                }
                return Ok(parsed);
            }
        }
    }

    if parsed.scheme() != "https" {
        return Err(UpdateError::with_status(400, "下载地址必须是 https"));
    }
    let allowed = ALLOWED_HOSTS.contains(&host.as_str()) || host.ends_with(ALLOWED_HOST_SUFFIX);
    if !allowed {
        return Err(UpdateError::with_status(
            400,
            format!("下载地址域名不在允许列表内: {host}"),
        ));
    }
    Ok(parsed)
}

/// 本平台桌面安装包的**主**资产后缀（小写，**不含点**）：Windows 是 NSIS 的
/// `exe`、macOS 是磁盘映像 `dmg`、Linux 桌面壳是 `deb`（AppImage 启动时是
/// `AppImage` —— 两种产物的取舍见 [`desktop_shell_suffixes`]）。
///
/// 一个 Release 里会同时挂着多个平台的产物（各平台作业各自上传），所以
/// 「挑哪个资产」必须按平台分流 —— 否则 macOS 版会去下 exe，再被壳侧的
/// `verify_installer` 拦下来，表现为「明明有新版本却装不了」。
///
/// ── Linux 为什么要在运行期分叉 ──────────────────────────────
/// Linux 上「桌面壳」与「容器 / headless」是**同一份编译目标**
/// （`cfg!(target_os = "linux")` 两边都真），但两者的升级路径完全不同：
/// 桌面壳要下安装包交给壳去装，容器要下服务端包原地替换。判据只能是运行期的
/// 「本进程有没有 GUI 壳」—— 由桌面壳启动时经 [`set_desktop_shell`] 置位，
/// 容器里的 headless 二进制不置位。
///
/// 所以本函数与 [`installer_kind`] 不再能用 `const fn`；容器形态下这两个值
/// 不会被用到（[`pick_installer`] 在那里直接返回 None，界面据此藏起按钮）。
pub fn installer_suffix() -> &'static str {
    if cfg!(target_os = "macos") {
        "dmg"
    } else if cfg!(target_os = "windows") {
        "exe"
    } else if running_as_appimage() {
        "AppImage"
    } else {
        "deb"
    }
}

/// 安装包的形态标识（给界面用的：文案里要不要提 UAC / 挂载磁盘映像 / 系统
/// 安装器由它决定）。取值与 [`installer_suffix`] 一致，Linux 上跟着
/// [`running_as_appimage`] 走 —— 界面据此说「交给系统安装器」还是「替换文件」。
pub fn installer_kind() -> &'static str {
    if !desktop_shell() {
        return "server";
    }
    if cfg!(target_os = "macos") {
        "dmg"
    } else if cfg!(target_os = "windows") {
        "nsis"
    } else if running_as_appimage() {
        "appimage"
    } else {
        "deb"
    }
}

/// Linux 桌面壳的安装包**全部**候选后缀，按优先级排列（挑资产时逐个找，
/// 第一个命中即用）；非 Linux 是空表。
///
/// ── 次序按「用户装的是哪种」定，不是拍脑袋的偏好 ─────────────
/// 两种产物对应两种安装方式，装错了用户就升不了级（deb 用户拿到 AppImage
/// 只能自己 chmod +x 跑，AppImage 用户拿到 deb 要输密码装系统包）：
///   · 本进程是 AppImage 启动的（`APPIMAGE` 环境变量非空，见
///     [`running_as_appimage`]）→ 只认 AppImage：那是它唯一的更新方式
///     （换文件即升级）；
///   · 否则（deb 装的、或从源码跑的）→ deb 优先，AppImage 兜底
///     （兜底覆盖「只发了 AppImage 的版本」与手工命名的资产）。
fn desktop_shell_suffixes() -> &'static [&'static str] {
    if cfg!(target_os = "linux") {
        // AppImage 的资产名以 `.AppImage` 结尾，比对时统一转小写（见 pick_installer）
        if running_as_appimage() {
            &["AppImage"]
        } else {
            &["deb", "AppImage"]
        }
    } else {
        &[]
    }
}

/// 本平台要找的安装包后缀（小写、带点，按优先级排）。
///
/// 与 [`pick_installer`] 的候选表**同源**（那条直接调它）：界面在「有新版本但
/// 没有可下载的包」时要照着同一份后缀说明原因，各写一份迟早漂移成
/// 「说找不到 .deb、其实找的是 .AppImage」。
///
/// Linux 上按运行期形态分叉：桌面壳给 deb / AppImage，容器 / headless 给**空表**
/// —— 那时升级走 [`server_bundle_name`] 那份服务端包，与安装包后缀无关。
pub fn installer_suffixes() -> Vec<String> {
    if !desktop_shell() {
        return Vec::new();
    }
    if cfg!(target_os = "linux") {
        desktop_shell_suffixes()
            .iter()
            .map(|suffix| format!(".{}", suffix.to_lowercase()))
            .collect()
    } else {
        vec![format!(".{}", installer_suffix())]
    }
}

/// 本进程是不是由 AppImage 启动的。
///
/// AppImage 的运行时规范规定：启动时设置 `APPIMAGE` 环境变量指向该 AppImage
/// 文件自身（`runtime` 注入，用户无需做任何事）。因此它非空即「我是 AppImage」，
/// 这是判断「该给用户推哪种包」的唯一准确依据 —— 比猜路径名可靠（用户可能
/// 把文件改名、或从软链接启动）。
///
/// 为什么这条判据重要：AppImage 的升级方式是「用新文件覆盖旧文件」，
/// 给它推 deb 等于让用户去装一份系统包、而他那份 AppImage 永远停在旧版本。
fn running_as_appimage() -> bool {
    std::env::var("APPIMAGE")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

// ── 「本进程跑在桌面壳里吗」────────────────────────────────

/// 桌面壳标志。
///
/// 默认 false = headless / 容器形态（**未置位时的行为必须与改造前一致**：
/// Linux 上不挑桌面安装包，这与原来「Linux 恒返回 None」等价）。
/// 桌面壳在自己的启动路径里置位（`desktop-tauri/src/backend.rs`），
/// 容器里的 `agent2api-server` 不置位。
static DESKTOP_SHELL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 桌面壳启动时置位（幂等；与 `access::set_panel_gate` 同一模式）。
pub fn set_desktop_shell(on: bool) {
    DESKTOP_SHELL.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// 本进程是否跑在桌面壳里（见 [`DESKTOP_SHELL`]）。
pub fn desktop_shell() -> bool {
    DESKTOP_SHELL.load(std::sync::atomic::Ordering::Relaxed)
}

/// 资产的 `name` 字段（缺失或非字符串时给空串，与 Node 的 `String(item?.name || '')` 同）
fn asset_name(item: &Value) -> &str {
    item.get("name").and_then(Value::as_str).unwrap_or("")
}

/// 多个候选资产之间的择优顺序（按平台分流）。
///
/// ── Windows：优先 NSIS 安装器 ────────────────────────────────
/// NSIS 的产物名带 `-setup`，Release 里偶尔还会附一个裸 exe（免安装的
/// 主程序），那不是安装器 —— 装它不会建快捷方式、也不会登记卸载项。
///
/// ── macOS：按芯片优先级 ─────────────────────────────────────
/// 一次 macOS 构建可能上传 aarch64 / x86_64 / universal 三种 dmg：
///   - `universal` 两种芯片都能跑，最优；
///   - `aarch64` 是 Apple Silicon 原生；
///   - `x86_64` 只在 Intel 上能跑（Apple Silicon 要靠 Rosetta 转译）。
/// 三个都没有（用户手工命名的资产）时退回第一个 —— 至少是个 dmg。
///
/// ── Linux（桌面壳）：按架构，其次发行版标记 ──────────────────
/// 同一次 Linux 构建会出 amd64 与 arm64 两套产物（deb / AppImage 各一份），
/// 所以先按本机架构筛（`amd64` 与 `x86_64` / `arm64` 与 `aarch64` 是同一件事
/// 的两种写法，两类都要认），同架构多个时按发行版常见命名排个先后，都不命中
/// 就退回第一个 —— 至少是同后缀的产物。
fn pick_preferred<'a>(candidates: &[&'a Value]) -> Option<&'a Value> {
    if cfg!(target_os = "macos") {
        for keyword in ["universal", "aarch64", "arm64", "x86_64", "x64"] {
            if let Some(found) = candidates
                .iter()
                .find(|item| asset_name(item).to_lowercase().contains(keyword))
            {
                return Some(*found);
            }
        }
        return candidates.first().copied();
    }
    if cfg!(target_os = "linux") {
        // 本机架构的两种写法：`std::env::consts::ARCH` 给的是 x86_64 / aarch64，
        // 而 Docker 与多数发布产物用 amd64 / arm64（同一套词汇，见 selfupdate）。
        let keywords: &[&str] = match std::env::consts::ARCH {
            "aarch64" => &["aarch64", "arm64"],
            "x86_64" => &["x86_64", "amd64"],
            // 未知架构：不做架构筛选，下面的 `.first()` 兜底
            _ => &[],
        };
        for keyword in keywords {
            if let Some(found) = candidates
                .iter()
                .find(|item| asset_name(item).to_lowercase().contains(keyword))
            {
                return Some(*found);
            }
        }
        return candidates.first().copied();
    }
    candidates
        .iter()
        .find(|item| {
            let name = asset_name(item).to_lowercase();
            name.contains("setup") || name.contains("install")
        })
        .copied()
        .or_else(|| candidates.first().copied())
}

/// 从 Release 资产里挑**本平台**的安装包。
///
/// 后缀按 [`installer_suffix`] 分流（Windows → exe，macOS → dmg，Linux 桌面壳
/// → deb / AppImage），多个候选时按 [`pick_preferred`] 的优先级取一个。
/// 没有匹配的资产返回 None（界面据此把「下载并安装」按钮藏起来，而不是给一个
/// 点了报错的按钮）。
///
/// ── Linux：桌面壳才挑，容器恒为 None ────────────────────────
/// Linux 上桌面安装包只对**桌面壳**有意义（容器里的更新走
/// [`server_bundle_name`] 那份 tar.gz，见 selfupdate 模块）。而两者是同一份
/// 编译目标，所以这里按运行期标志 [`desktop_shell`] 分叉：
///   · 桌面壳（壳启动时置位）→ 按 deb / AppImage 挑资产；
///   · 容器 / headless（未置位）→ None，与改造前的行为逐字一致。
///
/// 不拦的话容器面板上会出现一个「下载并安装」按钮，点了只会得到
/// 「软件更新在网页端不可用」。
pub fn pick_installer(assets: Option<&Value>) -> Option<Value> {
    // 容器 / headless 是空表（见 [`installer_suffixes`]）：升级走服务端包，
    // 面板上不该出现「下载并安装」按钮，点了只会得到「软件更新在网页端不可用」
    let suffixes = installer_suffixes();
    if suffixes.is_empty() {
        return None;
    }
    let list = assets.and_then(Value::as_array)?;
    // 候选后缀按优先级排：Windows / macOS 各一个，Linux 桌面壳是
    // [deb, AppImage]（见 [`desktop_shell_suffixes`]）—— 第一个能挑到资产的
    // 后缀胜出，因此 deb 优先、AppImage 兜底。
    for suffix in &suffixes {
        let candidates: Vec<&Value> = list
            .iter()
            .filter(|item| asset_name(item).to_lowercase().ends_with(suffix.as_str()))
            .collect();
        if let Some(chosen) = pick_preferred(&candidates) {
            return Some(serde_json::json!({
                "name": asset_name(chosen),
                // `Number(installer.size) || 0`：非数字/NaN/0 都得 0
                "size": chosen.get("size").and_then(Value::as_u64).unwrap_or(0),
                "url": chosen.get("browser_download_url").and_then(Value::as_str).unwrap_or(""),
            }));
        }
    }
    None
}

/// 容器内更新包在 Release 资产里的名字：`agent2api-server-linux-<arch>.tar.gz`。
///
/// 包内含两个条目：可执行文件 `agent2api-server` 与界面目录 `ui/`（见
/// selfupdate 模块的解包约定）。**这个名字是三方约定的连接点** ——
/// CI 打包（.github/workflows/build.yml 的 linux-server job）、这里挑选资产、
/// 面板下载后按 `safe_file_name` 落到下载目录，三处逐字一致；改名要一起改。
///
/// 本机服务端包名；Windows 的 zip 仅供手动升级，Linux 的 tar.gz 也用于自动更新。
/// 桌面端仍使用独立的平台安装器，不取这份载荷。
pub fn server_bundle_name() -> Option<String> {
    if cfg!(target_os = "windows") && std::env::consts::ARCH == "x86_64" {
        return Some("agent2api-server-windows-amd64.zip".to_string());
    }
    if !cfg!(target_os = "linux") {
        return None;
    }
    // Release 资产用 amd64 / arm64 的写法（与 docker 镜像的 TARGETARCH 同一套词汇）
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        _ => return None,
    };
    Some(format!("agent2api-server-linux-{arch}.tar.gz"))
}

/// Release 资产里出现过的后缀（小写、带点、去重，按首次出现排序）。
///
/// 界面在「有新版本但没有可下载的包」时用它说清「这一版只有 .dmg / .deb」——
/// 与 [`installer_suffixes`] 配对：一边是「本机要什么」，一边是「发布页上有什么」。
/// 取的是**过滤前**的清单：即使某个文件因更新源的访问方式被丢掉，也要说得出它存在过。
pub fn asset_suffixes(assets: &[Value]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for item in assets {
        let name = asset_name(item).to_lowercase();
        // 只认最后一个点之后的片段：`.tar.gz` 归到 `.gz`（容器内更新包按完整
        // 文件名认，见 [`server_bundle_name`]，不看这条）
        let Some(index) = name.rfind('.') else { continue };
        let suffix = &name[index..];
        if suffix.len() > 1 && !seen.iter().any(|known| known == suffix) {
            seen.push(suffix.to_string());
        }
    }
    seen
}

/// 按**精确资产名**在 Release 资产里取 `{name, size, url}`。
///
/// 与 [`pick_installer`] 的区别：那条按后缀挑「本平台的安装包」，这条要的是
/// 一个点名要的文件（服务端更新包 / 它的 `.sha256` 伴随文件）。找不到返回 None
/// —— 老版本 Release 没有挂这些资产时，面板据此退回「手动升级命令」的提示。
pub fn pick_asset_by_name(assets: Option<&Value>, wanted: &str) -> Option<Value> {
    let list = assets.and_then(Value::as_array)?;
    let chosen = list.iter().find(|item| asset_name(item) == wanted)?;
    Some(serde_json::json!({
        "name": asset_name(chosen),
        "size": chosen.get("size").and_then(Value::as_u64).unwrap_or(0),
        "url": chosen.get("browser_download_url").and_then(Value::as_str).unwrap_or(""),
    }))
}

/// Android APK 的资产名约定：`Agent2API-<版本>.apk`（CI 的 android job 按这个
/// 形状重命名，安装文档里写的也是它）。
///
/// 与 [`pick_installer`] 分开的原因：那条按**本机平台**挑安装包，而手机端要的
/// 恰恰是「另一个平台」的产物 —— 网关跑在电脑上，用户却在手机上点更新。所以
/// 这条不看 `cfg!(target_os)`，只按后缀 `.apk` 挑。
///
/// 多个候选（例如同时挂了 universal 与分架构包）时取第一个：当前 CI 只产一个，
/// 等真出现多包再补优先级。
pub fn pick_android_apk(assets: Option<&Value>) -> Option<Value> {
    let list = assets.and_then(Value::as_array)?;
    let chosen = list
        .iter()
        .find(|item| asset_name(item).to_lowercase().ends_with(".apk"))?;
    Some(serde_json::json!({
        "name": asset_name(chosen),
        "size": chosen.get("size").and_then(Value::as_u64).unwrap_or(0),
        "url": chosen.get("browser_download_url").and_then(Value::as_str).unwrap_or(""),
    }))
}

/// JS `String.prototype.trim` 的空白集合 = White_Space ∪ {U+FEFF}。
/// 这里为 safe_file_name 的 `.trim()` 复制一份：规则只有一行，重复的代价
/// 小于把它提升成全局公共设施（项目里另有几处同款实现，各自模块内可见）。
fn js_trim(value: &str) -> &str {
    value.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}

/// 文件名安全化：只取基名，去掉路径分隔符与可疑字符（对应 Node 版 safeFileName）。
///
/// Node 是 `basename(name).replace(/[\\/:*?"<>|]/g, '_').trim()`，
/// 空结果回落到 `workbuddy-update.exe`（本项目按平台取后缀，见 [`installer_suffix`]）。
/// 注意 `basename` 在 Windows 上
/// 同时识别 `\` 与 `/`，与 Node 的 path.basename（POSIX 语义，只认 `/`）
/// 有差别 —— 但紧接着的替换会把残留的 `\` 变成 `_`，最终文件名同样不含路径分隔符，
/// 安全性一致（这里是**有意**用 Windows 语义，避免把 `..\evil.exe` 原样留下）。
pub fn safe_file_name(name: &str) -> String {
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_string();
    let cleaned: String = base
        .chars()
        .map(|ch| {
            if matches!(ch, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let cleaned = js_trim(&cleaned);
    if cleaned.is_empty() {
        // 兜底名也要带本平台的后缀：下游 `verify_installer` 会校验后缀，
        // 给一个错后缀的兜底名等于把「资产名异常」变成「装不了」。
        // Linux 取候选里的第一个（deb）—— 两者壳侧都放行，见
        // `allowed_installer_suffixes`。
        let suffix = if cfg!(target_os = "linux") {
            desktop_shell_suffixes().first().copied().unwrap_or("deb")
        } else {
            installer_suffix()
        };
        format!("agent2api-update.{suffix}")
    } else {
        cleaned.to_string()
    }
}
