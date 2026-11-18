//! Free续期的私有原行和Pending来源；只处理当前Free，不读取Pass或按时间恢复发送资格。

use std::{any::Any, panic::{catch_unwind, AssertUnwindSafe}, sync::{Arc, Weak}};

use serde_json::Value;
use zeroize::Zeroizing;

use super::{cached_key, sql, AccountStore, FreeEnrollmentError as Error};
use crate::server::db::{CodeArtsTransaction, CredentialSource, FreeEnrollmentTransaction};
use sql::credential_version::cline_free_enrollment::{refresh, RetainedRows};

pub(crate) struct FreeRefreshOrigin {
    store: AccountStore,
    source: CredentialSource,
    ready: refresh::Ready,
}

pub(crate) struct FreeRefreshPending {
    origin: FreeRefreshOrigin,
    operation: String,
    binding: Arc<()>,
}

pub(crate) struct FreeRefreshCheck<'a> {
    owner: &'a FreeRefreshPending,
    key: Zeroizing<[u8; 32]>,
    replacement: Option<(&'a Value, &'a Value)>,
}

#[must_use]
pub(crate) struct FreeRefreshFinal {
    confirmed: bool,
    _retained: RetainedRows,
    _deferred: [Option<Box<dyn Any + Send>>; 2],
}

impl FreeRefreshFinal {
    pub(crate) fn into_result(self) -> Result<(), Error> {
        if self.confirmed { Ok(()) } else { Err(Error::Unavailable) }
    }
}

impl AccountStore {
    pub(crate) fn read_free_refresh_origin(&self, account: &str) -> Result<FreeRefreshOrigin, Error> {
        let key = cached_key().map_err(|_| Error::KeyUnavailable)?;
        let mut retained = RetainedRows::new();
        let db = self.database().ok_or(Error::Unavailable)?;
        let source = db.capture_cline_free_enrollment_source().map_err(|_| Error::Unavailable)?;
        let mut candidate = None;
        let outcome = {
            let _account = self.guard();
            db.cline_free_enrollment_transaction(&source, |conn| {
                candidate = Some(refresh::read(conn, &key, account, &mut retained)?);
                Ok::<(), Error>(())
            })
        };
        match outcome {
            FreeEnrollmentTransaction::Committed { storage_closed: true, .. } => {}
            FreeEnrollmentTransaction::Rejected { error, storage_closed: true } => return Err(error),
            FreeEnrollmentTransaction::NotEntered { error, .. } => return Err(Error::Transaction(error)),
            _ => return Err(Error::Unavailable),
        }
        let ready = candidate.ok_or(Error::Unavailable)?;
        Ok(FreeRefreshOrigin { store: self.clone(), source, ready })
    }
}

impl FreeRefreshOrigin {
    pub(crate) fn account(&self) -> &str { self.ready.record.id() }
    pub(crate) fn secret(&self) -> &Value { self.ready.record.get("pluginSecret").unwrap_or(&Value::Null) }
    pub(crate) fn public(&self) -> &Value { self.ready.record.get("pluginPublic").unwrap_or(&Value::Null) }
    pub(crate) fn proxy(&self) -> &Value { self.ready.record.get("proxy").unwrap_or(&Value::Null) }
    pub(crate) fn expiring(&self) -> bool {
        let now = crate::server::logging::now_ms();
        now > 0 && self.secret()["expiresAt"].as_i64().filter(|expiry| *expiry > 0)
            .is_some_and(|expiry| expiry.saturating_sub(600_000) <= now)
    }

    pub(crate) fn consume(self) -> Result<FreeRefreshPending, Error> {
        let key = cached_key().map_err(|_| Error::KeyUnavailable)?;
        let operation = random()?;
        let mut retained = RetainedRows::new();
        let db = self.store.database().ok_or(Error::Unavailable)?;
        let outcome = {
            let _account = self.store.guard();
            db.cline_free_enrollment_transaction(&self.source, |conn|
                refresh::begin(conn, &key, &self.ready, &operation, &mut retained))
        };
        if !matches!(outcome, FreeEnrollmentTransaction::Committed { storage_closed: true, .. }) {
            return Err(Error::Unavailable);
        }
        Ok(FreeRefreshPending { origin: self, operation, binding: Arc::new(()) })
    }
}

impl FreeRefreshPending {
    pub(crate) fn account(&self) -> &str { self.origin.account() }
    pub(crate) fn binding(&self) -> Weak<()> { Arc::downgrade(&self.binding) }
    pub(crate) fn matches(&self, binding: &Weak<()>) -> bool { Weak::ptr_eq(binding, &self.binding()) }

    pub(crate) fn prepare_send(&self) -> Result<FreeRefreshCheck<'_>, Error> {
        self.prepare(None)
    }
    pub(crate) fn prepare_completion<'a>(&'a self, secret: &'a Value, public: &'a Value) -> Result<FreeRefreshCheck<'a>, Error> {
        self.prepare(Some((secret, public)))
    }
    fn prepare<'a>(&'a self, replacement: Option<(&'a Value, &'a Value)>) -> Result<FreeRefreshCheck<'a>, Error> {
        Ok(FreeRefreshCheck { owner: self, key: cached_key().map_err(|_| Error::KeyUnavailable)?, replacement })
    }
}

impl FreeRefreshCheck<'_> {
    /// 仅原Call门内同步执行；回调只消费原内存状态，候选/原行/异常均带出全部锁后释放。
    pub(crate) fn run(&self, consume: impl FnOnce() -> bool) -> FreeRefreshFinal {
        let mut retained = RetainedRows::new();
        let mut commit = None;
        let mut accepted = false;
        let mut action_unwind = None;
        let mut deferred = None;
        let mut consume = Some(consume);
        let origin = &self.owner.origin;
        let attempt = catch_unwind(AssertUnwindSafe(|| {
            origin.store.bound_credential_transaction(&origin.source, &mut commit, &mut |conn| {
                match catch_unwind(AssertUnwindSafe(|| {
                    refresh::verify_pending(conn, &self.key, &origin.ready, &self.owner.operation, &mut retained)?;
                    if !consume.take().is_some_and(|consume| consume()) { return Err(Error::Interrupted); }
                    if let Some((secret, public)) = self.replacement {
                        refresh::complete(conn, &self.key, &origin.ready, &self.owner.operation, secret, public, &mut retained)?;
                    }
                    accepted = true;
                    Ok::<(), Error>(())
                })) {
                    Ok(Ok(())) => true,
                    Ok(Err(_)) => false,
                    Err(payload) => { action_unwind = Some(payload); false },
                }
            })
        }));
        let confirmed = match attempt {
            Ok(Ok(CodeArtsTransaction::Committed { storage_closed: true, .. })) => accepted && commit == Some(true),
            Ok(_) => false,
            Err(payload) => { deferred = Some(payload); false },
        };
        FreeRefreshFinal { confirmed, _retained: retained, _deferred: [action_unwind, deferred] }
    }
}

fn random() -> Result<String, Error> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| Error::RandomUnavailable)?;
    if bytes.iter().all(|byte| *byte == 0) { return Err(Error::RandomUnavailable); }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
