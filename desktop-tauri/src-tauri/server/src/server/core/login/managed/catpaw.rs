//! CatPaw Passport 的固定宿主登录链：本轮出口、双通道一次消费、真实身份回执。
//! 协议来自本仓原生 CatPaw login.rs；公开 dsh-catpaw 固定提交仅作交叉核对。
//! 不提供任意 URL/头/轮询参数，任何错误均不重放已取得的 token。
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::time::{sleep_until, Instant};
use tokio_util::sync::CancellationToken;

use super::{loopback::Lease, unavailable, Call, LoginTaskHandle, RuntimeBinding, Session, State};
use crate::server::{
    core::{egress::managed::FrozenHttpClient, plugin_host::wasm::managed_policy::{CATPAW_LOGIN_ENTRY, CATPAW_PROFILE}},
    errors::GatewayError,
};

const LOGIN_CONFIG: &str = "https://catx.nocode.cn/api/gateway/passport/login-config";
const POLL_TOKEN: &str = "https://catx.nocode.cn/api/gateway/passport/poll-token";
const CURRENT_USER: &str = "https://catx.nocode.cn/api/gateway/passport/current-user";
const BODY_LIMIT: usize = 64 * 1024;
const MAX_POLLS: u16 = 300;

#[derive(Default)]
pub(super) struct SessionState {
    preparing_claimed: bool,
    client: Option<FrozenHttpClient>,
    witness: Option<IdentityWitness>,
}

struct IdentityWitness {
    call: String,
    step: String,
    flow: String,
    runtime: Arc<RuntimeBinding>,
    digest: [u8; 32],
    uid: String,
    login_name: String,
    identity: String,
}

/// state/sid 只由宿主随机生成，租约发布后不可修改；token 从不放在此轻量状态中。
pub(super) struct Passport {
    pub(super) state: String,
    sid: String,
    authorization_url: String,
    polls: u16,
    next_poll: Instant,
    pub(super) poll_cancel: CancellationToken,
}

pub(super) struct Opened {
    pub(super) passport: Passport,
    client: FrozenHttpClient,
}

fn protocol() -> GatewayError { GatewayError::with_status(502, "CatPaw 登录服务响应无效，未保存账号；请重新发起") }
fn transport() -> GatewayError { GatewayError::with_status(502, "CatPaw 登录请求失败，未切换出口或重试；请重新发起") }
fn expired() -> GatewayError { GatewayError::with_status(504, "CatPaw 本次登录请求超时，未重放登录材料；请重新发起") }

pub(super) fn applies(provider: &str, method: u64) -> bool { provider == "catpaw" && method == 0 }

