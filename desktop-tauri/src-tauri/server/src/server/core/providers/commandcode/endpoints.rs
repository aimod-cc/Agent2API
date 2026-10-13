//! Command Code 上游地址、协议版本与**逐字请求头**。
//!
//! ── 上游长什么样（规格：`_recon/commandcode-spec.md`，参考
//! `Acankao/commandcode-proxy/proxy.mjs`）─────────────────────────
//! ```text
//!   POST /alpha/generate              聊天生成（唯一生成入口）
//!   POST /alpha/fingerprint/record    设备指纹上报（每 key 首次 + 每 8h+2h）
//!   POST /alpha/lifecycle-events      生命周期事件上报（与指纹并行、同频）
//!   GET  /provider/v1/models          模型目录（5 分钟缓存，失败回落内置清单）
//!   GET  /alpha/billing/credits       额度 / 限流窗口（本家只用它做 key 探活）
//! ```
//! 单一域名 `https://api.commandcode.ai`，**没有国内 / 国际两套域名**（规格 §9），
//! 因此本家是单 provider、单地区。
//!
//! ── 伪装成官方 CLI：头集合就是协议的一部分 ─────────────────────
//! 上游按「像不像官方 CLI」做风控，参考实现逐字对齐了 CLI 的常量表：
//!   - `User-Agent: cli`（**不是** Node/reqwest 默认值）——
//!     `egress` 的默认 UA 是 `"undici"`，这里每个请求都必须显式覆盖它；
//!   - `x-command-code-version` 报「**已实现的**协议版本」而不是 npm 上的最新版：
//!     真机是「形状 + 版本号」自洽的组合，版本号瞎跟会变成「自称最新版、
//!     却说旧方言」（参考实现只对 npm 漂移打告警，从不自动改这个值）；
//!   - 指纹预请求与生成请求**共用同一张头表**（早期只有生成请求带 `cli` UA，
//!     同一账号的两种请求来自两种 UA 是可直接观测的破绽）；
//!   - **不发 `x-co-flag`**（不在 CLI 常量表里，发出去是破绽）。
//!
//! ── 两处参考之间不一致的取值（本文件的取舍）────────────────────
//!   1. `x-command-code-version`：proxy 报 `1.53.1`（对齐 CLI 源码），
//!      10router 报 `0.25.7`（明显过时）。取 **1.53.1** —— proxy 是旗舰参考，
//!      它的 `test/` 把这组头写死成回归护栏；
//!   2. `x-cli-environment`：proxy 用 `production`（测试断言锁定），
//!      10router 的 billing 调用用 `cli`。规格 §3.3 判断「旗舰参考对齐 CLI
//!      源码」，取 **`production`**；两个取值都可经环境变量覆盖
//!      （`COMMANDCODE_CLI_ENVIRONMENT`），将来实测出真值不必改代码。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

/// 默认基址（规格 §2.1；参考的 `config.apiBase`）
pub const DEFAULT_BASE_URL: &str = "https://api.commandcode.ai";

/// 本反代**实际实现**的 wire 协议版本（对齐 `command-code@1.53.1` 的方言）。
///
/// 不要写成「npm 上的最新版」：这个值是自称的协议版本，代码里的字段形状才是
/// 实际方言；两者一旦分叉，上报的版本号就成了自相矛盾的证据（见模块头）。
/// 参考实现的做法是「只告警、不自动升」，本实现不做 npm 版本检查，
/// 需要时由环境变量覆盖。
pub const PROTOCOL_VERSION: &str = "1.53.1";

/// 官方 CLI 的 User-Agent 常量（一个字都不能改）
pub const CLI_USER_AGENT: &str = "cli";

/// CLI 环境标识的默认值（规格 §3.3 的取舍，见模块头第 2 条）
pub const CLI_ENVIRONMENT: &str = "production";

/// 生成请求的路径
pub const GENERATE_PATH: &str = "/alpha/generate";
/// 设备指纹上报路径
pub const FINGERPRINT_PATH: &str = "/alpha/fingerprint/record";
/// 生命周期事件路径
pub const LIFECYCLE_PATH: &str = "/alpha/lifecycle-events";
/// 模型目录路径
pub const MODELS_PATH: &str = "/provider/v1/models";
/// 额度 / 限流窗口路径（key 探活用）
pub const BILLING_CREDITS_PATH: &str = "/alpha/billing/credits";

/// 读环境变量覆盖（空值视为未设置，去掉尾部斜杠）
fn env_override(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

/// 基址：`COMMANDCODE_API_BASE`（本仓命名口径）或 `CC_API_BASE`（参考实现的
/// 变量名）可覆盖，前者优先。
pub fn base_url() -> String {
    env_override("COMMANDCODE_API_BASE")
        .or_else(|| env_override("CC_API_BASE"))
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

/// 协议版本：`COMMANDCODE_PROTOCOL_VERSION` / `CC_VERSION` 可覆盖
/// （参考实现里 `CC_VERSION` 就是这个角色）
pub fn protocol_version() -> String {
    env_override("COMMANDCODE_PROTOCOL_VERSION")
        .or_else(|| env_override("CC_VERSION"))
        .unwrap_or_else(|| PROTOCOL_VERSION.to_string())
}

/// CLI 环境标识：`COMMANDCODE_CLI_ENVIRONMENT` / `CC_CLI_ENVIRONMENT` 可覆盖
pub fn cli_environment() -> String {
    env_override("COMMANDCODE_CLI_ENVIRONMENT")
        .or_else(|| env_override("CC_CLI_ENVIRONMENT"))
        .unwrap_or_else(|| CLI_ENVIRONMENT.to_string())
}

/// 全 URL（含路径）
pub fn url(path: &str) -> String {
    format!("{}{path}", base_url())
}

/// 一家共用的 CLI 伪装头（生成 / 指纹 / 生命周期三处都要这几项）。
///
/// `User-Agent` 显式给 `cli`：`send_raw` 与生成路径最终都走 `egress` 的
/// Client，而它的默认 UA 是 `"undici"` —— 不覆盖就等于自报「我不是 CLI」。
pub fn cli_headers() -> Vec<(String, String)> {
    vec![
        ("User-Agent".to_string(), CLI_USER_AGENT.to_string()),
        ("x-command-code-version".to_string(), protocol_version()),
        ("x-cli-environment".to_string(), cli_environment()),
    ]
}

/// 鉴权头（`Authorization: Bearer <user_key>`）——
/// 预请求与模型目录聚合都从这里取，别处不要再拼一遍
pub fn auth_headers(api_key: &str) -> Vec<(String, String)> {
    vec![(
        "Authorization".to_string(),
        format!("Bearer {}", api_key.trim()),
    )]
}

/// 条件头 `x-cmd-zdr: 1`（零数据留存；仅当客户端带了 `x-cmd-zdr: 1` 时下发）。
///
/// 参考实现有两条触发路径：配置项 `zdr` 与客户端入站头。本仓没有这个配置项，
/// 只跟随客户端头 —— 客户端明确要求零留存时才带上，其余请求保持 CLI 默认形态。
pub fn zdr_header(client_headers: &axum::http::HeaderMap) -> Option<(String, String)> {
    let requested = client_headers
        .get("x-cmd-zdr")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .unwrap_or("");
    (requested == "1").then(|| ("x-cmd-zdr".to_string(), "1".to_string()))
}
