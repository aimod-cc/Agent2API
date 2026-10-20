//! 千问办公（QwenWork 中国版）的域名与端点常量。
//!
//! ── 这家与 Qoder 的关系（先说清，避免把它当 Qoder 抄）──────────
//! 认证链**逐字复用 Qoder**（`/api/v1/jobToken/exchange`、`deviceToken/*` 的
//! 路径与请求体完全一致），但**服务拆成了三个域名**，职责互不重叠：
//!
//! ```text
//!   openapi.qoder.com.cn       凭据（jobToken/exchange、deviceToken/*）
//!                              —— 只认 COSY 签名/TAT，普通 Bearer 回 401/503
//!   gateway.qwenwork.cn        账号 + 额度 + 模型目录 + **签发 MaaS 密钥**
//!                              —— 认普通 `Authorization: Bearer <JWT>`
//!   maas.xiaoluozi.cn          **推理**（OpenAI 兼容）
//!                              —— 认 `Bearer sk-mm-v1.*`（MaaS access-key）
//! ```
//!
//! ── 为什么推理域名不在 qwenwork.cn 下（不要「修正」它）───────────
//! `maas.xiaoluozi.cn` 这个域名看起来像抄错了，但它**不是猜的**：
//! 它是 `POST {gateway}/api/desktop-sessions` 响应体里 `base_url` 字段的
//! 原值，客户端拿到后直接用（`main.js` 的 `mintAccessKey`：
//! `typeof d.base_url=="string" && (serverProvidedBaseUrl = d.base_url.trim())`）。
//! 因此这里的常量只是**兜底默认值**，运行时优先用签发响应里的 `base_url`。
//!
//! ── 推理链路为什么不需要 COSY（这是本适配器最省事的地方）─────────
//! 千问办公自己也不打推理：它把请求交给一个「本地 OpenAI 兼容代理」
//! （`ENGINE_MODES` 里每条都是 `modelTransport:"local-proxy"`），
//! 上游 `maas.xiaoluozi.cn` 本身就是标准 `/v1/chat/completions`。
//! 所以本家**走通用无状态转发**，`is_stateful` 保持默认 false ——
//! 既不需要 `cosy.rs` 那套签名，也不需要 `protocol.rs` 那层信封拆解。

/// 账号域：凭据（`jobToken/exchange`、`deviceToken/*`）。
///
/// 独立启动的客户端不经过它（登录态已落盘），但保留常量以便将来接
/// `PAT → 登录态` 的路径时不用再找。
pub const OPENAPI_BASE: &str = "https://openapi.qoder.com.cn";

/// 网关域：账号信息、额度、模型目录、MaaS 密钥签发。
///
/// 认普通 `Authorization: Bearer <JWT>`（实测 200，无需 COSY）。
pub const GATEWAY_BASE: &str = "https://gateway.qwenwork.cn";

/// 推理域的**兜底**默认值。
///
/// 实际应优先用 `/api/desktop-sessions` 响应里的 `base_url` ——
/// 上游换域名时这个常量会过期，而响应字段不会。改这里之前先读模块头。
pub const DEFAULT_MAAS_BASE: &str = "https://maas.xiaoluozi.cn";

/// MaaS 密钥签发路径（`POST`，body `{}` 或 `{"session_id": "..."}`）。
///
/// 响应 `201`：`{access_token, base_url, expires_at_ms, session_id, ttl_ms}`，
/// `ttl_ms` 实测 7200000（2 小时）。响应里的 `session_id` 会作为推理请求的
/// `request_id` 回显，可用来把一次推理对回一个签发会话。
pub const MINT_PATH: &str = "/api/desktop-sessions";

/// 模型目录（`GET`，普通 Bearer）。
///
/// 响应 `schema_version: 2` 的 `catalog` 数组，每项含 `key`（模型 id）、
/// `i18n.display_name`、`price_factor`、`max_input_tokens`、`context_config`
/// 与 `format`（实测全为 `openai/chat-completions`）。
pub const CHAT_MODES_PATH: &str = "/api/desktop/chat-modes";

/// 用户资料（`GET`，普通 Bearer）。
pub const USERINFO_PATH: &str = "/api/v1/userinfo";

