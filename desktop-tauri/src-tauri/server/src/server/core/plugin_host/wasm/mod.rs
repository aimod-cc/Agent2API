//! 无 WASI 的嵌入式插件；每次调用独立实例，账号、网络与临时资源由主程序持有。

pub(crate) mod account_origin;
pub(crate) mod aws_oidc;
pub(crate) mod catalog;
pub(super) mod catpaw_account;
mod catpaw_images;
pub(super) mod catpaw_turn;
pub(in crate::server) use catpaw_turn::output::{
    OutputGuard as CatPawOutputGuard, OutputSlot as CatPawOutputSlot,
    unconfirmed as catpaw_output_unconfirmed,
};
pub(super) mod checkin_call;
pub(in crate::server::core) use checkin_call::{OfficialDailyFact, ObservationView};
pub(crate) mod codearts_enrollment;
mod codearts_refresh;
mod codearts_session;
pub(crate) mod desktop_credentials;
pub(crate) mod desktop_launch;
mod inline_images;
mod doubao_upload;
mod doubao_turn;
mod monkeycode_cap;
pub(crate) mod codearts_login;
pub(crate) mod cline_free_enrollment;
mod cline_free_refresh;
mod client_device;
mod doubao_rewards;
mod doubao_local_gate;
mod compilation;
mod context;
mod diagnostics;
pub(super) mod conversation;
mod errors;
mod external_refresh;
mod fuel;
mod http;
pub(super) mod inbound_images;
pub(super) mod install_source;
mod limits;
mod legacy_credentials;
mod legacy_secret;
mod login;
pub(crate) mod managed;
pub(crate) mod managed_policy;
pub(crate) mod managed_task;
pub(crate) mod manifest;
pub(crate) mod native;
mod progress;
mod protocol;
mod qoder_umid;
mod quwork_device;
mod public_view;
mod resource_fetch;
mod resources;
mod runtime;
mod runtime_info;
mod services;
mod state;
pub(super) mod stream_failure;
pub(crate) mod tasks;
pub(crate) mod verification;
pub(crate) mod query_queue;
mod host;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agent2api_provider_protocol as proto;

use context::Sink;
use runtime::Runtime;

const MAX_MESSAGE: usize = 16 * 1024 * 1024;

pub(super) struct Embedded {
    pub runtime: tokio::sync::RwLock<Option<Arc<Runtime>>>,
    pub streams: Mutex<HashMap<i64, Sink>>,
    pub idle: tokio::sync::Notify,
    pub active: std::sync::atomic::AtomicUsize,
    pub shutdown: super::lifecycle::shutdown::State,
    // 编译/INIT前发布并保留当前弱Shared，关闭可立即通知而不等待starting/Runtime锁。
    pub loading: Mutex<Option<std::sync::Weak<context::Shared>>>,
}

impl Default for Embedded {
    fn default() -> Self {
        Self { runtime: tokio::sync::RwLock::new(None), streams: Mutex::new(HashMap::new()),
            idle: tokio::sync::Notify::new(), active: std::sync::atomic::AtomicUsize::new(0),
            shutdown: Default::default(), loading: Mutex::new(None) }
    }
}

fn error(message: impl Into<String>) -> proto::WireError {
    proto::WireError::with_code(message, proto::WireError::INTERNAL)
}

pub(crate) fn allocation() -> &'static str { compilation::allocation() }

