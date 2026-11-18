//! 主程序签发的账号验证调用；仅固定模型提交与领取轮询/回传可进入短通道。
pub(super) mod lease;
mod native;
pub(in crate::server::core::plugin_host) use native::call as native_claim;

use std::sync::{Arc, OnceLock, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::OwnedSemaphorePermit;
use tokio::time::Instant;

use super::context::{Context, Shared};
use super::super::Host;
use crate::server::core::account_store::AccountStore;

pub(crate) const CAPABILITY: &str = "verification.account.v1";
pub(crate) const CLAIM_CAPABILITY: &str = "verification.claim.v1";
pub(crate) const MAX_MESSAGE: usize = 256 * 1024;
pub(super) const TIMEOUT: Duration = Duration::from_secs(2);
pub(super) const FUEL: u64 = 50_000_000;
const MAX_CALLS: u32 = 128;
const MAX_TTL: i64 = 120_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action { ModelPush, ClaimPoll, ClaimComplete }

impl Action {
    fn name(self) -> &'static str {
        match self { Self::ModelPush => "captcha.push", Self::ClaimPoll => "claim.verification.poll", Self::ClaimComplete => "claim.verification.complete" }
    }
    fn claim(self) -> bool { self != Self::ModelPush }
}

pub(crate) fn guarded_action(action: &str) -> bool {
    matches!(action, "captcha.push" | "claim.verification.poll" | "claim.verification.complete")
}

pub(super) struct Permits {
    _local: OwnedSemaphorePermit,
    _global: crate::server::core::runtime_capacity::Ticket,
}

fn failure(status: u16, code: &str, message: &str) -> proto::WireError {
    proto::WireError::with_code(message, format!("provider_http:{}", json!({ "status": status, "code": code })))
}

pub(super) fn unavailable() -> proto::WireError {
    failure(503, "plugin_unavailable", "验证码执行体当前不可用，请稍后重试")
}

pub(super) fn slots(shared: &Shared) -> Result<Arc<Permits>, proto::WireError> {
    let busy = || failure(429, "verification_busy", "验证码提交通道正忙，请稍后重试");
    let local = shared.verification_slots.clone().try_acquire_owned().map_err(|_| busy())?;
    let global = crate::server::core::runtime_capacity::try_acquire(
        crate::server::core::runtime_capacity::Class::Interactive, &shared.options.provider).map_err(|_| busy())?;
    Ok(Arc::new(Permits { _local: local, _global: global }))
}

/// 只标识实际安装的原生执行体；脚本、Wasm、占位适配器都不是旧全局票据池。
pub(crate) fn legacy_native(provider: &str) -> bool {
    super::super::serves_provider(provider) && super::super::host_for_provider(provider)
        .is_some_and(|host| !host.options.script && host.options.wasm.is_none())
}

/// 空账号旧提交不能绕过已安装但停用或加载失败的 WASM 归属。
pub(crate) fn legacy_global_allowed() -> bool {
    let registry = super::super::PackRegistry::load();
    let owners = super::super::ownership::owners(&registry);
    !crate::server::core::providers::zcode::region::Region::ALL.into_iter().any(|region| {
        let owner = owners.get(region.provider_id());
        registry.packs.iter().any(|pack| owner == Some(&pack.id) && pack.entry.ends_with(".wasm"))
    })
}

/// 字段私有且不实现反序列化；只有本模块能构造，插件不能凭 JSON 打开短通道。
#[derive(Clone)]
pub(crate) struct VerificationCall {
    host: Weak<Host>,
    shared: Weak<Shared>,
    store: AccountStore,
    generation: u64,
    provider: String,
    account: String,
    identity: String,
    credential_digest: [u8; 32],
    snapshot: Arc<Value>,
    action: Action,
    committed: Arc<AtomicBool>,
    state_slot: Arc<OnceLock<(String, String, u32)>>,
    payload: Value,
    deadline: Instant,
    // 本地快照读取即使超时后仍在退出，也继续占住通道，不堆积无界阻塞任务。
    permits: Arc<Permits>,
}

fn digest(secret: &Value) -> Result<[u8; 32], proto::WireError> {
    let bytes = serde_json::to_vec(secret).map_err(|_| unavailable())?;
    Ok(Sha256::digest(bytes).into())
}

fn text<'a>(value: &'a Value, key: &str, limit: usize) -> Result<&'a str, proto::WireError> {
    value[key].as_str().filter(|value| !value.trim().is_empty() && value.len() <= limit
        && !value.chars().any(char::is_control))
        .ok_or_else(|| failure(400, "bad_params", "验证码提交缺少有效的账号或配置标识"))
}

