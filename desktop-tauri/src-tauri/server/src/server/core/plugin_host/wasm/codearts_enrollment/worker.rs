//! 确认只装配一个原保存工作；取消等待不abort同步事务，原Host与句柄持到实际收尾。

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{atomic::Ordering, Arc};

use tokio::{runtime::Handle, sync::oneshot};

use super::{slots, EnrollmentError, Materials, Operation, Phase};
use crate::server::core::account_store::credential_version::EnrollmentCommitOutcome;
use crate::server::core::plugin_host::{activity::UsageLease, Host};

pub(super) struct JobState {
    handle: Option<tokio::task::JoinHandle<()>>,
    waiting: bool,
    settled: bool,
}

impl Default for JobState {
    fn default() -> Self {
        Self {
            handle: None,
            waiting: false,
            settled: false,
        }
    }
}

struct Active {
    host: Arc<Host>,
    _usage: Arc<UsageLease>,
}

impl Drop for Active {
    fn drop(&mut self) {
        self.host.embedded.active.fetch_sub(1, Ordering::AcqRel);
        self.host.embedded.idle.notify_waiters();
    }
}

struct Guard {
    operation: Arc<Operation>,
    done: bool,
}

impl Guard {
    fn finish(mut self) {
        self.operation.completed.store(true, Ordering::Release);
        self.operation.job_changed.notify_waiters();
        self.operation.cleanup_done.cancel();
        self.done = true;
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.done {
            self.operation
                .stop(EnrollmentError::SaveUnknown, Phase::Failed);
            self.operation.completed.store(true, Ordering::Release);
            self.operation.job_changed.notify_waiters();
        }
    }
}

pub(super) fn start(operation: Arc<Operation>) -> Result<(), EnrollmentError> {
    let runtime = Handle::try_current().map_err(|_| EnrollmentError::Unavailable)?;
    let host = operation.binding.host()?;
    let usage = host
        .activity
        .acquire()
        .ok_or(EnrollmentError::Unavailable)?;
    let first = operation.admit(|_, state| {
        if state.confirmed {
            return Ok(false);
        }
        if state.phase != Phase::AwaitingConfirmation || state.materials.is_none() {
            return Err(EnrollmentError::Unavailable);
        }
        state.confirmed = true;
        Ok(true)
    })?;
    if !first {
        return Ok(());
    }
    let permit = match slots().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            operation.stop(EnrollmentError::Busy, Phase::Failed);
            operation.completed.store(true, Ordering::Release);
            operation.job_changed.notify_waiters();
            return Err(EnrollmentError::Busy);
        }
    };
    operation
        .admit(|original, state| {
            if !std::ptr::eq(original, host.as_ref()) || state.entered || !state.confirmed {
                return Err(EnrollmentError::Unavailable);
            }
            original.embedded.active.fetch_add(1, Ordering::AcqRel);
            Ok(())
        })
        .inspect_err(|error| {
            operation.stop(*error, Phase::Failed);
            operation.completed.store(true, Ordering::Release);
            operation.job_changed.notify_waiters();
        })?;
    let active = Active {
        host,
        _usage: usage,
    };
    let guard = Guard {
        operation: operation.clone(),
        done: false,
    };
    let (begin, ready) = oneshot::channel();
    let materials = operation.admit(|_, state| {
        if !state.confirmed || state.entered {
            return Err(EnrollmentError::Unavailable);
        }
        Ok(state.materials.take().ok_or(EnrollmentError::Unavailable)?)
    })?;
    let mut saved_job = operation
        .job
        .lock()
        .map_err(|_| EnrollmentError::SaveUnknown)?;
    if saved_job.handle.is_some() || saved_job.waiting || saved_job.settled {
        return Err(EnrollmentError::SaveUnknown);
    }
    let op = operation.clone();
    let job = runtime.spawn_blocking(move || {
        let _active = active;
        let _permit = permit;
        if ready.blocking_recv().is_err() {
            return;
        }
        let entry = op.admit(|_, state| {
            if !state.confirmed || state.entered || state.phase != Phase::AwaitingConfirmation {
                return Err(EnrollmentError::Unavailable);
            }
            if materials.input.expires_at_ms() <= crate::server::logging::now_ms() {
                return Err(EnrollmentError::Expired);
            }
            state.entered = true;
            state.phase = Phase::Saving;
            state.saved = None;
            state.ready = None;
            state.message = "CodeArts账号与一次机会正在同次保存，最终结果尚未确认；不会再次提交";
            Ok(())
        });
        if let Err(error) = entry {
            op.stop(error, Phase::Failed);
            guard.finish();
            return;
        }
        let Materials { input, origin } = materials;
        let attempt = catch_unwind(AssertUnwindSafe(|| origin.commit_initial(input)));
        let mut deferred = None;
        let outcome = match attempt {
            Ok(result) => result,
            Err(payload) => {
                deferred = Some(payload);
                EnrollmentCommitOutcome::CommitUnknown {
                    storage_closed: false,
                }
            }
        };
        record(&op, outcome);
        drop(deferred);
        guard.finish();
    });
    saved_job.handle = Some(job);
    drop(saved_job);
    operation.job_changed.notify_waiters();
    begin.send(()).map_err(|_| EnrollmentError::SaveUnknown)
}

