//! OrcaTerm 适配器：把会话式协议层接进 provider 注册表。
//!
//! ── 这一层解决什么问题 ──────────────────────────────────────
//! 协议实现（建会话 / SSE 翻译 / 续期 / 余额）全部收在 `providers/orcaterm/`
//! （`chat.rs` / `credentials.rs` / `balance.rs`），对外只留一个口子：
//! `chat::run_chat(...) -> ForwardOutcome`。本文件就是那个「接线」：把转发编排
//! 给的散装入参（原始 body、账号 id、出站代理、流式标志、记账槽）转调
//! `run_chat`。装配以外的逻辑一行都不在这里。
//!
//! ── 与 KukuAI 同属「有状态」但会话是**请求内**的 ──────────────
//! `is_stateful()` 返回 true：上游一次对话 = 建会话 → SSE 两步，塞不进单请求
//! 契约（`build_chat_request` 返回 503 防御，理由与 CatPaw / KukuAI 相同）。
//! 但会话**不跨请求**（每个请求新建、用毕即弃），因此没有会话注册表 /
//! 轮次状态机 —— `chat::run_chat` 单函数完成全部时序。
//!
//! ── 凭证：OAuth JWT，**有真续期接口**（与 KukuAI / CatPaw 的关键差别）──
//! `supports_refresh = true`：临期主动刷（`credentials_expiring`，窗口 10 分钟）
//! 与 401 强制刷（`refresh_access_token`）都走 `OAuthRefreshToken`，结果回写
//! 账号记录（`credentials::refresh` 内部，含单飞）。
//!
//! ── 错误分类：一律 Fatal（与 CatPaw / KukuAI 同一核对结论）────
//! 上游的限额形态没有 HTTP 429（配额耗尽回流内 `LLM_ERROR` 帧），有状态路径
//! 对 Fatal 的既有处置就是原样透传 —— 配额月度重置，做冷却记账的收益为负
//! （冷却键是账号级配额而非单模型，误冷却一整个月比透传错误更糟）。
//!
//! ── 思考等级绑定不接（诚实声明）──────────────────────────────
//! 对话请求体没有思考档位字段（setting 里只有模型名与工具配置；上游模型自带
//! 思考且默认开启），没有可注入的键 —— 按 trait 契约「没有证据就不注入」
//! 保持默认 Skip。思考增量按 DeepSeek 事实标准放 `delta.reasoning_content`
//! 下发（见 `chat.rs`）。
//!
//! ── 硬约束 ──────────────────────────────────────────────────
//! release 是 `panic=abort`：本文件零 unwrap/expect/panic，不持任何锁跨 await。

use std::sync::Arc;

use axum::http::HeaderMap;
use serde_json::Value;

use crate::server::core::account_store::AccountStore;
use crate::server::core::proxies::ResolvedProxy;
use crate::server::core::upstream::usage::RequestTelemetry;
use crate::server::core::upstream::ForwardOutcome;
use crate::server::errors::GatewayError;

use super::{balance, chat, credentials, models};
use crate::server::core::providers::adapter::{
    ChatRequestPlan, ModelRefreshOutcome, ProviderAdapter, UpstreamErrorClass,
};
use crate::server::core::providers::{kind_id, ProviderKind};

/// OrcaTerm 适配器（无状态单例：会话是请求内的，凭证在账号存储里，
/// 本结构不持有任何字段）。
pub struct OrcatermAdapter;

/// 进程级实例（`adapter_for` 返回它的 `&'static` 引用）
pub static ORCATERM_ADAPTER: OrcatermAdapter = OrcatermAdapter;

