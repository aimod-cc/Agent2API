//! Kiro原Callback的弱对象来源与一次准备槽；不从flow/id/普通state重建receipt。
//! worker必须先核原Host/Runtime，再经with_current进入Session短门消费；不在门内联网。

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, Weak,
};

use serde_json::Value;
use zeroize::Zeroizing;

use super::super::{kiro, RuntimeBinding, Session};
use super::{Action, Call, Material};
use crate::server::errors::GatewayError;

pub(in crate::server::core::login::managed::call) struct Slot {
    attempted: AtomicBool,
    prepared: Mutex<Option<Arc<kiro::Prepared>>>,
}
impl Default for Slot {
    fn default() -> Self {
        Self {
            attempted: AtomicBool::new(false),
            prepared: Mutex::new(None),
        }
    }
}

/// 只由原take准备签发，无serde/Clone/公开构造器；弱Session+Material+Receipt对象关系。
pub(crate) struct KiroCallSource {
    session: Weak<Session>,
    material: Weak<Material>,
    receipt: Weak<kiro::Receipt>,
    prepared: Weak<kiro::Prepared>,
    runtime: Weak<RuntimeBinding>,
    id: Zeroizing<String>,
    expected: Zeroizing<String>,
    action: Action,
}

impl KiroCallSource {
    pub(crate) fn matches(&self, call: &Call) -> bool {
        self.action == Action::Callback
            && call.action == self.action
            && kiro::applies(call.provider(), call.method_index())
            && self.id.as_str() == call.id()
            && self.expected.as_str() == call.expected()
            && self
                .session
                .upgrade()
                .is_some_and(|session| Arc::ptr_eq(&session, &call.session))
            && self
                .material
                .upgrade()
                .is_some_and(|material| Arc::ptr_eq(&material, &call.material))
    }
    pub(crate) fn same(&self, other: &Self) -> bool {
        self.action == other.action
            && self.id.as_str() == other.id.as_str()
            && self.expected.as_str() == other.expected.as_str()
            && Weak::ptr_eq(&self.session, &other.session)
            && Weak::ptr_eq(&self.material, &other.material)
            && Weak::ptr_eq(&self.receipt, &other.receipt)
            && Weak::ptr_eq(&self.prepared, &other.prepared)
            && Weak::ptr_eq(&self.runtime, &other.runtime)
    }

    /// 调用方已持原Session state；只try原Material小槽，不进入Session/Task/Host锁。
    pub(in crate::server::core::login::managed) fn matches_state(
        &self,
        state: &super::super::State,
        expected: &Arc<kiro::Prepared>,
    ) -> bool {
        let Some(session) = self.session.upgrade() else {
            return false;
        };
        let Some(material) = self.material.upgrade() else {
            return false;
        };
        if self.action != Action::Callback
            || !session.valid()
            || !kiro::applies(&session.provider, session.method_index)
            || state.active.as_deref() != Some(self.id.as_str())
            || state.callback_step.as_deref() != Some(self.expected.as_str())
            || !material.taken.load(Ordering::Acquire)
            || material.kiro_advanced.load(Ordering::Acquire)
            || !material.kiro_exchange.attempted.load(Ordering::Acquire)
            || tokio::time::Instant::now() >= expected.deadline()
        {
            return false;
        }
        let Ok(receipt) = material.kiro_receipt.try_lock() else {
            return false;
        };
        let Some(receipt) = receipt.as_ref() else {
            return false;
        };
        let Ok(prepared) = material.kiro_exchange.prepared.try_lock() else {
            return false;
        };
        let Some(prepared) = prepared.as_ref() else {
            return false;
        };
        Weak::ptr_eq(&self.receipt, &Arc::downgrade(receipt))
            && Weak::ptr_eq(&self.prepared, &Arc::downgrade(prepared))
            && Arc::ptr_eq(prepared, expected)
            && Arc::ptr_eq(prepared.receipt(), receipt)
            && Weak::ptr_eq(&self.runtime, &Arc::downgrade(prepared.runtime()))
            && state.runtime.as_ref().is_some_and(|runtime| {
                Arc::ptr_eq(runtime, prepared.runtime())
                    && !runtime.stopped.is_cancelled()
                    && (runtime.alive)()
            })
            && receipt.matches_exchange_state(state)
    }

