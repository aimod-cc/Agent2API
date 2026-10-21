//! Account-group data layer.
//!
//! This module owns the SQLite group service and the pure expansion/scope types.
//! API keys, HTTP handlers, ServerState wiring, and request routing are separate
//! follow-up layers and are intentionally not registered here.

use std::sync::{Arc, Mutex};

use crate::server::db::Db;

pub mod legacy;
pub mod resolver;
pub mod scope;
pub mod store;

#[cfg(test)]
mod key_contract_tests;
#[cfg(test)]
mod legacy_cleanup_tests;
#[cfg(test)]
mod migration_tests;
#[cfg(test)]
mod resolver_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
pub(crate) mod test_bootstrap;

pub use resolver::{resolve, MissingGroupMember, ResolvedAccountGroup};
pub use scope::AccountGroupScope;
pub use store::{AccountGroup, AccountGroupError, AccountGroupMember, MemberType};

struct Inner {
    db: Option<Db>,
    management_lock: Arc<Mutex<()>>,
}

/// Shared account-group persistence façade and management-lock owner.
#[derive(Clone)]
pub struct AccountGroupService {
    inner: Arc<Inner>,
}

impl AccountGroupService {
    pub fn new(db: Option<Db>) -> Self {
        Self {
            inner: Arc::new(Inner {
                db,
                management_lock: Arc::new(Mutex::new(())),
            }),
        }
    }

    /// Run a complete cross-store management operation under the shared lock.
    pub fn with_management_lock<R>(&self, action: impl FnOnce() -> R) -> R {
        let _guard = match self.inner.management_lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        action()
    }

    pub fn list(&self) -> Result<Vec<AccountGroup>, AccountGroupError> {
        store::list(self.inner.db.as_ref())
    }

    pub fn get(&self, group_id: &str) -> Result<AccountGroup, AccountGroupError> {
        store::get(self.inner.db.as_ref(), group_id)?
            .ok_or_else(|| AccountGroupError::not_found("账号组不存在"))
    }

    pub fn create(&self, name: &str) -> Result<AccountGroup, AccountGroupError> {
        let (display, name_key) = store::normalize_name(name)?;
        store::create(self.inner.db.as_ref(), display, name_key)
    }

    pub fn rename(&self, group_id: &str, name: &str) -> Result<AccountGroup, AccountGroupError> {
        let (display, name_key) = store::normalize_name(name)?;
        store::rename(self.inner.db.as_ref(), group_id, display, name_key)
    }

    pub fn replace_members(
        &self,
        group_id: &str,
        members: Vec<AccountGroupMember>,
    ) -> Result<AccountGroup, AccountGroupError> {
        store::replace_members(self.inner.db.as_ref(), group_id, members)
    }

    /// Delete a group and its persisted members.
    ///
    /// Callers that coordinate this operation with Key or account writes must
    /// invoke it through `with_management_lock`; this layer deliberately does
    /// not read Key JSON.
    pub fn delete(&self, group_id: &str) -> Result<(), AccountGroupError> {
        if store::delete(self.inner.db.as_ref(), group_id)? {
            Ok(())
        } else {
            Err(AccountGroupError::not_found("账号组不存在"))
        }
    }

    pub fn resolve(
        &self,
        group_id: &str,
        accounts: &[serde_json::Value],
    ) -> Result<ResolvedAccountGroup, AccountGroupError> {
        let group = self.get(group_id)?;
        Ok(resolver::resolve(&group.members, accounts))
    }
}
