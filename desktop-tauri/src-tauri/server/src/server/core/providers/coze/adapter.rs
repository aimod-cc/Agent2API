//! Coze 适配器：把 v3/chat 协议层接进 provider 注册表。
//!
//! ── 这一层解决什么问题 ──────────────────────────────────────
//! 协议实现（PAT 铸造 / SSE 翻译 / 多轮拍平）全部收在 `providers/coze/` 的
//! `chat.rs`，对外只留一个口子：`chat::run_chat(...) -> ForwardOutcome`。
//! 本文件就是那个「接线」：把转发编排的散装入参原样转调，装配以外的逻辑
//! 一行都不在这里（与 kuku / trae 同一分工）。
//!
//! ── 为什么 `is_stateful = true` ─────────────────────────────
//! 上游 `/v3/chat` 是 **SSE 事件流**（`conversation.message.delta` /
//! `conversation.chat.completed` / `done`），事件名与字段全不是 OpenAI 形状；
//! 翻译必须发生在「读流」这一层，而单请求路径（`build_chat_request` +
//! `send_chat_request`）只负责拼包与透传，没有翻译位。所以走
//! `forward_conversation` 这条有状态路径 —— 它保住编排层的账号轮换 /
//! 限额冷却 / usage 记账，同时把「怎么读这条流」交给本家。
//! 注意「有状态」在这里指的是**协议有状态**，不是会话跨请求：Coze 的
//! `auto_save_history` 也由网关自己拍平历史，不存 conversation_id。
//!
//! ── 凭证：Cookie 整包，**无法刷新** ─────────────────────────
//! `sessionid_ss` 是网页登录态（不是 JWT，没有 refreshToken 概念），上游也
//! 没有刷新接口 —— `supports_refresh` 保持默认 false，401 后用户重新登录即可
//! （与 KukuAI / CatPaw 同一处境）。**PAT 不是账号级凭证**：它由
//! `credentials::ensure_pat` 按需铸造、进程内缓存，不属于维护任务管辖。
//!
//! ── 思考等级 / 模型目录 ────────────────────────────────────
//!   - `reasoning_patch` 不覆写：v3/chat 的公开 body 里没有思考档位字段
//!     （档位在 bot 自己的开发页配置），没有证据就不注入；
//!   - `list_models` 出的是**用户的 robot 清单**（model 即 bot_id），
//!     有远程目录（workspaces → bots），所以 `supports_model_refresh = true`。
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

use super::{balance, chat, checkin, credentials, models};
use crate::server::core::providers::adapter::{
    ChatRequestPlan, ModelRefreshOutcome, ProviderAdapter, UpstreamErrorClass,
};
use crate::server::core::providers::{kind_id, ProviderKind};

/// Coze 适配器（无状态单例：凭证在账号存储、PAT 缓存在 `credentials` 的
/// 进程级槽里，本结构不持有任何字段）。
pub struct CozeAdapter;

impl ProviderAdapter for CozeAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Coze
    }

    /// v3/chat 是 SSE 事件流协议（事件名 / 字段都不是 OpenAI 形状），
    /// 翻译只能在读流这一层做 —— 走会话式转发（模块头）。
    fn is_stateful(&self) -> bool {
        true
    }

    /// 模型清单 = 用户的机器人清单（`models.rs`；model 即 bot_id）。
    fn list_models(&self) -> Vec<Value> {
        models::list()
    }

    /// **防御性报错**：本家走会话式转发，不走单请求路径（模块头）。
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

    /// 一律 Fatal：上游业务错误在 `chat::run_chat` 内部已归一到 `GatewayError`
    /// （见 `chat.rs` 的 `business_error`），本方法在正常路径上走不到。
    fn classify_error(&self, status: u16, error_body: &Value) -> UpstreamErrorClass {
        let message = error_body
            .get("message")
            .or_else(|| error_body.get("msg"))
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .unwrap_or("上游错误")
            .to_string();
        UpstreamErrorClass::Fatal { status, message, upstream_code: None }
    }

    /// 远程目录：`GET /v1/workspaces` → `GET /v1/bots?workspace_id=`（PAT）。
    ///
    /// 没有可用登录态时返回 `unchanged()`（不是错误）；`force` 透传，
    /// `false` 走 10 分钟 TTL 早退。
    fn refresh_models<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        force: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ModelRefreshOutcome> + Send + 'a>> {
        Box::pin(async move { models::refresh(store, account_id, force).await })
    }

    fn supports_model_refresh(&self) -> bool {
        true
    }

    /// 凭证 = Cookie 整包（记录里存成 `accessToken` 的 Cookie 头串）。
    fn ensure_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let credentials = credentials::snapshot_for(store, account_id)?;
            Ok(credentials.cookie_header())
        })
    }

    /// 有余额概念：`trade/credit/balance` + `balance/detail` + `subscription_v2`。
    fn supports_usage(&self) -> bool {
        true
    }

    /// 查余额 / 最近到期 / 订阅（`balance.rs`，归一化形状见 trait 文档）。
    fn query_usage<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Value, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move { balance::query_usage(store, account_id).await })
    }

    /// 支持网页登录：打开 `www.coze.cn`，用户扫码 / 短信登录后壳侧读
    /// WebView2 的 Cookie 存储交回（`login.rs` 模块头）。
    fn supports_web_login(&self) -> bool {
        true
    }

    /// 登录页地址 + 一次性 state（`login.rs`）。
    fn build_login_url(&self) -> Option<(String, String)> {
        let state = super::login::new_login_state();
        Some((super::login::build_login_url(&state), state))
    }

    /// 会话式转发入口：转调 `chat::run_chat`。
    ///
    /// 账号选路、限额冷却、provider 轮询、telemetry 记账都在编排层；
    /// PAT 铸造、SSE 翻译、多轮拍平都在 `chat.rs`。本函数只做装配。
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
        Box<dyn std::future::Future<Output = Result<ForwardOutcome, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            chat::run_chat(store, account_id, body, proxy, stream, telemetry).await
        })
    }
}

/// 每日签到（`billing::checkin` 的 claim 口）—— 见 `checkin.rs`。
pub async fn claim_checkin(
    store: &AccountStore,
    account_id: &str,
) -> Result<Value, GatewayError> {
    checkin::claim_daily_checkin(store, account_id).await
}
