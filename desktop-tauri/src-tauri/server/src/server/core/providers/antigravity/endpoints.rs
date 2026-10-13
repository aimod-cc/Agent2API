//! Antigravity（Google AI IDE）的上游地址、OAuth 常量与**逐字请求头**。
//!
//! ── 上游长什么样（规格 `_recon/antigravity-spec.md`）──────────────
//! 这一家有三个平面，域名与协议各不相同：
//!
//! ```text
//!   Google OAuth        https://accounts.google.com/o/oauth2/v2/auth
//!                       https://oauth2.googleapis.com/token        （取 / 刷 token）
//!                       https://www.googleapis.com/oauth2/v2/userinfo
//!   Cloud Code Assist   https://<env>-cloudcode-pa.googleapis.com/v1internal
//!                       （推理、模型目录、project 发现都在这一个平面上）
//!   （没有自家的登录页：网页登录走 Google OAuth，见 `oauth.rs`）
//! ```
//!
//! ── v1internal 的冒号语法（最容易写错的一处）────────────────────
//! `/v1internal` 是**路径前缀**，方法名用 `:` 直接接在后面，**没有**
//! `/models/{model}:...` 那一段（那是 `generativelanguage.googleapis.com`
//! 的形态，本家不用），模型名放在请求体里：
//!
//! ```text
//!   POST {base}:streamGenerateContent?alt=sse     流式推理（聊天转发走这条）
//!   POST {base}:generateContent                   非流式推理（登记备用，见 adapter.rs）
//!   POST {base}:fetchAvailableModels              模型目录 + 每模型配额
//!   POST {base}:retrieveUserQuotaSummary          分组额度（周 / 5 小时窗）
//!   POST {base}:loadCodeAssist                    订阅档 + cloudaicompanionProject
//!   POST {base}:onboardUser                       无 project 时开通
//!   POST {base}:countTokens                       计费（本网关不用）
//! ```
//!
//! ── 三个环境（不是地区）与各自的用途 ────────────────────────────
//! `sandbox` → `daily` → `prod` 是**环境**差异（规格 §6：本家没有地域参数、
//! 端点全球统一）。Manager 的策略是聊天流量优先 sandbox/daily 以规避 prod 的
//! 429（`V1_INTERNAL_BASE_URL_FALLBACKS`），本仓照做并把顺序放在
//! [`V1_BASE_URLS`]；**project 发现是唯一例外** —— 它固定走 prod
//! （规格 §3.4 + 9router `appConstants` 的原话：「the daily host rejects these
//! auth/onboarding calls」），见 [`PROJECT_BASE_URL`]。
//!
//! ── User-Agent 的两条硬约束（规格 §3.2 / §7.1）──────────────────
//!   1. 出站 UA **必须含 `antigravity`**（不区分大小写），否则上游直接 403；
//!   2. 版本号必须 `>= 4.3.0`（[`KNOWN_STABLE_VERSION`]）：低版本会被上游按
//!      老客户端处理，分段模型返回 404，所以 [`sanitize_user_agent`] 把它抬到
//!      下限。**绝不透传客户端入站的 UA** —— 本模块的 UA 全部由常量拼出。
//!
//! ── 明确不要发的头 ──────────────────────────────────────────
//! `x-goog-api-client`：Manager 的注释写明它属于 IDE 的 JS 层、官方客户端出口
//! **不发**，发了反而形成「Electron + Node.js」的矛盾指纹（v4.1.24 移除了它）。
//! 本模块的请求头是**从零拼**的（没有入站头透传通道），因此它天然不会出现 ——
//! 将来给这家加头时也不要把它加回来。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

/// Antigravity 的公开 OAuth 客户端 id（企业版客户端，两套参考实现逐字一致）。
///
/// 它是**公开**客户端凭证（随官方客户端分发），不是用户机密 —— 但仍只出现在
/// form 请求体里，不进日志（见 `oauth.rs` 的日志口径）。
pub const CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";

/// 与 [`CLIENT_ID`] 配套的 client secret（同上，公开值）。
pub const CLIENT_SECRET: &str = "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";

/// 授权码端点（网页登录的 authorization endpoint，见 `oauth::build_authorize_url`）
pub const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";

