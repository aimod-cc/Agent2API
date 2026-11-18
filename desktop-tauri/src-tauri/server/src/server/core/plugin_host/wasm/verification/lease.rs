//! 领取等待的宿主租约；与会话状态共用同一互斥量，不占两秒交互许可。
mod transaction;

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use agent2api_provider_protocol as proto;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{Action, VerificationCall, digest, failure, text, unavailable, CLAIM_CAPABILITY};
use super::super::context::{Context, Shared};
use super::super::state::{self, ConversationState, Transient};
use super::super::super::Host;
use crate::server::core::account_store::{AccountStore, plugin_accounts};

pub(crate) const MAX_WAIT_MS: u64 = 50_000;
const MAX_PENDING: usize = 128;

#[derive(Clone)]
struct Binding {
    provider: String,
    account: String,
    identity: String,
    raw_identity: Value,
    credential: [u8; 32],
}

impl Binding {
    fn from_record(record: &Value) -> Result<Self, proto::WireError> {
        let account = text(record, "id", 512)?.to_string();
        let identity = record["identity"].as_str().unwrap_or(&account).to_string();
        if identity.len() > 512 || identity.chars().any(char::is_control) { return Err(unavailable()); }
        Ok(Self { provider: text(record, "provider", 128)?.to_string(), account, identity,
            raw_identity: record["identity"].clone(), credential: digest(&record["secret"])? })
    }
    fn matches(&self, record: &Value) -> bool {
        record["id"] == self.account && record["provider"] == self.provider && record["identity"] == self.raw_identity
            && digest(&record["secret"]).is_ok_and(|digest| digest == self.credential)
    }
    fn caller(&self, call: &VerificationCall) -> bool {
        self.provider == call.provider && self.account == call.account && self.identity == call.identity
            && self.raw_identity == call.snapshot["identity"] && self.credential == call.credential_digest
    }
}

struct Life {
    id: u64,
    host: Weak<Host>,
    shared: Weak<Shared>,
    generation: u64,
    deadline: Instant,
    cancel: CancellationToken,
}

impl Life {
    // 仅原子/弱引用读取，可在会话状态锁内调用，不另取注册表锁。
    fn alive(&self) -> bool {
        Instant::now() < self.deadline && !self.cancel.is_cancelled()
            && self.shared.upgrade().is_some_and(|shared| !shared.cancel.is_cancelled())
            && self.host.upgrade().is_some_and(|host| host.ready() && host.generation.load(Ordering::Acquire) == self.generation)
    }
}

pub(crate) struct Owner {
    life: Arc<Life>,
    store: AccountStore,
    account: String,
    provider: String,
}

impl Owner {
    pub(in crate::server::core::plugin_host::wasm) fn new(host: &Host, shared: &Arc<Shared>, method: &str,
        params: &Value, timeout: Duration) -> Result<Option<Self>, proto::WireError> {
        if !shared.options.allows(CLAIM_CAPABILITY) || !matches!(method, "checkin.claim" | "claim")
            && !(method == "admin.action" && params["action"] == "claim.claim") { return Ok(None); }
        let provider = &shared.options.provider;
        let current = super::super::super::host_for_provider(provider).filter(|current| std::ptr::eq(current.as_ref(), host))
            .ok_or_else(unavailable)?;
        let account = text(params, "account", 512)?.to_string();
        static IDS: AtomicU64 = AtomicU64::new(1);
        let id = IDS.fetch_update(Ordering::AcqRel, Ordering::Acquire, |id| id.checked_add(1)).map_err(|_| unavailable())?;
        Ok(Some(Self { life: Arc::new(Life { id, host: Arc::downgrade(&current), shared: Arc::downgrade(shared),
            generation: host.generation.load(Ordering::Acquire), deadline: Instant::now() + timeout,
            cancel: shared.cancel.child_token() }), store: plugin_accounts::shared().ok_or_else(unavailable)?,
            account, provider: provider.clone() }))
    }

