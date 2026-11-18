//! 与领取状态 CAS 同锁提交的状态转换。检查失败或版本冲突均不能占用/消费租约。
use super::*;

pub(super) struct Transition {
    pub value: Option<Value>,
    change: Change,
}

enum Change {
    None,
    Publish(String, Value),
    Assign(String, String),
    Submit(String, bool),
    Consume(String),
}

impl Transition {
    fn none() -> Self { Self { value: None, change: Change::None } }
    pub fn commit(self, leases: &mut Registry, state: &mut Transient, context: &Context) {
        match self.change {
            Change::None => {}
            Change::Publish(id, meta) => { if let Some(entry) = leases.entries.get_mut(&id) { entry.meta = Some(meta); } }
            Change::Assign(id, client) => { if let Some(entry) = leases.entries.get_mut(&id) { entry.client = Some(client); } }
            Change::Submit(id, terminal) => {
                if terminal { leases.remove(&id, state); }
                else if let Some(entry) = leases.entries.get_mut(&id) { entry.phase = Phase::Submitted; }
                if let Some(call) = &context.verification { call.committed.store(true, Ordering::Release); }
            }
            Change::Consume(id) => { leases.remove(&id, state); }
        }
    }
}

fn denied() -> proto::WireError { failure(403, "forbidden", "领取验证状态与宿主租约不匹配") }

