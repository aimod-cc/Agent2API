//! Immutable request-time account-group snapshot.

use serde_json::Value;

/// A request-local canonical account-group view.
///
/// The fields are intentionally private. Routing receives only the stable group
/// id and the canonical account slice; membership bookkeeping stays an
/// implementation detail of scope construction.
#[derive(Clone, Debug, PartialEq)]
pub struct AccountGroupScope {
    group_id: String,
    accounts: Vec<Value>,
}

impl AccountGroupScope {
    /// Construct the only supported scope shape from canonical ordered accounts.
    pub fn from_canonical(group_id: impl Into<String>, accounts: Vec<Value>) -> Self {
        Self {
            group_id: group_id.into(),
            accounts,
        }
    }

    pub fn group_id(&self) -> &str {
        &self.group_id
    }

    pub fn accounts(&self) -> &[Value] {
        &self.accounts
    }
}
