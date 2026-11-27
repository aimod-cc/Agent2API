//! 进程内存中的隐私映射保险库：只驻留内存，绝不写库或日志。
//!
//! 支持两种模式：
//! - 稳定模式（默认）：`{{KIND_xxxxxxxx}}`，同值映射同一占位符，保护提示词缓存与多轮还原；
//! - 单次随机隐藏类型模式：`{{A2A_<32hex>}}`，仅存活于单次请求内存，跨请求隔离且不暴露类型。

use std::collections::HashMap;
use std::fmt::Write as _;
use std::ops::Deref;
use std::sync::{Arc, OnceLock, RwLock};

use hmac::{Hmac, Mac};
use regex::Regex;
use sha2::Sha256;

const GLOBAL_MAX_ENTRIES: usize = 8192;
const GLOBAL_MAX_TOTAL_BYTES: usize = 8 * 1024 * 1024;
const EPHEMERAL_MAX_ENTRIES: usize = 4096;
const EPHEMERAL_MAX_TOTAL_BYTES: usize = 16 * 1024 * 1024;
const MAX_VALUE_BYTES: usize = 64 * 1024;
pub(super) const MAX_RESTORE_EXPANSION: usize = 16 * 1024 * 1024;

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

/// 单次转发请求持有的保险库句柄：稳定模式零拷贝借用全局实例，随机模式持有请求级独立实例。
#[derive(Clone)]
pub enum RequestVault {
    Global(&'static PrivacyVault),
    Ephemeral(Arc<PrivacyVault>),
}

impl RequestVault {
    pub fn global() -> Self {
        Self::Global(PrivacyVault::global())
    }

    pub fn ephemeral_opaque() -> Self {
        Self::Ephemeral(Arc::new(PrivacyVault::ephemeral_opaque()))
    }

    pub fn for_ephemeral_mode(ephemeral_opaque: bool) -> Self {
        if ephemeral_opaque {
            Self::ephemeral_opaque()
        } else {
            Self::global()
        }
    }

    pub fn is_ephemeral(&self) -> bool {
        matches!(self, Self::Ephemeral(_))
    }
}

impl Deref for RequestVault {
    type Target = PrivacyVault;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Global(v) => v,
            Self::Ephemeral(v) => v,
        }
    }
}

pub struct PrivacyVault {
    key: [u8; 32],
    opaque: bool,
    max_entries: usize,
    max_total_bytes: usize,
    evict_on_full: bool,
    inner: RwLock<VaultInner>,
}