/// 额度用量（`GET`，普通 Bearer）。
///
/// 响应含 `total_usage_percentage`、`is_quota_exceeded`、`user_quota` 等。
pub const QUOTA_USAGE_PATH: &str = "/api/v2/quota/usage";

/// 推理路径（拼在 base 之后，base 已含或不含 `/v1` 都能正确合并，
/// 见 [`chat_completions_url`]）。
pub const CHAT_COMPLETIONS_PATH: &str = "/v1/chat/completions";

// ─── 钱包 / 套餐 / 签到（`/api/v1/adapter/...` 一族）───────────────
//
// ── 这族端点有两个坑，抄 Qoder 时**不能照抄** ────────────────────
//   1. **外面包了一层 `{"code":"ok","data":{...}}` 信封**（Qoder 的
//      `/api/v2/quota/usage` 没有）。所以这族的取值必须先下钻 `data` ——
//      直接读顶层会全部读成 null，而 null 在界面上表现为「这家没有余额」。
//   2. 路径多一段 `/adapter/`。`probe2.py` 当时按「直觉路径」试了 30 多个
//      全落空，就是因为少了它。

/// 钱包（`GET`）：**余额、最近到期时间、最近到期积分**的唯一数据源。
///
/// 响应（实测，`data` 段）：
/// ```json
/// {"active_wallets": {"wallets": [{"balance": 97.43,  "valid_to": "2026-10-20T00:00:00+08:00"},
///                                 {"balance": 1641.25,"valid_to": "2026-11-28T21:19:24+08:00"}],
///                     "total": 2, "page_size": 20, "page_number": 1},
///  "daily_credits":    {"total_balance": 97.43},
///  "longterm_credits": {"total_balance": 1641.25},
///  "monthly_credits":  {"total_balance": 0},
///  "expiring_soon":    {"count": 0, "total_balance": 0, "wallets": []}}
/// ```
/// ── 三个**容易搞反**的点 ────────────────────────────────────
///   1. `wallets[]` 每项**只有 `balance` 与 `valid_to`，没有名字**。
///      「最近到期」= `valid_to` 最早的那一项及其余额；
///   2. `daily_credits` **含在** `longterm_credits` 之外、两者相加就是
///      `account-context.data.quota.remaining`（实测 97.43 + 1641.25 =
///      1738.69）—— 所以余额主口径取 `quota.remaining`，不要在这里相加；
///   3. `expiring_soon` 是上游**自己算好的**「即将到期」桶（含 `count` /
///      `total_balance` / `wallets`），优先用它，回落才自己从 `wallets[]` 挑。
pub const WALLETS_PATH: &str = "/api/v1/adapter/user/wallets";

/// 账号上下文（`GET`，需带 `?include=user,plan,quota,page`）。
///
/// `data.plan` 段给出 `{pid: "subscription-cn-free", name: "个人免费版",
/// next_due_date: null, ...}` —— 前端的「套餐名」与「有效期」两列读它。
/// `data.quota` 段给出 `{total: null, used: null, remaining: 1738.6854,
/// exceeded: false}` —— **余额主口径**（免费版 `total`/`used` 是 null，
/// 但 `remaining` 有真值）。
///
/// **`include` 是必填的**：不写时 `plan` / `quota` 段整个缺失（实测），
/// 看上去像「上游没给」，实际是没要。
pub const ACCOUNT_CONTEXT_PATH: &str = "/api/v1/adapter/user/account-context";

/// `account-context` 必须带的 `include` 参数（顺序与客户端一致）。
pub const ACCOUNT_CONTEXT_INCLUDE: &str = "user,plan,quota,page";

/// 领取每日积分（`POST`）。**这就是本家的「签到」**。
///
/// ── 两个实测坑 ──────────────────────────────────────────────
///   1. **不能带任何 body**。带 `{}` 会回
///      `400 INVALID_MULE_RUN_REQUEST: "claim accepts no query or request body"`
///      —— 这与「POST 习惯上要带个空对象」的直觉相反，抄 Qoder 的
///      `claim_campaign`（那里明写 `body {}`）会直接踩中；
///   2. 响应也是信封：`{"code":"ok","data":{"should_show":false,"credits":0}}`。
///      当天已领回 `credits: 0`（幂等，不报错）。
///
/// 注意**没有**配套的 `GET` 查询端点（`/daily-credit-notice` 实测 404），
/// 所以「今天签没签」只能靠 claim 自己的返回值回答。
pub const DAILY_CREDIT_CLAIM_PATH: &str = "/api/v1/adapter/user/daily-credit-notice/claim";