pub(crate) fn contract() -> serde_json::Value {
    serde_json::json!({ "abi": manifest::ABI, "capabilities": manifest::CAPABILITIES,
        "limits": { "messageBytes": MAX_MESSAGE, "streamChunkBytes": 64 * 1024, "resourceBytes": 96 * 1024 * 1024,
            "stackBytes": compilation::MAX_WASM_STACK_BYTES,
            "accountStateBytes": 2 * 1024 * 1024, "conversationTtlMs": 86_400_000,
            "publicImageBytes": resource_fetch::MAX_IMAGE_BYTES, "publicDownloadBytes": resource_fetch::MAX_TOTAL_BYTES,
            "protocolConverters": protocol::MAX_CONVERTERS, "protocolChunkBytes": protocol::MAX_CHUNK_BYTES,
            "protocolOutputBytes": protocol::MAX_OUTPUT_BYTES, "protocolStreamBytes": protocol::MAX_STREAM_BYTES,
            "verificationMessageBytes": verification::MAX_MESSAGE, "verificationTimeoutMs": 2000,
            "verificationFuel": verification::FUEL, "verificationHostCalls": 128 },
        "compute": fuel::contract(),
        "scheduling": { "owner": "host", "legacyConcurrency": "advisory", "classes": ["conversation", "interactive", "background", "image"],
            "capacity": crate::server::core::runtime_capacity::limits() },
        "state": { "login": "discard_on_reload", "conversation": "preserve_on_reload_reset_on_restart",
            "account": "encrypted_persistent_compare_exchange", "schema": "plugin_owned" },
        "settings": { "schema": "init.settings_schema", "group": "init.settings_group",
            "types": ["string", "number", "boolean", "select"], "placement": "host_owned",
            "values": "host_saved_overrides", "secrets": "account_store_only", "runtimeOverrides": "settings.overrides" },
        "images": "normalized_result_resources", "catalog": "account_snapshots_provider_union",
        "tasks": { "declarations": "tasks.list", "categories": "tasks.categories", "placement": "host_owned",
            "categoryValues": ["checkin", "growth", "rewards", "maintenance", "other"],
            "categoryGrantsPermissions": false, "accountScope": "current_provider_current_account" },
        "desktopLaunch": desktop_launch::contract(),
        "nativeHelpers": "explicit_grant_pinned_package", "network": "https_exact_origins_no_redirects",
        "accountOrigin": { "capability": account_origin::CAPABILITY, "method": "http.open_account",
            "authorization": "current_installed_runtime_current_account_routes", "source": "current_account_secret",
            "proxySupported": true },
        "protocol": { "capability": "protocol.convert.v1", "request": ["chat", "anthropic", "responses"],
            "stream": ["chat", "anthropic", "responses", "gemini"], "output": "chat_sse", "handles": "current_call_only" },
        "verification": { "capability": verification::CAPABILITY, "claimCapability": verification::CLAIM_CAPABILITY,
            "actions": ["captcha.push", "claim.verification.poll", "claim.verification.complete"],
            "lease": ["verification.lease.open", "verification.lease.check", "verification.lease.close"],
            "leaseMaxMs": verification::lease::MAX_WAIT_MS,
            "entry": "host_owned_account_call", "network": false, "concurrency": 4, "globalConcurrency": crate::server::core::runtime_capacity::limits().interactive },
        "managedLogin": { "capability": "login.managed.v1", "steps": "host_owned_compare_exchange",
            "methods": ["login.context", "login.input.take", "login.step.publish", "login.loopback.open", "login.loopback.close"],
            "browserModes": ["external"], "captcha": { "engine": "aliyun", "protocol": "v2" },
            "input": "current_trusted_call_once", "lifetime": "login_task" },
        "sms": { "capability": "login.sms.v1", "methods": ["login.sms.send", "login.sms.verify"],
            "transport": "admin.action", "state": "host_owned_opaque_ticket", "sendStatus": ["sent", "captcha"] } })
}

pub(crate) fn forget(provider: &str) {
    tasks::forget(provider);
    state::forget_provider(provider);
}

pub fn owns(provider: &str) -> bool {
    super::host_for_provider(provider).is_some_and(|host| host.options.wasm.is_some())
}

pub(crate) fn has_capability(provider: &str, capability: &str) -> bool {
    super::host_for_provider(provider).and_then(|host| host.options.wasm.clone())
        .is_some_and(|options| options.allows(capability))
}