impl ProviderAdapter for OrcatermAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OrcaTerm
    }

    /// 上游是「建会话 → SSE」的两步时序（固有性质，见模块头）。
    ///
    /// 编排层据此把本 provider 分流到 [`Self::forward_conversation`]，
    /// 而不是 `build_chat_request` + `send_chat_request` 那条单请求路径。
    fn is_stateful(&self) -> bool {
        true
    }

    /// 模型清单：静态表（上游没有可拉的清单接口，见 `models.rs` 模块头）。
    fn list_models(&self) -> Vec<Value> {
        models::list()
    }

    /// **防御性报错**：OrcaTerm 走会话式转发，不走单请求路径（模块头）。
    fn build_chat_request(
        &self,
        _account: &Value,
        _body: &Value,
        _client_headers: &HeaderMap,
    ) -> Result<ChatRequestPlan, GatewayError> {
        Err(GatewayError::with_status(
            503,
            format!(
                "{} 走会话式转发，不走单请求路径（内部错误：编排层未按 is_stateful 分流）",
                kind_id(self.kind())
            ),
        ))
    }

    /// 一律 Fatal（模块头的核对结论）：上游没有 HTTP 429，限额错误回流内
    /// `LLM_ERROR` 帧，在 `chat::run_chat` 内部归一成 GatewayError 透传。
    fn classify_error(&self, status: u16, error_body: &Value) -> UpstreamErrorClass {
        let message = error_body
            .get("message")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .unwrap_or("上游错误")
            .to_string();
        UpstreamErrorClass::Fatal { status, message, upstream_code: None }
    }

    /// 拉取远程模型目录：**没有**远程接口（见 `models.rs` 模块头的排除依据），
    /// 恒返回「没刷」（不是失败 —— 界面上这家本就该显示 `fixed` 固定清单，
    /// 由 [`Self::supports_model_refresh`] 的 false 驱动）。
    fn refresh_models<'a>(
        &'a self,
        _store: &'a AccountStore,
        _account_id: &'a str,
        _force: bool,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = ModelRefreshOutcome> + Send + 'a>,
    > {
        Box::pin(async move { ModelRefreshOutcome::unchanged() })
    }

    /// 没有远程模型目录（清单来自客户端内置，见 `models.rs` 模块头）。
    fn supports_model_refresh(&self) -> bool {
        false
    }

    /// 凭证：从账号记录取 JWT（临期续期交给 [`Self::refresh_access_token`]，
    /// 这里不刷 —— trait 语义是「取可用 token」，消费方是探活类动作）。
    fn ensure_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let credentials = credentials::snapshot_for(store, account_id)?;
            Ok(credentials.access_token)
        })
    }

    /// 支持**网页登录**：官方 OAuth 授权页 → 微信扫码 → `orcaterm://oauth_callback`
    /// → 后端用 `OAuthExchangeToken` 换令牌落账号（见 `login.rs` 模块头）。
    fn supports_web_login(&self) -> bool {
        true
    }

    /// 构造授权页地址 + 一次性 state（`login.rs`）。
    ///
    /// `state` 同时是 OAuth 的 `session_id`：回调 URL 里带的就是它，后端据此
    /// 换令牌（见 `login.rs` 模块头）。
    fn build_login_url(&self) -> Option<(String, String)> {
        let state = super::login::new_login_state();
        Some((super::login::build_login_url(&state), state))
    }

    /// 回调换令牌：回调 URL 里的 `session_id` → `OAuthExchangeToken` → 落账号。
    ///
    /// 与 CodeArts / AutoClaw 同款形态（回调 URL 交由后端换码），差别在于这里的
    /// 凭据是 `session_id` 而非 `code`（见 `login.rs` 模块头）。
    fn exchange_login_code<'a>(
        &'a self,
        store: &'a AccountStore,
        callback_url: &'a str,
        state: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let session_id = super::login::callback_session_id(callback_url).ok_or_else(|| {
                GatewayError::with_status(400, "OrcaTerm 回调里没有 session_id，请重新发起网页登录")
            })?;
            // 逐字比对发起时生成的那个值（深度防御：调用链上游已校验过 state，
            // 但「只有本模块知道该长什么样」的校验放在这里最可靠）。
            if session_id != state {
                return Err(GatewayError::with_status(
                    400,
                    "OrcaTerm 回调的 session_id 与本次登录不匹配，请重新发起网页登录",
                ));
            }
            let credentials = super::login::exchange_session(&session_id).await?;
            let account = store
                .add_orcaterm_account(&credentials, None, "web")
                .map_err(|error| GatewayError::with_status(error.status_code, error.message))?;
            let account_id = account
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let label = account
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("OrcaTerm 账号")
                .to_string();
            crate::server::logging::log("[Login]", &format!("✅ OrcaTerm 网页登录成功: {label}"));
            Ok(account_id)
        })
    }

    /// 支持续期：上游有 `OAuthRefreshToken`（2026-10-16 实测，见
    /// `credentials.rs` 模块头）。
    fn supports_refresh(&self) -> bool {
        true
    }

    /// 该账号的 JWT 是否已过期或临期（窗口 10 分钟；`exp` 判不出 → false）。
    ///
    /// **实现不持锁跨 await**：取快照（同步）即返回，真正的网络动作在
    /// [`Self::refresh_access_token`] 里。
    fn credentials_expiring(&self, store: &AccountStore, account_id: &str) -> bool {
        match credentials::snapshot_for(store, account_id) {
            Ok(credentials) => credentials.is_expiring(),
            Err(_) => false,
        }
    }

    /// 401 后的**强制**刷新（`OAuthRefreshToken`，单飞 + 回写账号记录）。
    fn refresh_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let credentials = credentials::snapshot_for(store, account_id)?;
            let next = credentials::refresh(store, &credentials, account_id, true).await?;
            Ok(next.access_token)
        })
    }

    /// 有余额概念：`DescribeUserBillingStatus` 的月度 AI 配额（`balance.rs`）。
    fn supports_usage(&self) -> bool {
        true
    }

    /// 查余额（`balance.rs`，归一化形状见 trait 文档）。
    fn query_usage<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move { balance::query_usage(store, account_id).await })
    }

    /// 会话式转发入口：转调 `chat::run_chat`。
    ///
    /// ── 分工（哪些判断不属于这里）──────────────────────────────
    ///   账号选路、限额冷却、provider 轮询、telemetry 记账都在编排层；
    ///   协议时序（建会话 / SSE 翻译 / 401 自愈）都在 `chat.rs`。
    ///   本函数只做**装配**：把散装入参原样转调。
    fn forward_conversation<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        body: &'a Value,
        _client_headers: &'a HeaderMap,
        proxy: Option<ResolvedProxy>,
        stream: bool,
        telemetry: &'a Arc<RequestTelemetry>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<ForwardOutcome, GatewayError>> + Send + 'a,
        >,
    > {
        Box::pin(async move {
            chat::run_chat(store, account_id, body, proxy, stream, telemetry).await
        })
    }
}
