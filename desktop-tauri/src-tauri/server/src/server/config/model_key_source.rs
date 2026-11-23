//! 模型 Key 的进程内原配置权威；不授予匿名、环境 Key 或管理会话配对资格。
//!
//! 写侧：既有 usage 锁（需要时）→ CONFIG_UPDATE_LOCK → 短 CONFIG 克隆（释放）
//! → 本门撤销（释放）→ 存储/CONFIG 发布（释放）→ 本门启用（释放）。
//! 发送终检：本门 → Signal → Host → Runtime → 原账号/Db 的短同步 try 终检。
//! 本门内不得解析/解密、记录日志、执行 IO、重入配置或反向获取写侧更新锁。

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::RuntimeConfig;
use crate::server::core::{api_keys, key_scope::KeyScope};
use crate::server::request_stats::ApiKeyIdentity;

struct Record {
    revision: u64,
    revoked: AtomicBool,
    notification: CancellationToken,
}

struct Authority {
    generation: u64,
    revision: u64,
    revoked: AtomicBool,
    notification: CancellationToken,
    records: Vec<Arc<Record>>,
}

struct Published {
    authority: Arc<Authority>,
    // 只有权威持定义；Source/Record 均不保留明文或密文 Key。
    definition: Definition,
}

struct State {
    generation: u64,
    revision: u64,
    current: Option<Arc<Published>>,
    replacement: bool,
    failed: bool,
}

static AUTHORITY: Mutex<State> = Mutex::new(State {
    generation: 0,
    revision: 0,
    current: None,
    replacement: false,
    failed: false,
});

struct Definition {
    keys: Option<Value>,
    legacy: Option<Value>,
    listed: bool,
    slots: usize,
}

impl Definition {
    fn from_raw(raw: &Map<String, Value>) -> Self {
        let keys = raw.get(api_keys::KEY_API_KEYS).cloned();
        let legacy = raw.get(api_keys::KEY_API_KEY).cloned();
        let listed = matches!(keys.as_ref(), Some(Value::Array(_)));
        let slots = match keys.as_ref() {
            Some(Value::Array(items)) => items.len(),
            _ => usize::from(legacy.is_some()),
        };
        Self { keys, legacy, listed, slots }
    }

    fn same(&self, other: &Self) -> bool {
        self.keys == other.keys && self.legacy == other.legacy
    }

    // 内部槽只定位刚捕获的原定义，不向调用者提供 index factory。
    fn slot_raw(&self, index: usize) -> Option<Map<String, Value>> {
        if index >= self.slots {
            return None;
        }
        let mut raw = Map::new();
        if self.listed {
            let item = self.keys.as_ref()?.as_array()?.get(index)?.clone();
            raw.insert(api_keys::KEY_API_KEYS.to_string(), Value::Array(vec![item]));
        } else {
            raw.insert(api_keys::KEY_API_KEY.to_string(), self.legacy.as_ref()?.clone());
        }
        Some(raw)
    }
}

/// 字段封闭且无 Clone/Debug/Default/serde；外部不能从 Key、ID、日志或快照构造。
/// HTTP 若需可克隆扩展，只能持有原对象的 Arc<Source>。
pub(crate) struct Source {
    authority: Arc<Authority>,
    record: Arc<Record>,
    generation: u64,
    revision: u64,
    identity: ApiKeyIdentity,
}

impl Source {
    /// 纯对象关系比较，不取门、不判存活；current 必须另由 with_current 核对。
    pub(crate) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.authority, &other.authority)
            && Arc::ptr_eq(&self.record, &other.record)
            && self.generation == other.generation
            && self.revision == other.revision
    }

    pub(crate) fn is_current(&self) -> bool {
        self.with_current(|| ()).is_some()
    }

    /// 保持原权威门覆盖 action；只允许短同步核对/CAS，不能等待或执行 IO。
    /// 禁止从 action 重入本模块/config；下层须依约使用 try 终检。
    pub(crate) fn with_current<R>(&self, action: impl FnOnce() -> R) -> Option<R> {
        // 使用阻塞加锁避免高并发瞬时冲突误判不可用；锁异常时按 fail-closed 返回 None。
        let state = AUTHORITY.lock().ok()?;
        if !matches_current(&state, &self.authority, &self.record, self.generation, self.revision) {
            return None;
        }
        Some(action())
    }

    pub(crate) fn revocation(&self) -> CancellationToken {
        self.record.notification.clone()
    }

    /// 仅显示事实；不含 token，不能用于授权或重新构造 Source。
    pub(crate) fn log_identity(&self) -> ApiKeyIdentity {
        self.identity.clone()
    }
}

