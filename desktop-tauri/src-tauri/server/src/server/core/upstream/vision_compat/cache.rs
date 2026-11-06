use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

struct Entry { text: String, created: Instant }
static CACHE: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
const TTL: Duration = Duration::from_secs(900);
const MAX_ENTRIES: usize = 128;
const MAX_BYTES: usize = 4 * 1024 * 1024;

pub(super) fn key(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts { digest.update((part.len() as u64).to_le_bytes()); digest.update(part.as_bytes()); }
    format!("{:x}", digest.finalize())
}

pub(super) fn get(key: &str) -> Option<String> {
    let mut entries = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    entries.retain(|_, entry| entry.created.elapsed() < TTL);
    entries.get(key).map(|entry| entry.text.clone())
}

pub(super) fn put(key: String, text: String) {
    if text.is_empty() || text.len() > 65_536 { return; }
    let mut entries = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    entries.retain(|_, entry| entry.created.elapsed() < TTL);
    while entries.len() >= MAX_ENTRIES || entries.values().map(|entry| entry.text.len()).sum::<usize>() + text.len() > MAX_BYTES {
        let Some(oldest) = entries.iter().min_by_key(|(_, entry)| entry.created).map(|(key, _)| key.clone()) else { break; };
        entries.remove(&oldest);
    }
    entries.insert(key, Entry { text, created: Instant::now() });
}
