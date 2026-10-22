//! AtomCode（AtomGit）上游地址与身份常量（环境变量可覆盖）。
//!
//! ── 上游长什么样（逆向来源：官方 Rust CLI `atomgit-atomcode/atomcode` 与
//! 本仓已归档的 Go 参考实现 `cpa-jethub-plugins/plugins/atomcode/`）──
//! AtomCode 是 AtomGit（GitCode）家的编码 Agent 客户端，三条平面各一个域：
//!
//! ```text
//!   登录代理（OAuth）  https://acs.atomgit.com                       无本地回调监听
//!   CodingPlan REST    https://api.gitcode.com/api/v5               目录 / 套餐 / 领取 / 用量
//!   聊天网关           https://api-ai.gitcode.com/v1                 OpenAI Chat Completions
//! ```
//!
//! ── 三条必须写下来的实测事实（每一条都能让整条链路静默失效）─────
//!
//! 1. **聊天网关刻意不是官方默认的那个**。官方 CLI 的默认网关是
//!    `https://llm-api.atomgit.com/v1`（`crates/atomcode-config/src/endpoints.rs:92`），
//!    但它要求一套**闭源请求签名**（`atomcode-codingplan-crypto::sign_v1`，官方
//!    开源树里只有一个 `unreachable!()` 占位 stub，实测由 `scripts/build-official.sh`
//!    在构建官方包时覆盖）。缺签名时它回
//!    `403 {"detail":{"code":"ATOMCODE_SIG_MISSING"}}`。
//!    我们走的是 **`api-ai.gitcode.com`** —— 官方 CLI 自己在
//!    `crates/atomcode-auth/src/gateway_crypto.rs:140-142` 把它认成 first-party 网关，
//!    同一个 OAuth bearer、同一批模型，**不需要签名**。
//!
//! 2. **网关是按 User-Agent 决定要不要签名的**。同一条有效 bearer、同一个请求体，
//!    只改 UA 的判别式实测（2026-10-01，打在 `POST /v1/chat/completions`）：
//!
//!    ```text
//!       (不带 User-Agent)          -> 200 正常回答
//!       curl/8.5.0                 -> 200 正常回答
//!       cpa-jethub-atomcode/0.1.0  -> 200 正常回答
//!       atomcode/5.2.0             -> 403 ATOMCODE_SIG_MISSING
//!       atomcode/5.1.0             -> 403 ATOMCODE_SIG_MISSING
//!    ```
//!
//!    ⇒ 本家**绝不能**自称 `atomcode/<version>`：那正是触发签名门的开关。
//!    `ADAPTER_USER_AGENT` 因此是**功能性常量**，不是外观问题。
//!
//! 3. **`models-v2` 每个条目自带的 `base_url` 指向需要签名的官方网关**（也是
//!    `llm-api.atomgit.com`）—— 目录里那个字段只作展示，**不要**拿它替换
//!    `gateway_base`，否则整家立刻 403。
//!
//! ── 与别家不同的一点：登录没有本地回调 ──────────────────────────
//! 授权回调由代理自己在其域名上终结（`acs.atomgit.com/callback`），**不回到**
//! 运行网关的这台机器。所以本家没有 loopback 监听器、也没有固定回调端口：
//! 发起登录拿 `state`，之后靠**轮询** `auth/check` 判完成 —— 与 ZCode 的
//! CLI 登录同一形状（见 `providers::atomcode::oauth` 的模块头）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

/// 登录代理默认基址（`acs.atomgit.com`，官方 `endpoints.rs:90`）
pub const DEFAULT_BROKER_BASE: &str = "https://acs.atomgit.com";

/// CodingPlan REST 默认基址（官方 `endpoints.rs:91`）
pub const DEFAULT_CODINGPLAN_API_BASE: &str = "https://api.gitcode.com/api/v5";

/// 聊天网关默认基址。
///
/// ⚠️ 刻意不是官方默认的 `https://llm-api.atomgit.com/v1`（那个要闭源签名）。
/// 见本文件模块头第 1 条。
pub const DEFAULT_GATEWAY_BASE: &str = "https://api-ai.gitcode.com/v1";

/// 本家发出的 User-Agent。
///
/// **功能性常量**：网关按 UA 决定是否索要闭源签名，自称官方客户端会被
/// `403 ATOMCODE_SIG_MISSING` 直接拒掉（模块头第 2 条的实测表）。
/// 这里如实标识本网关自己，绝不伪装成官方客户端。
pub const ADAPTER_USER_AGENT: &str = concat!("agent2api-atomcode/", env!("CARGO_PKG_VERSION"));

/// 登录代理的 `provider` 查询参数（官方 `attempt_login` 固定发 `atomgit`）
pub const BROKER_PROVIDER: &str = "atomgit";

/// 登录代理路径（相对 [`broker_base_url`]，官方 `oauth.rs:38-52`）
pub const BROKER_LOGIN_PATH: &str = "/auth/login";
/// 轮询「用户授权完了没」（官方 `oauth.rs:453`）
pub const BROKER_CHECK_PATH: &str = "/auth/check";
/// 取回令牌包（官方 `oauth.rs:517`）
pub const BROKER_TOKEN_PATH: &str = "/auth/token";
/// 续期（**轮换**：同时下发新的 access 与新的 refresh）
pub const BROKER_REFRESH_PATH: &str = "/oauth/refresh";

