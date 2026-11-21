use serde_json::Value;

use super::{policy_cooldowns::PolicyCooldownState, AccountStore};

/// 使用已加载的冷却快照为单条账号补充 `providerCooldown`，避免批量场景重复查库。
pub(crate) fn decorate_with_cooldowns(
    mut account: Value,
    cooldowns: &PolicyCooldownState,
    model: &str,
    now: i64,
) -> Value {
    let provider = crate::server::core::routing::provider_of(&account);
    if let Some(entry) = cooldowns.cooldown_for(provider, model, now) {
        account["providerCooldown"] = entry;
    }
    account
}

impl AccountStore {
    pub(crate) fn decorate_policy_cooldown(&self, mut account: Value, model: &str) -> Value {
        let provider = crate::server::core::routing::provider_of(&account);
        if let Some(entry) = self.policy_cooldown_for(provider, model) { account["providerCooldown"] = entry; }
        account
    }
}