fn check_payload(action: Action, payload: &Value, provider: &str, account: &str) -> Result<(), proto::WireError> {
    let fields = payload.as_object().ok_or_else(|| failure(400, "bad_params", "验证码提交必须是 JSON 对象"))?;
    let allowed: &[&str] = match action {
        Action::ModelPush => &["provider", "accountId", "configId", "tokens"],
        Action::ClaimPoll => &["provider", "accountId", "clientId", "ready"],
        Action::ClaimComplete => &["provider", "accountId", "clientId", "requestId", "planId", "configId", "status", "param"],
    };
    if fields.keys().any(|key| !allowed.contains(&key.as_str()))
        || serde_json::to_vec(payload).map_err(|_| unavailable())?.len() > MAX_MESSAGE {
        return Err(failure(400, "bad_params", "验证码提交字段无效或超过大小限制"));
    }
    if text(payload, "provider", 128)? != provider || text(payload, "accountId", 512)? != account {
        return Err(failure(403, "forbidden", "验证码提交与当前账号归属不一致"));
    }
    if action.claim() {
        text(payload, "clientId", 128)?;
        if action == Action::ClaimPoll {
            if !payload["ready"].is_boolean() { return Err(failure(400, "bad_params", "领取验证轮询缺少就绪状态")); }
        } else {
            text(payload, "requestId", 128)?;
            text(payload, "planId", 256)?;
            text(payload, "configId", 256)?;
            let status = text(payload, "status", 16)?;
            if !["verified", "cancelled", "timeout", "failed"].contains(&status)
                || status == "verified" && payload["param"].as_str().is_none_or(|param|
                    param.trim().is_empty() || param.len() > 16_384 || param.contains(['\r', '\n', '\0']))
                || status != "verified" && payload.get("param").is_some_and(|value| value.as_str() != Some("")) {
                return Err(failure(400, "bad_params", "领取验证回传状态或凭据格式无效"));
            }
        }
        return Ok(());
    }
    text(payload, "configId", 256)?;
    let tokens = payload["tokens"].as_array().filter(|tokens| !tokens.is_empty() && tokens.len() <= 12)
        .ok_or_else(|| failure(400, "bad_params", "验证码数量必须为 1 到 12 条"))?;
    for item in tokens {
        if item.as_object().is_none_or(|fields| fields.len() != 1 || !fields.contains_key("param"))
            || item["param"].as_str().is_none_or(|param| param.trim().is_empty() || param.len() > 16_384
                || param.contains(['\r', '\n', '\0'])) {
            return Err(failure(400, "bad_params", "验证码凭据格式无效"));
        }
    }
    Ok(())
}

async fn snapshot(store: AccountStore, provider: String, account: String, permits: Arc<Permits>) -> Result<Value, proto::WireError> {
    crate::server::core::blocking::run(move || {
        let _permits = permits;
        store.plugin_verification_snapshot(&provider, &account)
    }).await.map_err(|_| unavailable())?
        .ok_or_else(|| failure(409, "verification_account_changed", "验证码账号已删除或归属已变化"))
}

impl VerificationCall {
    pub(super) async fn for_action(host: &Arc<Host>, shared: &Arc<Shared>, store: &AccountStore,
        provider: &str, account: &str, action: &str, payload: &Value, deadline: Instant, permits: Arc<Permits>) -> Result<Self, proto::WireError> {
        let action = match action {
            "captcha.push" => Action::ModelPush,
            "claim.verification.poll" => Action::ClaimPoll,
            "claim.verification.complete" => Action::ClaimComplete,
            _ => return Err(failure(403, "forbidden", "该操作不能使用验证码交互通道")),
        };
        if provider != shared.options.provider || !shared.options.allows(CAPABILITY)
            || action.claim() && !shared.options.allows(CLAIM_CAPABILITY) {
            return Err(failure(403, "forbidden", "插件未声明所需账号验证码交互能力"));
        }
        check_payload(action, payload, provider, account)?;
        if !host.ready() || !host.callbacks.provider_allowed(provider) || !super::super::serves_provider(provider)
            || super::super::registry::is_switching(provider)
            || super::super::host_for_provider(provider).is_none_or(|current| !Arc::ptr_eq(&current, host)) {
            return Err(unavailable());
        }
        let generation = host.generation.load(Ordering::Acquire);
        let record = snapshot(store.clone(), provider.to_string(), account.to_string(), permits.clone()).await?;
        if record["public"]["enabled"] == false && !action.claim() {
            return Err(failure(409, "verification_account_changed", "模型验证码账号已停用"));
        }
        // 沿用普通账号上下文的旧身份形态；未声明 identity.v1 的历史账号允许空身份。
        let identity = record["identity"].as_str().unwrap_or(account);
        if identity.len() > 512 || identity.chars().any(char::is_control) { return Err(unavailable()); }
        let identity = identity.to_string();
        if serde_json::to_vec(&record).map_err(|_| unavailable())?.len() > MAX_MESSAGE {
            return Err(failure(413, "plugin_resource_limit", "验证码账号快照超过大小限制"));
        }
        let call = Self { host: Arc::downgrade(host), shared: Arc::downgrade(shared), store: store.clone(),
            generation, provider: provider.to_string(), account: account.to_string(), identity,
            credential_digest: digest(&record["secret"])?, snapshot: Arc::new(record), action,
            committed: Arc::new(AtomicBool::new(false)), state_slot: Arc::new(OnceLock::new()),
            payload: payload.clone(), deadline, permits };
        call.check_host()?;
        Ok(call)
    }