/// CodingPlan 路径（相对 [`codingplan_base_url`]，官方 `client.rs:213,254,287,318`）
pub const CODINGPLAN_CLAIM_PATH: &str = "/coding-plan/claim-v2";
/// 模型目录：`?plan_type=<Max|Pro|Lite>`
pub const CODINGPLAN_MODELS_PATH: &str = "/coding-plan/models-v2";
/// 套餐状态：滚动额度窗口在这里
pub const CODINGPLAN_STATUS_PATH: &str = "/coding-plan/status-v2";
/// 近 60 天用量
pub const CODINGPLAN_USAGE_PATH: &str = "/coding-plan/usage";

/// 聊天路径（相对 [`gateway_base_url`]）
pub const GATEWAY_CHAT_PATH: &str = "/chat/completions";

/// 本家全部请求的超时（毫秒）。官方两个客户端都是 5s 连接 + 10s 总时长，
/// 这里给得宽松些：额度窗口与目录驱动的首字节可能偏慢，而超时会把一次
/// 本可成功的调用变成「上游不可达」。
pub const REQUEST_TIMEOUT_MS: u64 = 30_000;

/// 目录拉取的超时（毫秒）：目录是维护动作，不值得让用户等太久
pub const CATALOG_TIMEOUT_MS: u64 = 20_000;

/// 提前多久续期（秒）。官方只在 `created_at + expires_in` **已经过去**时才刷
/// （`oauth.rs:1050-1060`），那在懒调度下会把一个已经死掉的令牌发出去。
/// 提前刷在这里更安全还有第二个理由：**续期会轮换 refresh token**，
/// 所以「access 已经死了之后再刷」仍然能成功，而「拿死令牌发一次请求」不能。
pub const REFRESH_WINDOW_SECONDS: i64 = 3600;

/// 读环境变量覆盖（空值视为未设置），去掉尾部斜杠
fn env_override(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

/// 登录代理基址：`ATOMCODE_BROKER_BASE` 可覆盖
pub fn broker_base_url() -> String {
    env_override("ATOMCODE_BROKER_BASE").unwrap_or_else(|| DEFAULT_BROKER_BASE.to_string())
}

/// CodingPlan REST 基址：`ATOMCODE_CODINGPLAN_API_BASE` 可覆盖
pub fn codingplan_base_url() -> String {
    env_override("ATOMCODE_CODINGPLAN_API_BASE")
        .unwrap_or_else(|| DEFAULT_CODINGPLAN_API_BASE.to_string())
}

/// 聊天网关基址：`ATOMCODE_GATEWAY_BASE` 可覆盖。
///
/// ⚠️ 指向 `https://llm-api.atomgit.com/v1` **不会工作**（要闭源签名，见模块头）。
pub fn gateway_base_url() -> String {
    env_override("ATOMCODE_GATEWAY_BASE").unwrap_or_else(|| DEFAULT_GATEWAY_BASE.to_string())
}

/// 聊天补全的完整 URL
pub fn gateway_chat_url() -> String {
    format!("{}{}", gateway_base_url(), GATEWAY_CHAT_PATH)
}

/// 目录 URL（带档位）
pub fn models_url(plan_type: &str) -> String {
    format!(
        "{}{}?plan_type={}",
        codingplan_base_url(),
        CODINGPLAN_MODELS_PATH,
        plan_type
    )
}

/// 套餐状态 URL
pub fn status_url() -> String {
    format!("{}{}", codingplan_base_url(), CODINGPLAN_STATUS_PATH)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 网关基址绝不能被解析成需要签名的官方网关。
    ///
    /// 这条是回归护栏：官方默认值一旦被谁「顺手对齐」成 `llm-api.atomgit.com`，
    /// 整家会以 `403 ATOMCODE_SIG_MISSING` 全量失效，而错误文案看起来像鉴权问题。
    #[test]
    fn gateway_base_is_never_the_signed_official_host() {
        let base = gateway_base_url();
        assert!(
            !base.contains("llm-api.atomgit.com"),
            "网关基址指向了需要闭源签名的官方网关：{base}"
        );
        assert_eq!(base, DEFAULT_GATEWAY_BASE);
    }

    /// 适配器不得自称官方客户端 —— 那正是触发签名门的开关。
    #[test]
    fn adapter_user_agent_never_claims_to_be_official() {
        let ua = ADAPTER_USER_AGENT.to_ascii_lowercase();
        assert!(
            !ua.starts_with("atomcode/"),
            "User-Agent 自称官方客户端会触发 ATOMCODE_SIG_MISSING：{ADAPTER_USER_AGENT}"
        );
        assert!(ua.starts_with("agent2api-atomcode/"));
    }

    /// URL 拼接：三条平面各自独立，且目录 URL 带档位参数。
    #[test]
    fn urls_are_composed_from_the_right_planes() {
        assert_eq!(
            gateway_chat_url(),
            format!("{DEFAULT_GATEWAY_BASE}{GATEWAY_CHAT_PATH}")
        );
        assert_eq!(status_url(), format!("{DEFAULT_CODINGPLAN_API_BASE}{CODINGPLAN_STATUS_PATH}"));
        assert_eq!(
            models_url("Lite"),
            format!("{DEFAULT_CODINGPLAN_API_BASE}{CODINGPLAN_MODELS_PATH}?plan_type=Lite")
        );
        // 三条平面的 host 互不相同（曾被混用的组合就是把目录 base_url 装进网关）
        assert!(!gateway_base_url().contains("api.gitcode.com/api"));
        assert!(!codingplan_base_url().contains("api-ai.gitcode.com"));
    }
}
