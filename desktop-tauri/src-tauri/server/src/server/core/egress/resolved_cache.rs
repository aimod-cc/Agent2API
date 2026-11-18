use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::*;

struct Entry { client: reqwest::Client, used: Instant }

pub(super) fn client(proxy: Option<&ResolvedProxy>, system_proxy: bool, host: &str, addresses: &[SocketAddr],
    websocket: bool) -> Result<reqwest::Client, String> {
    static CACHE: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    let timeouts = crate::server::config::timeout_settings();
    let transport = proxy.map(crate::server::core::proxies::chain::endpoint).transpose()?;
    let proxy = transport.as_ref();
    let key = format!("{}|system={system_proxy}|ws={websocket}|dns={host}:{addresses:?}", cache_key(proxy, &timeouts));
    let cache = CACHE.get_or_init(Mutex::default);
    {
        let mut entries = cache.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(entry) = entries.get_mut(&key) {
            entry.used = Instant::now();
            return Ok(entry.client.clone());
        }
    }
    let builder = resolved_builder(proxy, system_proxy, host, addresses)?;
    let client = if websocket { builder.http1_only() } else { builder }.build()
        .map_err(|_| "无法创建出站连接".to_string())?;
    let max = (crate::server::core::runtime_capacity::limits().memory_bytes / (256 * 1024)).clamp(32, 4096);
    let mut entries = cache.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(entry) = entries.get_mut(&key) {
        entry.used = Instant::now();
        return Ok(entry.client.clone());
    }
    entries.retain(|_, entry| entry.used.elapsed() < Duration::from_secs(300));
    while entries.len() >= max {
        let Some(oldest) = entries.iter().min_by_key(|(_, entry)| entry.used).map(|(key, _)| key.clone()) else { break; };
        entries.remove(&oldest);
    }
    entries.insert(key, Entry { client: client.clone(), used: Instant::now() });
    Ok(client)
}
