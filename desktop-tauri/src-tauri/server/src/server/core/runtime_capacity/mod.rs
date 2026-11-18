mod memory;
mod monitor;
mod queue;
mod scope;

pub(crate) use memory::{Allocation, MemoryBudget};
pub(crate) use queue::{acquire, try_acquire, Ticket};
pub(crate) use scope::{background, background_if, background_scope, caller, caller_scope};

use std::sync::{Arc, Mutex, OnceLock};

use agent2api_provider_protocol as proto;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::Notify;

use crate::server::config::runtime_capacity::{self, CapacityMode, CapacitySettings};

pub const MIB: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class { Conversation, Interactive, Background, Image, Network, Request, ImageRequest }

impl Class {
    fn index(self) -> usize {
        match self { Self::Request => 5, Self::ImageRequest => 6, Self::Conversation => 0,
            Self::Interactive => 1, Self::Background => 2, Self::Image => 3, Self::Network => 4 }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    pub conversations: usize,
    pub interactive: usize,
    pub background: usize,
    pub background_per_provider: usize,
    pub images: usize,
    pub connections: usize,
    pub memory_bytes: usize,
    pub queue_requests: usize,
    pub queue_bytes: usize,
    pub queue_wait_seconds: u64,
    pub cpu_pressure: bool,
    pub memory_pressure: bool,
}

impl Limits {
    fn for_class(&self, class: Class) -> usize {
        match class { Class::Conversation | Class::Request => self.conversations, Class::Interactive => self.interactive,
            Class::Background => self.background, Class::Image | Class::ImageRequest => self.images, Class::Network => self.connections }
    }
}

pub(super) struct Manager {
    state: Mutex<queue::State>,
    changed: Notify,
}

pub(super) fn manager() -> &'static Arc<Manager> {
    static MANAGER: OnceLock<Arc<Manager>> = OnceLock::new();
    MANAGER.get_or_init(|| Arc::new(Manager { state: Mutex::new(queue::State::default()), changed: Notify::new() }))
}

pub fn changed() { manager().changed.notify_waiters(); }

pub fn limits() -> Limits {
    effective(&runtime_capacity::current().settings, monitor::sample())
}

fn effective(settings: &CapacitySettings, resources: monitor::Resources) -> Limits {
    let auto_memory = (resources.total_bytes / 2) as usize;
    let memory_bytes = if settings.memory_mib == 0 { auto_memory.max(16 * MIB) }
        else { settings.memory_mib.saturating_mul(MIB).min((resources.total_bytes * 3 / 4) as usize).max(16 * MIB) };
    let process_ceiling = resources.total_bytes.saturating_mul(4) / 5;
    let memory_pressure = resources.available_bytes < resources.total_bytes / 10
        || resources.container_bytes.unwrap_or(resources.process_bytes) >= process_ceiling;
    let cpu_pressure = resources.cpu_percent >= 90.0;
    let automatic = settings.mode == CapacityMode::Auto;
    let conversations = if automatic {
        settings.max_conversations.min((memory_bytes / (4 * MIB)).max(8))
    } else { settings.max_conversations };
    Limits {
        conversations, interactive: settings.max_interactive,
        background: if cpu_pressure || memory_pressure { 0 } else if automatic {
            settings.max_background.min(resources.cores.saturating_mul(4).max(2))
        } else { settings.max_background },
        background_per_provider: settings.background_per_provider,
        images: if cpu_pressure || memory_pressure { 0 } else if automatic {
            settings.max_images.min((memory_bytes / (64 * MIB)).max(1))
        } else { settings.max_images },
        connections: settings.max_connections.min(resources.connection_ceiling),
        memory_bytes, queue_requests: settings.queue_requests,
        queue_bytes: settings.queue_memory_mib.saturating_mul(MIB).min(memory_bytes / 4).max(8 * MIB),
        queue_wait_seconds: settings.queue_wait_seconds, cpu_pressure, memory_pressure,
    }
}

pub fn snapshot() -> Value {
    let configured = runtime_capacity::current();
    let resources = monitor::sample();
    let limits = effective(&configured.settings, resources);
    let state = manager().state.lock().unwrap_or_else(|error| error.into_inner());
    json!({ "settings": configured.settings, "revision": configured.revision, "error": configured.error,
        "runtime": { "allocation": crate::server::core::plugin_host::wasm::allocation(),
            "isolatedInstances": true, "preparedImports": true },
        "effective": limits, "resources": resources,
        "usage": { "conversations": state.active[0], "interactive": state.active[1], "background": state.active[2],
            "images": state.active[3], "connections": state.active[4], "memoryBytes": state.memory,
            "queued": state.waiters.len(), "queueBytes": state.queue_bytes,
            "requests": state.active[5], "imageRequests": state.active[6] },
        "rejected": state.rejected })
}

pub(crate) fn busy() -> proto::WireError {
    proto::WireError::with_code("运行容量暂时不足，排队等待已结束，请稍后重试", "plugin_busy")
}

pub(crate) fn exhausted() -> proto::WireError {
    proto::WireError::with_code("运行内存预算不足，未继续分配资源", "plugin_resource_limit")
}

pub fn retry_after_seconds() -> u64 { 1 }

pub fn value_bytes(value: &Value) -> usize {
    match value {
        Value::String(text) => text.len().saturating_mul(2).saturating_add(64),
        Value::Array(items) => items.iter().fold(64usize, |total, item| total.saturating_add(value_bytes(item))),
        Value::Object(fields) => fields.iter().fold(64usize, |total, (key, item)| {
            total.saturating_add(key.len()).saturating_add(64).saturating_add(value_bytes(item))
        }),
        _ => 32,
    }
}
