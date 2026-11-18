//! 本机壳的短效管理会话：只在进程内签发与保存，不提供 HTTP 签发入口。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokio_util::sync::CancellationToken;

use super::{now_ms, sha256_hex, ACCESS_TTL};

struct Entry {
    expires_at: i64,
    revoked: CancellationToken,
}

static TOKENS: OnceLock<Mutex<HashMap<String, Arc<Entry>>>> = OnceLock::new();

fn tokens() -> &'static Mutex<HashMap<String, Arc<Entry>>> {
    TOKENS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn remove_expired(table: &mut HashMap<String, Arc<Entry>>, now: i64) -> Vec<Arc<Entry>> {
    let expired = table.iter().filter(|(_, entry)| entry.expires_at <= now)
        .map(|(digest, _)| digest.clone()).collect::<Vec<_>>();
    expired.into_iter().filter_map(|digest| table.remove(&digest)).collect()
}

fn notify_revoked(entries: Vec<Arc<Entry>>) {
    for entry in entries { entry.revoked.cancel(); }
}

#[derive(Clone)]
pub(crate) struct Source {
    digest: String,
    entry: Arc<Entry>,
}

impl Source {
    pub(crate) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.entry, &other.entry)
    }

    pub(crate) fn is_current(&self) -> bool {
        self.with_current(|| ()).is_some()
    }

    pub(crate) fn revocation(&self) -> CancellationToken {
        self.entry.revoked.clone()
    }

    /// 撤销与本次同步准入共用同一会话锁；闭包不得等待、联网或重新进入会话表。
    pub(crate) fn with_current<R>(&self, action: impl FnOnce() -> R) -> Option<R> {
        let table = tokens().try_lock().ok()?;
        let current = table.get(&self.digest)?;
        if !Arc::ptr_eq(current, &self.entry) || current.expires_at <= now_ms() {
            return None;
        }
        Some(action())
    }
}

pub(crate) fn capture(token: &str) -> Option<Source> {
    if token.is_empty() || token.len() > 512 {
        return None;
    }
    let digest = sha256_hex(token);
    let table = tokens().try_lock().ok()?;
    let entry = table.get(&digest).filter(|entry| entry.expires_at > now_ms())?.clone();
    Some(Source { digest, entry })
}

#[derive(Clone)]
pub struct Session {
    pub access_token: String,
    pub expires_at: i64,
}

pub fn issue() -> Result<Session, String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| "无法生成本机管理会话".to_string())?;
    let access_token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let now = now_ms();
    let expires_at = now.saturating_add(ACCESS_TTL.as_millis() as i64);
    let mut table = tokens().lock().map_err(|_| "本机管理会话状态不可用".to_string())?;
    let removed = remove_expired(&mut table, now);
    let digest = sha256_hex(&access_token);
    if table.contains_key(&digest) {
        drop(table);
        notify_revoked(removed);
        return Err("无法生成唯一的本机管理会话".into());
    }
    table.insert(digest, Arc::new(Entry { expires_at, revoked: CancellationToken::new() }));
    drop(table);
    notify_revoked(removed);
    Ok(Session { access_token, expires_at })
}

pub fn valid(token: &str) -> bool {
    valid_any(std::iter::once(token))
}

pub(super) fn valid_any<'a>(candidates: impl IntoIterator<Item = &'a str>) -> bool {
    let Ok(mut table) = tokens().lock() else { return false };
    let removed = remove_expired(&mut table, now_ms());
    let valid = candidates.into_iter().any(|token| !token.is_empty() && table.contains_key(&sha256_hex(token)));
    drop(table);
    notify_revoked(removed);
    valid
}

pub fn revoke(token: &str) {
    let Ok(mut table) = tokens().lock() else { return };
    let removed = table.remove(&sha256_hex(token)).into_iter().collect();
    drop(table);
    notify_revoked(removed);
}

pub(super) fn revoke_any(candidates: &[String]) {
    let Ok(mut table) = tokens().lock() else { return };
    let removed = candidates.iter().filter_map(|token| table.remove(&sha256_hex(token))).collect();
    drop(table);
    notify_revoked(removed);
}
