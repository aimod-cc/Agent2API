use std::sync::{Arc, Weak};

use super::{EnrollmentError, Operation, Phase};

struct Guard {
    operation: Weak<Operation>,
    done: bool,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.done {
            if let Some(operation) = self.operation.upgrade() {
                operation.stop(EnrollmentError::Unavailable, Phase::Failed);
            }
        }
    }
}

pub(super) fn register(operation: &Arc<Operation>) -> Result<(), EnrollmentError> {
    let runtime =
        tokio::runtime::Handle::try_current().map_err(|_| EnrollmentError::Unavailable)?;
    let deadline = operation.deadline;
    let revoked = operation.manager.revocation();
    let stopped = operation.binding.stopped.clone();
    let done = operation.cleanup_done.clone();
    let guard = Guard {
        operation: Arc::downgrade(operation),
        done: false,
    };
    runtime.spawn(async move {
        let mut guard = guard;
        let reason = tokio::select! {
            biased;
            _ = done.cancelled() => None,
            _ = revoked.cancelled() => Some((EnrollmentError::Unavailable, Phase::Failed)),
            _ = stopped.cancelled() => Some((EnrollmentError::Unavailable, Phase::Failed)),
            _ = tokio::time::sleep_until(deadline) => Some((EnrollmentError::Expired, Phase::Expired)),
        };
        if let Some((error, phase)) = reason {
            if let Some(operation) = guard.operation.upgrade() {
                operation.stop(error, phase);
            }
        }
        guard.done = true;
    });
    Ok(())
}