    /// Host→Session→原Material；只在短同步action内CAS，不validate重入、阻塞或发网络。
    pub(crate) fn with_current<R>(
        &self,
        call: &Call,
        action: impl FnOnce() -> Result<R, GatewayError>,
    ) -> Result<R, GatewayError> {
        if !self.matches(call) || call.published() {
            return Err(kiro_invalid());
        }
        let state = call.session.state.try_lock().map_err(|_| kiro_invalid())?;
        if !call.session.valid()
            || !call.material.taken.load(Ordering::Acquire)
            || call.material.kiro_advanced.load(Ordering::Acquire)
            || !call
                .material
                .kiro_exchange
                .attempted
                .load(Ordering::Acquire)
        {
            return Err(kiro_invalid());
        }
        let receipt = call
            .material
            .kiro_receipt
            .try_lock()
            .map_err(|_| kiro_invalid())?;
        let receipt = receipt.as_ref().ok_or_else(kiro_invalid)?;
        let prepared = call
            .material
            .kiro_exchange
            .prepared
            .try_lock()
            .map_err(|_| kiro_invalid())?;
        let prepared = prepared.as_ref().ok_or_else(kiro_invalid)?;
        if !Weak::ptr_eq(&self.receipt, &Arc::downgrade(receipt))
            || !Weak::ptr_eq(&self.prepared, &Arc::downgrade(prepared))
            || !Arc::ptr_eq(prepared.receipt(), receipt)
            || !Weak::ptr_eq(&self.runtime, &Arc::downgrade(prepared.runtime()))
            || !state.runtime.as_ref().is_some_and(|runtime| {
                Arc::ptr_eq(runtime, prepared.runtime())
                    && !runtime.stopped.is_cancelled()
                    && (runtime.alive)()
            })
            || !receipt.exchange_current(&state, call)
            || tokio::time::Instant::now() >= prepared.deadline()
        {
            return Err(kiro_invalid());
        }
        action()
    }
}

fn kiro_invalid() -> GatewayError {
    GatewayError::with_status(
        409,
        "Kiro原Callback兑换来源已变化、未完整消费或已准备，未发送授权码",
    )
    .with_code("kiro_exchange_source_invalid")
}

impl Call {
    /// params必须是原SDK候选正文的UTF8 JSON String，不收URL/headers/method/proxy自由信封。
    /// take先永久占准备；校验失败也不退还，不返回缓存材料或恢复once输入。
    pub(crate) fn take_kiro_exchange(
        &self,
        params: &Value,
    ) -> Result<kiro::KiroExchangeInput, GatewayError> {
        self.validate("kiro")?;
        if !kiro::applies(self.provider(), self.method_index())
            || !self.is_callback()
            || self.published()
            || !self.material.taken.load(Ordering::Acquire)
            || self.material.kiro_advanced.load(Ordering::Acquire)
        {
            return Err(kiro_invalid());
        }
        let mut deferred_receipt = None;
        let mut deferred_runtime = None;
        let mut deferred = None;
        let result = (|| {
            let state = self.session.state.try_lock().map_err(|_| kiro_invalid())?;
            if !self.session.valid()
                || state.active.as_deref() != Some(self.id())
                || self
                    .material
                    .kiro_exchange
                    .attempted
                    .swap(true, Ordering::AcqRel)
            {
                return Err(kiro_invalid());
            }
            let receipt = self
                .material
                .kiro_receipt
                .try_lock()
                .map_err(|_| kiro_invalid())?;
            let receipt = receipt
                .as_ref()
                .filter(|receipt| !receipt.choice() && receipt.exchange_current(&state, self))
                .ok_or_else(kiro_invalid)?
                .clone();
            deferred_receipt = Some(receipt.clone());
            let runtime = state
                .runtime
                .as_ref()
                .filter(|runtime| !runtime.stopped.is_cancelled() && (runtime.alive)())
                .ok_or_else(kiro_invalid)?
                .clone();
            deferred_runtime = Some(runtime.clone());
            let mut slot = self
                .material
                .kiro_exchange
                .prepared
                .try_lock()
                .map_err(|_| kiro_invalid())?;
            if slot.is_some() {
                return Err(kiro_invalid());
            }
            let prepared = kiro::Prepared::from_candidate(
                receipt.clone(),
                params,
                runtime.clone(),
                self.session.deadline,
            )?;
            deferred = Some(prepared.clone());
            let source = KiroCallSource {
                session: Arc::downgrade(&self.session),
                material: Arc::downgrade(&self.material),
                receipt: Arc::downgrade(&receipt),
                prepared: Arc::downgrade(&prepared),
                runtime: Arc::downgrade(&runtime),
                id: Zeroizing::new(self.id().to_string()),
                expected: Zeroizing::new(self.expected().to_string()),
                action: self.action,
            };
            *slot = Some(prepared.clone());
            Ok(kiro::KiroExchangeInput::new(source, prepared))
        })();
        drop(deferred);
        drop(deferred_receipt);
        drop(deferred_runtime);
        match result {
            Ok(input) => Ok(input),
            Err(error) => {
                self.session.close();
                Err(error)
            }
        }
    }
}

pub(in crate::server::core::login::managed::call) fn finish(call: &Call) {
    let removed = call
        .material
        .kiro_exchange
        .prepared
        .lock()
        .ok()
        .and_then(|mut slot| slot.take());
    drop(removed);
}
