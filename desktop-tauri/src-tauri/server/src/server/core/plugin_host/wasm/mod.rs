//! 无 WASI 的嵌入式插件；每次调用独立实例，账号、网络与临时资源由主程序持有。

pub(crate) mod catalog;
mod context;
mod errors;
mod http;
mod limits;
mod login;
pub(crate) mod manifest;
pub(crate) mod native;
mod public_view;
mod resources;
mod runtime;
mod services;
mod state;
pub(crate) mod tasks;
mod host;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use agent2api_provider_protocol as proto;
use tokio::sync::Semaphore;

use context::Sink;
use runtime::Runtime;

const MAX_MESSAGE: usize = 16 * 1024 * 1024;

pub(super) struct Embedded {
    pub runtime: tokio::sync::RwLock<Option<Arc<Runtime>>>,
    pub streams: Mutex<HashMap<i64, Sink>>,
    pub idle: tokio::sync::Notify,
    pub active: std::sync::atomic::AtomicUsize,
}

impl Default for Embedded {
    fn default() -> Self {
        Self { runtime: tokio::sync::RwLock::new(None), streams: Mutex::new(HashMap::new()),
            idle: tokio::sync::Notify::new(), active: std::sync::atomic::AtomicUsize::new(0) }
    }
}

fn compile_slot() -> &'static Semaphore {
    static SLOT: OnceLock<Semaphore> = OnceLock::new();
    SLOT.get_or_init(|| Semaphore::new(1))
}

fn global_slots(streaming: bool) -> &'static Semaphore {
    static STREAMS: OnceLock<Semaphore> = OnceLock::new();
    static CONTROL: OnceLock<Semaphore> = OnceLock::new();
    if streaming { STREAMS.get_or_init(|| Semaphore::new(16)) }
    else { CONTROL.get_or_init(|| Semaphore::new(8)) }
}

fn error(message: impl Into<String>) -> proto::WireError {
    proto::WireError::with_code(message, proto::WireError::INTERNAL)
}

pub(crate) fn contract() -> serde_json::Value {
    serde_json::json!({ "abi": manifest::ABI, "capabilities": manifest::CAPABILITIES,
        "limits": { "messageBytes": MAX_MESSAGE, "streamChunkBytes": 64 * 1024, "resourceBytes": 96 * 1024 * 1024,
            "accountStateBytes": 2 * 1024 * 1024, "conversationTtlMs": 86_400_000 },
        "state": { "login": "discard_on_reload", "conversation": "preserve_on_reload_reset_on_restart",
            "account": "encrypted_persistent_compare_exchange", "schema": "plugin_owned" },
        "settings": { "schema": "init.settings_schema", "group": "init.settings_group",
            "types": ["string", "number", "boolean", "select"], "placement": "host_owned",
            "values": "host_saved_overrides", "secrets": "account_store_only", "runtimeOverrides": "settings.overrides" },
        "images": "normalized_result_resources", "catalog": "account_snapshots_provider_union",
        "nativeHelpers": "explicit_grant_pinned_package", "network": "https_exact_origins_no_redirects" })
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