fn random_hex() -> Result<String, GatewayError> {
    let mut bytes = [0; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| GatewayError::with_status(500, "无法生成 CatPaw 登录标识"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(super) fn token_valid(token: &str) -> bool {
    !token.is_empty() && token.len() <= 8192 && token.bytes().all(|byte| matches!(byte, 0x21..=0x7e) && byte != b';')
}

fn runtime_live(runtime: &RuntimeBinding) -> bool { !runtime.stopped.is_cancelled() && (runtime.alive)() }

fn runtime(call: &Call) -> Result<Arc<RuntimeBinding>, GatewayError> {
    call.validate(call.provider())?;
    call.session.state.lock().map_err(|_| unavailable())?.runtime.clone().ok_or_else(unavailable)
}

/// 只借原 LoginTaskContext 的完整选择，不读取新代理配置或从兼容 JSON 反造来源。
pub(super) async fn prepare_open(call: &Call, callback: &str) -> Result<Opened, GatewayError> {
    if !applies(call.provider(), call.method_index()) || !call.is_start() { return Err(unavailable()); }
    let runtime = runtime(call)?;
    let task = call.task.upgrade().ok_or_else(unavailable)?;
    let selected = task.context.as_ref().and_then(|context| context.proxy.as_ref())
        .ok_or_else(|| GatewayError::with_status(400, "请明确选择本次 CatPaw 登录的直连或代理出口"))?.selected.clone();
    {
        let mut state = call.session.state.lock().map_err(|_| unavailable())?;
        if !call.session.valid() || state.catpaw.preparing_claimed || !state.leases.is_empty()
            || state.active.as_deref() != Some(call.id()) { return Err(unavailable()); }
        state.catpaw.preparing_claimed = true;
    }
    let deadline = call.session.deadline.min(Instant::now() + Duration::from_secs(10));
    let client = tokio::select! {
        biased;
        _ = call.ended() => return Err(unavailable()),
        _ = runtime.stopped.cancelled() => return Err(unavailable()),
        _ = sleep_until(deadline) => return Err(expired()),
        result = FrozenHttpClient::from_path(selected) => result.map_err(|_| transport())?,
    };
    call.validate(call.provider())?;
    let value = request(&call.session, &runtime, &client, LOGIN_CONFIG, None, deadline, None).await?;
    call.validate(call.provider())?;
    let entry = value["data"]["loginEntryUrl"].as_str().ok_or_else(protocol)?;
    // 不跟随上游任意入口、额外 query、userinfo、fragment 或降级 HTTP。
    if entry != CATPAW_LOGIN_ENTRY { return Err(GatewayError::with_status(403, "CatPaw 返回的授权入口尚未获主程序批准")); }
    let state = random_hex()?;
    let sid = random_hex()?;
    let mut url = url::Url::parse(CATPAW_LOGIN_ENTRY).map_err(|_| protocol())?;
    url.query_pairs_mut().append_pair("state", &state).append_pair("redirect", callback).append_pair("sid", &sid);
    Ok(Opened { passport: Passport { state, sid, authorization_url: url.to_string(), polls: 0,
        next_poll: Instant::now() + Duration::from_secs(1), poll_cancel: CancellationToken::new() }, client })
}

impl Opened {
    pub(super) fn response(&self, id: &str, uri: &str, expires_at: i64) -> Value {
        json!({ "leaseId": id, "callbackUrl": uri, "expiresAt": expires_at,
            "state": self.passport.state, "sid": self.passport.sid, "authorizationUrl": self.passport.authorization_url })
    }
    pub(super) fn install(self, state: &mut State) -> Passport {
        state.catpaw.client = Some(self.client);
        self.passport
    }
}

pub(super) fn authorization(lease: &Lease, raw: &str) -> Result<String, GatewayError> {
    let passport = lease.passport.as_ref().ok_or_else(unavailable)?;
    if lease.profile != CATPAW_PROFILE || raw != passport.authorization_url { return Err(unavailable()); }
    Ok(passport.state.clone())
}

/// 此类型只能从严格 POST 解析器产生，普通管理输入不能构造 callback 材料。
pub(super) struct PostToken { token: String, state: String }
impl PostToken {
    pub(super) fn parsed(token: String, state: String) -> Result<Self, GatewayError> {
        if !token_valid(&token) || state.len() != 32 || !state.bytes().all(|byte| byte.is_ascii_hexdigit()) { return Err(protocol()); }
        Ok(Self { token, state })
    }
}

fn ready<'a>(state: &'a State, id: &str, step: &str) -> Result<&'a Lease, GatewayError> {
    let current = state.step.as_ref().ok_or_else(unavailable)?;
    let lease = state.leases.get(id).ok_or_else(unavailable)?;
    if state.runtime.as_ref().is_none_or(|runtime| !runtime_live(runtime)) { return Err(unavailable()); }
    if current.id != step || current.status != "ready" || current.browser.is_none()
        || current.lease.as_deref() != Some(id) || lease.profile != CATPAW_PROFILE
        || lease.closed() || lease.step.as_deref() != Some(step) || lease.passport.is_none() { return Err(unavailable()); }
    Ok(lease)
}

/// 排队即暂占本轮唯一候选：取消当前 poll，后到 token 不得替换或重新送出。
pub(super) fn queue_post(session: &Session, id: &str, input: &PostToken) -> Result<String, GatewayError> {
    let mut state = session.state.lock().map_err(|_| unavailable())?;
    if !session.valid() || state.runtime.as_ref().is_none_or(|runtime| !runtime_live(runtime)) { return Err(unavailable()); }
    let step = state.step.as_ref().ok_or_else(unavailable)?.id.clone();
    let lease = ready(&state, id, &step)?;
    let passport = lease.passport.as_ref().ok_or_else(unavailable)?;
    if lease.queued || input.state != passport.state || lease.official_state.as_deref() != Some(input.state.as_str()) {
        return Err(unavailable());
    }
    if !session.valid() { return Err(unavailable()); }
    let lease = state.leases.get_mut(id).ok_or_else(unavailable)?;
    lease.queued = true;
    if let Some(passport) = &lease.passport { passport.poll_cancel.cancel(); }
    Ok(step)
}

pub(super) fn preflight_live(session: &Session, id: &str) -> bool {
    let Ok(state) = session.state.lock() else { return false; };
    if !session.valid() || state.runtime.as_ref().is_none_or(|runtime| !runtime_live(runtime)) { return false; }
    state.step.as_ref().is_some_and(|step| ready(&state, id, &step.id).is_ok_and(|lease| !lease.queued))
}

fn consume(state: &mut State, id: &str, step: &str) -> Result<(), GatewayError> {
    ready(state, id, step)?;
    let lease = state.leases.get_mut(id).ok_or_else(unavailable)?;
    lease.consumed = true;
    lease.queued = false;
    lease.cancel.cancel();
    lease.official_state.take();
    if let Some(passport) = &lease.passport { passport.poll_cancel.cancel(); }
    state.callback_step = Some(step.to_string());
    let step = state.step.as_mut().ok_or_else(unavailable)?;
    step.status = "processing";
    step.browser = None;
    Ok(())
}

pub(super) fn claim_post(call: &Call, id: &str, expected: &str, input: PostToken) -> Result<Value, GatewayError> {
    call.validate(call.provider())?;
    let mut state = call.session.state.lock().map_err(|_| unavailable())?;
    if !call.session.valid() || state.active.as_deref() != Some(call.id()) || call.expected() != expected { return Err(unavailable()); }
    let lease = ready(&state, id, expected)?;
    if !lease.queued || lease.passport.as_ref().is_none_or(|passport| passport.state != input.state) { return Err(unavailable()); }
    if !call.session.valid() { return Err(unavailable()); }
    consume(&mut state, id, expected)?;
    Ok(json!({ "token": input.token, "state": input.state, "channel": "loopback" }))
}

struct Poll {
    id: String,
    step: String,
    state: String,
    sid: String,
    client: FrozenHttpClient,
    runtime: Arc<RuntimeBinding>,
    interrupted: CancellationToken,
}

fn poll_begin(call: &Call) -> Result<Option<Poll>, GatewayError> {
    call.validate(call.provider())?;
    let mut state = call.session.state.lock().map_err(|_| unavailable())?;
    if !call.session.valid() || state.active.as_deref() != Some(call.id()) { return Err(unavailable()); }
    let step = state.step.as_ref().ok_or_else(unavailable)?;
    let id = step.lease.as_ref().ok_or_else(unavailable)?.clone();
    if step.status != "ready" { return Err(unavailable()); }
    let lease = ready(&state, &id, call.expected())?;
    if lease.queued { return Ok(None); }
    let client = state.catpaw.client.clone().ok_or_else(unavailable)?;
    let runtime = state.runtime.clone().ok_or_else(unavailable)?;
    let lease = state.leases.get_mut(&id).ok_or_else(unavailable)?;
    let passport = lease.passport.as_mut().ok_or_else(unavailable)?;
    if Instant::now() < passport.next_poll { return Ok(None); }
    if passport.polls >= MAX_POLLS { return Err(expired()); }
    passport.polls += 1;
    passport.next_poll = Instant::now() + Duration::from_secs(1);
    Ok(Some(Poll { id, step: call.expected().to_string(), state: passport.state.clone(), sid: passport.sid.clone(),
        client, runtime, interrupted: passport.poll_cancel.clone() }))
}

/// 在原 task.operation 锁下执行；没有嵌套 receive_callback 或第二个后台轮询器。
pub(super) async fn poll(call: &mut Call) -> Result<bool, GatewayError> {
    if !applies(call.provider(), call.method_index()) { return Ok(false); }
    let Some(poll) = poll_begin(call)? else { return Ok(false); };
    let mut url = url::Url::parse(POLL_TOKEN).map_err(|_| protocol())?;
    url.query_pairs_mut().append_pair("sid", &poll.sid);
    let deadline = call.session.deadline.min(Instant::now() + Duration::from_secs(5));
    let result = request(&call.session, &poll.runtime, &poll.client, url.as_str(), None, deadline, Some(&poll.interrupted)).await;
    // 先在锁外解析并释放响应体；错误暂不传播，POST已经抢先时必须丢弃这一路结果。
    let result = result.and_then(|value| {
        let data = value.get("data").ok_or_else(protocol)?;
        if data.is_null() { return Ok(None); }
        data.as_str().filter(|token| token_valid(token)).map(|token| Some(token.to_string())).ok_or_else(protocol)
    });
    call.validate(call.provider())?;
    {
        let mut state = call.session.state.lock().map_err(|_| unavailable())?;
        if !call.session.valid() || state.active.as_deref() != Some(call.id())
            || state.runtime.as_ref().is_none_or(|runtime| !Arc::ptr_eq(runtime, &poll.runtime) || !runtime_live(runtime)) {
            return Err(unavailable());
        }
        let lease = ready(&state, &poll.id, &poll.step)?;
        let passport = lease.passport.as_ref().ok_or_else(unavailable)?;
        if passport.sid != poll.sid || passport.state != poll.state { return Err(unavailable()); }
        if lease.queued { return Ok(false); }
        match &result {
            Ok(Some(_)) => {
                if !call.session.valid() { return Err(unavailable()); }
                consume(&mut state, &poll.id, &poll.step)?;
            }
            Ok(None) => return Ok(false),
            Err(_) => {}
        }
    }
    let token = result?.ok_or_else(protocol)?;
    call.promote_catpaw(json!({ "token": token, "state": poll.state, "channel": "poll" }))?;
    Ok(true)
}

fn processing(call: &Call) -> Result<FrozenHttpClient, GatewayError> {
    call.validate(call.provider())?;
    let state = call.session.state.lock().map_err(|_| unavailable())?;
    if !call.session.valid() || state.active.as_deref() != Some(call.id())
        || state.runtime.as_ref().is_none_or(|runtime| !runtime_live(runtime))
        || state.callback_step.as_deref() != Some(call.expected())
        || state.step.as_ref().is_none_or(|step| step.id != call.expected() || step.status != "processing")
        || state.catpaw.witness.is_some() { return Err(unavailable()); }
    state.catpaw.client.clone().ok_or_else(unavailable)
}

fn user_text(value: Option<&Value>) -> Result<String, GatewayError> {
    let text = match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) if value.is_u64() || value.is_i64() => value.to_string(),
        _ => return Err(protocol()),
    };
    if text.len() > 1024 || text.chars().count() > 256 || text.trim() != text || text.contains(';') || text.chars().any(char::is_control) {
        return Err(protocol());
    }
    Ok(text)
}

