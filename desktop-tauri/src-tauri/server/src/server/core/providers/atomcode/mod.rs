//! AtomCode（AtomGit）适配器：账号管理**加推理转发**。
//!
//! ── 上游长什么样 ────────────────────────────────────────────
//! 三条平面（细节见各子模块的模块头）：
//!
//! ```text
//!   登录代理（OAuth）  {broker}/auth/{login,check,token} + /oauth/refresh
//!   CodingPlan REST    {codingplan}/coding-plan/{status,models,claim,usage}-v2
//!   聊天网关           {gateway}/chat/completions             OpenAI Chat Completions
//! ```
//!
//! 本家**无状态**（一次 HTTP 请求 = 一次回答，上游就是 OpenAI 协议），
//! `is_stateful` / `supports_chat` 走默认值，转发链只需 `build_chat_request`。
//!
//! ── 四处与别家不同的取舍（都写在各自模块头，这里给索引）─────────
//!   1. **有续期，而且会轮换 refresh token**（`supports_refresh = true`）——
//!      续期必须单飞、必须写回新的一对，见 `oauth.rs` 与 `refresh.rs` 的模块头；
//!   2. **登录没有本地回调**：授权在代理域名上终结，靠轮询判完成 ——
//!      与 ZCode 的 CLI 登录同形，见 `oauth.rs`；
//!   3. **额度是「每窗口调用次数」**，不是积分也不是 token —— 见 `usage.rs`；
//!   4. **网关会把失败包装成成功**：`HTTP 200` + 正文 `参数错误`
//!      （或一页上游 HTML 报错），见 `errors.rs`。
//!
//! ── 还没接的一半：浏览器登录的界面侧 ──────────────────────────
//! 服务端的登录链路（`oauth::BrokerLogin`：发起 → 轮询 → 取令牌）已经就位，
//! 但**登录任务表与壳侧的「打开登录页」**还没接（`supports_web_login` 仍是
//! 默认的 false）。在那之前，账号只能经 `POST /api/accounts` 用粘贴凭据添加
//! （`accessToken` + `refreshToken`，字段名见 `credentials.rs`）。
//! 这一条是**如实说明**：界面上的「浏览器登录」按钮此刻会提示不支持，
//! 而不是弹一个永远不会完成的窗口。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic。

pub mod chat;
pub mod credentials;
pub mod endpoints;
pub mod errors;
pub mod models;
pub mod oauth;
pub mod usage;

use axum::http::HeaderMap;
use serde_json::Value;

use crate::server::core::account_store::AccountStore;
use crate::server::core::providers::adapter::{
    ChatRequestPlan, ModelRefreshOutcome, ProviderAdapter, UpstreamErrorClass,
};
use crate::server::core::providers::{ProviderKind, refresh_flight};
use crate::server::errors::GatewayError;
use crate::server::logging;

/// 适配器实例（`adapter_for(ProviderKind::Atomcode)` 返回这一个）。
///
/// **无状态**：它不持有任何数据 —— 凭证在账号存储、目录在 `models.rs` 的
/// 进程级句柄（可重建展示数据）。
pub struct AtomcodeAdapter;

/// 静态单例
pub static ATOMCODE_ADAPTER: AtomcodeAdapter = AtomcodeAdapter;

