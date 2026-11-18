//! 内置订阅的公共接线，不承载各家上游协议。

use std::sync::Arc;

use axum::http::HeaderMap;
use serde_json::{json, Map, Value};

use super::{
    adapter::{ChatRequestPlan, ProviderAdapter},
    chatgpt_subscription, claude_subscription, codex_subscription, ProviderKind,
};
use crate::server::core::{account_store::AccountStore, protocol::native::WireProtocol};
use crate::server::errors::GatewayError;

pub const KINDS: [ProviderKind; 3] = [
    ProviderKind::ClaudeSubscription,
    ProviderKind::ChatgptSubscription,
    ProviderKind::CodexSubscription,
];

pub fn contains(provider: &str) -> bool {
    matches!(
        provider,
        claude_subscription::ID | chatgpt_subscription::ID | codex_subscription::ID
    )
}

pub fn adapter(provider: &str) -> Option<Arc<dyn ProviderAdapter>> {
    match provider {
        claude_subscription::ID => Some(Arc::new(claude_subscription::ClaudeSubscriptionAdapter)),
        chatgpt_subscription::ID => {
            Some(Arc::new(chatgpt_subscription::ChatgptSubscriptionAdapter))
        }
        codex_subscription::ID => Some(Arc::new(codex_subscription::CodexSubscription)),
        _ => None,
    }
}

pub fn native_protocol(provider: &str) -> Option<WireProtocol> {
    matches!(provider, chatgpt_subscription::ID | codex_subscription::ID)
        .then_some(WireProtocol::Responses)
}

pub fn build_native_request(
    provider: &str,
    account: &Value,
    body: &Value,
    headers: &HeaderMap,
) -> Result<ChatRequestPlan, GatewayError> {
    match provider {
        chatgpt_subscription::ID => {
            chatgpt_subscription::build_native_request(account, body, headers)
        }
        codex_subscription::ID => codex_subscription::build_native_request(account, body, headers),
        _ => Err(GatewayError::bad_request(
            "该内置提供商没有对应的原生协议入口",
        )),
    }
}

pub fn login_timeout_ms(provider: &str) -> u64 {
    if provider == codex_subscription::ID {
        codex_subscription::LOGIN_TTL_MS
    } else {
        crate::server::core::login::LOGIN_TIMEOUT_MS
    }
}

pub fn account_features() -> Map<String, Value> {
    KINDS
        .into_iter()
        .map(|kind| {
            let provider = super::kind_id(kind);
            let adapter = super::adapter::adapter_for(kind);
            let expiry = if kind == ProviderKind::ClaudeSubscription {
                Vec::<&str>::new()
            } else {
                vec!["expiresAt"]
            };
            (
                provider.to_string(),
                json!({
                    "usage": adapter.supports_usage(), "edition": false,
                    "identifier": ["email", "uid"], "expiry": expiry,
                    "email_as_name": true, "long_lived": false,
                }),
            )
        })
        .collect()
}

pub fn refresh_after_account_saved(store: AccountStore, provider: &str, account_id: &str) {
    let Some(adapter) = adapter(provider) else {
        return;
    };
    let account_id = account_id.to_string();
    crate::spawn_task(async move {
        let result = adapter.refresh_models(&store, &account_id, true).await;
        if let Some(message) = result.message {
            crate::server::logging::log(
                "[Models]",
                &format!(
                    "{} 账号已保存，首次获取模型失败：{message}",
                    adapter.label()
                ),
            );
        }
        super::catalog::invalidate();
    });
}

/// MCP 子进程必须先于 GUI 单实例、日志及数据库初始化分流，stdout 仅承载协议。
pub fn dispatch_helper() -> Option<u8> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some(claude_subscription::MCP_HELPER_ARGUMENT) {
        return None;
    }
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "无法创建 Claude MCP 运行时".to_string())
        .and_then(|runtime| {
            let result = runtime.block_on(claude_subscription::run_mcp_helper(args.collect()));
            runtime.shutdown_timeout(std::time::Duration::from_secs(1));
            result
        });
    Some(match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    })
}
