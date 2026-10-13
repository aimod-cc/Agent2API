//! KukuAI 适配器：把会话式协议层接进 provider 注册表。
//!
//! ── 这一层解决什么问题 ──────────────────────────────────────
//! 协议实现（建会话 / 分配算力 / SSE 翻译）全部收在 `providers/kuku/`
//! （`chat.rs`），对外只留一个口子：`chat::run_chat(...) -> ForwardOutcome`。
//! 本文件就是那个「接线」：把转发编排给的散装入参（原始 body、账号 id、
//! 出站代理、流式标志、记账槽）转调 `run_chat`。装配以外的逻辑一行都不在这里。
//!
//! ── 与 CatPaw 同属「有状态」但会话是**请求内**的 ──────────────
//! `is_stateful()` 返回 true：上游一次对话 = 建会话 → 分配算力 → SSE 三步，
//! 塞不进单请求契约（`build_chat_request` 返回 503 防御，理由与 CatPaw 相同）。
//! 但 KukuAI 的会话**不跨请求**（每次新建、用毕即弃），因此没有会话注册表 /
//! 轮次状态机 —— `chat::run_chat` 单函数完成全部时序。
//!
//! ── 凭证：三个 Cookie（BDUSS/STOKEN/gfprotpl），**无法刷新** ────
//! BDUSS 是百度通行证登录态（不是 JWT，没有 refreshToken 概念），上游也没有
//! 刷新接口 —— `supports_refresh` 保持默认 false，401 后用户重新登录/重新导入
//! 即可（与 CatPaw 同一处境）。`supports_web_login` 同理保持 false：
//! KukuAI 没有官方 OAuth 授权页可走（登录态在客户端 / 手动粘贴）。
//!
//! ── 思考等级绑定不接（诚实声明）──────────────────────────────
//! 上游有 `think_mode` 档位，但网关的通用等级 → `think_mode` 的映射没有实测
//! 证据（参考实现固定用 3），按 trait 契约「没有证据就不注入」返回 Skip。
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

/// KukuAI 适配器（无状态单例：会话是请求内的，凭证在账号存储里，
/// 本结构不持有任何字段）。
pub struct KukuAdapter;

/// 进程级实例（`adapter_for` 返回它的 `&'static` 引用）
pub static KUKU_ADAPTER: KukuAdapter = KukuAdapter;

impl ProviderAdapter for KukuAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Kuku
    }

    /// 上游是「建会话 → 分配算力 → SSE」的三步时序（固有性质）。
    ///
    /// 编排层据此把本 provider 分流到 [`Self::forward_conversation`]，
    /// 而不是 `build_chat_request` + `send_chat_request` 那条单请求路径。
    fn is_stateful(&self) -> bool {
        true
    }

    /// 模型清单：**远程目录优先**，未拉到用静态兜底（`models.rs`）。
    fn list_models(&self) -> Vec<Value> {
        models::list()
    }

    /// **防御性报错**：KukuAI 走会话式转发，不走单请求路径（模块头）。
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

    /// 一律 Fatal：上游业务错误在 `chat::run_chat` 内部已经归一到
    /// `GatewayError`（状态码与文案按参考实现的口径映射好），本方法在
    /// 正常路径上走不到（与 CatPaw 同一核对结论）。
    fn classify_error(&self, status: u16, error_body: &Value) -> UpstreamErrorClass {
        let message = error_body
            .get("message")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .unwrap_or("上游错误")
            .to_string();
        UpstreamErrorClass::Fatal { status, message, upstream_code: None }
    }

    /// 拉取远程模型目录（`GET /wenchain/genflowpro/model_list`，见 `models.rs`）。
    ///
    /// 凭证取当前账号（与转发同一套 Cookie）；没有可用登录态时返回
    /// `unchanged()`（不是错误）。`force` 一路透传：`false` 走 TTL 早退
    /// （自动路径），`true` 真打上游（用户手动点了「刷新模型清单」）。
    fn refresh_models<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        force: bool,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = ModelRefreshOutcome> + Send + 'a>,
    > {
        Box::pin(async move { models::refresh(store, account_id, force).await })
    }

    /// 有远程模型目录（`/wenchain/genflowpro/model_list`）。
    fn supports_model_refresh(&self) -> bool {
        true
    }

    /// 凭证：从账号记录取 BDUSS（返回 token 字符串 = bduss；uid 在
    /// `forward_conversation` 里随完整凭证取）。
    ///
    /// 本家**没有**环境变量旁路与桌面端实时登录态兜底（桌面端导入会落成一条
    /// 账号记录，见账号层）—— 因此 `account_id` 为空时也走账号记录
    /// （本家组内最优账号），取不到就报错并说明该怎么补。
    fn ensure_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, GatewayError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let credentials = credentials::snapshot_for(store, account_id)?;
            Ok(credentials.bduss)
        })
    }

    /// 有余额概念：`GET /bizapi/gfpro/getgfvipremain`（`balance.rs`）。
    ///
    /// 与 CatPaw 不同，KukuAI **没有独立查询凭证** —— 转发用的三个 Cookie
    /// 直接能查，因此 `supports_usage = true` 后每个账号都能查到。
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

    /// 支持**网页登录**：打开百度通行证官方登录页（短信验证码 / 扫码都在这张
    /// 页上，官方页面自己处理风控），登录成功后由登录窗口的注入脚本把登录态
    /// Cookie 交回网关（见 `login.rs` 模块头）。
    fn supports_web_login(&self) -> bool {
        true
    }

    /// 构造 passport 登录页地址 + 一次性 state（`login.rs`）。
    fn build_login_url(&self) -> Option<(String, String)> {
        let state = super::login::new_login_state();
        Some((super::login::build_login_url(&state), state))
    }

    /// 会话式转发入口：转调 `chat::run_chat`。
    ///
    /// ── 分工（哪些判断不属于这里）──────────────────────────────
    ///   账号选路、限额冷却、provider 轮询、telemetry 记账都在编排层；
    ///   协议时序（建会话 / 分配算力 / SSE 翻译）都在 `chat.rs`。
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
