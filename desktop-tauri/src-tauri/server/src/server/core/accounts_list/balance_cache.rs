use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::balance;
use crate::server::core::usage_records::UsageRecord;

struct CachedBalance {
    members: Vec<Arc<Value>>,
    records: Arc<HashMap<String, UsageRecord>>,
    filtered: bool,
    offset: i32,
    checked_at: Instant,
    value: Value,
}

fn cache() -> &'static Mutex<HashMap<String, CachedBalance>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedBalance>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn summary(provider: &str, members: &[Arc<Value>], records: &Arc<HashMap<String, UsageRecord>>, filtered: bool, now: i64, offset: i32) -> Value {
    if let Ok(cache) = cache().lock() {
        if let Some(cached) = cache.get(provider) {
            if cached.members.len() == members.len() && cached.members.iter().zip(members).all(|(left, right)| Arc::ptr_eq(left, right))
                && Arc::ptr_eq(&cached.records, records)
                && cached.filtered == filtered && cached.offset == offset
                && cached.checked_at.elapsed() < Duration::from_secs(1) {
                return cached.value.clone();
            }
        }
    }
    let value = balance::summary(members.iter().map(Arc::as_ref), records, filtered, now, offset);
    if let Ok(mut cache) = cache().lock() {
        cache.retain(|_, cached| cached.checked_at.elapsed() < Duration::from_secs(1));
        if cache.len() < 256 {
            cache.insert(provider.to_string(), CachedBalance { members: members.to_vec(), records: records.clone(),
                filtered, offset, checked_at: Instant::now(), value: value.clone() });
        }
    }
    value
}
