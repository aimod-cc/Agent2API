//! 原Call try门 -> 原账号/Db终检 -> 单次阶段消费；门内不联网/await/spawn。
//! RowFinalCheck及任何异常载荷在全部外层门已释放之后拆开，不复活失效来源。

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::Ordering;

use super::{DailyError, Owner, ORIGIN, PROVIDER};
use super::super::{CheckinCallRef, State};
use crate::server::core::account_store::credential_version::{RowFinalCheck, RowProvider};

pub(super) async fn checkpoint(owner: &Owner, transition: Option<(u8, u8)>) -> Result<(), DailyError> {
    super::super::super::doubao_local_gate::wait(owner.deadline, || DailyError::SourceUnavailable, || {
        checkpoint_once(owner, transition)
    }).await
}

fn checkpoint_once(owner: &Owner, transition: Option<(u8, u8)>) -> Result<RowFinalCheck<Result<(), DailyError>>, DailyError> {
    owner.clock()?;
    let attempt = catch_unwind(AssertUnwindSafe(|| {
        let call = &owner.call;
        let state = call.source.signal.state.try_lock().map_err(|_| DailyError::SourceUnavailable)?;
        active(call, &state)?;
        let host = call.source.host.upgrade().ok_or(DailyError::SourceUnavailable)?;
        let _lifecycle = host.lifecycle.try_lock().map_err(|_| DailyError::SourceUnavailable)?;
        let current = host.embedded.runtime.try_read().map_err(|_| DailyError::SourceUnavailable)?;
        let runtime = current.as_ref().ok_or(DailyError::SourceChanged)?;
        if !host.ready() || host.closing.load(Ordering::Acquire) || host.activity.retired()
            || super::super::super::super::runtime::admission_closed()
            || host.generation.load(Ordering::Acquire) != call.source.generation
            || !call.matches_runtime(runtime) { return Err(DailyError::SourceChanged); }
        if !runtime.shared.options.allows("http.v1") || !runtime.shared.options.allows("http.timeout.v1")
            || !runtime.shared.options.allows("checkin.rolling.v1")
            || runtime.shared.options.check_url(ORIGIN, false).is_err() { return Err(DailyError::Unauthorized); }
        if !owner.egress.matches(call) || owner.egress.account().provider() != RowProvider::DoubaoWork
            || owner.egress.account().account() != call.source.account { return Err(DailyError::SourceChanged); }
        let checked = owner.egress.account().try_final_check(|| {
            // Call可在Db try锁前失效，消费点再次核对原轻量信号，不据此前的成功缓存许可。
            active(call, &state)?;
            if !host.ready() || host.closing.load(Ordering::Acquire)
                || super::super::super::super::runtime::admission_closed()
                || host.generation.load(Ordering::Acquire) != call.source.generation {
                return Err(DailyError::SourceChanged);
            }
            owner.clock()?;
            owner.egress.client().client().map_err(|_| DailyError::SourceUnavailable)?;
            if let Some((expected, next)) = transition { owner.advance(expected, next)?; }
            Ok(())
        });
        // 移到外层Result，绝不在本门内into_state/格式化异常；无同步锁跨await。
        Ok(checked)
    }));
    match attempt {
        Ok(result) => result,
        Err(payload) => { drop(payload); Err(DailyError::SourceUnavailable) },
    }
}

fn active(call: &CheckinCallRef, state: &State) -> Result<(), DailyError> {
    if call.source.provider != PROVIDER || !state.bound || !state.entered || !state.context_entered
        || !state.daily_egress_issued || state.execution.is_some() || state.reason.is_some()
        || call.source.signal.ended.is_cancelled() || call.source.runtime_cancel.is_cancelled() {
        return Err(DailyError::CallInactive);
    }
    if tokio::time::Instant::now() >= call.source.deadline { return Err(DailyError::Deadline); }
    Ok(())
}
