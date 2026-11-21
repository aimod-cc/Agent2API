use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rusqlite::Connection;
use serde_json::Value;

/// 下拉筛选项缓存：避免每次打开「请求日志」都对百万级大表做 4 次全表 GROUP BY。
///
/// 命中规则：
/// - 只要数据库未清理/压缩、外部进程未改库，且新写入的请求未出现缓存外的新值，就沿用内存快照；
/// - 流量活跃且仅复用已知值时，最长 5 分钟做一次后台自然刷新，以更新「按出现次数排序」的前后顺位；
/// - 一旦出现未见过的新模型 / 提供商 / 客户端 / API Key，或发生清理 / 裁剪 / 重算，立即标脏并在下一次请求重算。
const MEMORY_TRUST_WINDOW: Duration = Duration::from_secs(15);
const ACTIVE_REFRESH_INTERVAL: Duration = Duration::from_secs(300);

struct CachedFilters {
    external: i64,
    database: u64,
    requests: u64,
    computed_at: Instant,
    models: HashSet<String>,
    providers: HashSet<String>,
    clients: HashSet<String>,
    api_keys: HashSet<String>,
    full: bool,
    value: Value,
}

#[derive(Default)]
pub(super) struct FilterOptionsCache {
    dirty: AtomicBool,
    cached: Mutex<Option<CachedFilters>>,
}

impl FilterOptionsCache {
    pub fn new() -> Self {
        Self {
            dirty: AtomicBool::new(true),
            cached: Mutex::new(None),
        }
    }

    pub fn invalidate(&self) {
        self.dirty.store(true, Ordering::Release);
    }

    /// 记账写入时轻量核对是否引入了下拉清单尚未知晓的新枚举值。
    pub fn note_written(
        &self,
        model: &str,
        upstream_model: &str,
        provider: &str,
        client: &str,
        api_key_filter: &str,
    ) {
        if self.dirty.load(Ordering::Acquire) {
            return;
        }
        let Ok(guard) = self.cached.try_lock() else {
            return;
        };
        let Some(cached) = guard.as_ref() else {
            self.dirty.store(true, Ordering::Release);
            return;
        };
        if cached.full {
            // 清单已达上限时仍可能受频率排序影响，交由时间兜底更新，不因长尾项反复标脏。
            return;
        }
        let unseen = (!model.is_empty() && !cached.models.contains(model))
            || (!upstream_model.is_empty() && !cached.models.contains(upstream_model))
            || (!provider.is_empty() && !cached.providers.contains(provider))
            || (!client.is_empty() && !cached.clients.contains(client))
            || (!api_key_filter.is_empty() && !cached.api_keys.contains(api_key_filter));
        if unseen {
            self.dirty.store(true, Ordering::Release);
        }
    }

    /// 短窗内无需进库锁，直接读取内存快照。
    pub fn read_fast(&self, current_database: u64, current_requests: u64) -> Option<Value> {
        if self.dirty.load(Ordering::Acquire) {
            return None;
        }
        let guard = self.cached.lock().unwrap_or_else(|error| error.into_inner());
        let cached = guard.as_ref()?;
        if cached.database != current_database {
            return None;
        }
        if cached.requests != current_requests && cached.computed_at.elapsed() >= ACTIVE_REFRESH_INTERVAL {
            return None;
        }
        if cached.computed_at.elapsed() < MEMORY_TRUST_WINDOW {
            return Some(cached.value.clone());
        }
        None
    }

    /// 持库锁后按外部版本号与脏标记核对是否复用缓存；未命中才执行给定查询闭包。
    pub fn read_or_compute(
        &self,
        conn: &Connection,
        current_database: u64,
        current_requests: u64,
        max_options: usize,
        compute: impl FnOnce(&Connection) -> rusqlite::Result<Value>,
    ) -> rusqlite::Result<Value> {
        let external: i64 = conn.pragma_query_value(None, "data_version", |row| row.get(0))?;
        let dirty = self.dirty.load(Ordering::Acquire);
        if !dirty {
            let guard = self.cached.lock().unwrap_or_else(|error| error.into_inner());
            if let Some(cached) = guard.as_ref() {
                let version_ok = cached.external == external && cached.database == current_database;
                let fresh_enough = cached.requests == current_requests
                    || cached.computed_at.elapsed() < ACTIVE_REFRESH_INTERVAL;
                if version_ok && fresh_enough {
                    return Ok(cached.value.clone());
                }
            }
        }
        let value = compute(conn)?;
        let snapshot = build_cached(external, current_database, current_requests, max_options, value.clone());
        {
            let mut guard = self.cached.lock().unwrap_or_else(|error| error.into_inner());
            *guard = Some(snapshot);
            self.dirty.store(false, Ordering::Release);
        }
        Ok(value)
    }
}

fn build_cached(
    external: i64,
    database: u64,
    requests: u64,
    max_options: usize,
    value: Value,
) -> CachedFilters {
    let models = value["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<HashSet<_>>();
    let providers = value["providers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<HashSet<_>>();
    let clients = value["clients"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("value").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<HashSet<_>>();
    let api_keys = value["apiKeys"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("value").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<HashSet<_>>();
    let full = models.len() >= max_options
        || providers.len() >= max_options
        || clients.len() >= max_options
        || api_keys.len() >= max_options;
    CachedFilters {
        external,
        database,
        requests,
        computed_at: Instant::now(),
        models,
        providers,
        clients,
        api_keys,
        full,
        value,
    }
}