// ─── 网页登录（PKCE 设备授权，与 Qoder 同一套协议）────────────────

/// 授权页（`GET`，**匿名**）：把 user agent 带去这里点「授权」。
///
/// 挂在网关域而不是独立登录站（实测 `gateway.qwenwork.cn/device/selectAccounts`
/// 回 302 跳登录页；`qwenwork.cn` 裸域回 404、`www` 回 301 但路径不存在）。
pub const DEVICE_AUTH_PATH: &str = "/device/selectAccounts";

/// 轮询（`GET`，**匿名**）：`?nonce=&verifier=&challenge_method=S256`。
///
/// 未授权时回 202 或 404（两种情况都按「继续等」处理，与 Qoder 一致）。
/// 参数不全时回 400 `INVALID_DEVICE_FLOW` —— 那是参数问题，不是端点不存在。
pub const DEVICE_POLL_PATH: &str = "/api/v1/deviceToken/poll";

/// 设备授权的 `client_id`（从客户端 `main.js` 的 `CLIENT_ID` 表取，
/// 中国版那一项）。**别改成 Qoder 的值** —— 两家 client 注册互不相通。
pub const DEVICE_CLIENT_ID: &str = "e883ade2-e6e3-4d6d-adf7-f92ceff5fdcb";

/// 授权完成后的回调协议（自定义 scheme，客户端注册的 `qwenwork-cn://`）。
pub const DEVICE_REDIRECT_URI: &str = "qwenwork-cn://";

/// 把「base + 推理路径」拼成一个不会出现 `/v1/v1` 的 URL。
///
/// 上游 `base_url` 有时带 `/v1` 后缀、有时不带（`getEngineMaasApiBaseUrl`
/// 在客户端里对同一件事做过 `endsWith("/v1") ? n : n + "/v1"` 的兜底），
/// 所以这里对两种情况都归一。
pub fn chat_completions_url(base: &str) -> String {
    let trimmed = base.trim().trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        format!("{trimmed}/chat/completions")
    } else {
        format!("{trimmed}{CHAT_COMPLETIONS_PATH}")
    }
}

/// 拼接网关域上的绝对 URL。
pub fn gateway_url(path: &str) -> String {
    format!("{}{}", GATEWAY_BASE.trim_end_matches('/'), path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_url_does_not_double_v1() {
        assert_eq!(
            chat_completions_url("https://maas.xiaoluozi.cn"),
            "https://maas.xiaoluozi.cn/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("https://maas.xiaoluozi.cn/"),
            "https://maas.xiaoluozi.cn/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("https://maas.xiaoluozi.cn/v1"),
            "https://maas.xiaoluozi.cn/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("https://maas.xiaoluozi.cn/v1/"),
            "https://maas.xiaoluozi.cn/v1/chat/completions"
        );
    }

    #[test]
    fn gateway_url_joins_cleanly() {
        assert_eq!(
            gateway_url(MINT_PATH),
            "https://gateway.qwenwork.cn/api/desktop-sessions"
        );
    }

    /// 钱包/签到那一族**必须**带 `/adapter/` 段：去掉它会静默 404，
    /// 而 404 在界面上表现为「这家没有余额」，很难往路径写错上想。
    #[test]
    fn adapter_endpoints_keep_the_adapter_segment() {
        for path in [
            WALLETS_PATH,
            ACCOUNT_CONTEXT_PATH,
            DAILY_CREDIT_CLAIM_PATH,
        ] {
            assert!(
                path.starts_with("/api/v1/adapter/"),
                "{path} 少了 /adapter/ 段"
            );
        }
    }

    /// `include` 少了哪一段，上游就整个不回那一段 —— 不是「回了空」。
    #[test]
    fn account_context_include_lists_every_segment() {
        for segment in ["user", "plan", "quota", "page"] {
            assert!(
                ACCOUNT_CONTEXT_INCLUDE.split(',').any(|item| item == segment),
                "include 缺 {segment}"
            );
        }
    }
}