fn record(operation: &Operation, outcome: EnrollmentCommitOutcome) {
    let mut state = operation
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    match outcome {
        EnrollmentCommitOutcome::Committed {
            receipt,
            storage_closed,
        } => {
            state.saved = Some(true);
            state.ready = Some(receipt.ready_registered());
            state.storage_closed = Some(storage_closed);
            state.phase = Phase::Saved;
            state.account = Some(receipt.account().to_string());
            state.message = if storage_closed {
                "CodeArts账号与一次机会已确认保存；未发送换证，也不表示令牌已验证"
            } else {
                "CodeArts账号与一次机会已提交，但存储收尾未确认，暂不可用；不会再次保存"
            };
        }
        EnrollmentCommitOutcome::NotEntered(_) => {
            state.saved = Some(false);
            state.ready = Some(false);
            state.phase = Phase::Failed;
            state.message = "CodeArts本次保存未进入，未登记一次机会；不会自动再次提交";
        }
        EnrollmentCommitOutcome::Rejected { storage_closed, .. } => {
            state.storage_closed = Some(storage_closed);
            state.saved = storage_closed.then_some(false);
            state.ready = storage_closed.then_some(false);
            state.phase = Phase::Failed;
            state.message = if storage_closed {
                "CodeArts首次登记已拒绝并回滚，旧槽或旧来源不能通过手填材料解封"
            } else {
                EnrollmentError::SaveUnknown.message()
            };
        }
        EnrollmentCommitOutcome::CommitUnknown { storage_closed } => {
            state.saved = None;
            state.ready = None;
            state.storage_closed = Some(storage_closed);
            state.phase = Phase::Failed;
            state.message = EnrollmentError::SaveUnknown.message();
        }
    }
    state.ended_at = Some(tokio::time::Instant::now());
}

struct WaitingJob {
    operation: Arc<Operation>,
    job: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for WaitingJob {
    fn drop(&mut self) {
        if let Some(job) = self.job.take() {
            let mut slot = self
                .operation
                .job
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if slot.handle.is_none() {
                slot.handle = Some(job);
            }
            slot.waiting = false;
            drop(slot);
            self.operation.job_changed.notify_waiters();
        }
    }
}

pub(super) async fn wait(operation: Arc<Operation>) {
    loop {
        let changed = operation.job_changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        let job = {
            let mut slot = operation
                .job
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if slot.settled {
                return;
            }
            if !slot.waiting && slot.handle.is_some() {
                slot.waiting = true;
                slot.handle.take()
            } else if !slot.waiting
                && slot.handle.is_none()
                && operation.completed.load(Ordering::Acquire)
            {
                return;
            } else {
                None
            }
        };
        let mut waiting = WaitingJob {
            operation: operation.clone(),
            job,
        };
        if let Some(job) = waiting.job.as_mut() {
            let _ = job.await;
            waiting.job.take();
            let mut slot = operation
                .job
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            slot.waiting = false;
            slot.settled = true;
            drop(slot);
            operation.job_changed.notify_waiters();
            return;
        }
        changed.await;
    }
}

pub(super) fn release_ready(operation: &Operation) -> bool {
    operation.completed.load(Ordering::Acquire)
        && operation.job.try_lock().is_ok_and(|slot| {
            !slot.waiting
                && (slot.settled
                    || slot
                        .handle
                        .as_ref()
                        .is_none_or(tokio::task::JoinHandle::is_finished))
        })
}