fn matches_current(
    state: &State,
    authority: &Arc<Authority>,
    record: &Arc<Record>,
    generation: u64,
    revision: u64,
) -> bool {
    if state.failed || state.replacement || authority.revoked.load(Ordering::Acquire)
        || record.revoked.load(Ordering::Acquire)
        || authority.generation != generation || authority.revision != revision
        || record.revision != revision
    {
        return false;
    }
    state.current.as_ref().is_some_and(|current| {
        Arc::ptr_eq(&current.authority, authority)
            && current.authority.records.iter().any(|original| Arc::ptr_eq(original, record))
    })
}

/// 唯一签发入口，仅供真实 model_access 鉴权成功链接入，不供插件/JSON 使用。
/// 短门捕获原权威 → 门外既有解析/解密 → 同门终检原对象；未命中/换代返回 None。
/// 不接环境变量分支，也不接受 ApiKeyEntry、日志、用户名、ID、hash 或外部 index。
pub(crate) fn authenticate_key(presented: &str) -> Option<(KeyScope, Source)> {
    if presented.is_empty() {
        return None;
    }
    let current = {
        // 门内仅克隆 Arc 与核对原子标记，不克隆配置 JSON，避免高并发 try_lock 竞态误报 401。
        let state = AUTHORITY.lock().ok()?;
        if state.failed || state.replacement {
            return None;
        }
        let current = state.current.as_ref()?.clone();
        if current.authority.revoked.load(Ordering::Acquire) {
            return None;
        }
        current
    };
    // entries_from 可能解密/记录日志，必须在所有权威/CONFIG 锁外执行。
    for (index, record) in current.authority.records.iter().enumerate() {
        if current.authority.revoked.load(Ordering::Acquire) {
            return None;
        }
        let Some(raw) = current.definition.slot_raw(index) else {
            continue;
        };
        let Some(entry) = api_keys::entries_from(&raw).into_iter()
            .find(|entry| entry.is_usable() && entry.key == presented)
        else { continue; };
        let scope = KeyScope::from_entry(&entry);
        let identity = ApiKeyIdentity { id: entry.id, name: entry.name, source: "key".to_string() };
        let state = AUTHORITY.lock().ok()?;
        if !matches_current(&state, &current.authority, record, current.authority.generation, record.revision) {
            return None;
        }
        return Some((scope, Source {
            generation: current.authority.generation,
            revision: record.revision,
            authority: current.authority.clone(),
            record: record.clone(),
            identity,
        }));
    }
    None
}

fn revoke_locked(state: &mut State) -> Option<Arc<Published>> {
    let original = state.current.take()?;
    original.authority.revoked.store(true, Ordering::Release);
    for record in &original.authority.records {
        record.revoked.store(true, Ordering::Release);
    }
    Some(original)
}

fn notify(original: Option<Arc<Published>>) {
    if let Some(original) = original {
        original.authority.notification.cancel();
        for record in &original.authority.records {
            record.notification.cancel();
        }
    }
}

fn revoke(replacement: bool, failed: bool) -> bool {
    let (original, usable) = match AUTHORITY.lock() {
        Ok(mut state) => {
            state.replacement |= replacement;
            state.failed |= failed;
            (revoke_locked(&mut state), !state.failed)
        }
        Err(error) => {
            let mut state = error.into_inner();
            state.replacement |= replacement;
            state.failed = true;
            (revoke_locked(&mut state), false)
        }
    };
    // cancel 会唤醒其他工作，只能在短门释放后发送。
    notify(original);
    usable
}