fn valid_state(value: &Value, now: i64) -> Result<(), proto::WireError> {
    let fields = value.as_object().ok_or_else(denied)?;
    if fields.keys().any(|key| !["binding", "clients", "request", "consumed"].contains(&key.as_str()))
        || value.to_string().len() > 64 * 1024 { return Err(denied()); }
    text(value, "binding", 256)?;
    let clients = value["clients"].as_object().filter(|clients| clients.len() <= 32).ok_or_else(denied)?;
    for (id, client) in clients {
        if id.trim().is_empty() || id.len() > 128 || id.chars().any(char::is_control)
            || !client["ready"].is_boolean() || client.as_object().is_none_or(|fields| fields.len() != 2)
            || client["seenAt"].as_i64().is_none_or(|at| at <= 0 || at > now || now.saturating_sub(at) >= 40_000) {
            return Err(denied());
        }
    }
    let consumed = value["consumed"].as_array().filter(|items| items.len() <= 128).ok_or_else(denied)?;
    for item in consumed {
        if item.as_object().is_none_or(|fields| fields.len() != 2)
            || item["hash"].as_str().is_none_or(|hash| hash.len() != 32 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
            || item["at"].as_i64().is_none_or(|at| at <= 0 || at > now || now.saturating_sub(at) >= 120_000) {
            return Err(denied());
        }
    }
    if !value["request"].is_null() && !value["request"].is_object() { return Err(denied()); }
    Ok(())
}

fn meta(entry: &Entry, id: &str, request: &Value) -> Result<Value, proto::WireError> {
    if request.as_object().is_none_or(|fields| fields.len() != 10 || fields.keys().any(|key|
        !["requestId", "provider", "accountId", "planId", "planName", "configId", "config", "expiresAt", "clientId", "reply"].contains(&key.as_str())))
        || request["requestId"] != id || request["provider"] != entry.binding.provider || request["accountId"] != entry.binding.account
        || request["planId"] != entry.plan || request["configId"] != entry.config || request["expiresAt"].as_i64() != Some(entry.expires) {
        return Err(denied());
    }
    if request["planName"].as_str().is_none_or(|name| name.len() > 512 || name.chars().any(char::is_control)) { return Err(denied()); }
    let config = &request["config"];
    if config.as_object().is_none_or(|fields| fields.len() != 9 || fields.keys().any(|key|
        !["enabled", "engine", "provider", "accountId", "configId", "prefix", "sceneId", "region", "skipModelRequest"].contains(&key.as_str())))
        || config["enabled"] != true || config["engine"] != "aliyun" || config["provider"] != entry.binding.provider
        || config["accountId"] != entry.binding.account || config["configId"] != entry.config || !config["skipModelRequest"].is_boolean() {
        return Err(denied());
    }
    for key in ["prefix", "sceneId", "region"] { text(config, key, 256)?; }
    let mut result = request.clone();
    result.as_object_mut().ok_or_else(denied)?.remove("clientId");
    result.as_object_mut().ok_or_else(denied)?.remove("reply");
    Ok(result)
}

fn prune_public(before: &Value, now: i64) -> Value {
    if !before.is_object() { return json!({ "binding": null, "clients": {}, "request": null, "consumed": [] }); }
    let mut value = before.clone();
    if let Some(clients) = value["clients"].as_object_mut() {
        clients.retain(|_, client| client["seenAt"].as_i64().is_some_and(|at| at > 0 && at <= now && now.saturating_sub(at) < 40_000));
    }
    if let Some(used) = value["consumed"].as_array_mut() {
        used.retain(|item| item["at"].as_i64().is_some_and(|at| at > 0 && at <= now && now.saturating_sub(at) < 120_000));
    }
    value
}

fn poll_fields(call: &VerificationCall, before: &Value, next: &Value, bound: bool, now: i64) -> Result<(), proto::WireError> {
    let mut expected = prune_public(before, now);
    if !before.is_object() || !bound && expected["binding"] != next["binding"] {
        expected = json!({ "binding": next["binding"], "clients": {}, "request": null, "consumed": [] });
    }
    let client = call.payload["clientId"].as_str().ok_or_else(denied)?;
    if !expected["clients"].is_object() { expected["clients"] = json!({}); }
    let supplied = &next["clients"][client];
    if supplied["ready"] != call.payload["ready"] { return Err(denied()); }
    expected["clients"][client] = supplied.clone();
    if expected["binding"] != next["binding"] || expected["clients"] != next["clients"]
        || expected["consumed"] != next["consumed"] { return Err(denied()); }
    Ok(())
}

pub(super) fn prepare(leases: &Registry, context: &Context, namespace: &str, key: &str,
    params: &Value, before: &Value) -> Result<Transition, proto::WireError> {
    let next = &params["value"];
    let now = crate::server::logging::now_ms();
    let call = context.verification.as_ref().filter(|call| call.action.claim());
    let slot = leases.entries.iter().find(|(_, entry)| entry.namespace == namespace && entry.slot.key == key);
    if context.verification.as_ref().is_some_and(|call| !call.action.claim()) {
        if slot.is_none() { return Ok(Transition::none()); }
        return Err(denied());
    }
    if call.is_none() && slot.is_none() && !(context.claim_owner.is_some() && key == "claim-verification") {
        return Ok(Transition::none());
    }
    if let Some(call) = call {
        if params["ttl_ms"].as_i64().is_none_or(|ttl| !(1000..=120_000).contains(&ttl)) { return Err(denied()); }
        // 执行体/凭据已在锁外复核；锁内只处理已绑定字段及原子状态。
        // 一旦本账号有活租约，轮询/回传不能借其权限写另一个状态槽。
        if leases.entries.values().any(|entry| entry.binding.provider == call.provider && entry.binding.account == call.account
            && (entry.namespace != namespace || entry.slot.key != key)) { return Err(denied()); }
        if params["key"] != key || params["scope"] != "conversation" { return Err(denied()); }
    }
    valid_state(next, now)?;
    let Some((id, entry)) = slot else {
        if !next["request"].is_null() { return Err(denied()); }
        if let Some(call) = call {
            if call.action == Action::ClaimPoll { poll_fields(call, before, next, false, now)?; }
            else {
                let mut expected = prune_public(before, now);
                if !expected.is_object() || expected["binding"] != next["binding"] {
                    expected = json!({ "binding": next["binding"], "clients": {}, "request": null, "consumed": [] });
                }
                expected["request"] = Value::Null;
                if expected != *next { return Err(denied()); }
            }
        }
        return Ok(Transition::none());
    };
    if !entry.alive() || !entry.access(context) || params["schema"].as_u64() != Some(entry.slot.schema as u64) { return Err(denied()); }
    let old = &before["request"];
    let request = &next["request"];
    let owner = context.claim_owner.as_ref().is_some_and(|owner| owner.life.id == entry.owner_id);
    if owner {
        if request.is_null() {
            if old["requestId"] != *id { return Ok(Transition::none()); }
            let expected = prune_public(before, now);
            if next["binding"] != expected["binding"] || next["clients"] != expected["clients"]
                || next["consumed"] != expected["consumed"] { return Err(denied()); }
            return Ok(Transition { value: None, change: Change::Consume(id.clone()) });
        }
        let metadata = meta(entry, id, request)?;
        if let Some(saved) = &entry.meta {
            let expected = prune_public(before, now);
            if *saved != metadata || old != request || before["binding"] != next["binding"]
                || expected["clients"] != next["clients"] || expected["consumed"] != next["consumed"] { return Err(denied()); }
            return Ok(Transition::none());
        }
        if !old.is_null() || !request["clientId"].is_null() || !request["reply"].is_null() { return Err(denied()); }
        let expected = prune_public(before, now);
        if before["binding"] != next["binding"] || expected["clients"] != next["clients"] || expected["consumed"] != next["consumed"] {
            return Err(denied());
        }
        return Ok(Transition { value: None, change: Change::Publish(id.clone(), metadata) });
    }
    let call = call.ok_or_else(denied)?;
    if old.is_null() && entry.meta.is_none() {
        if !request.is_null() || call.action != Action::ClaimPoll { return Err(denied()); }
        poll_fields(call, before, next, true, now)?;
        return Ok(Transition::none());
    }
    if entry.meta.as_ref() != Some(&meta(entry, id, old)?) || old["clientId"].as_str() != entry.client.as_deref() {
        return Err(denied());
    }
    if call.action == Action::ClaimPoll {
        poll_fields(call, before, next, true, now)?;
        if request.is_null() || entry.meta.as_ref() != Some(&meta(entry, id, request)?) || old["reply"] != request["reply"] { return Err(denied()); }
        if old["clientId"] == request["clientId"] { return Ok(Transition::none()); }
        if entry.phase != Phase::Waiting || entry.client.is_some() || call.payload["ready"] != true
            || request["clientId"] != call.payload["clientId"] { return Err(denied()); }
        return Ok(Transition { value: None, change: Change::Assign(id.clone(), text(&call.payload, "clientId", 128)?.to_string()) });
    }
    if call.action == Action::ClaimComplete && call.payload["requestId"] != *id {
        if *next != prune_public(before, now) { return Err(denied()); }
        return Ok(Transition::none());
    }
    if call.action == Action::ClaimComplete && (call.payload["planId"] != entry.plan
        || call.payload["configId"] != entry.config || entry.client.as_deref() != call.payload["clientId"].as_str()) { return Err(denied()); }
    if call.action == Action::ClaimComplete && entry.phase == Phase::Submitted {
        if *next != prune_public(before, now) { return Err(denied()); }
        return Ok(Transition::none());
    }
    if call.action != Action::ClaimComplete || entry.phase != Phase::Waiting || call.committed.load(Ordering::Acquire)
        || call.payload["requestId"] != *id || call.payload["planId"] != entry.plan || call.payload["configId"] != entry.config
        || entry.client.as_deref() != call.payload["clientId"].as_str() || entry.client.is_none() || !old["reply"].is_null()
        || request.is_null() || entry.meta.as_ref() != Some(&meta(entry, id, request)?)
        || old["clientId"] != request["clientId"] { return Err(denied()); }
    let expected = prune_public(before, now);
    if next["binding"] != before["binding"] || next["clients"] != expected["clients"] { return Err(denied()); }
    let verified = call.payload["status"] == "verified";
    let proof = if verified { call.payload["param"].as_str().ok_or_else(denied)? } else { "" };
    if request["reply"] != json!({ "status": call.payload["status"], "param": proof }) { return Err(denied()); }
    let mut consumed = expected["consumed"].as_array().cloned().ok_or_else(denied)?;
    if verified {
        use sha2::{Digest, Sha256};
        let hash = format!("{:x}", Sha256::digest(proof.as_bytes()))[..32].to_string();
        if consumed.iter().any(|item| item["hash"] == hash) { return Err(failure(428, "verification_replayed", "这份领取验证已经使用，请重新验证")); }
        let appended = next["consumed"].as_array().and_then(|items| items.last()).filter(|item| item["hash"] == hash).ok_or_else(denied)?;
        consumed.push(appended.clone());
    }
    if next["consumed"] != json!(consumed) { return Err(denied()); }
    let mut value = next.clone();
    if !verified { value["request"] = Value::Null; }
    Ok(Transition { value: Some(value), change: Change::Submit(id.clone(), !verified) })
}
