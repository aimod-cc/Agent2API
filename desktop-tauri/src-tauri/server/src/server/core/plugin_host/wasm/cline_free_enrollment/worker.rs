//! Cline Free首次登记的唯一同步保存工作；取消等待不撤回已进入的真实事务。

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{atomic::Ordering, Arc};

use tokio::{runtime::Handle, sync::oneshot};

use super::{slots, EnrollmentError, Materials, Operation, Phase};
use crate::server::core::account_store::credential_version::FreeEnrollmentCommitOutcome;
use crate::server::core::plugin_host::{activity::UsageLease, Host};

#[derive(Default)]
pub(super) struct JobState {
    handle: Option<tokio::task::JoinHandle<()>>,
    waiting: bool,
    settled: bool,
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
        state.materials.take().ok_or(EnrollmentError::Unavailable)
    })?;
    let mut slot = operation
        .job
        .lock()
        .map_err(|_| EnrollmentError::SaveUnknown)?;
    if slot.handle.is_some() || slot.waiting || slot.settled {
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
            if materials
                .input
                .expires_at_ms()
                .is_some_and(|at| at <= crate::server::logging::now_ms())
            {
                return Err(EnrollmentError::Expired);
            }
            state.entered = true;
            state.phase = Phase::Saving;
            state.saved = None;
            state.ready = None;
            state.message = "Cline Free新账号和一次机会正在同次保存，结果尚未确认；不会再次提交";
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
            Ok(outcome) => outcome,
            Err(payload) => {
                deferred = Some(payload);
                FreeEnrollmentCommitOutcome::CommitUnknown {
                    storage_closed: false,
                }
            }
        };
        record(&op, outcome);
        drop(deferred);
        guard.finish();
    });
    slot.handle = Some(job);
    drop(slot);
    operation.job_changed.notify_waiters();
    begin.send(()).map_err(|_| EnrollmentError::SaveUnknown)
}

fn record(operation: &Operation, outcome: FreeEnrollmentCommitOutcome) {
    let mut state = operation
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    match outcome {
        FreeEnrollmentCommitOutcome::Committed {
            receipt,
            storage_closed,
        } => {
            state.saved = Some(true);
            state.ready = Some(receipt.ready_registered());
            state.storage_closed = Some(storage_closed);
            state.phase = Phase::Saved;
            state.account = Some(receipt.account().to_string());
            state.message = if storage_closed {
                "Cline Free新账号与受控续期已登记；本次未验证令牌或换证，之后只对原账号单次续期，结果不明不重发"
            } else {
                "Cline Free新账号和一次机会已提交，但存储收尾未确认，暂不可用；不会再次保存"
            };
        }
        FreeEnrollmentCommitOutcome::NotCommitted(error) => {
            state.saved = Some(false);
            state.ready = Some(false);
            state.phase = Phase::Failed;
            state.message = error.message();
        }
        FreeEnrollmentCommitOutcome::CommitUnknown { storage_closed } => {
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
            } else if !slot.waiting && operation.completed.load(Ordering::Acquire) {
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
