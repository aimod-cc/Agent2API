//! 已预留Preparing记录的借用门；普通使用只读，唯一转换封闭在finalizing子模块。
//! 只按CallGate -> Table -> ScopeGate -> Record取得；持门时只能作固定同步核对。

mod finalizing;
pub(in crate::server::core::plugin_host::wasm::managed_task) use finalizing::{
    finalize, CreatePermit, FinalConsume, FinalizeFailure,
};

use std::sync::{atomic::Ordering, Arc, MutexGuard, Weak};

use super::{lifecycle, table, CallGate, NotSentReason, Phase, Record, Table};

/// 构造器封闭，绑定原调用借用期；字段顺序保证Record -> ScopeGate -> Table逆序解锁。
#[must_use]
pub(in crate::server::core::plugin_host::wasm) struct PreparingGuard<'a> {
    record: &'a Arc<Record>,
    state: MutexGuard<'a, lifecycle::State>,
    _scope: MutexGuard<'a, ()>,
    _table: MutexGuard<'static, Table>,
}

impl PreparingGuard<'_> {
    pub(in crate::server::core::plugin_host::wasm) fn record(&self) -> &Arc<Record> {
        self.record
    }

    /// 不再获取Record锁，不使用record.snapshot；结束意图与原期限仍须在取槽前后检查。
    pub(in crate::server::core::plugin_host::wasm) fn matches(&self, gate: &CallGate<'_>) -> bool {
        gate.owns_record(self.record)
            && gate.runtime() == &self.record.scope().binding
            && self.record.binding().matches_call(self.record.task_call())
            && self.record.binding().in_runtime(gate.runtime())
            && self.record.scope().accepting.load(Ordering::Acquire)
            && !self.record.scope().runtime_cancel.is_cancelled()
            && self.state.reserved
            && self.state.runner_present
            && self.state.snapshot.phase() == Phase::Preparing
            && self.state.snapshot.frozen()
            && !self.state.snapshot.submitted()
            && !self.record.submitted()
            && self.state.create_deadline.is_none()
            && self.record.preparing_error().is_none()
    }
}

/// 仅借用已由reserve登记的原记录；Table/Scope各try一次，Record沿用既有短锁。
/// 不要求abort已经移交，但必须存在同对象running预留；这不签发启动worker或Creating资格。
pub(in crate::server::core::plugin_host::wasm::managed_task) fn try_preparing<'a>(
    record: &'a Arc<Record>,
    gate: &'a CallGate<'_>,
) -> Result<PreparingGuard<'a>, NotSentReason> {
    if !gate.owns_record(record) || !record.binding().matches_call(record.task_call()) {
        return Err(NotSentReason::InvalidBinding);
    }
    let table = table()
        .try_lock()
        .map_err(|_| NotSentReason::RuntimeUnavailable)?;
    if !table
        .calls
        .iter()
        .any(|item| Weak::ptr_eq(item, &Arc::downgrade(record)))
        || !table
            .scopes
            .iter()
            .any(|item| Weak::ptr_eq(item, &Arc::downgrade(record.scope())))
        || table
            .running
            .iter()
            .filter(|item| Arc::ptr_eq(&item.record, record))
            .count()
            != 1
    {
        return Err(NotSentReason::InvalidBinding);
    }
    let scope = record
        .scope()
        .gate
        .try_lock()
        .map_err(|_| NotSentReason::RuntimeUnavailable)?;
    let state = record.lock();
    let guard = PreparingGuard {
        record,
        state,
        _scope: scope,
        _table: table,
    };
    if !guard.matches(gate) {
        return Err(record
            .preparing_error()
            .unwrap_or(NotSentReason::InvalidBinding));
    }
    Ok(guard)
}
