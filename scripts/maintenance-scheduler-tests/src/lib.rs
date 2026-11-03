//! Isolated tests of the production scheduler module. Persistence/pool adapters are stubs;
//! the companion runtime test verifies the real HTTP API and SQLite persistence.
pub mod server {
    pub mod errors {
        #[derive(Debug)]
        pub struct GatewayError {
            pub status: i32,
            pub message: String,
        }
        impl GatewayError {
            pub fn with_status(status: i32, message: impl Into<String>) -> Self {
                Self {
                    status,
                    message: message.into(),
                }
            }
        }
    }
    pub mod config {
        use serde_json::{Map, Value};
        pub fn with_raw<R>(f: impl FnOnce(&Map<String, Value>) -> R) -> Option<R> {
            Some(f(&Map::new()))
        }
        pub fn update_raw_field(_: &str, _: Value) -> bool {
            true
        }
    }
    pub mod core {
        pub mod proxy_pool {
            use serde_json::Value;
            pub struct Proxy {
                pub host: String,
                pub protocol: String,
                pub port: Option<u16>,
                pub username: String,
                pub password: String,
            }
            pub fn list() -> Vec<Value> {
                vec![]
            }
            pub fn resolve_reference(_: &str) -> Result<Proxy, String> {
                Err("fixture pool empty".into())
            }
        }
        pub use crate::production as maintenance;
    }
}
#[path = "../../../desktop-tauri/src-tauri/server/src/server/core/maintenance.rs"]
pub mod production;
pub use server::core::proxy_pool;
