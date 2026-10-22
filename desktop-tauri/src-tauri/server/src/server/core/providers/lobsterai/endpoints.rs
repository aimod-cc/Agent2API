//! LobsterAI（有道龙虾）上游地址与身份常量。
//!
//! ── 上游长什么样 ────────────────────────────────────────────
//! 网易有道系的桌面 Agent，两个域都在有道名下（CNAME 都指向网易广州 ALB）：
//!
//! ```text
//!   登录门户   https://lobsterai.youdao.com        浏览器授权页（手机号 / 微信）
//!   API        https://lobsterai-server.youdao.com  换码 / 续期 / 目录 / 对话 / 积分
//! ```
//!
//! 对话端点是 **OpenAI 兼容的原生协议**（`POST /api/proxy/v1/chat/completions`），
//! 但**只收 SSE**：`stream:false` 上游直接回 HTTP 500（Go 参考 `executor.go:18-21`
//! 实测）。本网关的转发入口「无论客户端要不要流式，上游恒以 `stream:true` 请求」
//! （`core/upstream/mod.rs` 的注释原文），所以这条要求天然满足，适配器**不要**去动
//! `stream` 字段。
//!
//! ── 两个 `X-LobsterAI-Client-*` 头是功能性的，不是装饰 ──────────
//! `ClientCapabilities` 声明客户端能力，服务端**按它过滤**：
//!
//! ```text
//!   不带该头        -> 目录里 25 个模型（没有 kimi-k3）
//!   带 kimi-k3-agentic-v1 -> 26 个（kimi-k3 才出现）
//! ```
//!
//! 另一个能力位 `thinking-level-control-v1` 是「把思考关掉」这条参数的前提 ——
//! 缺它时发 `reasoning_effort: "off"` 会得到 HTTP 500。两个都实测过
//! （见参考实现的 `lobsterai-product.ts` 与 `headers.go` 的注释）。
//!
//! 版本头走的是**日期式版本**（形如 `2026.9.4`），由上游的更新接口给出；
//! 而 `User-Agent` 照抄参考实现里那个固定的 `LobsterAI/0.1.0` —— 它**故意不跟**
//! 真实版本，动态的那份只由 `X-LobsterAI-Client-Version` 承载。
//!
//! ── 没有任何签名 ────────────────────────────────────────────
//! 纯 `Bearer`。没有 HMAC / DPoP / PKCE，也没有腾讯系的 `X-Domain` /
//! `X-Product` / `X-IDE-*` 那批归属头 —— 那些发过去轻则被忽略、重则把请求归到
//! 错误的客户端形态上（参考实现的模块头原话）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

/// 上游 API 主机（换码 / 续期 / 目录 / 对话 / 积分都在它下面）
pub const API_BASE: &str = "https://lobsterai-server.youdao.com";

/// 浏览器登录门户主机
pub const PORTAL_BASE: &str = "https://lobsterai.youdao.com";

/// 客户端版本查询接口。**这是第三方主机，响应不是业务信封**：版本号在
/// `data.value`（参考实现 `lobsterai-product.ts:443` 的说明）。
pub const CLIENT_VERSION_API: &str =
    "https://api-overmind.youdao.com/openapi/get/luna/hardware/lobsterai/prod/update";

/// 查不到版本时用的兜底值
pub const FALLBACK_CLIENT_VERSION: &str = "2026.9.4";

/// `User-Agent`：照抄参考实现，**故意固定**不跟随真实版本
pub const USER_AGENT: &str = "LobsterAI/0.1.0";

/// 声明的客户端能力。两个能力位都是功能性的：
/// `kimi-k3-agentic-v1` 是模型列表的准入门（缺它少一个模型）、
/// `thinking-level-control-v1` 是 `reasoning_effort: "off"` 的前提（缺它 HTTP 500）。
pub const CLIENT_CAPABILITIES: &str = "kimi-k3-agentic-v1,thinking-level-control-v1";