/// Winner 独占一次 current-user，成功前 token 对插件不可见；失败不按尾号/sid造身份。
pub(super) async fn prepare_input(call: &Call) -> Result<(), GatewayError> {
    if !applies(call.provider(), call.method_index()) || !call.is_callback() { return Ok(()); }
    let client = processing(call)?;
    let runtime = runtime(call)?;
    let material = call.take_pending_catpaw()?;
    if material.as_object().is_none_or(|fields| fields.len() != 3
        || fields.keys().any(|key| !["token", "state", "channel"].contains(&key.as_str()))) { return Err(protocol()); }
    let token = material["token"].as_str().filter(|token| token_valid(token)).ok_or_else(protocol)?;
    let deadline = call.session.deadline.min(Instant::now() + Duration::from_secs(10));
    let response = request(&call.session, &runtime, &client, CURRENT_USER, Some(token), deadline, None).await?;
    drop(processing(call)?);
    let data = response.get("data").filter(|data| data.is_object()).ok_or_else(protocol)?;
    let uid = user_text(data.get("userId"))?;
    let login_name = user_text(data.get("userName"))?;
    let identity = if uid.is_empty() { &login_name } else { &uid };
    if identity.is_empty() || identity == "desktop-auth" || identity.len() > 512
        || [&uid, &login_name].iter().any(|value| value.contains(token)) { return Err(protocol()); }
    let identity = identity.to_string();
    drop(response);
    let witness = IdentityWitness { call: call.id().to_string(), step: call.expected().to_string(), flow: call.flow().to_string(),
        runtime: runtime.clone(), digest: Sha256::digest(token.as_bytes()).into(),
        uid: uid.clone(), login_name: login_name.clone(), identity };
    let mut material = material;
    material["user"] = json!({ "uid": uid, "loginName": login_name });
    {
        let mut state = call.session.state.lock().map_err(|_| unavailable())?;
        if !call.session.valid() || state.active.as_deref() != Some(call.id())
            || state.runtime.as_ref().is_none_or(|current| !Arc::ptr_eq(current, &runtime) || !runtime_live(current))
            || state.callback_step.as_deref() != Some(call.expected()) || state.catpaw.witness.is_some() { return Err(unavailable()); }
        state.catpaw.witness = Some(witness);
    }
    call.install_catpaw_input(material)?;
    Ok(())
}

