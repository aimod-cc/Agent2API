use serde_json::Value;

use super::AccountStore;

impl AccountStore {
    pub(crate) fn decorate_policy_cooldown(&self, mut account: Value, model: &str) -> Value {
        let provider = crate::server::core::routing::provider_of(&account);
        if let Some(entry) = self.policy_cooldown_for(provider, model) { account["providerCooldown"] = entry; }
        account
    }
}
