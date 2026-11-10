use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::types::{self, Entry};

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct Cache {
    pub version: u32,
    pub market: Vec<Entry>,
    pub github: Vec<Entry>,
    pub market_at: i64,
    pub github_at: i64,
    pub market_retry: i64,
    pub github_retry: i64,
    pub stars_retry: i64,
    pub market_error: Option<String>,
    pub github_error: Option<String>,
    pub github_missing: usize,
    pub ignored: BTreeMap<String, (String, i64)>,
    /// 最近一次刷新里读取失败的仓库（网络 / HTTP 类，会自动重试）
    pub github_failures: Vec<CatalogFailure>,
    /// 清单不是合法 JSON 的仓库：与忽略生命周期同待遇，留下原因供错误详情展示
    pub github_unreadable: Vec<UnreadableManifest>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub(super) struct CatalogFailure {
    pub repository: String,
    pub message: String,
    pub retry_at: i64,
}

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct UnreadableManifest {
    pub repository: String,
    pub detail: String,
    pub at: i64,
}

#[derive(Default)]
struct State {
    root: PathBuf,
    cache: Cache,
    entries: Arc<Vec<Entry>>,
    refreshing: bool,
    completed: usize,
    total: usize,
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default())).lock().unwrap_or_else(|error| error.into_inner())
}

fn load(state: &mut State) {
    let root = super::super::root();
    if state.root == root { return; }
    let mut cache = std::fs::metadata(root.join("catalog.json")).ok().filter(|meta| meta.len() <= 16 * 1024 * 1024)
        .and_then(|_| std::fs::read(root.join("catalog.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<Cache>(&bytes).ok())
        .filter(|cache| cache.version == 1).unwrap_or_default();
    if cache.github.iter().any(|entry| !entry.is_provider()) {
        cache.github.retain(Entry::is_provider);
        cache.github_at = 0;
    }
    let entries = Arc::new(types::merge(&cache.market, &cache.github));
    *state = State { root, cache, entries, ..State::default() };
}

pub(super) fn entries() -> Arc<Vec<Entry>> {
    let mut state = lock();
    load(&mut state);
    state.entries.clone()
}

pub(super) fn snapshot() -> Cache {
    let mut state = lock();
    load(&mut state);
    state.cache.clone()
}

pub(super) fn update(change: impl FnOnce(&mut Cache)) {
    let mut state = lock();
    load(&mut state);
    change(&mut state.cache);
    state.cache.version = 1;
    state.entries = Arc::new(types::merge(&state.cache.market, &state.cache.github));
}

pub(super) fn persist() {
    let mut state = lock();
    load(&mut state);
    let Ok(bytes) = serde_json::to_vec(&state.cache) else { return; };
    if bytes.len() > 16 * 1024 * 1024 { return; }
    if let Err(error) = super::super::storage::atomic_write(&state.root.join("catalog.json"), &bytes) {
        crate::server::logging::verbose("[PluginCatalog]", &format!("插件清单缓存保存失败：{error}"));
    }
}

pub(super) fn progress(refreshing: bool, completed: usize, total: usize) {
    let mut state = lock();
    load(&mut state);
    state.refreshing = refreshing;
    state.completed = completed;
    state.total = total;
}

pub(super) fn status() -> Value {
    let mut state = lock();
    load(&mut state);
    json!({
        "refreshing": state.refreshing, "completed": state.completed, "total": state.total,
        "marketCheckedAt": state.cache.market_at, "githubCheckedAt": state.cache.github_at,
        "marketError": state.cache.market_error, "githubError": state.cache.github_error,
        "marketRetryAt": state.cache.market_retry, "githubRetryAt": state.cache.github_retry,
        "githubMissing": state.cache.github_missing, "topic": super::github::TOPIC,
        "githubFailures": state.cache.github_failures, "githubUnreadable": state.cache.github_unreadable,
    })
}
