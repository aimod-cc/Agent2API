//! 固定豆包领奖记忆只沿原行来源读取和单次保存；持久Pending不签发下一次发送资格。

use std::{any::Any, panic::{catch_unwind, AssertUnwindSafe}, sync::Arc};

use serde_json::Value;
use zeroize::Zeroizing;

use super::{cached_key, RowProvider, RowSource};
use crate::server::core::account_store::sql::credential_version::doubao_state::{rewards as sql, StateError, StateFailure};
use crate::server::db::CodeArtsTransaction;

pub(in crate::server::core) struct RewardSnapshot { original: Arc<RowSource>, memory: Value }
pub(in crate::server::core) struct RewardPending { snapshot: RewardSnapshot, operation: String, allow_submission: bool }
pub(in crate::server::core) struct RewardCheck<'a> {
    owner: &'a RewardPending,
    key: Zeroizing<[u8; 32]>,
    replacement: Option<Value>,
}
#[must_use]
pub(in crate::server::core) struct RewardFinal {
    confirmed: bool,
    _retained: [sql::Retained; 2],
    _deferred: [Option<Box<dyn Any + Send>>; 2],
}
impl RewardFinal { pub(in crate::server::core) fn confirmed(self) -> bool { self.confirmed } }

impl RowSource {
    pub(in crate::server::core) fn read_doubao_rewards(self) -> Result<RewardSnapshot, StateError> {
        if self.provider() != RowProvider::DoubaoWork { return Err(StateError::State(StateFailure::WrongProvider)); }
        let key = cached_key().map_err(StateError::Account)?;
        let mut retained = sql::Retained::new();
        let mut memory = None;
        let mut deferred = None;
        let mut action_unwind = None;
        let mut commit = None;
        let attempt = catch_unwind(AssertUnwindSafe(|| self.store.bound_credential_transaction(&self.source, &mut commit, &mut |conn| {
            match catch_unwind(AssertUnwindSafe(|| sql::read(conn, &key, &self.inner, &mut retained))) {
                Ok(Ok(value)) => { memory = Some(value); true },
                Ok(Err(_)) => false,
                Err(payload) => { action_unwind = Some(payload); false },
            }
        })));
        let confirmed = match attempt {
            Ok(Ok(CodeArtsTransaction::Committed { storage_closed: true, .. })) => commit == Some(true),
            Ok(_) => false,
            Err(payload) => { deferred = Some(payload); false },
        };
        drop(retained); drop([action_unwind, deferred]);
        if !confirmed { return Err(StateError::State(StateFailure::Unavailable)); }
        Ok(RewardSnapshot { original: Arc::new(self), memory: memory.ok_or(StateError::State(StateFailure::Unavailable))? })
    }
}

impl RewardSnapshot {
    pub(in crate::server::core) fn account(&self) -> &RowSource { &self.original }
    pub(in crate::server::core) fn has(&self, group: &str, item: &str, task: &str) -> bool {
        self.memory.pointer("/value/entries").and_then(Value::as_array).is_some_and(|entries|
            entries.iter().any(|entry| entry["group"] == group && entry["item"] == item && entry["task"] == task))
    }
    pub(in crate::server::core) fn has_pending(&self, group: &str, item: &str, task: &str) -> bool {
        self.memory.pointer("/value/entries").and_then(Value::as_array).is_some_and(|entries|
            entries.iter().any(|entry| entry["group"] == group && entry["item"] == item
                && entry["task"] == task && entry["state"] == "pending"))
    }

    pub(in crate::server::core) fn matching(&self, id: &str) -> bool {
        self.memory.pointer("/value/entries").and_then(Value::as_array).is_some_and(|entries| entries.iter().any(|entry| {
            use sha2::{Digest, Sha256};
            let material = serde_json::json!([entry["group"], entry["item"], entry["task"]]).to_string();
            id == format!("doubao-task-{:x}", Sha256::digest(material.as_bytes()))
        }))
    }
    pub(in crate::server::core) fn pending_count(&self) -> usize {
        self.memory.pointer("/value/entries").and_then(Value::as_array).map_or(0, |entries|
            entries.iter().filter(|entry| entry["state"] == "pending").count())
    }
    pub(in crate::server::core) fn resume_pending(self, group: &str, item: &str, task: &str) -> Result<RewardPending, StateError> {
        let operation = self.memory.pointer("/value/entries").and_then(Value::as_array)
            .and_then(|entries| entries.iter().find(|entry| entry["group"] == group && entry["item"] == item
                && entry["task"] == task && entry["state"] == "pending"))
            .and_then(|entry| entry["operation"].as_str()).ok_or(StateError::State(StateFailure::CandidateChanged))?.to_string();
        Ok(RewardPending { snapshot: self, operation, allow_submission: false })
    }