/// 用授权码换令牌（**不带 Authorization**）
pub const EXCHANGE_PATH: &str = "/api/auth/exchange";
/// 续期（**不带 Authorization**）
pub const REFRESH_PATH: &str = "/api/auth/refresh";
/// 模型目录
pub const MODELS_PATH: &str = "/api/models/available";
/// 对话（OpenAI 兼容，SSE only）
pub const CHAT_PATH: &str = "/api/proxy/v1/chat/completions";
/// loopback 回调路径
pub const CALLBACK_PATH: &str = "/auth/callback";
/// 积分余额
pub const PROFILE_SUMMARY_PATH: &str = "/api/user/profile-summary";

/// 每日签到三步：查活动位 → 取该活动上下文 → 执行 check_in
pub const ACTIVITY_SLOT_PATH: &str = "/api/client-activities/slot";
/// 见 [`ACTIVITY_SLOT_PATH`]（活动码拼在路径后面）
pub const ACTIVITY_CONTEXT_PATH: &str = "/api/client-activities";

/// 活动位查询的三个固定参数（照抄真实客户端的形态）。
///
/// `platform=win32` 是**冒充桌面客户端形态**，与运行本网关的宿主系统无关 ——
/// 参考实现特意注明这一点。
pub const SLOT_PLACEMENT: &str = "desktop_sidebar";
/// 见 [`SLOT_PLACEMENT`]
pub const SLOT_CONTAINER_API_VERSION: &str = "2";
/// 见 [`SLOT_PLACEMENT`]
pub const SLOT_PLATFORM: &str = "win32";

/// 控制面请求超时（毫秒）
pub const REQUEST_TIMEOUT_MS: u64 = 30_000;
/// 目录请求超时（毫秒）
pub const CATALOG_TIMEOUT_MS: u64 = 20_000;
/// 目录缓存时长（毫秒）
pub const CATALOG_TTL_MS: i64 = 2 * 60 * 60 * 1000;
/// 已解析的客户端版本缓存时长（毫秒）——12 小时，版本不常变
pub const VERSION_CACHE_TTL_MS: i64 = 12 * 60 * 60 * 1000;