impl ProviderAdapter for AtomcodeAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Atomcode
    }

    /// 模型清单来自远程目录（`models.rs` 的进程缓存 + 落盘缓存）
    fn list_models(&self) -> Vec<Value> {
        models::list()
    }

    /// 构造 `POST {网关}/chat/completions`（OpenAI 协议，body 几乎原样透传）。
    fn build_chat_request(
        &self,
        account: &Value,
        body: &Value,
        client_headers: &HeaderMap,
    ) -> Result<ChatRequestPlan, GatewayError> {
        chat::build_chat_request(account, body, client_headers)
    }

    /// 上游错误分类（判定顺序本身是契约，见 `errors::classify_gateway_error`）：
    ///   1. `ATOMCODE_SIG_MISSING`（打到了需要签名的主机）→ `Fatal`，**先于** 401/403；
    ///   2. 401 / 403 → `TokenExpired`（文案含 `codingplan` 时是「没领套餐」→ `Fatal`）；
    ///   3. 429 → `QuotaLimited`（网关不给结构化恢复时间）；
    ///   4. 其余 → 共用内容拦截分类。
    fn classify_error(&self, status: u16, error_body: &Value) -> UpstreamErrorClass {
        errors::classify_gateway_error(status, error_body)
    }

    /// 有远程目录（`GET {codingplan}/coding-plan/models-v2`，见 `models.rs`）
    fn supports_model_refresh(&self) -> bool {
        true
    }

    /// 刷模型目录：用指定账号（空串 = 库里第一个可用账号）的令牌。
    ///
    /// 「一个账号都没有」不是失败 —— 那是「用户还没添加账号」的正常状态，
    /// 返回 `unchanged()`（脚本 / CI 不该看到一个红色失败，与 Loomy 同口径）。
    fn refresh_models<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        force: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelRefreshOutcome> + Send + 'a>> {
        Box::pin(async move {
            let record = store.atomcode_account_record(account_id);
            if record.is_none() {
                if account_id.is_empty() {
                    logging::verbose("[Models]", "AtomCode 模型目录刷新跳过：尚未添加账号");
                    return ModelRefreshOutcome::unchanged();
                }
                return ModelRefreshOutcome::failed("指定的账号不存在或不可用，请重新选择");
            }
            let credentials = match credentials::from_record(record.as_ref()) {
                Ok(credentials) => credentials,
                Err(error) => return ModelRefreshOutcome::failed(error.message),
            };
            models::refresh(&credentials, force).await
        })
    }

    /// 取可用访问令牌。
    ///
    /// ⚠️ **这里不发网络请求**（本方法是同步语义的取值口，且会在转发路径上被调）：
    /// 临期只记一行提示，真正的续期由维护链路（`supports_refresh` +
    /// `refresh_access_token`）与 401 后的强制刷新负责。
    fn ensure_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>>
    {
        Box::pin(async move {
            let record = store.atomcode_account_record(account_id);
            if !account_id.is_empty() && record.is_none() {
                return Err(GatewayError::with_status(
                    401,
                    format!("AtomCode 账号 {account_id} 不存在（请重新添加）"),
                ));
            }
            let credentials = credentials::from_record(record.as_ref())?;
            if credentials.expiring() {
                logging::verbose(
                    "[AtomCode]",
                    "账号令牌已过期或临期，将按需续期（续期会轮换 refresh token）",
                );
            }
            Ok(credentials.access_token)
        })
    }

    /// 有续期接口（`POST {broker}/oauth/refresh`）
    fn supports_refresh(&self) -> bool {
        true
    }

    /// 强制续期（401 之后 / 维护链路）：刷新并**把新的一对写回记录**。
    ///
    /// ── 为什么必须写回，以及为什么必须单飞 ───────────────────────
    /// 续期**轮换** refresh token：代理同时下发新 access 与新 refresh，
    /// 旧 access 立即失效。丢掉新的 refresh 等于让账号退化成必须重新登录；
    /// 两个并发续期互相作废（后写覆盖前者）—— 因此这里走本仓统一的
    /// `refresh_flight::Table` 做单飞，键含**两个令牌的指纹**
    /// （令牌一变就是新的一轮，与 Cline 同一口径）。
    fn refresh_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>>
    {
        Box::pin(async move {
            let record = store.atomcode_account_record(account_id);
            let credentials = credentials::from_record(record.as_ref())?;
            if credentials.refresh_token.is_empty() {
                return Err(GatewayError::with_status(
                    401,
                    "AtomCode 账号没有 refresh_token，无法续期，请重新登录",
                ));
            }
            let key = format!(
                "{}:{}:{}:{}",
                store.file_string(),
                account_id,
                refresh_flight::fingerprint(&credentials.access_token),
                refresh_flight::fingerprint(&credentials.refresh_token),
            );
            match ATOMCODE_FLIGHTS.get_or_init(refresh_flight::Table::new).join(&key) {
                refresh_flight::Join::Waiter(waiter) => {
                    waiter.wait().await.map(|fresh| fresh.access_token)
                }
                refresh_flight::Join::Leader(leader) => {
                    let result = refresh_and_save(store, &credentials).await;
                    leader.finish(result.clone());
                    result.map(|fresh| fresh.access_token)
                }
            }
        })
    }

    /// 临期判定：`createdAt + expiresIn`（**令牌不是 JWT，读不到 `exp`**），
    /// 提前 [`endpoints::REFRESH_WINDOW_SECONDS`] 报临期。
    fn credentials_expiring(&self, store: &AccountStore, account_id: &str) -> bool {
        if account_id.is_empty() {
            return false;
        }
        match store.atomcode_account_record(account_id) {
            Some(record) => credentials::from_record(Some(&record))
                .map(|credentials| credentials.expiring())
                .unwrap_or(false),
            None => false,
        }
    }

    /// 有额度概念（滚动调用次数窗口，见 `usage.rs`）
    fn supports_usage(&self) -> bool {
        true
    }

    /// 查额度：`GET {codingplan}/coding-plan/status-v2` 的窗口归一。
    /// 401 由调用方走统一的「刷新后重试」链路。
    fn query_usage<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>>
    {
        Box::pin(async move { usage::query_usage(store, account_id).await })
    }

    /// AtomCode **没有**「默认模型」概念（不指定模型时按目录/上游默认）
    fn supports_default_model(&self) -> bool {
        false
    }
}