    pub(in crate::server::core) fn begin(self, group: &str, item: &str, task: &str) -> Result<RewardPending, StateError> {
        let key = cached_key().map_err(StateError::Account)?;
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes).map_err(|_| StateError::State(StateFailure::Unavailable))?;
        if bytes.iter().all(|byte| *byte == 0) { return Err(StateError::State(StateFailure::Unavailable)); }
        let operation = bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        let next = sql::pending(&self.memory, group, item, task, &operation, crate::server::logging::now_ms())?;
        let mut retained = sql::Retained::new();
        let mut commit = None;
        let mut action_unwind = None;
        let outcome = self.original.store.bound_credential_transaction(&self.original.source, &mut commit, &mut |conn| {
            match catch_unwind(AssertUnwindSafe(|| sql::write(conn, &key, &self.original.inner, &self.memory, &next, &mut retained))) {
                Ok(Ok(())) => true, Ok(Err(_)) => false, Err(payload) => { action_unwind = Some(payload); false },
            }
        });
        let confirmed = matches!(outcome, Ok(CodeArtsTransaction::Committed { storage_closed: true, .. })) && commit == Some(true);
        drop(retained); drop(action_unwind);
        if !confirmed { return Err(StateError::State(StateFailure::Unavailable)); }
        Ok(RewardPending { snapshot: RewardSnapshot { original: self.original, memory: next }, operation, allow_submission: true })
    }
}

impl RewardPending {
    pub(in crate::server::core) fn account(&self) -> &RowSource { self.snapshot.account() }
    pub(in crate::server::core) fn prepare(&self, completion: bool) -> Result<RewardCheck<'_>, StateError> {
        if !completion && !self.allow_submission { return Err(StateError::State(StateFailure::CandidateChanged)); }
        let replacement = if completion { Some(sql::confirmed(&self.snapshot.memory, &self.operation, crate::server::logging::now_ms())?) } else { None };
        Ok(RewardCheck { owner: self, key: cached_key().map_err(StateError::Account)?, replacement })
    }
}
impl RewardCheck<'_> {
    /// 原Call门内只核原账号与整份Pending，并消费本次内存状态；大原行与异常带出门后释放。
    pub(in crate::server::core) fn run(&self, consume: impl FnOnce() -> bool) -> RewardFinal {
        let mut retained = [sql::Retained::new(), sql::Retained::new()];
        let mut commit = None;
        let mut deferred = None;
        let mut action_unwind = None;
        let mut consumed = false;
        let mut consume = Some(consume);
        let original = &self.owner.snapshot.original;
        let expected = &self.owner.snapshot.memory;
        let attempt = catch_unwind(AssertUnwindSafe(|| original.store.bound_credential_transaction(&original.source, &mut commit, &mut |conn| {
            match catch_unwind(AssertUnwindSafe(|| {
                sql::verify(conn, &self.key, &original.inner, expected, &mut retained[0])?;
                if !consume.take().is_some_and(|consume| consume()) { return Err(StateError::State(StateFailure::Unavailable)); }
                consumed = true;
                if let Some(next) = &self.replacement {
                    sql::write(conn, &self.key, &original.inner, expected, next, &mut retained[1])?;
                }
                Ok::<(), StateError>(())
            })) {
                Ok(Ok(())) => true, Ok(Err(_)) => false, Err(payload) => { action_unwind = Some(payload); false },
            }
        })));
        let confirmed = match attempt {
            Ok(Ok(CodeArtsTransaction::Committed { storage_closed: true, .. })) => consumed && commit == Some(true),
            Ok(_) => false, Err(payload) => { deferred = Some(payload); false },
        };
        RewardFinal { confirmed, _retained: retained, _deferred: [action_unwind, deferred] }
    }
}
