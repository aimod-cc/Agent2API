//! Claude 订阅的内置 Rust 桥接，不直接使用订阅令牌调用 Anthropic HTTP。
//!
//! 移植依据：Acankao/magpie/internal/{gateway/claude_subscription.go,
//! claudebridge/mcp.go,provider/claude_*}。目录初始化另参考
//! Acankao/hapi/cli/src/modules/common/claudeModels.ts（不发送模型提示词）。
//! 只使用未修改的原生 Claude Code；不移植 TUI 的 Bun preload、入口伪装、
//! 私有签名或 transcript 改写。-p 与交互模式的额度口径不保证相同。

mod accounts;
mod bridge;
mod errors;
mod events;
mod mcp;
mod media;
mod messages;
mod models;
mod private_fs;
mod process;
mod runtime;
mod sessions;
mod tools;
mod usage;

use std::{future::Future, pin::Pin, sync::Arc};

use agent2api_provider_protocol::{LoginPoll, LoginStart, NormalizeInput, NormalizedAccount};
use axum::http::HeaderMap;
use serde_json::{json, Value};

use super::adapter::{ChatRequestPlan, ModelRefreshOutcome, ProviderAdapter, ReasoningPatch, UpstreamErrorClass};
use super::ProviderKind;
use crate::server::core::upstream::{usage::RequestTelemetry, ForwardOutcome};
use crate::server::core::{account_store::AccountStore, proxies::ResolvedProxy};
use crate::server::errors::GatewayError;

type FutureResult<'a, T> = Pin<Box<dyn Future<Output = Result<T, GatewayError>> + Send + 'a>>;
pub const ID: &str = "claude-subscription";
pub const LABEL: &str = "Claude 订阅";
pub const MCP_HELPER_ARGUMENT: &str = "--claude-mcp-helper";
pub static ADAPTER: ClaudeSubscriptionAdapter = ClaudeSubscriptionAdapter;
pub struct ClaudeSubscriptionAdapter;

pub fn refresh_meta() -> (bool, i64) {
    models::refresh_meta()
}

/// 两种主程序都必须在常规启动（日志、数据库、Tauri）之前分派此隐藏入口。
/// args 不含可执行文件名和 MCP_HELPER_ARGUMENT，只包含 callback/tools 文件路径。
/// stdout 仅用于 MCP JSON-RPC；失败应写 stderr 并返回非零退出码。
pub async fn run_mcp_helper(args: Vec<String>) -> Result<(), String> {
    mcp::run_helper(args).await
}

/// 网关停机可主动调用；正常请求取消和流被丢弃也会各自回收子进程树。
pub fn shutdown() {
    sessions::shutdown();
}