    pub(super) fn params(&self) -> Value {
        json!({ "provider": self.provider, "account": self.account, "action": self.action.name(), "payload": self.payload })
    }

    pub(super) fn remaining(&self) -> Duration { self.deadline.saturating_duration_since(Instant::now()) }

    fn check_host(&self) -> Result<(), proto::WireError> {
        if Instant::now() >= self.deadline { return Err(failure(504, "timeout", "验证码提交超时，请重新读取当前账号状态")); }
        let host = self.host.upgrade().ok_or_else(unavailable)?;
        let shared = self.shared.upgrade().ok_or_else(unavailable)?;
        if !host.ready() || host.generation.load(Ordering::Acquire) != self.generation || shared.cancel.is_cancelled()
            || !shared.options.allows(CAPABILITY) || !host.callbacks.provider_allowed(&self.provider)
            || super::super::registry::is_switching(&self.provider) || host.activity.retired()
            || !super::super::serves_provider(&self.provider)
            || super::super::host_for_provider(&self.provider).is_none_or(|current| !Arc::ptr_eq(&current, &host)) {
            return Err(unavailable());
        }
        Ok(())
    }

    pub(super) async fn validate(&self) -> Result<(), proto::WireError> {
        self.check_host()?;
        let record = snapshot(self.store.clone(), self.provider.clone(), self.account.clone(), self.permits.clone()).await?;
        self.check_host()?;
        if record["id"].as_str() != Some(self.account.as_str()) || record["provider"].as_str() != Some(self.provider.as_str())
            || record["public"]["enabled"] == false && !self.action.claim()
            || record["identity"] != self.snapshot["identity"]
            || record["identity"].as_str().unwrap_or(&self.account) != self.identity.as_str()
            || digest(&record["secret"])? != self.credential_digest {
            return Err(failure(409, "verification_account_changed", "账号凭据已变化，请重新获取验证码配置"));
        }
        Ok(())
    }

    pub(super) fn check_context(&self, context: &Context) -> Result<(), proto::WireError> {
        self.check_host()?;
        if context.account != self.account || context.identity != self.identity || context.shared.options.provider != self.provider
            || self.shared.upgrade().is_none_or(|shared| !Arc::ptr_eq(&shared, &context.shared)) {
            return Err(failure(403, "forbidden", "验证码调用不属于当前账号上下文"));
        }
        Ok(())
    }
}

impl Context {
    pub(super) fn bind_verification(&mut self, call: VerificationCall) {
        self.account = call.account.clone();
        self.identity = call.identity.clone();
        self.proxy = Value::Null;
        self.proxy_selected = true;
        self.system_proxy_when_unset = false;
        self.verification = Some(call);
    }

    pub(super) fn message_limit(&self) -> usize {
        if self.verification.is_some() { MAX_MESSAGE } else { super::MAX_MESSAGE }
    }

    /// 一个入口统一约束短通道，返回 Some 时已完成处理（账号仅返回绑定快照）。
    pub(super) async fn verification_guard(&self, method: &str, params: &Value) -> Result<Option<Value>, proto::WireError> {
        let Some(call) = &self.verification else { return Ok(None); };
        call.check_context(self)?;
        if self.calls > MAX_CALLS || serde_json::to_vec(params).map_err(|_| unavailable())?.len() > MAX_MESSAGE {
            return Err(failure(429, "plugin_resource_limit", "验证码交互超过调用或消息限制"));
        }
        match method {
            "account.get" => {
                self.permission("accounts.cas.v1")?;
                if params.as_object().is_none_or(|fields| !fields.is_empty()) {
                    return Err(failure(400, "bad_params", "验证码账号读取不接受自选参数"));
                }
                let mut record = (*call.snapshot).clone();
                record["accountMaintenanceCall"] = json!(call.action.claim());
                return Ok(Some(record));
            }
            "state.get" | "state.compare_exchange" => {
                if params["scope"] != "conversation" {
                    return Err(failure(403, "forbidden", "验证码交互只允许当前身份的临时会话状态"));
                }
                if method == "state.compare_exchange" && params["ttl_ms"].as_i64().is_none_or(|ttl| !(1000..=MAX_TTL).contains(&ttl)) {
                    return Err(failure(400, "bad_params", "验证码临时状态有效期不得超过两分钟"));
                }
                if call.action.claim() {
                    let slot = (text(params, "conversation", 256)?.to_string(), text(params, "key", 128)?.to_string(),
                        params["schema"].as_u64().and_then(|schema| u32::try_from(schema).ok()).filter(|schema| *schema > 0)
                            .ok_or_else(|| failure(400, "bad_params", "领取状态格式版本无效"))?);
                    if call.state_slot.get_or_init(|| slot.clone()) != &slot {
                        return Err(failure(403, "forbidden", "一次领取交互只能访问一个已绑定状态槽"));
                    }
                }
                if method == "state.compare_exchange" && !call.action.claim() { call.validate().await?; }
            }
            "verification.lease.check" if call.action.claim() => {}
            "settings.get" | "clock.now" => {}
            _ => return Err(failure(403, "forbidden", "验证码交互通道不允许网络、等待或账号写入")),
        }
        Ok(None)
    }
}