/// 固定恢复前入口；必须在真实替换前、尚未持有 Db/账号/Host 锁时调用。
/// 调用后普通更新不能重开；失败或调用方中断也不还原任何旧来源。
/// 只有后续真实 init/reload 成功发布才可建立全新代；同值也不复活旧代。
pub(crate) fn pre_replace() -> bool {
    revoke(true, false)
}

pub(super) fn before_reload() {
    let _ = pre_replace();
}

pub(super) fn fail_closed() {
    let _ = revoke(false, true);
}

/// previous/next 仅来自 CONFIG_UPDATE_LOCK 下的真实写侧快照，不是外部签发材料。
/// 定义包含 enabled/权限/Key 与未知记录字段；任何 Key 定义变化均先撤原代。
/// 无关配置改动不碰原权威，不从相等内容重建已撤销的记录。
pub(super) fn before_change(previous: &RuntimeConfig, next: &RuntimeConfig) {
    if previous.raw().get(api_keys::KEY_API_KEYS) != next.raw().get(api_keys::KEY_API_KEYS)
        || previous.raw().get(api_keys::KEY_API_KEY) != next.raw().get(api_keys::KEY_API_KEY)
    {
        let _ = revoke(false, false);
    }
}

pub(super) fn is_failed() -> bool {
    AUTHORITY
        .lock()
        .is_ok_and(|state| state.failed && !state.replacement)
}

#[derive(Clone, Copy)]
enum PublishMode {
    Normal,
    Resync,
    Reload,
}

/// 仅真实 CONFIG 发布成功后调用，且仍持写侧更新锁；本门内不读 CONFIG。
/// 正常发布保留原代；reload 即使内容完全相同仍永久替换旧代，并在门自身未中毒时从外层 failed 自愈。
pub(super) fn published(next: &RuntimeConfig, reloaded: bool) {
    let mode = if reloaded {
        PublishMode::Reload
    } else {
        PublishMode::Normal
    };
    publish_inner(next, mode);
}

/// 从数据库重同步配置后的自愈入口；不清除外部尚未收尾的 replacement 替换期封锁。
pub(super) fn republish_from_db(next: &RuntimeConfig, clear_replacement: bool) {
    let mode = if clear_replacement {
        PublishMode::Reload
    } else {
        PublishMode::Resync
    };
    publish_inner(next, mode);
}

fn publish_inner(next: &RuntimeConfig, mode: PublishMode) {
    let definition = Definition::from_raw(next.raw());
    let original = match AUTHORITY.lock() {
        Ok(mut state) => {
            match mode {
                PublishMode::Normal => {
                    if state.failed || state.replacement {
                        return;
                    }
                    if state.current.as_ref().is_some_and(|current| current.definition.same(&definition)) {
                        return;
                    }
                }
                PublishMode::Resync => {
                    if state.replacement {
                        return;
                    }
                    if !state.failed
                        && state.current.as_ref().is_some_and(|current| current.definition.same(&definition))
                    {
                        return;
                    }
                }
                PublishMode::Reload => {}
            }
            let original = revoke_locked(&mut state);
            let Some(generation) = state.generation.checked_add(1) else {
                state.failed = true;
                drop(state);
                notify(original);
                return;
            };
            let Some(revision) = state.revision.checked_add(1) else {
                state.failed = true;
                drop(state);
                notify(original);
                return;
            };
            state.generation = generation;
            state.revision = revision;
            state.failed = false;
            if matches!(mode, PublishMode::Reload) {
                state.replacement = false;
            }
            let records = (0..definition.slots).map(|_| Arc::new(Record {
                revision, revoked: AtomicBool::new(false), notification: CancellationToken::new(),
            })).collect();
            state.current = Some(Arc::new(Published {
                authority: Arc::new(Authority {
                    generation, revision, revoked: AtomicBool::new(false),
                    notification: CancellationToken::new(), records,
                }),
                definition,
            }));
            original
        }
        Err(error) => {
            let mut state = error.into_inner();
            state.failed = true;
            revoke_locked(&mut state)
        }
    };
    notify(original);
}
