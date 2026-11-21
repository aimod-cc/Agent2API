//! Rust 内置 ChatGPT 订阅，来源为 Acankao/magpie/internal/provider/chatgpt_api.go。
//! 这是动态注册的 Sign in with ChatGPT → api.openai.com/v1/responses，
//! 不是 Codex 后端，也不是普通 OpenAI Key 或脚本插件。
//!
//! Magpie 的 siwcNoUsage 明确不提供此路线的套餐余额/窗口查询，本模块不猜接口、
//! 不拿单次响应 token usage 冒充剩余额度。签到/领取也没有本地参考链路。
//! SIWC 的产品适用范围仍需维护者确认；技术实现不代表允许共享他人套餐或通用转售。

mod account;
mod credentials;
mod diagnostic;
mod error;
mod host;
mod http;
mod import;
mod listener;
mod login;
mod models;
mod oidc;
mod request;

use std::future::Future;
use std::pin::Pin;

use agent2api_provider_protocol::{
    LoginCallback, LoginPoll, LoginStart, NormalizeInput, NormalizedAccount,
};
use axum::http::HeaderMap;
use serde_json::Value;

use crate::server::core::account_store::AccountStore;
use crate::server::core::login::LoginCallbackResult;
use crate::server::core::providers::adapter::{
    ChatRequestPlan, ModelRefreshOutcome, ProviderAdapter, ReasoningPatch, RetryAdvice,
    UpstreamErrorClass,
};
use crate::server::core::providers::ProviderKind;
use crate::server::errors::GatewayError;

pub use models::{refresh_meta, CACHE_SCOPE};
pub use request::build_native_request;

pub const ID: &str = "chatgpt-subscription";
const ISSUER: &str = "https://auth.openai.com";
const AUTHORIZE_URL: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/api/accounts/oauth/token";
const API_BASE: &str = "https://api.openai.com/v1";
const RESOURCE: &str = API_BASE;
const DYNAMIC_CLIENT: &str = "dynamic_agent_client";
const DIRECT_SCOPE: &str = "chatgpt.tokens.use.direct";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const USER_AGENT: &str = concat!("Agent2API/", env!("CARGO_PKG_VERSION"));
const USAGE_URL: &str = "https://chatgpt.com/settings/usage";

type Task<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub struct ChatgptSubscriptionAdapter;
pub static ADAPTER: ChatgptSubscriptionAdapter = ChatgptSubscriptionAdapter;

