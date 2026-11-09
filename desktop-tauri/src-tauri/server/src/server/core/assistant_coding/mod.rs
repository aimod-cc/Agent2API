use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod client;
pub mod config;
pub mod executable;
mod policy;
pub mod process;
mod process_guard;
pub mod projection;
mod history;
mod metadata;
pub mod preferences;
pub mod projects;
pub mod project_directories;
pub mod fork;
pub mod usage;
pub mod sessions;
pub mod state;
pub mod storage;
pub mod permissions;
mod transport;
mod terminal_gateway;
mod terminal_policy;
mod terminal_process;
pub mod terminal;
mod private_directory;

#[derive(Debug, Clone)]
pub struct CodingError {
    pub status: u16,
    pub message: String,
    pub code: &'static str,
}

impl CodingError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self { status, message: message.into(), code: "coding_error" }
    }

    pub fn uncertain(message: impl Into<String>) -> Self {
        Self { status: 502, message: message.into(), code: "coding_delivery_uncertain" }
    }
}

impl From<String> for CodingError {
    fn from(message: String) -> Self { Self::new(500, message) }
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Engine { Codex }

pub fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

pub fn now() -> i64 { crate::server::logging::now_ms() }
