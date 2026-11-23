//! 进程本地会话粘滞。管理读取不续期，清除后在途请求不得把旧绑定写回来。

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Clone)]
struct StickyEntry {
    id: String,
    account_id: String,
    model: String,
    source: String,
    created_at: i64,
    last_seen: i64,
    ttl_ms: i64,
    hits: u64,
}

#[derive(Default)]
struct StickyTable {
    entries: HashMap<String, StickyEntry>,
    generation: u64,
}

#[derive(Default)]
pub(super) struct StickySessions {
    table: Mutex<StickyTable>,
}

pub(super) fn entry_id(key: &str, model: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(model.trim().to_ascii_lowercase().as_bytes());
    hasher.update(b"\0");
    hasher.update(key.as_bytes());
    format!("{:x}", hasher.finalize())
}

impl StickySessions {
    pub(super) fn generation(&self) -> u64 {
        self.table.lock().map(|table| table.generation).unwrap_or(0)
    }

    pub(super) fn lookup(&self, key: &str, model: &str, now: i64, ttl_ms: i64) -> Option<String> {
        if key.is_empty() || ttl_ms <= 0 { return None; }
        let id = entry_id(key, model);
        let mut table = self.table.lock().ok()?;
        table.entries.retain(|_, entry| now.saturating_sub(entry.last_seen) <= entry.ttl_ms);
        let entry = table.entries.get_mut(&id)?;
        if now.saturating_sub(entry.last_seen) > ttl_ms {
            table.entries.remove(&id);
            return None;
        }
        entry.last_seen = now;
        entry.ttl_ms = ttl_ms;
        entry.hits = entry.hits.saturating_add(1);
        Some(entry.account_id.clone())
    }

    pub(super) fn bind(
        &self, key: &str, model: &str, account_id: &str, source: &str,
        now: i64, ttl_ms: i64, generation: u64,
    ) {
        if key.is_empty() || account_id.is_empty() || ttl_ms <= 0 { return; }
        let Ok(mut table) = self.table.lock() else { return };
        if table.generation != generation { return; }
        table.entries.retain(|_, entry| now.saturating_sub(entry.last_seen) <= entry.ttl_ms);
        let id = entry_id(key, model);
        if table.entries.len() >= 10_000 && !table.entries.contains_key(&id) {
            if let Some(oldest) = table.entries.iter().min_by_key(|(_, entry)| entry.last_seen)
                .map(|(id, _)| id.clone())
            {
                table.entries.remove(&oldest);
            }
        }
        let entry = table.entries.entry(id.clone()).or_insert_with(|| StickyEntry {
            id, account_id: account_id.to_string(), model: model.to_string(),
            source: source.to_string(), created_at: now, last_seen: now, ttl_ms, hits: 0,
        });
        entry.account_id = account_id.to_string();
        entry.last_seen = now;
        entry.ttl_ms = ttl_ms;
    }

    pub(super) fn list(&self, model: Option<&str>, now: i64, ttl_for: impl Fn(&str) -> i64) -> Vec<Value> {
        let Ok(mut table) = self.table.lock() else { return Vec::new() };
        table.entries.retain(|_, entry| {
            let ttl = ttl_for(&entry.model).min(entry.ttl_ms);
            ttl > 0 && now.saturating_sub(entry.last_seen) <= ttl
        });
        let mut entries: Vec<&StickyEntry> = table.entries.values()
            .filter(|entry| model.map_or(true, |model| entry.model.eq_ignore_ascii_case(model)))
            .collect();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.last_seen));
        entries.into_iter().map(|entry| json!({
            "id": entry.id,
            "keyPreview": format!("{}…", &entry.id[..12]),
            "accountId": crate::server::core::routing_groups::account_key(&entry.account_id),
            "candidateId": entry.account_id,
            "memberId": crate::server::core::routing_groups::member_key(&entry.account_id),
            "model": entry.model,
            "source": entry.source,
            "createdAt": entry.created_at,
            "lastSeen": entry.last_seen,
            "expiresAt": entry.last_seen + ttl_for(&entry.model).min(entry.ttl_ms),
            "remainingMs": (entry.last_seen + ttl_for(&entry.model).min(entry.ttl_ms) - now).max(0),
            "hits": entry.hits,
        })).collect()
    }

    /// 某个候选失败后解除该会话的绑定（仅当绑定的正是这个候选且世代未变）。
    pub(super) fn forget(&self, key: &str, model: &str, candidate: &str, generation: u64) {
        if key.is_empty() || candidate.is_empty() { return; }
        let Ok(mut table) = self.table.lock() else { return };
        if table.generation != generation { return; }
        let id = entry_id(key, model);
        if table.entries.get(&id).is_some_and(|entry| entry.account_id == candidate) {
            table.entries.remove(&id);
        }
    }

    pub(super) fn clear(&self, id: Option<&str>, model: Option<&str>) -> usize {
        let Ok(mut table) = self.table.lock() else { return 0 };
        let before = table.entries.len();
        table.entries.retain(|key, entry| {
            !(id.map_or(true, |id| id == key)
                && model.map_or(true, |model| entry.model.eq_ignore_ascii_case(model)))
        });
        table.generation = table.generation.wrapping_add(1);
        before - table.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forget_removes_only_matching_candidate_in_current_generation() {
        let sticky = StickySessions::default();
        let gen = sticky.generation();
        sticky.bind("k1", "m1", "a1", "test", 1000, 60_000, gen);
        // 候选不匹配时不解绑（避免覆盖新成功绑定的候选）
        sticky.forget("k1", "m1", "a2", gen);
        assert_eq!(sticky.lookup("k1", "m1", 1001, 60_000).as_deref(), Some("a1"));
        // 世代过期时不解绑
        sticky.forget("k1", "m1", "a1", gen.wrapping_add(1));
        assert_eq!(sticky.lookup("k1", "m1", 1002, 60_000).as_deref(), Some("a1"));
        // 匹配候选且同世代时立即解绑
        sticky.forget("k1", "m1", "a1", gen);
        assert!(sticky.lookup("k1", "m1", 1003, 60_000).is_none());
    }
}