impl PrivacyVault {
    pub fn global() -> &'static Self {
        static GLOBAL: OnceLock<PrivacyVault> = OnceLock::new();
        GLOBAL.get_or_init(|| {
            Self::with_options(false, GLOBAL_MAX_ENTRIES, GLOBAL_MAX_TOTAL_BYTES, true)
        })
    }

    /// 临时确定性保险库（用于“试一试”预览或调试落盘脱敏，不污染全局映射）。
    pub fn ephemeral() -> Self {
        Self::with_options(false, EPHEMERAL_MAX_ENTRIES, EPHEMERAL_MAX_TOTAL_BYTES, true)
    }

    /// 单次请求随机且隐藏类型的保险库（`{{A2A_<32hex>}}`，请求结束自动销毁）。
    pub fn ephemeral_opaque() -> Self {
        Self::with_options(true, EPHEMERAL_MAX_ENTRIES, EPHEMERAL_MAX_TOTAL_BYTES, false)
    }

    fn with_options(
        opaque: bool,
        max_entries: usize,
        max_total_bytes: usize,
        evict_on_full: bool,
    ) -> Self {
        let mut key = [0u8; 32];
        fill_random_bytes(&mut key);
        Self {
            key,
            opaque,
            max_entries,
            max_total_bytes,
            evict_on_full,
            inner: RwLock::new(VaultInner::default()),
        }
    }

    pub fn is_opaque(&self) -> bool {
        self.opaque
    }

    pub fn stats(&self) -> usize {
        self.inner
            .read()
            .map(|guard| guard.by_placeholder.len())
            .unwrap_or_else(|poisoned| poisoned.into_inner().by_placeholder.len())
    }

    /// 铸造占位符：
    /// - 稳定模式：同一 `(kind, original)` 在进程生命周期内映射同一 `{{KIND_xxxxxxxx}}`；
    /// - 随机隐藏类型模式：单次请求内同一 `original` 映射同一随机 `{{A2A_<32hex>}}`。
    pub fn mint(&self, kind: &str, original: &str) -> String {
        let norm_kind = super::rules::normalize_kind(kind);
        if original.len() > MAX_VALUE_BYTES {
            return redacted_fallback(&norm_kind, self.opaque);
        }
        // 不透明模式下按原值去重，避免同值在不同规则下分裂为两个随机 token。
        let orig_key = if self.opaque {
            (String::new(), original.to_string())
        } else {
            (norm_kind.clone(), original.to_string())
        };
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
            let candidate = if self.opaque {
                generate_opaque_placeholder()
            } else {
                self.compute_deterministic_placeholder(&norm_kind, original, attempt)
            };
            if let Some(occupied) = guard.by_placeholder.get(&candidate) {
                if occupied.original == original && (self.opaque || occupied.kind == norm_kind) {
                    return candidate;
                }
                continue;
            }
            let entry_bytes = original.len().saturating_add(candidate.len());
            if !self.evict_on_full
                && (guard.by_placeholder.len() >= self.max_entries
                    || guard.total_bytes.saturating_add(entry_bytes) > self.max_total_bytes)
            {
                return redacted_fallback(&norm_kind, self.opaque);
            }
            while (guard.by_placeholder.len() >= self.max_entries
                || guard.total_bytes.saturating_add(entry_bytes) > self.max_total_bytes)
                && !guard.by_placeholder.is_empty()
            {
                evict_oldest(&mut guard, self.opaque);
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
        redacted_fallback(&norm_kind, self.opaque)
    }

    /// 接收花括号内部文本（如 `API_KEY_k3v9x2mq` 或 `A2A_<32hex>`），忽略大小写与前后空白匹配回原始值。
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

    pub fn restore_text(
        &self,
        text: &str,
        json_string_escape: bool,
    ) -> (String, Vec<(String, usize)>) {
        let mut budget = MAX_RESTORE_EXPANSION;
        self.restore_text_with_budget(text, json_string_escape, &mut budget)
    }

    pub(super) fn restore_text_with_budget(
        &self,
        text: &str,
        json_string_escape: bool,
        budget: &mut usize,
    ) -> (String, Vec<(String, usize)>) {
        if !text.contains("{{") {
            return (text.to_string(), Vec::new());
        }
        let re = placeholder_extract_regex();
        let mut out = String::with_capacity(text.len());
        let mut last = 0usize;
        let mut counts: HashMap<String, usize> = HashMap::new();
        for caps in re.captures_iter(text) {
            let Some(whole) = caps.get(0) else { continue };
            let inner = if let (Some(k), Some(h)) = (caps.get(1), caps.get(2)) {
                format!("{}_{}", k.as_str(), h.as_str())
            } else if let (Some(k), Some(h)) = (caps.get(3), caps.get(4)) {
                format!("{}_{}", k.as_str(), h.as_str())
            } else {
                continue;
            };
            out.push_str(&text[last..whole.start()]);
            if let Some((original, kind)) = self.lookup(&inner) {
                if let Some(next_budget) = budget.checked_sub(original.len()) {
                    *budget = next_budget;
                    if json_string_escape {
                        push_json_escaped_inner(&mut out, &original);
                    } else {
                        out.push_str(&original);
                    }
                    *counts.entry(kind).or_insert(0) += 1;
                } else {
                    out.push_str(whole.as_str());
                }
            } else {
                out.push_str(whole.as_str());
            }
            last = whole.end();
        }
        out.push_str(&text[last..]);
        let stats = sorted_counts(counts);
        (out, stats)
    }

    fn compute_deterministic_placeholder(
        &self,
        norm_kind: &str,
        original: &str,
        attempt: u32,
    ) -> String {
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

fn redacted_fallback(norm_kind: &str, opaque: bool) -> String {
    if opaque {
        "[REDACTED]".to_string()
    } else {
        format!("[{norm_kind}_REDACTED]")
    }
}

fn fill_random_bytes(buf: &mut [u8]) {
    if getrandom::getrandom(buf).is_ok() {
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = u128::from(std::process::id());
    let seed = now ^ (pid << 64) ^ 0x9e37_79b9_7f4a_7c15_u128;
    for (idx, byte) in buf.iter_mut().enumerate() {
        *byte = ((seed >> ((idx % 16) * 8)) & 0xff) as u8 ^ (idx as u8).wrapping_mul(31);
    }
}

fn generate_opaque_placeholder() -> String {
    let mut random = [0u8; 16];
    fill_random_bytes(&mut random);
    let mut token = String::with_capacity(40);
    token.push_str("{{A2A_");
    for byte in random {
        let _ = write!(token, "{byte:02x}");
    }
    token.push_str("}}");
    token
}

fn evict_oldest(guard: &mut VaultInner, opaque: bool) {
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
        let key = if opaque {
            (String::new(), removed.original)
        } else {
            (removed.kind, removed.original)
        };
        guard.by_original.remove(&key);
    }
}

pub(super) fn normalize_placeholder_inner(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let (kind_part, hash_part) = trimmed.rsplit_once('_')?;
    if kind_part.eq_ignore_ascii_case("A2A")
        && hash_part.len() == 32
        && hash_part.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Some(format!("{{{{A2A_{}}}}}", hash_part.to_ascii_lowercase()));
    }
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
        Regex::new(
            r"\{\{[ \t]*(?:([Aa]2[Aa])_([0-9A-Fa-f]{32})|([A-Za-z][A-Za-z0-9_]{0,23})_([A-Za-z2-7]{8}))[ \t]*\}\}",
        )
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
