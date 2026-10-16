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

/// 默认仓库（本项目的上游）；fork 后可自行覆盖（Node 版 DEFAULT_REPO）
pub const DEFAULT_REPO: &str = "aimod-cc/agent2api";

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
/// 本仓库「tag 带 `-` ↔ 标 pre-release」是同一条约定（见 agent.md 的发版流程），
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

/// 从 Release **列表**（`/releases`）里挑「当前该更新到的一版」。
///
/// 这是 beta 通道的取版方式：`/releases/latest` 由 GitHub 自己排除预发布，
/// 想要 beta 就得自己从列表里挑。规则：
///   · 跳过草稿（带令牌请求时草稿也会出现在列表里，它绝不该被当成可下载的一版）；
///   · 版本号认不出来的标签不参与比较；
///   · 其余按**版本号**取最高的一个（`3.2.0-beta.1` > `3.1.0`）。
///
/// ── 两种模式：`only_prerelease` ─────────────────────────────
/// 调用方（`check::fetch_release`）按「当前跑的是不是 beta 版」决定：
///   · `false`（当前跑正式版，比如刚打开开关的人）：**预发布一并参与**，取最高。
///     beta 的核心号总排在正式版前面，所以正常会拿到某个 beta；仓库一个 beta
///     都没有时照旧拿正式版更新 —— 打开开关不该让人收不到任何更新。
///   · `true`（当前就在 beta 版上）：**只看预发布**。同核心号的转正版
///     （`3.1.0` > `3.1.0-beta.1`）不算「有更新」，不会把人从 beta 频道拽回
///     正式版 —— 加入 beta 的人要的是「留在 beta 上」，回到正式版由用户自己
///     关开关决定。列表里没有可用 beta 时返回 `None`（无更新），
///     **不回落到正式版**：回落等于用「转正」把人踢出 beta 频道。
///
/// 不降级由调用方那次 `compare_versions(挑中的标签, 当前版本) > 0` 兜住：beta 版
/// 比同核心号的正式版小、但比更早的正式版大，于是「关掉开关后不会被降级、
/// 等正式版版号超上来才收到」这条自然成立（`3.1.0-beta.1` 的用户在正式版发到
/// `3.1.0` 之前不会收到任何东西）。
///
/// 为什么按版本号而不是列表顺序：列表按创建时间倒序，而热修（先发 3.1.0、
/// 后补 3.0.9）会让创建时间与版本号不一致 —— 面板要的是「版本最高的一版」。
/// 版本号相同时保留先遇到的（= 创建时间更晚的那条）。
pub fn pick_listed_release<'a>(payload: &'a Value, only_prerelease: bool) -> Option<&'a Value> {
    let list = payload.as_array()?;
    let mut best: Option<&Value> = None;
    let mut first: Option<&Value> = None;
    for item in list {
        if item.get("draft").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        if first.is_none() {
            first = Some(item);
        }
        let tag = item.get("tag_name").and_then(Value::as_str).unwrap_or("");
        if parse_version(tag).is_none() {
            continue;
        }
        // 「不要用转正版把人踢出 beta 频道」：只在这里筛预发布（见函数说明）
        if only_prerelease && !is_prerelease_version(tag) {
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
    // 兜底取列表第一条（= 创建时间最新）只在「一个能认的版本号都没有」时用；
    // 只看预发布的那条路上不用 —— 它要的是「没有 beta 就没有更新」，不是
    // 「随便挑一条充数」。
    if only_prerelease { best } else { best.or(first) }
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

/// 本平台的安装包后缀（小写，**不含点**）：Windows 是 NSIS 的 `exe`，
/// macOS 是磁盘映像 `dmg`。
///
/// 一个 Release 里会同时挂着两个平台的产物（Windows 作业与 macOS 作业各自
/// 上传），所以「挑哪个资产」必须按平台分流 —— 否则 macOS 版会去下 exe，
/// 再被壳侧的 `verify_installer` 拦下来，表现为「明明有新版本却装不了」。
///
/// 编译期常量：目标平台在构建时就定了，没有运行期判断的余地。
pub const fn installer_suffix() -> &'static str {
    if cfg!(target_os = "macos") {
        "dmg"
    } else {
        "exe"
    }
}

/// 安装包的形态标识（给界面用的：文案里要不要提 UAC 由它决定）
pub const fn installer_kind() -> &'static str {
    if cfg!(target_os = "macos") {
        "dmg"
    } else {
        "nsis"
    }
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
/// 后缀按 [`installer_suffix`] 分流（Windows → exe，macOS → dmg），
/// 多个候选时按 [`pick_preferred`] 的优先级取一个。没有匹配的资产返回 None
/// （界面据此把「下载并安装」按钮藏起来，而不是给一个点了报错的按钮）。
///
/// ── Linux 恒为 None（容器 / headless）────────────────────────
/// 桌面安装包在 Linux 上没有任何意义，而 [`installer_suffix`] 只有
/// macos / 其它两支 —— 不拦的话 Linux 会去挑 Windows 的 exe，面板上就出现
/// 一个「下载并安装」按钮，点了只会得到「软件更新在网页端不可用」。
/// 容器里的更新走另一条路：[`server_bundle_name`] 那份 tar.gz（见 selfupdate 模块）。
pub fn pick_installer(assets: Option<&Value>) -> Option<Value> {
    if cfg!(target_os = "linux") {
        return None;
    }
    let list = assets.and_then(Value::as_array)?;
    // 带点的后缀拼一次放循环外（循环里每次 format 会白分配）
    let suffix = format!(".{}", installer_suffix());
    let candidates: Vec<&Value> = list
        .iter()
        .filter(|item| asset_name(item).to_lowercase().ends_with(&suffix))
        .collect();
    let chosen = pick_preferred(&candidates)?;
    Some(serde_json::json!({
        "name": asset_name(chosen),
        // `Number(installer.size) || 0`：非数字/NaN/0 都得 0
        "size": chosen.get("size").and_then(Value::as_u64).unwrap_or(0),
        "url": chosen.get("browser_download_url").and_then(Value::as_str).unwrap_or(""),
    }))
}

/// 容器内更新包在 Release 资产里的名字：`agent2api-server-linux-<arch>.tar.gz`。
///
/// 包内含两个条目：可执行文件 `agent2api-server` 与界面目录 `ui/`（见
/// selfupdate 模块的解包约定）。**这个名字是三方约定的连接点** ——
/// CI 打包（.github/workflows/build.yml 的 linux-server job）、这里挑选资产、
/// 面板下载后按 `safe_file_name` 落到下载目录，三处逐字一致；改名要一起改。
///
/// 非 Linux 目标返回 None：桌面端（Windows / macOS）的更新走安装包，
/// 不存在「容器内替换」这条路径。
pub fn server_bundle_name() -> Option<String> {
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

/// JS `String.prototype.trim` 的空白集合 = White_Space ∪ {U+FEFF}。
///
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
        // 给一个错后缀的兜底名等于把「资产名异常」变成「装不了」
        format!("agent2api-update.{}", installer_suffix())
    } else {
        cleaned.to_string()
    }
}
