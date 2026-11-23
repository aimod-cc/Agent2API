mod memory;
mod monitor;
mod queue;
mod scope;

pub(crate) use memory::{Allocation, MemoryBudget};
pub(crate) use queue::{acquire_detailed, record_slot_rejection, try_acquire, Ticket};
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
    #[serde(skip)]
    pub(super) memory_ceiling: bool,
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
    snapshot_inputs().0
}

pub(super) fn snapshot_inputs() -> (Limits, monitor::Resources) {
    let resources = monitor::sample();
    (effective(&runtime_capacity::current().settings, resources), resources)
}

fn effective(settings: &CapacitySettings, resources: monitor::Resources) -> Limits {
    let auto_memory = (resources.total_bytes / 2) as usize;
    let memory_bytes = if settings.memory_mib == 0 { auto_memory.max(16 * MIB) }
        else { settings.memory_mib.saturating_mul(MIB).min((resources.total_bytes * 3 / 4) as usize).max(16 * MIB) };
    let process_ceiling = resources.total_bytes.saturating_mul(4) / 5;
    let memory_ceiling = resources.container_bytes.unwrap_or(resources.process_bytes) >= process_ceiling;
    // 仅在 macOS 内核压力达到 4 级（critical）或本进程/容器占用 ≥ 总内存 4/5 时判为内存紧张；
    // 不再看「系统可用内存 < 总内存 10%」，避免其它程序吃内存或文件缓存误伤网关。
    let host_memory_pressure = resources.os_pressure_level.is_some_and(|level| level >= 4);
    let memory_pressure = host_memory_pressure || memory_ceiling;
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
        queue_wait_seconds: settings.queue_wait_seconds, cpu_pressure, memory_pressure, memory_ceiling,
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
        "rejected": state.rejected,
        "rejectedReasons": state.reject_counts.as_value() })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RejectReason {
    MemoryPressure,
    MemoryCeiling,
    CpuPressure,
    ConcurrencyLimit,
    ProviderLimit,
    QueueFull,
    QueueMemoryFull,
    PluginSlotLimit,
    QueueTimeout,
}

impl RejectReason {
    const ALL: [Self; 9] = [
        Self::MemoryPressure,
        Self::MemoryCeiling,
        Self::CpuPressure,
        Self::ConcurrencyLimit,
        Self::ProviderLimit,
        Self::QueueFull,
        Self::QueueMemoryFull,
        Self::PluginSlotLimit,
        Self::QueueTimeout,
    ];

    fn index(self) -> usize {
        match self {
            Self::MemoryPressure => 0,
            Self::MemoryCeiling => 1,
            Self::CpuPressure => 2,
            Self::ConcurrencyLimit => 3,
            Self::ProviderLimit => 4,
            Self::QueueFull => 5,
            Self::QueueMemoryFull => 6,
            Self::PluginSlotLimit => 7,
            Self::QueueTimeout => 8,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::MemoryPressure => "memory_pressure",
            Self::MemoryCeiling => "memory_ceiling",
            Self::CpuPressure => "cpu_pressure",
            Self::ConcurrencyLimit => "concurrency_limit",
            Self::ProviderLimit => "provider_limit",
            Self::QueueFull => "queue_full",
            Self::QueueMemoryFull => "queue_memory_full",
            Self::PluginSlotLimit => "plugin_slot_limit",
            Self::QueueTimeout => "queue_timeout",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::MemoryPressure => "系统内存压力过大",
            Self::MemoryCeiling => "进程或容器内存已达上限",
            Self::CpuPressure => "系统CPU负载过高",
            Self::ConcurrencyLimit => "并发数已达上限",
            Self::ProviderLimit => "提供商并发已达上限",
            Self::QueueFull => "排队请求数已达上限",
            Self::QueueMemoryFull => "排队内存预算已达上限",
            Self::PluginSlotLimit => "插件执行槽位已满",
            Self::QueueTimeout => "容量排队等待超时",
        }
    }

