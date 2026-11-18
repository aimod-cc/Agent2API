//! Codex ChatGPT 订阅的 Rust 内置提供商。
//!
//! 移植依据：Acankao/magpie/internal/provider/{account,signin,codex_device_signin,
//! codex_models,codex_request,subscription_usage}.go 与 internal/gateway/codex*.go。
//! 使用 Codex 官方公开 OAuth 客户端，不走 ChatGPT API 动态 Sign-in、不运行插件。
//! 仅接受主动设备授权或用户提交的凭据内容；永远不扫描本机 Codex 登录文件。
//! 签到/领取没有移植动作，不把未知能力包装为已支持；额度重置与购买不自动执行。

mod account;
mod credentials;
mod errors;
mod http;
mod login;
mod models;
mod request;
mod usage;

use std::future::Future;
use std::pin::Pin;

use agent2api_provider_protocol::{LoginPoll, LoginStart, NormalizeInput, NormalizedAccount};
use axum::http::HeaderMap;
use serde_json::Value;

use crate::server::core::account_store::AccountStore;
use crate::server::errors::GatewayError;

use super::adapter::{
    ChatRequestPlan, ModelRefreshOutcome, ProviderAdapter, ReasoningPatch, UpstreamErrorClass,
};
use super::ProviderKind;

pub const ID: &str = "codex-subscription";
pub use login::LOGIN_TTL_MS;
pub use models::{refresh_meta, CACHE_SCOPE};
pub use request::build_native_request;

pub struct CodexSubscription;
pub static ADAPTER: CodexSubscription = CodexSubscription;

type Async<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

impl ProviderAdapter for CodexSubscription {
    fn kind(&self) -> ProviderKind {
        ProviderKind::CodexSubscription
    }
    fn provider_id(&self) -> &'static str {
        ID
    }
    fn label(&self) -> &'static str {
        "Codex 订阅"
    }
    fn list_models(&self) -> Vec<Value> {
        models::list()
    }
    fn advertise_models(&self, store: &AccountStore, manifest: Vec<Value>) -> Vec<Value> {
        models::advertise(store, manifest)
    }

    fn build_chat_request(
        &self,
        account: &Value,
        body: &Value,
        client_headers: &HeaderMap,
    ) -> Result<ChatRequestPlan, GatewayError> {
        request::chat(account, body, client_headers)
    }

    fn ensure_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> Async<'a, Result<String, GatewayError>> {
        Box::pin(credentials::ensure(store, account_id, false))
    }

    fn refresh_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> Async<'a, Result<String, GatewayError>> {
        Box::pin(credentials::ensure(store, account_id, true))
    }

    fn credentials_expiring(&self, store: &AccountStore, account_id: &str) -> bool {
        credentials::expiring(store, account_id)
    }
    fn supports_refresh(&self) -> bool {
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
    fn supports_account_user_agent(&self) -> bool {
        false
    }
    fn sse_model_rewrite(&self) -> bool {
        true
    }
    fn is_error_response(&self, status: u16, headers: &HeaderMap) -> bool {
        !(200..300).contains(&status)
            || headers
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| {
                    value
                        .split(';')
                        .next()
                        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
                })
    }

    fn refresh_models<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
        force: bool,
    ) -> Async<'a, ModelRefreshOutcome> {
        Box::pin(models::refresh(store, account_id, force))
    }

    fn query_usage<'a>(
        &'a self,
        store: &'a AccountStore,
        account_id: &'a str,
    ) -> Async<'a, Result<Value, GatewayError>> {
        Box::pin(usage::query(store, account_id))
    }

    fn normalize_account<'a>(
        &'a self,
        _store: &'a AccountStore,
        input: NormalizeInput,
    ) -> Async<'a, Result<NormalizedAccount, GatewayError>> {
        Box::pin(async move { account::normalize(input) })
    }

    fn plugin_account_view<'a>(
        &'a self,
        secret: &'a Value,
    ) -> Async<'a, Result<Value, GatewayError>> {
        Box::pin(async move {
            let tokens = account::Tokens::read(secret)?;
            Ok(tokens.public(
                None,
                secret.get("needsRelogin").and_then(Value::as_bool) == Some(true),
            ))
        })
    }

    fn login_methods_metadata<'a>(
        &'a self,
        _store: &'a AccountStore,
    ) -> Async<'a, Result<Option<Value>, GatewayError>> {
        Box::pin(async { Ok(Some(login::metadata())) })
    }

    fn admin_action<'a>(
        &'a self,
        _store: &'a AccountStore,
        _account_id: &'a str,
        action: &'a str,
        _payload: &'a Value,
    ) -> Async<'a, Result<Value, GatewayError>> {
        Box::pin(async move {
            match action {
                "auth.methods" => Ok(login::metadata()),
                _ => Err(GatewayError::with_status(501, "Codex 尚未接入这项管理操作")),
            }
        })
    }

    fn build_login_flow_with_options<'a>(
        &'a self,
        options: Value,
    ) -> Async<'a, Result<Option<LoginStart>, GatewayError>> {
        Box::pin(async move { login::start(options).await.map(Some) })
    }

    fn login_poll_async<'a>(&'a self, flow: &'a str) -> Async<'a, Result<LoginPoll, GatewayError>> {
        Box::pin(async move { login::poll(flow) })
    }

    fn login_cancel_async<'a>(&'a self, flow: &'a str) -> Async<'a, Result<(), GatewayError>> {
        Box::pin(async move {
            login::cancel(flow);
            Ok(())
        })
    }

    fn persist_login_account(&self, account: &NormalizedAccount) -> Result<String, GatewayError> {
        login::persist(account, None)
    }

    fn persist_login_account_with_proxy(
        &self,
        account: &NormalizedAccount,
        proxy: Option<&Value>,
    ) -> Result<String, GatewayError> {
        login::persist(account, proxy)
    }

    fn classify_error(&self, status: u16, error_body: &Value) -> UpstreamErrorClass {
        errors::classify(status, error_body)
    }
    fn classify_conversation_error(&self, error: &GatewayError) -> UpstreamErrorClass {
        errors::classify_conversation(error)
    }

    fn quota_cooldown_models(&self, account_id: &str, wire_model: &str) -> Vec<String> {
        crate::server::core::account_store::plugin_accounts::shared()
            .map(|store| models::cooldown_models(&store, account_id, wire_model))
            .unwrap_or_else(|| vec![wire_model.to_string()])
    }

    fn reasoning_patch(&self, level: &str, model: &str, body: &Value) -> ReasoningPatch {
        match request::reasoning_patch(level, model) {
            ReasoningPatch::Set { value, .. }
                if body.get("input").is_some() && body.get("messages").is_none() =>
            {
                let mut reasoning = body
                    .get("reasoning")
                    .filter(|value| value.is_object())
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                reasoning["effort"] = value;
                ReasoningPatch::Set {
                    field: "reasoning",
                    value: reasoning,
                }
            }
            patch => patch,
        }
    }
}
