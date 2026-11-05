use std::sync::Arc;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::server::{config, logging};
use crate::server::core::account_store::{sql, state::StoredAccount, AccountStore};

pub(crate) fn has_restrictions(profile: &config::RoutingProfile) -> bool {
    profile.scope() == config::RoutingProviderScope::Selected
        || !profile.disabled_providers.is_empty() || !profile.excluded_accounts.is_empty()
}

fn excluded(profile: &config::RoutingProfile, account: &StoredAccount) -> bool {
    !profile.includes_provider(&account.provider())
        || profile.excluded_accounts.iter().any(|id| id == account.id())
}

pub(crate) fn legacy_snapshot(profile: Option<&config::RoutingProfile>, accounts: &[StoredAccount]) -> Option<Value> {
    let profile = profile.filter(|profile| has_restrictions(profile))?;
    let mut ordered: Vec<_> = accounts.iter().collect();
    ordered.sort_by(|left, right| left.id().cmp(right.id()));
    let states: Vec<_> = ordered.iter().map(|account| json!({
        "id": account.id(), "provider": account.provider(), "name": account.name(), "enabled": account.enabled(),
    })).collect();
    let revision = format!("{:x}", Sha256::digest(json!({ "profile": profile, "accounts": states }).to_string().as_bytes()));
    let affected: Vec<_> = ordered.into_iter().filter(|account| account.enabled() && excluded(profile, account))
        .map(|account| json!({ "id": account.id(), "name": account.name(), "provider": account.provider(),
            "providerLabel": crate::server::core::providers::label_of(&account.provider()) })).collect();
    Some(json!({ "revision": revision, "accounts": affected }))
}

pub(crate) fn migrate(store: &AccountStore, expected: &str) -> Result<(), String> {
    let update = config::CONFIG_UPDATE_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let guard = store.guard();
    let state = store.load(&guard);
    let count = store.with_conn(&guard, sql::count_all).map_err(|error| error.message)?;
    if count != state.accounts.len() as i64 {
        return Err("账号数据未完整读取，未转换旧版全局参与设置".into());
    }
    let mut next = config::current();
    let snapshot = legacy_snapshot(next.routing_profiles.global.as_ref(), &state.accounts)
        .ok_or("旧版全局参与设置已转换，请刷新当前配置")?;
    if snapshot.get("revision").and_then(Value::as_str) != Some(expected) {
        return Err("全局参与设置或账号状态已变化，请刷新后重新确认转换".into());
    }
    let mut profiles = (*next.routing_profiles).clone();
    let global = profiles.global.as_mut().ok_or("旧版全局参与设置不存在")?;
    let mut disabled = Vec::new();
    let patch = json!({ "enabled": false }).as_object().cloned().unwrap_or_default();
    for mut account in state.accounts {
        if !account.enabled() || !excluded(global, &account) { continue; }
        AccountStore::apply_patch(&mut account, &patch, |_| Ok(None)).map_err(|error| error.message)?;
        account.set_updated_at(logging::now_ms());
        disabled.push(account);
    }
    global.provider_scope = Some(config::RoutingProviderScope::All);
    global.disabled_providers.clear();
    global.excluded_accounts.clear();
    let serialized = serde_json::to_value(&profiles).map_err(|error| error.to_string())?;
    // 账号禁用与旧名单清除必须一起提交，不能出现放开路由但尚未禁用账号的中间态。
    store.with_conn_mut(&guard, |conn| {
        let tx = conn.transaction()?;
        for account in &disabled { sql::update_in_place(&tx, account)?; }
        tx.execute("INSERT INTO kv(key,value) VALUES('routingProfiles',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            rusqlite::params![serialized.to_string()])?;
        tx.commit()
    }).map_err(|error| error.message)?;
    next.raw.insert(config::KEY_ROUTING_PROFILES.into(), serialized);
    next.routing_profiles = Arc::new(profiles);
    crate::server::core::providers::catalog::invalidate();
    *config::CONFIG.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(next);
    crate::server::core::providers::catalog::invalidate();
    drop(guard);
    drop(update);
    for account in &disabled { store.invalidate_catpaw_sessions(account.id(), &account.provider()); }
    logging::log("[Routing]", &format!("已转换旧版全局参与设置，禁用 {} 个账号；独立路由参与名单保持不变", disabled.len()));
    crate::server::core::cloud_sync::auto_sync::notify_change();
    Ok(())
}