/// token 端点：**取 token 与刷新都打这一个**（form 表单，见 `oauth.rs`）
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// userinfo（Manager 用 v2；9router 用 v1，两者都能返回 email —— 规格 §8.4。
/// 本仓登记两个常量，实际取值链按 v2 → v1 兜底）
pub const USERINFO_URL_V2: &str = "https://www.googleapis.com/oauth2/v2/userinfo";
/// userinfo v1 形态（同上）
pub const USERINFO_URL_V1: &str = "https://www.googleapis.com/oauth2/v1/userinfo";

/// OAuth 授权范围的 scope 列表（Manager `get_auth_url_with_client` 逐字；
/// 9router 少了 `openid` 也能跑通）。**没有 `cloud-platform` 一定失败** ——
/// v1internal 的全部方法都要求它。
pub const SCOPES: [&str; 6] = [
    "openid",
    "https://www.googleapis.com/auth/cloud-platform",
    "https://www.googleapis.com/auth/userinfo.email",
    "https://www.googleapis.com/auth/userinfo.profile",
    "https://www.googleapis.com/auth/cclog",
    "https://www.googleapis.com/auth/experimentsandconfigs",
];

/// 刷新用 grant_type（逐字；不是 `refreshToken`）
pub const GRANT_TYPE_REFRESH: &str = "refresh_token";
/// 授权码换 token 用 grant_type（网页登录用，见 `oauth::exchange_code`）
pub const GRANT_TYPE_AUTHORIZATION_CODE: &str = "authorization_code";

/// Cloud Code Assist 的三个环境基址（顺序 = 优先级，见模块头）。
pub const V1_BASE_URL_SANDBOX: &str = "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal";
/// daily（9router 的聊天流量固定用这一条）
pub const V1_BASE_URL_DAILY: &str = "https://daily-cloudcode-pa.googleapis.com/v1internal";
/// prod（兜底；429 比另外两个多）
pub const V1_BASE_URL_PROD: &str = "https://cloudcode-pa.googleapis.com/v1internal";

/// 候选基址顺序（规格 §3.1 / §7.15：sandbox → daily → prod，避免 prod 429）。
/// 模型目录刷新按它依次尝试，429/5xx/传输失败就换下一条。
pub const V1_BASE_URLS: [&str; 3] = [V1_BASE_URL_SANDBOX, V1_BASE_URL_DAILY, V1_BASE_URL_PROD];

/// project 发现（loadCodeAssist / onboardUser）**固定用 prod**：daily 会拒这两
/// 个调用（规格 §3.4 + 9router 的注释原话）。
pub const PROJECT_BASE_URL: &str = V1_BASE_URL_PROD;

/// 流式推理方法名（冒号拼在基址之后；`?alt=sse` 必带）—— 聊天转发走这条
pub const METHOD_STREAM_GENERATE: &str = "streamGenerateContent";
/// 非流式推理方法名 —— 登记备用（本仓聊天一律走流式端点，见 `adapter.rs`）
pub const METHOD_GENERATE: &str = "generateContent";
/// 模型目录 + 每模型配额
pub const METHOD_FETCH_AVAILABLE_MODELS: &str = "fetchAvailableModels";
/// 分组额度（周 / 5 小时窗）—— 本步不用，登记给后续余额/配额展示
pub const METHOD_RETRIEVE_QUOTA_SUMMARY: &str = "retrieveUserQuotaSummary";
/// 订阅档 + `cloudaicompanionProject`
pub const METHOD_LOAD_CODE_ASSIST: &str = "loadCodeAssist";
/// 无 project 时开通
pub const METHOD_ONBOARD_USER: &str = "onboardUser";
/// token 计数（本网关不用，登记以免将来写错位置）
pub const METHOD_COUNT_TOKENS: &str = "countTokens";

/// 流式推理的查询串（逐字）
pub const STREAM_QUERY: &str = "?alt=sse";

/// 已知稳定版本下限（Manager `KNOWN_STABLE_VERSION`）：UA 里的版本号低于它会被
/// 抬上来，否则分段模型 404（规格 §3.2 约束 2）。
pub const KNOWN_STABLE_VERSION: &str = "4.3.0";

/// 默认 Chrome 版本段（Manager 的默认 UA 逐字）
pub const DEFAULT_CHROME: &str = "132.0.6834.160";
/// 默认 Electron 版本段（同上）
pub const DEFAULT_ELECTRON: &str = "39.2.3";