/// 续期单飞表（进程级；键含两个令牌的指纹，令牌一变就是新的一轮）
static ATOMCODE_FLIGHTS: std::sync::OnceLock<refresh_flight::Table<credentials::AtomcodeCredentials>> =
    std::sync::OnceLock::new();

/// 真的去打一次续期，并把**轮换后的令牌对**写回账号记录。
///
/// 抽成自由函数是为了让单飞分支的「Leader 职责」只出现在一处：
/// 发请求 → 合并（非空才覆盖）→ 落盘 → 返回新凭证。
async fn refresh_and_save(
    store: &AccountStore,
    credentials: &credentials::AtomcodeCredentials,
) -> Result<credentials::AtomcodeCredentials, GatewayError> {
    let bundle = oauth::refresh(&credentials.refresh_token).await?;
    // 轮换语义：非空才覆盖（代理可能省略 refresh_token / expires_in / user
    // —— 见 oauth.rs 的模块头，整包替换会让账号失去续期能力）
    let previous = credentials::TokenBundle {
        access_token: credentials.access_token.clone(),
        refresh_token: credentials.refresh_token.clone(),
        ..Default::default()
    };
    let merged = bundle.merge_into(&previous);
    // 落盘失败**不**把这次续期当成功：新的 access 已经在手上、但记录里还是旧的，
    // 下次仍会走一次续期（而旧的 refresh 可能已被轮换作废）。如实上报。
    let expires_at = merged.expires_at(logging::now_ms());
    let written = store.update_account_tokens(
        &credentials.id,
        Some(&merged.access_token),
        Some(&merged.refresh_token),
        (expires_at > 0).then_some(expires_at as f64),
        None,
    );
    if !written {
        return Err(GatewayError::with_status(
            502,
            "AtomCode 续期成功但写回账号失败（账号记录不存在？）：请重新登录",
        ));
    }
    logging::log("[AtomCode]", "令牌已续期并写回账号（refresh token 已轮换）");
    Ok(credentials::AtomcodeCredentials {
        id: credentials.id.clone(),
        access_token: merged.access_token,
        refresh_token: merged.refresh_token,
        // 复用上面已经算好的那个（在字段被 move 之前算的）
        expires_at,
        user_id: merged.user_id,
        name: merged.name,
    })
}
#[cfg(test)]
mod tests {
    use super::*;

    /// 适配器声明的能力集（这份断言存在的意义：改动能力位时会被迫读一遍模块头
    /// 里「为什么」的那几节）。
    #[test]
    fn adapter_declares_the_expected_capabilities() {
        let adapter = &ATOMCODE_ADAPTER;
        assert_eq!(adapter.kind(), ProviderKind::Atomcode);
        assert!(adapter.supports_chat(), "上游就是 OpenAI 协议，必然能转发");
        assert!(!adapter.is_stateful(), "一次 HTTP 请求 = 一次回答");
        assert!(adapter.supports_model_refresh(), "有 models-v2 目录接口");
        assert!(adapter.supports_refresh(), "有 /oauth/refresh 续期接口");
        assert!(adapter.supports_usage(), "有 status-v2 额度窗口");
        assert!(!adapter.supports_default_model(), "本家没有默认模型概念");
        // 浏览器登录的界面侧还没接（见模块头「还没接的一半」）
        assert!(!adapter.supports_web_login());
    }
}