    async fn snapshot(&self) -> Result<Value, proto::WireError> {
        if !self.life.alive() { return Err(unavailable()); }
        let host = self.life.host.upgrade().ok_or_else(unavailable)?;
        if !host.callbacks.provider_allowed(&self.provider) || !super::super::super::serves_provider(&self.provider)
            || super::super::super::registry::is_switching(&self.provider)
            || super::super::super::host_for_provider(&self.provider).is_none_or(|current| !Arc::ptr_eq(&current, &host)) {
            return Err(unavailable());
        }
        let store = self.store.clone();
        let provider = self.provider.clone();
        let account = self.account.clone();
        let record = crate::server::core::blocking::run(move || store.plugin_verification_snapshot(&provider, &account))
            .await.map_err(|_| unavailable())?.ok_or_else(|| failure(409, "verification_account_changed", "领取账号已删除或归属变化"))?;
        if !self.life.alive() || super::super::super::host_for_provider(&self.provider)
            .is_none_or(|current| !Arc::ptr_eq(&current, &host)) { return Err(unavailable()); }
        Ok(record)
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.life.cancel.cancel();
        let mut all = state::conversations().lock().unwrap_or_else(|error| error.into_inner());
        let ConversationState { state, leases } = &mut *all;
        let ids: Vec<_> = leases.entries.iter().filter(|(_, entry)| entry.owner_id == self.life.id).map(|(id, _)| id.clone()).collect();
        for id in ids { leases.remove(&id, state); }
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Slot { conversation: String, key: String, schema: u32 }

impl Slot {
    fn check(&self) -> Result<(), proto::WireError> {
        if self.conversation.is_empty() || self.conversation.len() > 256 || self.conversation.chars().any(char::is_control)
            || self.key.is_empty() || self.key.len() > 128 || self.key.chars().any(char::is_control) || self.schema == 0 {
            return Err(failure(400, "bad_params", "领取验证状态位置无效"));
        }
        Ok(())
    }
    fn namespace(&self, binding: &Binding) -> String {
        use sha2::{Digest, Sha256};
        format!("{}\0{}\0{:x}\0plugin:{}", binding.provider, binding.account, Sha256::digest(binding.identity.as_bytes()), self.conversation)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase { Waiting, Submitted }

struct Entry {
    owner: Weak<Life>,
    owner_id: u64,
    binding: Binding,
    slot: Slot,
    namespace: String,
    plan: String,
    config: String,
    deadline: Instant,
    expires: i64,
    client: Option<String>,
    phase: Phase,
    meta: Option<Value>,
    end: CancellationToken,
}

impl Entry {
    fn alive(&self) -> bool {
        let now = Instant::now();
        now < self.deadline && crate::server::logging::now_ms() < self.expires
            && self.owner.upgrade().is_some_and(|owner| owner.alive())
    }
    fn access(&self, context: &Context) -> bool {
        if let Some(owner) = &context.claim_owner {
            if owner.life.id == self.owner_id { return true; }
        }
        context.verification.as_ref().is_some_and(|call| call.action.claim() && self.binding.caller(call)
            && self.owner.upgrade().is_some_and(|owner| owner.generation == call.generation
                && Weak::ptr_eq(&owner.shared, &call.shared)))
    }
    fn status(&self) -> Value {
        json!({ "valid": true, "status": if self.phase == Phase::Waiting { "waiting" } else { "submitted" },
            "clientId": self.client, "expiresAt": self.expires,
            "remainingMs": self.deadline.saturating_duration_since(Instant::now()).as_millis() as u64 })
    }
}

#[derive(Default)]
pub(crate) struct Registry { entries: HashMap<String, Entry> }

impl Registry {
    fn remove(&mut self, id: &str, state: &mut Transient) -> bool {
        let Some(entry) = self.entries.remove(id) else { return false; };
        entry.end.cancel();
        state.clear_request(&entry.namespace, &entry.slot.key, id);
        true
    }
    fn prune(&mut self, state: &mut Transient) {
        let expired: Vec<_> = self.entries.iter().filter(|(_, entry)| !entry.alive()).map(|(id, _)| id.clone()).collect();
        for id in expired { self.remove(&id, state); }
    }
    pub(in crate::server::core::plugin_host::wasm) fn forget(&mut self, provider: &str, state: &mut Transient) {
        let ids: Vec<_> = self.entries.iter().filter(|(_, entry)| entry.binding.provider == provider).map(|(id, _)| id.clone()).collect();
        for id in ids { self.remove(&id, state); }
    }
}

fn invalid() -> Value { json!({ "valid": false, "status": "invalid" }) }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Open { plan_id: String, config_id: String, ttl_ms: u64, state: Slot }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request { request_id: String }

impl Context {
    pub(in crate::server::core::plugin_host::wasm) async fn verification_lease(&mut self, method: &str, params: &Value) -> Result<Value, proto::WireError> {
        self.permission(CLAIM_CAPABILITY)?;
        match method {
            "verification.lease.open" => {
                if self.verification.is_some() || !self.guards.is_empty() {
                    return Err(failure(403, "forbidden", "交互回传或持有账号锁时不能创建领取等待租约"));
                }
                let owner = self.claim_owner.as_ref().ok_or_else(|| failure(403, "forbidden", "只有当前领取调用可以创建验证租约"))?;
                let input: Open = serde_json::from_value(params.clone()).map_err(|_| failure(400, "bad_params", "领取验证租约参数无效"))?;
                input.state.check()?;
                text(params, "planId", 256)?; text(params, "configId", 256)?;
                if input.ttl_ms == 0 { return Err(failure(400, "bad_params", "领取验证等待时间无效")); }
                let record = owner.snapshot().await?;
                let binding = Binding::from_record(&record)?;
                if binding.provider != self.shared.options.provider || binding.account != self.account || binding.identity != self.identity {
                    return Err(failure(409, "verification_account_changed", "领取账号上下文已变化"));
                }
                let budget = Duration::from_millis(input.ttl_ms.min(MAX_WAIT_MS))
                    .min(owner.life.deadline.saturating_duration_since(Instant::now()));
                if budget.is_zero() { return Err(unavailable()); }
                let deadline = owner.life.deadline.min(Instant::now() + budget);
                let expires = crate::server::logging::now_ms().saturating_add(budget.as_millis() as i64);
                let mut nonce = [0u8; 32];
                getrandom::getrandom(&mut nonce).map_err(|_| unavailable())?;
                let id = format!("vclaim_{}", nonce.iter().map(|byte| format!("{byte:02x}")).collect::<String>());
                let end = CancellationToken::new();
                {
                    let mut all = state::conversations().lock().map_err(|_| unavailable())?;
                    let ConversationState { state, leases } = &mut *all;
                    leases.prune(state);
                    if !owner.life.alive() { return Err(unavailable()); }
                    if let Some(old) = leases.entries.iter().find(|(_, entry)| entry.binding.provider == binding.provider
                        && entry.binding.account == binding.account && !entry.binding.matches(&record)).map(|(id, _)| id.clone()) {
                        leases.remove(&old, state);
                    }
                    if leases.entries.contains_key(&id) || leases.entries.len() >= MAX_PENDING || leases.entries.values().any(|entry|
                        entry.owner_id == owner.life.id || entry.binding.provider == binding.provider && entry.binding.account == binding.account) {
                        return Err(failure(429, "verification_busy", "当前账号已有领取等待验证，或等待队列已满"));
                    }
                    let namespace = input.state.namespace(&binding);
                    leases.entries.insert(id.clone(), Entry { owner: Arc::downgrade(&owner.life), owner_id: owner.life.id,
                        binding, slot: input.state, namespace, plan: input.plan_id, config: input.config_id,
                        deadline, expires, client: None, phase: Phase::Waiting, meta: None, end: end.clone() });
                }
                let request = id.clone();
                let cancel = owner.life.cancel.clone();
                let wait_provider = self.shared.cancel.clone();
                crate::spawn_task(async move {
                    tokio::select! { _ = tokio::time::sleep_until(deadline) => {}, _ = cancel.cancelled() => {},
                        _ = wait_provider.cancelled() => {}, _ = end.cancelled() => return }
                    let mut all = state::conversations().lock().unwrap_or_else(|error| error.into_inner());
                    let ConversationState { state, leases } = &mut *all;
                    leases.remove(&request, state);
                });
                Ok(json!({ "requestId": id, "expiresAt": expires,
                    "remainingMs": deadline.saturating_duration_since(Instant::now()).as_millis() as u64 }))
            }
            "verification.lease.check" | "verification.lease.close" => {
                let input: Request = serde_json::from_value(params.clone()).map_err(|_| failure(400, "bad_params", "领取验证标识无效"))?;
                if input.request_id.is_empty() || input.request_id.len() > 128 { return Err(failure(400, "bad_params", "领取验证标识无效")); }
                if method.ends_with("close") {
                    if self.claim_owner.is_none() { return Err(failure(403, "forbidden", "只有原领取调用可以关闭租约")); }
                } else { self.validate_claim_account().await?; }
                let mut all = state::conversations().lock().map_err(|_| unavailable())?;
                let ConversationState { state, leases } = &mut *all;
                leases.prune(state);
                let Some(entry) = leases.entries.get(&input.request_id) else {
                    return Ok(if method.ends_with("close") { json!({ "closed": false }) } else { invalid() });
                };
                if !entry.access(self) { return Err(failure(403, "forbidden", "领取租约不属于当前调用或账号")); }
                if method.ends_with("close") {
                    if self.claim_owner.as_ref().is_none_or(|owner| owner.life.id != entry.owner_id) {
                        return Err(failure(403, "forbidden", "只有原领取调用可以关闭租约"));
                    }
                    return Ok(json!({ "closed": leases.remove(&input.request_id, state) }));
                }
                Ok(entry.status())
            }
            _ => Err(failure(403, "forbidden", "领取租约操作无效")),
        }
    }

    async fn validate_claim_account(&self) -> Result<(), proto::WireError> {
        if let Some(call) = &self.verification {
            if !call.action.claim() { return Err(failure(403, "forbidden", "模型验证码不能访问领取租约")); }
            call.validate().await?;
            let mut all = state::conversations().lock().map_err(|_| unavailable())?;
            let ConversationState { state, leases } = &mut *all;
            let changed: Vec<_> = leases.entries.iter().filter(|(_, entry)|
                entry.binding.provider == call.provider && entry.binding.account == call.account && !entry.binding.caller(call))
                .map(|(id, _)| id.clone()).collect();
            for id in changed { leases.remove(&id, state); }
            return Ok(());
        }
        let owner = self.claim_owner.as_ref().ok_or_else(|| failure(403, "forbidden", "当前调用没有领取租约权限"))?;
        let record = match owner.snapshot().await {
            Ok(record) => record,
            Err(error) => {
                let mut all = state::conversations().lock().map_err(|_| unavailable())?;
                let ConversationState { state, leases } = &mut *all;
                let ids: Vec<_> = leases.entries.iter().filter(|(_, entry)| entry.owner_id == owner.life.id).map(|(id, _)| id.clone()).collect();
                for id in ids { leases.remove(&id, state); }
                return Err(error);
            }
        };
        let mut all = state::conversations().lock().map_err(|_| unavailable())?;
        let ConversationState { state, leases } = &mut *all;
        let changed: Vec<_> = leases.entries.iter().filter(|(_, entry)| entry.owner_id == owner.life.id && !entry.binding.matches(&record))
            .map(|(id, _)| id.clone()).collect();
        for id in &changed { leases.remove(id, state); }
        if !changed.is_empty() { return Err(failure(409, "verification_account_changed", "领取账号凭据已变化，验证租约已取消")); }
        Ok(())
    }
}

pub(in crate::server::core::plugin_host::wasm) async fn state_invoke(context: &Context, method: &str,
    namespace: &str, key: &str, params: &Value) -> Result<Value, proto::WireError> {
    if context.claim_owner.is_some() || context.verification.as_ref().is_some_and(|call| call.action.claim()) {
        context.validate_claim_account().await?;
    }
    let mut all = state::conversations().lock().map_err(|_| unavailable())?;
    let ConversationState { state, leases } = &mut *all;
    leases.prune(state);
    if let Some(call) = context.verification.as_ref().filter(|call| call.action.claim()) {
        if leases.entries.values().any(|entry| entry.binding.provider == call.provider && entry.binding.account == call.account
            && (entry.namespace != namespace || entry.slot.key != key || params["schema"].as_u64() != Some(entry.slot.schema as u64))) {
            return Err(failure(403, "forbidden", "领取交互不能访问租约之外的状态槽"));
        }
    }
    if method == "state.get" { return state.invoke(method, namespace, key, params, 86_400_000, 32); }
    // 先判断版本，确保发布/分配/回传互相竞争时返回冲突而不是提前耗用租约。
    let expected = params["revision"].as_u64().ok_or_else(|| failure(400, "bad_params", "状态写入必须携带读取版本"))?;
    let current = state.invoke("state.get", namespace, key, params, 86_400_000, 32)?;
    if Some(expected) != current["revision"].as_u64() {
        return Ok(json!({ "updated": false, "revision": current["revision"] }));
    }
    let before = state.value(namespace, key).cloned().unwrap_or(Value::Null);
    let transition = transaction::prepare(leases, context, namespace, key, params, &before)?;
    let mut params = params.clone();
    if let Some(value) = &transition.value { params["value"] = value.clone(); }
    if leases.entries.values().any(|entry| entry.namespace == namespace && entry.slot.key == key) {
        params["ttl_ms"] = json!(params["ttl_ms"].as_i64().unwrap_or(120_000).clamp(1000, 120_000));
    }
    if let Some(id) = leases.entries.iter().find(|(_, entry)| entry.namespace == namespace && entry.slot.key == key && !entry.alive())
        .map(|(id, _)| id.clone()) {
        leases.remove(&id, state);
        let current = state.invoke("state.get", namespace, key, &params, 86_400_000, 32)?;
        return Ok(json!({ "updated": false, "revision": current["revision"] }));
    }
    let result = state.invoke(method, namespace, key, &params, 86_400_000, 32)?;
    if result["updated"] == true { transition.commit(leases, state, context); }
    Ok(result)
}

pub(in crate::server::core::plugin_host::wasm) fn project(call: &VerificationCall, result: Value) -> Result<Value, proto::WireError> {
    if !call.action.claim() { return Ok(result); }
    if call.action == Action::ClaimComplete {
        if call.committed.load(Ordering::Acquire) { return Ok(json!({ "accepted": true })); }
        let mut all = state::conversations().lock().map_err(|_| unavailable())?;
        let ConversationState { state, leases } = &mut *all;
        leases.prune(state);
        let entry = call.payload["requestId"].as_str().and_then(|id| leases.entries.get(id));
        if entry.is_some_and(|entry| !entry.binding.caller(call) || entry.client.as_deref() != call.payload["clientId"].as_str()
            || call.payload["planId"] != entry.plan || call.payload["configId"] != entry.config) {
            return Err(failure(403, "forbidden", "领取验证结果与租约绑定不一致"));
        }
        let reason = match entry {
            Some(entry) if entry.phase == Phase::Submitted => "already_submitted",
            Some(_) => return Err(failure(502, "plugin_protocol_error", "领取验证码没有写入有效回执")),
            None => "expired",
        };
        return Ok(json!({ "accepted": false, "reason": reason }));
    }
    let mut all = state::conversations().lock().map_err(|_| unavailable())?;
    let ConversationState { state, leases } = &mut *all;
    leases.prune(state);
    let task = result["task"]["requestId"].as_str().and_then(|id| leases.entries.get(id).map(|entry| (id, entry)))
        .filter(|(_, entry)| entry.binding.caller(call) && entry.phase == Phase::Waiting && entry.client.as_deref() == call.payload["clientId"].as_str())
        .and_then(|(id, entry)| state.value(&entry.namespace, &entry.slot.key).map(|value| (id, entry, &value["request"])))
        .filter(|(id, _, request)| request["requestId"] == *id)
        .map(|(_, entry, request)| json!({ "requestId": request["requestId"], "provider": entry.binding.provider,
            "accountId": entry.binding.account, "planId": entry.plan, "planName": request["planName"],
            "expiresAt": entry.expires, "remainingMs": entry.deadline.saturating_duration_since(Instant::now()).as_millis() as u64,
            "config": request["config"] })).unwrap_or(Value::Null);
    Ok(json!({ "supported": true, "pollMs": 5000, "active": !task.is_null(), "task": task }))
}