    pub fn client_message(self, class: Class) -> &'static str {
        match self {
            Self::MemoryPressure => "系统内存压力过大，暂缓接收后台或图片任务，请稍后重试",
            Self::MemoryCeiling => "网关进程或容器内存占用已接近上限，排队等待已结束，请稍后重试",
            Self::CpuPressure => "系统 CPU 负载过高，暂缓接收后台或图片任务，请稍后重试",
            Self::ConcurrencyLimit => {
                if matches!(class, Class::Request | Class::ImageRequest) {
                    "网关并发请求数已满，排队等待已结束，请稍后重试"
                } else {
                    "插件运行并发数已满，排队等待已结束，请稍后重试"
                }
            }
            Self::ProviderLimit => "当前提供商并发数已达上限，排队等待已结束，请稍后重试",
            Self::QueueFull => "网关排队请求数已达上限，请稍后重试",
            Self::QueueMemoryFull => "网关排队内存预算已达上限，请稍后重试",
            Self::PluginSlotLimit => "当前插件并发执行槽位已满，排队等待已结束，请稍后重试",
            Self::QueueTimeout => "运行容量排队等待超时，请稍后重试",
        }
    }

    pub fn retry_after_seconds(self) -> u64 {
        match self {
            Self::MemoryPressure | Self::MemoryCeiling | Self::CpuPressure | Self::QueueFull | Self::QueueMemoryFull => 2,
            Self::ConcurrencyLimit | Self::ProviderLimit | Self::PluginSlotLimit | Self::QueueTimeout => 1,
        }
    }
}

#[derive(Default)]
pub(super) struct RejectCounts([u64; 9]);

impl RejectCounts {
    pub(super) fn record(&mut self, reason: RejectReason) {
        let slot = &mut self.0[reason.index()];
        *slot = slot.saturating_add(1);
    }

    fn as_value(&self) -> Value {
        let mut map = serde_json::Map::new();
        for reason in RejectReason::ALL {
            map.insert(reason.code().to_string(), json!(self.0[reason.index()]));
        }
        Value::Object(map)
    }
}

/// 单次容量准入失败的详细诊断快照（供日志记录和客户端返回可区分的错误原因）。
#[derive(Clone, Debug)]
pub struct CapacityRejection {
    pub reason: RejectReason,
    pub class: Class,
    pub waited_ms: u64,
    pub active: usize,
    pub limit: usize,
    pub queued: usize,
    pub queue_limit: usize,
    pub queue_bytes: usize,
    pub queue_bytes_limit: usize,
    pub available_bytes: u64,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub os_pressure_level: Option<u8>,
}

impl CapacityRejection {
    pub fn message(&self) -> &'static str { self.reason.client_message(self.class) }

    pub fn retry_after_seconds(&self) -> u64 { self.reason.retry_after_seconds() }

    pub fn into_wire_error(self) -> proto::WireError {
        proto::WireError::with_code(self.message(), "plugin_busy")
    }

    pub fn describe(&self) -> String {
        let pressure = self.os_pressure_level.map_or_else(String::new, |level| format!(" 内核压力={level}"));
        format!(
            "原因={}({}) 等待={}ms 并发={}/{} 排队={}/{} 排队内存={}/{}MiB 内存占用={}/{}MiB 可用={}MiB{}",
            self.reason.label(),
            self.reason.code(),
            self.waited_ms,
            self.active,
            self.limit,
            self.queued,
            self.queue_limit,
            bytes_to_mib(self.queue_bytes as u64),
            bytes_to_mib(self.queue_bytes_limit as u64),
            bytes_to_mib(self.used_bytes),
            bytes_to_mib(self.total_bytes),
            bytes_to_mib(self.available_bytes),
            pressure,
        )
    }
}

#[derive(Clone, Debug)]
pub enum AcquireError {
    Cancelled,
    Rejected(CapacityRejection),
}

impl AcquireError {
    pub fn into_wire_error(self) -> proto::WireError {
        match self {
            Self::Cancelled => proto::WireError::with_code("容量等待已取消", proto::WireError::ABORTED),
            Self::Rejected(rejection) => rejection.into_wire_error(),
        }
    }
}

/// 将宿主机生成的 `plugin_busy` 静态错误文案还原为可信的 `(文案, 原因码, 建议重试秒数)`；
/// 非宿主机白名单文案统一收敛为兜底提示，防止插件自带文本透传。
pub(crate) fn classify_busy_message(message: &str) -> (&'static str, &'static str, u64) {
    for reason in RejectReason::ALL {
        for class in [Class::Conversation, Class::Request] {
            let expected = reason.client_message(class);
            if message == expected {
                return (expected, reason.code(), reason.retry_after_seconds());
            }
        }
    }
    if message == "当前账号还有其他操作，等待账号锁超时，请稍后重试" {
        return ("当前账号还有其他操作，等待账号锁超时，请稍后重试", "account_lock", 1);
    }
    ("插件正在处理其他操作，等待超时，请稍后重试", "plugin_busy", retry_after_seconds())
}

fn bytes_to_mib(bytes: u64) -> u64 { bytes / (MIB as u64) }

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