impl ProviderAdapter for ClaudeSubscriptionAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::ClaudeSubscription
    }
    fn provider_id(&self) -> &'static str {
        ID
    }
    fn label(&self) -> &'static str {
        LABEL
    }
    fn list_models(&self) -> Vec<Value> {
        models::list()
    }
    fn advertise_models(&self, store: &AccountStore, manifest: Vec<Value>) -> Vec<Value> {
        models::advertise(store, manifest)
    }
    fn is_stateful(&self) -> bool {
        true
    }
    fn supports_chat(&self) -> bool {
        true
    }
    fn supports_model_refresh(&self) -> bool {
        true
    }
    fn supports_usage(&self) -> bool {
        true
    }
    fn supports_web_login(&self) -> bool {
        true
    }
    fn supports_default_model(&self) -> bool {
        true
    }
    fn supports_account_user_agent(&self) -> bool {
        false
    }
    fn sse_model_rewrite(&self) -> bool {
        true
    }
    // 登录态续期由官方 CLI 自己负责；网关没有可单独调用的 token 刷新接口。
    fn supports_refresh(&self) -> bool {
        false
    }
    fn plugin_checkin_capability(&self) -> Option<bool> {
        Some(false)
    }

    fn build_chat_request(&self, _: &Value, _: &Value, _: &HeaderMap) -> Result<ChatRequestPlan, GatewayError> {
        Err(errors::local("claude_cli_required", "Claude 订阅必须经官方 Claude Code 转发，不能转换成 API Key 请求"))
    }

    fn ensure_access_token<'a>(&'a self, store: &'a AccountStore, account: &'a str) -> FutureResult<'a, String> {
        Box::pin(async move {
            let link = accounts::load(store, account)?;
            link.check_profile()?;
            // 有状态入口只校验引用；不读取、复制或回传令牌。
            Ok(String::new())
        })
    }

    fn refresh_models<'a>(
        &'a self,
        store: &'a AccountStore,
        account: &'a str,
        force: bool,
    ) -> Pin<Box<dyn Future<Output = ModelRefreshOutcome> + Send + 'a>> {
        Box::pin(async move { models::refresh(store, account, force).await })
    }

    fn reasoning_patch(&self, level: &str, _: &str, _: &Value) -> ReasoningPatch {
        match level.trim().to_ascii_lowercase().as_str() {
            "low" | "medium" | "high" | "xhigh" | "max" => {
                ReasoningPatch::Set { field: "reasoning_effort", value: json!(level.trim().to_ascii_lowercase()) }
            }
            _ => ReasoningPatch::Skip {
                reason: "该档位未对应到 Claude Code 的明确思考等级，保留客户端原值"
            },
        }
    }

    fn classify_error(&self, status: u16, body: &Value) -> UpstreamErrorClass {
        errors::classify(
            status,
            body.get("message").and_then(Value::as_str).unwrap_or("Claude Code 请求失败"),
            body.get("reset_at").and_then(Value::as_i64),
        )
    }

    fn classify_conversation_error(&self, error: &GatewayError) -> UpstreamErrorClass {
        error
            .conversation_class
            .clone()
            .unwrap_or_else(|| errors::classify(error.status_code as u16, &error.message, None))
    }

    fn error_policy_async<'a>(
        &'a self,
        _: u16,
        body: &'a Value,
    ) -> Pin<Box<dyn Future<Output = super::policy::ErrorPolicy> + Send + 'a>> {
        Box::pin(async move {
            let stop = body
                .get("code")
                .and_then(Value::as_str)
                .is_some_and(|code| code.starts_with("claude_session_") || code == "claude_response_started");
            super::policy::ErrorPolicy {
                stop,
                reason: if stop {
                    "Claude 会话或工具结果不能安全换号重放".into()
                } else {
                    String::new()
                },
                ..Default::default()
            }
        })
    }

    fn forward_conversation<'a>(
        &'a self,
        store: &'a AccountStore,
        account: &'a str,
        body: &'a Value,
        headers: &'a HeaderMap,
        proxy: Option<ResolvedProxy>,
        stream: bool,
        telemetry: &'a Arc<RequestTelemetry>,
    ) -> FutureResult<'a, ForwardOutcome> {
        Box::pin(async move { bridge::forward(store, account, body, headers, proxy, stream, telemetry).await })
    }

    fn normalize_account<'a>(
        &'a self,
        _: &'a AccountStore,
        input: NormalizeInput,
    ) -> FutureResult<'a, NormalizedAccount> {
        Box::pin(async move { accounts::normalize(input).await })
    }

    fn plugin_account_view<'a>(&'a self, secret: &'a Value) -> FutureResult<'a, Value> {
        Box::pin(async move { accounts::public_from_secret(secret) })
    }

    fn login_methods_metadata<'a>(&'a self, _: &'a AccountStore) -> FutureResult<'a, Option<Value>> {
        Box::pin(async { Ok(Some(accounts::methods())) })
    }

    fn build_login_flow_with_options<'a>(&'a self, options: Value) -> FutureResult<'a, Option<LoginStart>> {
        Box::pin(async move { accounts::start(options).await.map(Some) })
    }

    fn login_poll_async<'a>(&'a self, flow: &'a str) -> FutureResult<'a, LoginPoll> {
        Box::pin(async move { accounts::poll(flow) })
    }

    fn login_cancel_async<'a>(&'a self, flow: &'a str) -> FutureResult<'a, ()> {
        Box::pin(async move {
            accounts::cancel(flow);
            Ok(())
        })
    }

    fn persist_login_account(&self, account: &NormalizedAccount) -> Result<String, GatewayError> {
        accounts::persist(account, None)
    }

    fn persist_login_account_with_proxy(
        &self,
        account: &NormalizedAccount,
        proxy: Option<&Value>,
    ) -> Result<String, GatewayError> {
        accounts::persist(account, proxy)
    }

    fn query_usage<'a>(&'a self, store: &'a AccountStore, account: &'a str) -> FutureResult<'a, Value> {
        Box::pin(async move { usage::query(store, account).await })
    }

    fn admin_action<'a>(
        &'a self,
        _: &'a AccountStore,
        _: &'a str,
        action: &'a str,
        _: &'a Value,
    ) -> FutureResult<'a, Value> {
        Box::pin(async move {
            if action == "auth.methods" {
                return Ok(accounts::methods());
            }
            Err(GatewayError::with_status(501, "Claude 订阅未接入此管理动作"))
        })
    }
}
