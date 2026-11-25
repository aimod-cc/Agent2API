use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use hmac::{Hmac, Mac};
use regex::Regex;
use sha2::Sha256;

const MAX_ENTRIES: usize = 8192;
const MAX_TOTAL_BYTES: usize = 8 * 1024 * 1024;
const MAX_VALUE_BYTES: usize = 16 * 1024;
const BASE32_LOWER: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

type HmacSha256 = Hmac<Sha256>;

struct VaultEntry {
    original: String,
    kind: String,
    last_used: u64,
}

#[derive(Default)]
struct VaultInner {
    by_placeholder: HashMap<String, VaultEntry>,
    by_original: HashMap<(String, String), String>,
    total_bytes: usize,
    tick: u64,
}

/// 进程内存中的隐私映射保险库：只驻留内存，绝不写库或日志。
pub struct PrivacyVault {
    key: [u8; 32],
    inner: RwLock<VaultInner>,
}

impl PrivacyVault {
    pub fn global() -> &'static Self {
        static GLOBAL: OnceLock<PrivacyVault> = OnceLock::new();
        GLOBAL.get_or_init(Self::new)
    }

    pub fn ephemeral() -> Self {
        Self::new()
    }

    fn new() -> Self {
        let mut key = [0u8; 32];
        if getrandom::getrandom(&mut key).is_err() {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let pid = u128::from(std::process::id());
            let seed = now ^ (pid << 64) ^ 0x9e37_79b9_7f4a_7c15_u128;
            for (idx, byte) in key.iter_mut().enumerate() {
                *byte = ((seed >> ((idx % 16) * 8)) & 0xff) as u8 ^ (idx as u8).wrapping_mul(31);
            }
        }
        Self {
            key,
            inner: RwLock::new(VaultInner::default()),
        }
    }

    pub fn stats(&self) -> usize {
        self.inner
            .read()
            .map(|guard| guard.by_placeholder.len())
            .unwrap_or_else(|poisoned| poisoned.into_inner().by_placeholder.len())
    }

    /// 铸造确定性占位符：同一个 (kind, original) 在进程生命周期内返回同一占位符。
    pub fn mint(&self, kind: &str, original: &str) -> String {
        let norm_kind = super::rules::normalize_kind(kind);
        if original.len() > MAX_VALUE_BYTES {
            return format!("[{norm_kind}_REDACTED]");
        }
        let orig_key = (norm_kind.clone(), original.to_string());
        {
            let mut guard = self
                .inner
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.tick = guard.tick.saturating_add(1);
            let tick = guard.tick;
            if let Some(existing) = guard.by_original.get(&orig_key).cloned() {
                if let Some(entry) = guard.by_placeholder.get_mut(&existing) {
                    entry.last_used = tick;
                }
                return existing;
            }
            for attempt in 0..8u32 {
                let candidate = self.compute_placeholder(&norm_kind, original, attempt);
                if let Some(occupied) = guard.by_placeholder.get(&candidate) {
                    if occupied.original == original && occupied.kind == norm_kind {
                        return candidate;
                    }
                    continue;
                }
                let entry_bytes = original.len().saturating_add(candidate.len());
                while (guard.by_placeholder.len() >= MAX_ENTRIES
                    || guard.total_bytes.saturating_add(entry_bytes) > MAX_TOTAL_BYTES)
                    && !guard.by_placeholder.is_empty()
                {
                    evict_oldest(&mut guard);
                }
                guard.by_original.insert(orig_key, candidate.clone());
                guard.by_placeholder.insert(
                    candidate.clone(),
                    VaultEntry {
                        original: original.to_string(),
                        kind: norm_kind,
                        last_used: tick,
                    },
                );
                guard.total_bytes = guard.total_bytes.saturating_add(entry_bytes);
                return candidate;
            }
        }
        format!("[{norm_kind}_REDACTED]")
    }

    /// 接收花括号内部文本（如 `API_KEY_k3v9x2mq`），忽略大小写与前后空白匹配回原始值。
    pub fn lookup(&self, inner: &str) -> Option<(String, String)> {
        let normalized = normalize_placeholder_inner(inner)?;
        let mut guard = self
            .inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.tick = guard.tick.saturating_add(1);
        let tick = guard.tick;
        let entry = guard.by_placeholder.get_mut(&normalized)?;
        entry.last_used = tick;
        Some((entry.original.clone(), entry.kind.clone()))
    }

    pub fn restore_text(&self, text: &str, json_string_escape: bool) -> (String, Vec<(String, usize)>) {
        if !text.contains("{{") {
            return (text.to_string(), Vec::new());
        }
        let re = placeholder_extract_regex();
        let mut out = String::with_capacity(text.len());
        let mut last = 0usize;
        let mut counts: HashMap<String, usize> = HashMap::new();
        for caps in re.captures_iter(text) {
            let Some(whole) = caps.get(0) else { continue };
            let Some(kind_m) = caps.get(1) else { continue };
            let Some(hash_m) = caps.get(2) else { continue };
            let inner = format!("{}_{}", kind_m.as_str(), hash_m.as_str());
            out.push_str(&text[last..whole.start()]);
            if let Some((original, kind)) = self.lookup(&inner) {
                if json_string_escape {
                    push_json_escaped_inner(&mut out, &original);
                } else {
                    out.push_str(&original);
                }
                *counts.entry(kind).or_insert(0) += 1;
            } else {
                out.push_str(whole.as_str());
            }
            last = whole.end();
        }
        out.push_str(&text[last..]);
        let stats = sorted_counts(counts);
        (out, stats)
    }

    fn compute_placeholder(&self, norm_kind: &str, original: &str, attempt: u32) -> String {
        let Ok(mut mac) = HmacSha256::new_from_slice(&self.key) else {
            return format!("{{{{{norm_kind}_00000000}}}}");
        };
        mac.update(norm_kind.as_bytes());
        mac.update(&[0u8]);
        mac.update(original.as_bytes());
        if attempt > 0 {
            mac.update(&[0u8]);
            mac.update(&attempt.to_le_bytes());
        }
        let digest = mac.finalize().into_bytes();
        let mut tag = [0u8; 8];
        let val = ((u64::from(digest[0])) << 32)
            | ((u64::from(digest[1])) << 24)
            | ((u64::from(digest[2])) << 16)
            | ((u64::from(digest[3])) << 8)
            | u64::from(digest[4]);
        for (idx, slot) in tag.iter_mut().enumerate() {
            let shift = (7 - idx) * 5;
            *slot = BASE32_LOWER[((val >> shift) & 0x1f) as usize];
        }
        let suffix = std::str::from_utf8(&tag).unwrap_or("00000000");
        format!("{{{{{norm_kind}_{suffix}}}}}")
    }
}

