use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::server::core::update::website as official;

const PRIMARY_URL: &str = official::CDN_MODELS_CATALOG_URL;
const FALLBACK_URL: &str = official::API_MODELS_CATALOG_URL;
const TTL: Duration = Duration::from_secs(3600);
const RETRY: Duration = Duration::from_secs(60);
const LIMIT: usize = 32 * 1024 * 1024;

#[derive(Default)]
struct RefreshState { running: bool, retry_at: Option<Instant> }

fn state() -> &'static Mutex<RefreshState> {
    static STATE: OnceLock<Mutex<RefreshState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(RefreshState::default()))
}

pub(super) fn prepare() -> PathBuf {
    refresh(false);
    super::root().join("provider-catalog.json")
}

/// 当脚本插件宿主已启动或本地已有公共模型目录缓存时，随 `models.dev` 元数据刷新一并同步。
pub(crate) fn refresh_if_active(force: bool) {
    let active = super::runtime::host().is_some_and(|host| host.ready())
        || super::root().join("provider-catalog.json").exists();
    if active {
        refresh(force);
    }
}

pub(in crate::server::core::plugin_host) fn refresh(force: bool) {
    if !crate::server::config::models_dev_enabled() {
        return;
    }
    let path = super::root().join("provider-catalog.json");
    let fresh = std::fs::metadata(&path).ok().and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.elapsed().ok()).is_some_and(|age| age < TTL);
    if fresh && !force { return; }
    {
        let mut state = state().lock().unwrap_or_else(|error| error.into_inner());
        if state.running || (!force && state.retry_at.is_some_and(|at| Instant::now() < at)) { return; }
        state.running = true;
    }
    crate::spawn_task(async move {
        let result = tokio::time::timeout(Duration::from_secs(60), download(&path)).await
            .map_err(|_| "公共模型目录更新超时，保留本地缓存".to_string()).and_then(|result| result);
        {
            let mut state = state().lock().unwrap_or_else(|error| error.into_inner());
            state.running = false;
            state.retry_at = Some(Instant::now() + RETRY);
        }
        match result {
            Ok(()) => sync(),
            Err(error) => crate::server::logging::log_with_level("[Plugin]",
                &format!("公共模型目录后台更新未完成：{}", super::super::process::redact(&error)), "warn"),
        }
    });
}

async fn download(path: &Path) -> Result<(), String> {
    match download_from(PRIMARY_URL, path).await {
        Ok(()) => Ok(()),
        Err(_) => download_from(FALLBACK_URL, path).await,
    }
}

async fn download_from(url: &str, path: &Path) -> Result<(), String> {
    let headers = vec![
        ("Accept".to_string(), "application/json".to_string()),
        ("User-Agent".to_string(), "Agent2API-script-plugins".to_string()),
    ];
    let response = official::request(url, &headers, Some(60_000))
        .await
        .map_err(|error| error.message)?;
    if !response.status().is_success() { return Err(format!("官网模型目录返回 HTTP {}", response.status())); }
    if response.content_length().is_some_and(|size| size > LIMIT as u64) {
        return Err("模型目录超过允许大小".into());
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("读取模型目录失败：{error}"))?;
        if bytes.len().saturating_add(chunk.len()) > LIMIT { return Err("模型目录超过允许大小".into()); }
        bytes.extend_from_slice(&chunk);
    }
    let valid = serde_json::from_slice::<Value>(&bytes).ok().is_some_and(|value| value.as_object()
        .is_some_and(|providers| providers.values().any(|provider| provider["models"].is_object())));
    if !valid { return Err("模型目录格式无效，保留本地缓存".into()); }
    super::storage::atomic_write(path, &bytes)
}

#[derive(Deserialize)]
struct CatalogDefaults { id: String, models: Vec<Value> }

pub(super) fn sync() {
    let Some(host) = super::runtime::host().filter(|host| host.ready()) else { return };
    let Some(lease) = host.activity.acquire() else { return };
    crate::spawn_task(async move {
        let _lease = lease;
        let result = host.call_coded("script.catalog", json!({}), Duration::from_secs(10)).await;
        let Some(current) = super::runtime::host() else { return };
        if !Arc::ptr_eq(&host, &current) || !host.ready() { return; }
        let defaults = result.map_err(|error| error.message)
            .and_then(|value| serde_json::from_value::<Vec<CatalogDefaults>>(value).map_err(|error| error.to_string()));
        match defaults {
            Ok(defaults) => {
                for entry in defaults {
                    if super::super::runtime::host_for_provider(&entry.id)
                        .is_some_and(|owner| Arc::ptr_eq(&host, &owner)) {
                        super::super::rpc_cache::set_defaults(&entry.id, entry.models);
                    }
                }
            }
            Err(error) => crate::server::logging::verbose("[Plugin]",
                &format!("公共模型目录缓存同步未完成：{}", super::super::process::redact(&error))),
        }
    });
}