pub(super) fn validate_account(call: &Call, account: &agent2api_provider_protocol::NormalizedAccount) -> Result<(), GatewayError> {
    if !applies(call.provider(), call.method_index()) { return Ok(()); }
    if !call.is_callback() || !call.consumed_input() { return Err(unavailable()); }
    let state = call.session.state.lock().map_err(|_| unavailable())?;
    let witness = state.catpaw.witness.as_ref().ok_or_else(unavailable)?;
    let secret = &account.secret;
    let allowed = ["id", "uid", "loginName", "accessToken", "loginType", "tokenTail", "name", "source", "desktop"];
    if secret.as_object().is_none_or(|fields| fields.len() != allowed.len()
        || fields.keys().any(|key| !allowed.contains(&key.as_str()))) { return Err(protocol()); }
    let token = secret["accessToken"].as_str().filter(|token| token_valid(token)).ok_or_else(protocol)?;
    let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    if !call.session.valid() || state.active.as_deref() != Some(call.id()) || witness.call != call.id()
        || witness.step != call.expected() || witness.flow != call.flow()
        || state.runtime.as_ref().is_none_or(|runtime| !Arc::ptr_eq(runtime, &witness.runtime) || !runtime_live(runtime))
        || state.callback_step.as_deref() != Some(witness.step.as_str())
        || state.step.as_ref().is_none_or(|step| step.id != witness.step || step.status != "processing")
        || account.identity != witness.identity || secret["uid"].as_str() != Some(witness.uid.as_str())
        || secret["loginName"].as_str() != Some(witness.login_name.as_str()) || digest != witness.digest
        || secret["id"].as_str() != Some(witness.identity.as_str()) || secret["loginType"] != "passport"
        || secret["source"] != "browser" || secret["desktop"] != false || account.desktop { return Err(protocol()); }
    Ok(())
}