/// 读环境变量覆盖（空值视为未设置），去掉尾部斜杠
fn env_override(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

/// API 基址：`LOBSTERAI_API_BASE` 可覆盖
pub fn api_base() -> String {
    env_override("LOBSTERAI_API_BASE").unwrap_or_else(|| API_BASE.to_string())
}

/// 门户基址：`LOBSTERAI_PORTAL_BASE` 可覆盖
pub fn portal_base() -> String {
    env_override("LOBSTERAI_PORTAL_BASE").unwrap_or_else(|| PORTAL_BASE.to_string())
}

/// 对话端点完整 URL
pub fn chat_url() -> String {
    format!("{}{}", api_base(), CHAT_PATH)
}

/// 目录端点 URL（`keyfrom` 作为查询串，与参考实现一致）
pub fn models_url(keyfrom: &str) -> String {
    format!("{}{}?{}", api_base(), MODELS_PATH, keyfrom.trim())
}

/// 余额端点 URL
pub fn profile_summary_url() -> String {
    format!("{}{}", api_base(), PROFILE_SUMMARY_PATH)
}

/// 活动位 URL（三个固定参数）
pub fn activity_slot_url(client_version: &str) -> String {
    format!(
        "{}{}?placement={}&clientVersion={}&containerApiVersion={}&platform={}",
        api_base(),
        ACTIVITY_SLOT_PATH,
        SLOT_PLACEMENT,
        urlencode(client_version),
        SLOT_CONTAINER_API_VERSION,
        SLOT_PLATFORM
    )
}

/// 某个活动的上下文 URL
///
/// ⚠️ 末尾那个 `/context` 段不能省：上游的路径是三段式
/// `{code}/context?configRevision=…`，少了它整条请求落到 `/api/client-activities/{code}`
/// 上、直接被回 **HTTP 404**（签到因此稳定失败，报「活动上下文查询返回 HTTP 404」）。
/// 参考实现（Go `fetchActivityContext`、TS `lobsterai-credits.ts`）都是三段式。
pub fn activity_context_url(code: &str, config_revision: &str) -> String {
    format!(
        "{}{}/{}/context?configRevision={}",
        api_base(),
        ACTIVITY_CONTEXT_PATH,
        urlencode(code),
        urlencode(config_revision)
    )
}

/// 某个活动的 check_in URL
pub fn activity_checkin_url(code: &str) -> String {
    format!(
        "{}{}/{}/actions/check_in",
        api_base(),
        ACTIVITY_CONTEXT_PATH,
        urlencode(code)
    )
}

/// 极小的 query 转义（本家只有版本号 / 活动码 / 修订版三个值要拼进 URL）。
///
/// 不引依赖：这些值都可能来自上游响应（活动码、configRevision），一旦含
/// `&` / `=` / `?` 就会截断 query，所以必须转义而不是直接拼。
pub fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两个能力位都必须带；少一个就少了模型或直接 500（见模块头）。
    #[test]
    fn capabilities_cover_both_load_bearing_keys() {
        assert!(CLIENT_CAPABILITIES.contains("kimi-k3-agentic-v1"));
        assert!(CLIENT_CAPABILITIES.contains("thinking-level-control-v1"));
    }

    /// UA 固定不跟随版本；动态版本只走 `X-LobsterAI-Client-Version`。
    #[test]
    fn user_agent_is_the_fixed_reference_string() {
        assert_eq!(USER_AGENT, "LobsterAI/0.1.0");
        assert!(FALLBACK_CLIENT_VERSION.starts_with("2026."));
    }

    /// URL 拼接：三条平面各自独立，别把门户/API 混用。
    #[test]
    fn urls_are_composed_from_the_api_plane() {
        assert!(chat_url().starts_with(API_BASE));
        assert!(chat_url().ends_with(CHAT_PATH));
        assert!(profile_summary_url().ends_with(PROFILE_SUMMARY_PATH));
        assert!(!chat_url().contains("lobsterai.youdao.com/api") || chat_url().contains("-server."));
        // 门户主机是**另一个**域，不该出现在任何 API URL 里
        assert!(!models_url("k=v").contains("//lobsterai.youdao.com"));
    }

    /// 活动位 URL 必须带上那三个固定参数（照抄客户端形态）。
    #[test]
    fn slot_url_carries_the_three_fixed_params() {
        let url = activity_slot_url("2026.9.4");
        assert!(url.contains("placement=desktop_sidebar"), "{url}");
        assert!(url.contains("clientVersion=2026.9.4"), "{url}");
        assert!(url.contains("containerApiVersion=2"), "{url}");
        assert!(url.contains("platform=win32"), "{url}");
    }

    /// 活动码与修订版要转义（它们来自上游响应，可能含 query 元字符）。
    #[test]
    fn activity_urls_escape_upstream_values() {
        assert!(activity_context_url("a&b", "1=2").contains("a%26b"));
        assert!(activity_context_url("a&b", "1=2").contains("configRevision=1%3D2"));
        assert!(activity_checkin_url("c d").contains("c%20d"));
        assert_eq!(urlencode("abc-1.2_3~"), "abc-1.2_3~");
    }

    /// **路径形状**要逐段钉住，不能只测转义。
    ///
    /// 这一条是补的：先前只有上面那条「转义」用例，于是 `activity_context_url`
    /// 少写了 `/context` 一段都没被发现 —— 断言全绿，真机上签到却稳定 404
    /// （上游的路径是三段式，见该函数的说明）。凡是被上游用路径而不是参数
    /// 区分的东西，断言就得写到路径本身。
    #[test]
    fn activity_paths_have_the_exact_three_segment_shape() {
        let context = activity_context_url("daily_sign_in", "7");
        assert!(
            context.ends_with("/api/client-activities/daily_sign_in/context?configRevision=7"),
            "{context}"
        );
        let checkin = activity_checkin_url("daily_sign_in");
        assert!(
            checkin.ends_with("/api/client-activities/daily_sign_in/actions/check_in"),
            "{checkin}"
        );
    }
}