/// 环境变量：覆盖 UA 版本号（与 Manager 的 `env` 入口同款，出问题时不必重编译）
pub const ENV_VERSION: &str = "ANTIGRAVITY_VERSION";
/// 环境变量：整体覆盖出站 UA（仍会过 [`sanitize_user_agent`] 的两条硬约束）
pub const ENV_USER_AGENT: &str = "ANTIGRAVITY_USER_AGENT";

/// 读环境变量（空值视为未设置）
fn env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// 解析 `x.y.z`（三段都要有、都要是数字；否则 None）
fn parse_semver(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.split('.');
    let major = parts.next()?.trim().parse::<u32>().ok()?;
    let minor = parts.next()?.trim().parse::<u32>().ok()?;
    let patch = parts.next()?.trim().parse::<u32>().ok()?;
    Some((major, minor, patch))
}

/// 版本下限的元组形态
fn floor_version() -> (u32, u32, u32) {
    // 常量写错时回落到 (4, 3, 0) 而不是 panic —— 这个值只影响 UA 文案
    parse_semver(KNOWN_STABLE_VERSION).unwrap_or((4, 3, 0))
}

/// 客户端版本号（默认 [`KNOWN_STABLE_VERSION`]，`ANTIGRAVITY_VERSION` 可覆盖；
/// 覆盖值低于下限时抬到下限）。
pub fn version() -> String {
    let raw = env_value(ENV_VERSION).unwrap_or_else(|| KNOWN_STABLE_VERSION.to_string());
    match parse_semver(&raw) {
        Some(parsed) if parsed >= floor_version() => raw,
        // 解不出的覆盖值不静默用：回落到已知稳定版（比发一个畸形版本号安全）
        _ => KNOWN_STABLE_VERSION.to_string(),
    }
}

/// 按当前操作系统给 UA 的平台串（Manager `constants.rs` 逐字）。
pub fn platform_info() -> &'static str {
    match std::env::consts::OS {
        "macos" => "Macintosh; Intel Mac OS X 10_15_7",
        "windows" => "Windows NT 10.0; Win64; x64",
        _ => "X11; Linux x86_64",
    }
}

/// 默认出站 UA（Manager `USER_AGENT` 逐字）：
/// `Antigravity/{ver} ({platform}) Chrome/{chrome} Electron/{electron}`。
pub fn default_user_agent() -> String {
    format!(
        "Antigravity/{} ({}) Chrome/{} Electron/{}",
        version(),
        platform_info(),
        DEFAULT_CHROME,
        DEFAULT_ELECTRON
    )
}

/// 原生 OAuth 请求用的 UA（Manager `NATIVE_OAUTH_USER_AGENT` 逐字）：
/// `vscode/1.X.X (Antigravity/{ver})` —— 取 token / 刷新 / userinfo 用它，
/// 模型目录与 project 发现也用它（规格 §5.1 / §3.4 的逐字取值）。
pub fn oauth_user_agent() -> String {
    format!("vscode/1.X.X (Antigravity/{})", version())
}

/// 清洗一个**外部给的** UA（环境变量覆盖）：满足两条硬约束才放行。
///
/// 规则（照 Manager `sanitize_egress_user_agent` 的语义）：
///   - 空 / 不含 `antigravity`（不区分大小写）→ `None`，调用方回落到默认 UA；
///   - 版本段解析不出来 → `None`（宁可发默认的，也不发一个上游不认的形态）；
///   - 版本低于 [`KNOWN_STABLE_VERSION`] → 只把版本段换成下限值，其余原样保留。
pub fn sanitize_user_agent(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lowered = trimmed.to_ascii_lowercase();
    let key = "antigravity/";
    let start = lowered.find(key)?;
    let version_start = start + key.len();
    let tail = trimmed.get(version_start..)?;
    let parsed_text: String = tail
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect();
    let parsed = parse_semver(&parsed_text)?;
    if parsed >= floor_version() {
        return Some(trimmed.to_string());
    }
    let rest = trimmed.get(version_start + parsed_text.len()..)?;
    let head = trimmed.get(..version_start)?;
    Some(format!("{head}{KNOWN_STABLE_VERSION}{rest}"))
}

/// 生效的出站 UA：最好用 `ANTIGRAVITY_USER_AGENT`（过清洗），否则默认 UA。
pub fn user_agent() -> String {
    env_value(ENV_USER_AGENT)
        .and_then(|raw| sanitize_user_agent(&raw))
        .unwrap_or_else(default_user_agent)
}