/// 每次请求与完整响应体共用绝对截止；无跳转/自动重试，错误不包含URL、sid或令牌。
async fn request(session: &Session, runtime: &RuntimeBinding, client: &FrozenHttpClient, url: &str,
    token: Option<&str>, deadline: Instant, interrupt: Option<&CancellationToken>) -> Result<Value, GatewayError> {
    if !session.valid() || !runtime_live(runtime) || Instant::now() >= deadline { return Err(unavailable()); }
    let cancelled = async {
        if let Some(interrupt) = interrupt { interrupt.cancelled().await; }
        else { std::future::pending::<()>().await; }
    };
    let operation = async {
        let mut request = client.client().map_err(|_| transport())?.get(url).header("Accept", "application/json");
        if let Some(token) = token { request = request.header("X-Auth-Token", token); }
        let mut response = request.send().await.map_err(|_| transport())?;
        if !response.status().is_success() {
            return Err(GatewayError::with_status(502, format!("CatPaw 登录服务返回 HTTP {}，未保存账号", response.status().as_u16())));
        }
        if response.content_length().is_some_and(|size| size > BODY_LIMIT as u64) { return Err(protocol()); }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| transport())? {
            if bytes.len().saturating_add(chunk.len()) > BODY_LIMIT { return Err(protocol()); }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| protocol())?;
        if !value.is_object() || value["code"].as_i64() != Some(0) || value.get("data").is_none() { return Err(protocol()); }
        Ok(value)
    };
    let result = tokio::select! {
        biased;
        _ = session.cancel.cancelled() => return Err(unavailable()),
        _ = runtime.stopped.cancelled() => return Err(unavailable()),
        _ = cancelled => return Err(unavailable()),
        _ = sleep_until(deadline) => return Err(expired()),
        result = operation => result,
    };
    if !session.valid() || !runtime_live(runtime) { return Err(unavailable()); }
    if Instant::now() >= deadline { return Err(expired()); }
    result
}

/// 已排队POST的所有权跟随等待任务；从未poll或中断也销毁flow，不重发材料。
struct QueuedGuard { task: LoginTaskHandle, complete: bool }
impl QueuedGuard {
    fn task(&self) -> LoginTaskHandle { self.task.clone() }
    fn finish(&mut self) { self.complete = true; }
}
impl Drop for QueuedGuard {
    fn drop(&mut self) {
        if !self.complete { super::fail(&self.task, "CatPaw 登录回调处理被中断，未保存账号；请重新发起"); }
    }
}

pub(super) fn deliver_post(task: LoginTaskHandle, id: String, expected: String, input: PostToken)
    -> impl std::future::Future<Output = ()> + Send + 'static {
    let mut guard = QueuedGuard { task, complete: false };
    async move {
        let task = guard.task();
        let Ok(session) = super::session(&task) else { guard.finish(); return; };
        let _operation = tokio::select! {
            biased;
            _ = session.cancel.cancelled() => { guard.finish(); return; },
            _ = sleep_until(session.deadline) => { return; },
            operation = task.operation.lock() => operation,
        };
        let result = match Call::catpaw_callback(&task, &id, &expected, input) {
            Ok(call) => super::invoke(&task, &call).await.map(|_| ()),
            Err(error) => Err(error),
        };
        if let Err(error) = result { super::fail(&task, &error.message); }
        if task.snapshot().done || task.snapshot().canceled { task.cancel_provider_flow_locked(&session.flow).await; }
        guard.finish();
    }
}
