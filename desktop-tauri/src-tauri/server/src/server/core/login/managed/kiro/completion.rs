//! 固定Kiro独立工作完成见证；输入消费与候选账号不构成保存资格。

use std::sync::Arc;

use agent2api_provider_protocol::NormalizedAccount;

use super::super::{unavailable, Call};
use super::{Receipt, State};
use crate::server::{core::plugin_host::wasm::managed::KiroCompleted, errors::GatewayError};

pub(in crate::server::core::login::managed) struct Completion {
    original: Arc<KiroCompleted>,
}

impl Completion {
    fn current(&self, state: &State) -> bool {
        self.original.locally_complete() && self.original.input().matches_state(state)
    }

    pub(super) fn completes(&self, receipt: &Receipt, state: &State, step: &str) -> bool {
        self.current(state)
            && state.callback_step.as_deref() == Some(step)
            && self.original.input().belongs_to_receipt(receipt)
    }
}

impl Call {
    pub(crate) fn install_kiro_completed(
        &self,
        completed: Arc<KiroCompleted>,
    ) -> Result<(), GatewayError> {
        if !completed.matches(self) {
            return Err(unavailable());
        }
        completed.with_execution(self, || {
            completed.source().with_current(self, || {
                let mut slot = self
                    .session
                    .kiro_completed
                    .try_lock()
                    .map_err(|_| unavailable())?;
                if slot.is_some() {
                    return Err(unavailable());
                }
                *slot = Some(Completion {
                    original: completed.clone(),
                });
                Ok(())
            })
        })
    }

    fn kiro_completed(&self) -> Result<Arc<KiroCompleted>, GatewayError> {
        let slot = self
            .session
            .kiro_completed
            .try_lock()
            .map_err(|_| unavailable())?;
        slot.as_ref()
            .map(|completion| completion.original.clone())
            .ok_or_else(unavailable)
    }

    pub(crate) fn validate_kiro_account(
        &self,
        account: &NormalizedAccount,
    ) -> Result<(), GatewayError> {
        if self.provider() != "kiro" || self.method_index() != 0 {
            return Ok(());
        }
        let completed = self.kiro_completed()?;
        if !completed.matches(self) || !completed.matches_account(account) {
            return Err(unavailable());
        }
        completed.with_execution(self, || completed.source().with_current(self, || Ok(())))
    }

    pub(in crate::server::core::login::managed) fn kiro_can_complete(
        &self,
    ) -> Result<bool, GatewayError> {
        let completed = self.kiro_completed()?;
        if !completed.matches(self) {
            return Ok(false);
        }
        completed.with_execution(self, || {
            completed.source().with_current(self, || {
                let slot = self
                    .session
                    .kiro_completed
                    .try_lock()
                    .map_err(|_| unavailable())?;
                Ok(slot
                    .as_ref()
                    .is_some_and(|original| Arc::ptr_eq(&original.original, &completed)))
            })
        })
    }
}

pub(in crate::server::core::login::managed) fn persistable(
    session: &super::super::Session,
    state: &State,
) -> bool {
    session.kiro_completed.try_lock().is_ok_and(|slot| {
        slot.as_ref()
            .is_some_and(|completion| completion.current(state))
    })
}

pub(in crate::server::core::login::managed) fn completes(
    session: &super::super::Session,
    receipt: &Receipt,
    state: &State,
    step: &str,
) -> bool {
    session.kiro_completed.try_lock().is_ok_and(|slot| {
        slot.as_ref()
            .is_some_and(|completion| completion.completes(receipt, state, step))
    })
}