impl ProviderAdapter for ChatgptSubscriptionAdapter {
    fn kind(&self) -> ProviderKind {
        ProviderKind::ChatgptSubscription
    }
    fn provider_id(&self) -> &'static str {
        ID
    }
    fn label(&self) -> &'static str {
        "ChatGPT"
    }
    fn list_models(&self) -> Vec<Value> {
        models::list()
    }
    fn advertise_models(&self, store: &AccountStore, manifest: Vec<Value>) -> Vec<Value> {
        models::advertise(store, manifest)
    }
    fn supports_chat(&self) -> bool {
        true
    }
    fn supports_refresh(&self) -> bool {
        true
    }
    fn supports_model_refresh(&self) -> bool {
        true
    }
    fn supports_web_login(&self) -> bool {
        true
    }
    fn supports_usage(&self) -> bool {
        false
    }
    fn sse_model_rewrite(&self) -> bool {
        true
    }
    // 所有请求都用本应用真实 UA；不承诺目前管理请求未实现的自定义 UA 覆盖。
    fn supports_account_user_agent(&self) -> bool {
        false
    }

    fn build_chat_request(
        &self,
        account: &Value,
        body: &Value,
        headers: &HeaderMap,
    ) -> Result<ChatRequestPlan, GatewayError> {
        request::chat(account, body, headers)
    }

    fn ensure_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        id: &'a str,
    ) -> Task<'a, Result<String, GatewayError>> {
        Box::pin(async move {
            credentials::fresh(store, id, false)
                .await
                .map(|credentials| credentials.access_token)
        })
    }

    fn refresh_access_token<'a>(
        &'a self,
        store: &'a AccountStore,
        id: &'a str,
    ) -> Task<'a, Result<String, GatewayError>> {
        Box::pin(async move {
            credentials::fresh(store, id, true)
                .await
                .map(|credentials| credentials.access_token)
        })
    }

    fn credentials_expiring(&self, store: &AccountStore, id: &str) -> bool {
        credentials::expiring(store, id)
    }

    fn credentials_expiring_within(&self, store: &AccountStore, id: &str, min_lead_ms: i64) -> bool {
        credentials::expiring_within(store, id, min_lead_ms)
    }

    fn refresh_models<'a>(
        &'a self,
        store: &'a AccountStore,
        id: &'a str,
        force: bool,
    ) -> Task<'a, ModelRefreshOutcome> {
        Box::pin(async move { models::refresh(store, id, force).await })
    }

    fn is_error_response(&self, status: u16, headers: &HeaderMap) -> bool {
        // SIWC 请求固定 stream=true；2xx JSON 不是成功流，交给共享错误读取器。
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

    fn classify_error(&self, status: u16, body: &Value) -> UpstreamErrorClass {
        error::classify(status, body)
    }

    fn retry_decision_async<'a>(
        &'a self,
        _status: u16,
        body: &'a Value,
        attempt: usize,
        budget: usize,
    ) -> Task<'a, crate::server::core::providers::adapter::RetryDecision> {
        use crate::server::core::providers::adapter::RetryDecision;
        Box::pin(async move {
            let code = error::code(body);
            if matches!(
                code.as_str(),
                "subscription_sharing_user_not_eligible"
                    | "subscription_sharing_unsupported_capability"
                    | "subscription_sharing_route_not_supported"
            ) {
                return RetryDecision::Stop {
                    reason: error::message(400, &code),
                };
            }
            match self.retry_advice(body, attempt, budget) {
                Some(advice) => RetryDecision::Retry {
                    delay_ms: advice.delay_ms,
                    reason: advice.reason,
                },
                None => RetryDecision::Default,
            }
        })
    }

    fn retry_advice(&self, body: &Value, _attempt: usize, budget: usize) -> Option<RetryAdvice> {
        (budget > 0 && error::temporary(&error::code(body))).then(|| RetryAdvice {
            delay_ms: 1000,
            reason: "OpenAI 暂时无法确认 ChatGPT 套餐状态".to_string(),
        })
    }

    fn reasoning_patch(&self, level: &str, model: &str, body: &Value) -> ReasoningPatch {
        if crate::server::core::model_rules::reasoning_is_off(level) {
            return ReasoningPatch::Skip {
                reason: "未要求思考等级",
            };
        }
        let available = models::list().iter().any(|item| {
            item.get("id").and_then(Value::as_str) == Some(model)
                && item
                    .get("reasoningLevels")
                    .and_then(Value::as_array)
                    .is_some_and(|levels| levels.iter().any(|value| value.as_str() == Some(level)))
        });
        if available {
            if body.get("input").is_some() && body.get("messages").is_none() {
                let mut reasoning = body
                    .get("reasoning")
                    .filter(|value| value.is_object())
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                reasoning["effort"] = Value::String(level.to_string());
                return ReasoningPatch::Set {
                    field: "reasoning",
                    value: reasoning,
                };
            }
            ReasoningPatch::Set {
                field: "reasoning_effort",
                value: Value::String(level.to_string()),
            }
        } else {
            ReasoningPatch::Skip {
                reason: "ChatGPT 当前目录未声明这个思考等级",
            }
        }
    }

    fn login_methods_metadata<'a>(
        &'a self,
        store: &'a AccountStore,
    ) -> Task<'a, Result<Option<Value>, GatewayError>> {
        Box::pin(async move { Ok(Some(login::metadata(store))) })
    }

    fn login_start_async<'a>(&'a self) -> Task<'a, Option<LoginStart>> {
        Box::pin(async { login::start(serde_json::json!({})).await.ok() })
    }

    fn build_login_flow_with_options<'a>(
        &'a self,
        options: Value,
    ) -> Task<'a, Result<Option<LoginStart>, GatewayError>> {
        Box::pin(async move { login::start(options).await.map(Some) })
    }

    fn login_poll_async<'a>(&'a self, flow: &'a str) -> Task<'a, Result<LoginPoll, GatewayError>> {
        Box::pin(async move { login::poll(flow).await })
    }

    fn login_callback_result_async<'a>(
        &'a self,
        _store: &'a AccountStore,
        callback: LoginCallback,
    ) -> Task<'a, Result<Option<LoginCallbackResult>, GatewayError>> {
        Box::pin(async move { login::callback(callback).await.map(Some) })
    }

    fn login_cancel_async<'a>(&'a self, flow: &'a str) -> Task<'a, Result<(), GatewayError>> {
        Box::pin(async move {
            login::cancel(flow);
            Ok(())
        })
    }

    fn persist_login_account(&self, account: &NormalizedAccount) -> Result<String, GatewayError> {
        account::persist(account, None)
    }

    fn persist_login_account_with_proxy(
        &self,
        account: &NormalizedAccount,
        proxy: Option<&Value>,
    ) -> Result<String, GatewayError> {
        account::persist(account, proxy)
    }

    fn normalize_account<'a>(
        &'a self,
        _store: &'a AccountStore,
        input: NormalizeInput,
    ) -> Task<'a, Result<NormalizedAccount, GatewayError>> {
        Box::pin(async move { import::normalize(input).await })
    }

    fn plugin_account_view<'a>(
        &'a self,
        secret: &'a Value,
    ) -> Task<'a, Result<Value, GatewayError>> {
        Box::pin(async move { request::public_view(secret) })
    }

    fn query_usage<'a>(
        &'a self,
        _store: &'a AccountStore,
        _id: &'a str,
    ) -> Task<'a, Result<Value, GatewayError>> {
        Box::pin(async {
            Err(GatewayError::with_status(
                501,
                format!("ChatGPT SIWC 通道未提供套餐余额或窗口查询，请在 {USAGE_URL} 查看"),
            ))
        })
    }
}
