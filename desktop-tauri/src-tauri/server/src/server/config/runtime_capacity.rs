use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::server::errors::GatewayError;

pub const KEY_RUNTIME_CAPACITY: &str = "runtimeCapacity";
pub const MAX_EXECUTIONS: usize = 4096;
pub const POOL_EXECUTIONS: usize = MAX_EXECUTIONS + 256 + 256 + 128;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapacityMode {
    #[default]
    Auto,
    Manual,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CapacitySettings {
    pub mode: CapacityMode,
    pub max_conversations: usize,
    pub max_interactive: usize,
    pub max_background: usize,
    pub background_per_provider: usize,
    pub max_images: usize,
    pub max_connections: usize,
    pub memory_mib: usize,
    pub queue_requests: usize,
    pub queue_memory_mib: usize,
    pub queue_wait_seconds: u64,
    pub provider_limits: BTreeMap<String, usize>,
}

impl Default for CapacitySettings {
    fn default() -> Self {
        Self {
            mode: CapacityMode::Auto, max_conversations: 1024, max_interactive: 64,
            max_background: 32, background_per_provider: 8, max_images: 16,
            max_connections: 4096, memory_mib: 0, queue_requests: 2048,
            queue_memory_mib: 256, queue_wait_seconds: 10, provider_limits: BTreeMap::new(),
        }
    }
}

impl CapacitySettings {
    pub fn validate(&self) -> Result<(), String> {
        for (label, value, min, max) in [
            ("对话并发", self.max_conversations, 1, MAX_EXECUTIONS),
            ("交互并发", self.max_interactive, 4, 256),
            ("后台并发", self.max_background, 1, 256),
            ("单提供商后台并发", self.background_per_provider, 1, 64),
            ("图片并发", self.max_images, 1, 128),
            ("网络连接", self.max_connections, 64, 32768),
            ("排队请求", self.queue_requests, 1, 16384),
            ("排队内存", self.queue_memory_mib, 8, 4096),
        ] {
            if !(min..=max).contains(&value) {
                return Err(format!("{label}必须在 {min}～{max} 之间"));
            }
        }
        if self.memory_mib != 0 && !(128..=262144).contains(&self.memory_mib) {
            return Err("运行内存必须为自动，或在 128～262144 MiB 之间".into());
        }
        if !(1..=120).contains(&self.queue_wait_seconds) {
            return Err("排队等待必须在 1～120 秒之间".into());
        }
        if self.provider_limits.len() > 256 || self.provider_limits.iter().any(|(id, limit)| {
            id.is_empty() || id.len() > 128 || id.chars().any(char::is_control)
                || *limit == 0 || *limit > MAX_EXECUTIONS
        }) {
            return Err("提供商并发设置无效".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct CapacitySnapshot {
    pub settings: CapacitySettings,
    pub revision: String,
    pub error: Option<String>,
}

impl Default for CapacitySnapshot {
    fn default() -> Self { from_value(None) }
}

fn revision(value: Option<&Value>) -> String {
    format!("{:x}", Sha256::digest(value.cloned().unwrap_or(Value::Null).to_string().as_bytes()))
}

fn from_value(value: Option<&Value>) -> CapacitySnapshot {
    let result = value.map_or_else(|| Ok(CapacitySettings::default()), |value| {
        serde_json::from_value::<CapacitySettings>(value.clone()).map_err(|error| error.to_string())
            .and_then(|settings| { settings.validate()?; Ok(settings) })
    });
    match result {
        Ok(settings) => CapacitySnapshot { settings, revision: revision(value), error: None },
        Err(error) => CapacitySnapshot { settings: CapacitySettings::default(), revision: revision(value), error: Some(error) },
    }
}

pub(crate) fn from_raw(raw: &Map<String, Value>) -> Arc<CapacitySnapshot> {
    Arc::new(from_value(raw.get(KEY_RUNTIME_CAPACITY)))
}

pub fn current() -> Arc<CapacitySnapshot> {
    super::CONFIG.read().unwrap_or_else(|error| error.into_inner()).as_ref()
        .map(|config| config.runtime_capacity.clone()).unwrap_or_default()
}

pub fn save(settings: CapacitySettings, expected: &str) -> Result<(), GatewayError> {
    settings.validate().map_err(GatewayError::bad_request)?;
    let value = serde_json::to_value(settings).map_err(|error| GatewayError::new(error.to_string()))?;
    super::try_update_raw_field(KEY_RUNTIME_CAPACITY, |raw| {
        if revision(Some(raw)) != expected {
            return Err(GatewayError::with_status(409, "运行容量已在其他位置修改，请重新读取后再保存"));
        }
        *raw = value;
        Ok(())
    }, || GatewayError::new("运行容量保存失败，未应用本次修改"))?;
    crate::server::core::runtime_capacity::changed();
    Ok(())
}