fn evict_oldest(guard: &mut VaultInner) {
    let Some(oldest_key) = guard
        .by_placeholder
        .iter()
        .min_by_key(|(_, entry)| entry.last_used)
        .map(|(k, _)| k.clone())
    else {
        return;
    };
    if let Some(removed) = guard.by_placeholder.remove(&oldest_key) {
        let removed_bytes = removed.original.len().saturating_add(oldest_key.len());
        guard.total_bytes = guard.total_bytes.saturating_sub(removed_bytes);
        guard.by_original.remove(&(removed.kind, removed.original));
    }
}

pub(super) fn normalize_placeholder_inner(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let (kind_part, hash_part) = trimmed.rsplit_once('_')?;
    if kind_part.is_empty() || kind_part.len() > 24 || hash_part.len() != 8 {
        return None;
    }
    let mut chars = kind_part.bytes();
    if !chars.next().is_some_and(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    if !chars.all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return None;
    }
    if !hash_part
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_uppercase() || (b'2'..=b'7').contains(&b))
    {
        return None;
    }
    Some(format!(
        "{{{{{}_{}}}}}",
        kind_part.to_ascii_uppercase(),
        hash_part.to_ascii_lowercase()
    ))
}

pub(super) fn placeholder_extract_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\{\{[ \t]*([A-Za-z][A-Za-z0-9_]{0,23})_([A-Za-z2-7]{8})[ \t]*\}\}")
            .expect("占位符提取正则必须有效")
    })
}

pub(super) fn push_json_escaped_inner(out: &mut String, raw: &str) {
    if let Ok(encoded) = serde_json::to_string(raw) {
        if encoded.len() >= 2 {
            out.push_str(&encoded[1..encoded.len() - 1]);
            return;
        }
    }
    out.push_str(raw);
}

pub(super) fn sorted_counts(map: HashMap<String, usize>) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = map.into_iter().collect();
    out.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    out
}