/// 拼一个 v1internal 方法的完整 URL：`{base}:{method}`（冒号语法见模块头）。
pub fn method_url(base: &str, method: &str) -> String {
    format!("{base}:{method}")
}

/// 流式推理端点（`{base}:streamGenerateContent?alt=sse`）—— 聊天转发走这条
pub fn stream_generate_url(base: &str) -> String {
    format!("{base}:{METHOD_STREAM_GENERATE}{STREAM_QUERY}")
}

/// 非流式推理端点（`{base}:generateContent`）—— 登记备用（本仓聊天一律走流式端点）
pub fn generate_url(base: &str) -> String {
    method_url(base, METHOD_GENERATE)
}

/// 模型目录端点（`{base}:fetchAvailableModels`）
pub fn fetch_available_models_url(base: &str) -> String {
    method_url(base, METHOD_FETCH_AVAILABLE_MODELS)
}

/// project 发现：`loadCodeAssist`（**prod**，见 [`PROJECT_BASE_URL`]）
pub fn load_code_assist_url() -> String {
    method_url(PROJECT_BASE_URL, METHOD_LOAD_CODE_ASSIST)
}

/// project 开通：`onboardUser`（**prod**）
pub fn onboard_user_url() -> String {
    method_url(PROJECT_BASE_URL, METHOD_ONBOARD_USER)
}

/// 鉴权头（`Authorization: Bearer {access_token}`）。
///
/// 规格 §7.5：token **必须**放 Bearer 头，不能塞 query（`?key=` 那套是
/// `generativelanguage.googleapis.com` 的形态，本家不适用）。
pub fn bearer_header(access_token: &str) -> (String, String) {
    ("Authorization".to_string(), format!("Bearer {}", access_token.trim()))
}

/// 推理 / 模型目录这类 **content** 请求的头集合。
///
/// ── 为什么这里没有 `x-goog-user-project` ─────────────────────
/// 规格 §3.2 / 坑 #4：content 类请求（`generateContent` / `streamGenerateContent`）
/// 必须**不带** `x-goog-user-project`，带了会被上游拒；Manager 也是显式
/// `headers.remove("x-goog-user-project")`。本模块的做法是「从零拼头、不带它」，
/// 比「先加再删」少一处能忘。（project 仍然放在**请求体**的 `project` 字段里。）
///
/// 未带的两个指纹头（`x-machine-id` / `x-vscode-sessionid`）：规格 §8.2 明确
/// 它们**不是硬性必需**（9router 只带 Content-Type / Authorization / UA 也能通），
/// 本仓不实现设备指纹采集，故如实缺席（转发已接通，仍未引入 —— 见
/// `adapter.rs` 的模块头）。
pub fn content_headers(access_token: &str) -> Vec<(String, String)> {
    vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        bearer_header(access_token),
        ("User-Agent".to_string(), user_agent()),
        // 官方指纹增强（两套参考实现都带）。`x-client-name` 是身份，不带它
        // 上游也能工作，但带上更接近官方出口。
        ("x-client-name".to_string(), "antigravity".to_string()),
        ("x-client-version".to_string(), version()),
    ]
}

/// **非 content** 方法（模型目录 / project 发现 / 额度）的头集合。
///
/// 与 [`content_headers`] 的两处差别（都是有依据的，别顺手统一）：
///   1. UA 用 [`oauth_user_agent`]（`vscode/1.X.X (Antigravity/{ver})`）——
///      模型目录与 project 发现在两套参考实现里都走这个 UA（规格 §5.1 / §3.4）；
///   2. `project_id` 非空时带上 `x-goog-user-project`（规格 §3.2：这个头只给
///      非 content 方法；content 请求必须不带，见 [`content_headers`]）。
///      上游对这类方法忽略**请求体**里的 `project` 字段，这个头才是「哪个项目」
///      的表述。
pub fn admin_headers(access_token: &str, project_id: &str) -> Vec<(String, String)> {
    let mut headers = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        bearer_header(access_token),
        ("User-Agent".to_string(), oauth_user_agent()),
        ("x-client-name".to_string(), "antigravity".to_string()),
        ("x-client-version".to_string(), version()),
    ];
    let project = project_id.trim();
    if !project.is_empty() {
        headers.push(("x-goog-user-project".to_string(), project.to_string()));
    }
    headers
}
