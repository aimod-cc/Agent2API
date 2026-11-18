//! Trae 无state的原监听器收据、私有PKCE上下文与固定一次宿主兑换。
//! 只以同源GetUserInfo真实UID签发保存见证；不重兑、刷新、补回调身份或改道。

mod callback;
mod context;
mod identity;
mod network;
mod receiver;
mod response;
mod worker;

pub(super) use identity::{can_complete, persistable, validate_account};
pub(super) use worker::exchange;
pub(crate) use worker::{close_trae_for_host, trae_release_ready, wait_trae_for_host};

use std::sync::{atomic::Ordering, Arc, Weak};

use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use self::context::{ProtocolContext, Variant};
use super::{loopback::Lease, unavailable, Call, RuntimeBinding, Session, State};
use crate::server::core::{
    egress::managed::FrozenHttpClient,
    plugin_host::wasm::{managed_policy, manifest::Options},
};
use crate::server::errors::GatewayError;

/// 强 listener 所有者只存在原 SessionState；接收任务仅弱引用，不靠端口串构造来源。
struct ReceiverOwner {
    listener: Arc<TcpListener>,
    port: u16,
    // stop_accept只停止新连接；queued候选仍以原Session/租约claim，不等于授权失效。
    stop_accept: CancellationToken,
    revoked: CancellationToken,
}
impl Drop for ReceiverOwner {
    fn drop(&mut self) {
        self.stop_accept.cancel();
        self.revoked.cancel();
    }
}

struct PrivateContext {
    protocol: ProtocolContext,
    client: FrozenHttpClient,
    runtime: Arc<RuntimeBinding>,
    session: Weak<Session>,
    receiver: Weak<ReceiverOwner>,
}

#[derive(Default)]
pub(super) struct SessionState {
    issued: bool,
    opening: Option<String>,
    current: Option<String>,
    receiver: Option<Arc<ReceiverOwner>>,
    context: Option<PrivateContext>,
    publication_attempted: bool,
    published_step: Option<String>,
    claimed: bool,
    taken: bool,
    exchange: Option<Arc<worker::Operation>>,
    witness: Option<identity::Witness>,
}

impl SessionState {
    pub(super) fn revoke(&self) {
        if let Some(exchange) = &self.exchange {
            exchange.cancel();
        }
        if let Some(receiver) = &self.receiver {
            receiver.stop_accept.cancel();
            receiver.revoked.cancel();
        }
    }
}

/// 只有真实 listener 已queue、原Call重新claim后才创建，无Clone/serde/公开构造器。
pub(super) struct AuthCodeReceipt {
    receiver: Weak<ReceiverOwner>,
    session: Weak<Session>,
    runtime: Arc<RuntimeBinding>,
    lease: String,
    step: String,
    call: String,
    code: Zeroizing<String>,
}

/// 仅原Call的一次take产生；没有JSON/Clone入口，lease只是核对键，不替代对象归属。
pub(super) struct ExchangeInput {
    context: PrivateContext,
    code: Zeroizing<String>,
    call: String,
    step: String,
    lease: String,
}

pub(super) struct TakenContext(PrivateContext);
impl AuthCodeReceipt {
    pub(super) fn into_exchange(self, context: TakenContext) -> ExchangeInput {
        ExchangeInput {
            context: context.0,
            code: self.code,
            call: self.call,
            step: self.step,
            lease: self.lease,
        }
    }
}

/// 请求解析器与queue同时产生，不能由插件JSON、普通URL回调或按leaseId构造。
pub(super) struct QueuedCallback {
    receiver: Weak<ReceiverOwner>,
    session: Weak<Session>,
    runtime: Arc<RuntimeBinding>,
    lease: String,
    step: String,
    input: callback::Input,
}

pub(super) fn applies(provider: &str, method: u64) -> bool {
    provider == "trae" && method == 0
}
fn invalid() -> GatewayError {
    GatewayError::bad_request("Trae 授权材料不属于本次真实回环步骤，未继续")
}
fn live(state: &State) -> bool {
    state
        .runtime
        .as_ref()
        .is_some_and(|runtime| !runtime.stopped.is_cancelled() && (runtime.alive)())
}

/// 可调用的固定prepare实现；参数不接flow/provider/account/URL/header/代理/令牌。
pub(super) async fn prepare(
    call: &Call,
    params: &Value,
    options: &Options,
) -> Result<Value, GatewayError> {
    call.validate(call.provider())?;
    if !applies(call.provider(), call.method_index())
        || !call.is_start()
        || call.published()
        || options.provider != "trae"
        || !options.allows("login.managed.v1")
        || !options.allows("login.loopback.v1")
    {
        return Err(unavailable());
    }
    if params.as_object().is_none_or(|fields| {
        fields.len() != 2 || !fields.contains_key("expected") || !fields.contains_key("variant")
    }) || params["expected"].as_str() != Some(call.expected())
    {
        return Err(invalid());
    }
    let requested = params["variant"].as_str().ok_or_else(invalid)?;
    if requested != call.original_trae_variant()? {
        return Err(invalid());
    }
    let variant = Variant::parse(requested)?;
    let declaration = options
        .manifest
        .managed_login
        .as_ref()
        .ok_or_else(unavailable)?;
    let profile = declaration
        .profile("trae", 0, managed_policy::TRAE_PROFILE)
        .ok_or_else(|| {
            GatewayError::with_status(403, "Trae 无state固定回调配置尚未获准，未发起授权")
        })?;
    if profile.callback != managed_policy::CallbackKind::TraeCode
        || profile.host != "127.0.0.1"
        || profile.path != "/authorize"
        || profile.ports != &[0u16][..]
        || profile.origins != &[managed_policy::TRAE_AUTHORIZATION_ORIGIN][..]
    {
        return Err(invalid());
    }
    // 原任务的真实选择只取一次，不读取JSON代理、不重新解析配置、不回落系统出口。
    let task = call.task.upgrade().ok_or_else(unavailable)?;
    let selected = task
        .context
        .as_ref()
        .and_then(|context| context.proxy.as_ref())
        .ok_or_else(|| GatewayError::bad_request("请明确选择本次Trae登录的直连或代理出口"))?
        .selected
        .clone();
    let reservation = super::nonce()?;
    let runtime = {
        let mut state = call.session.state.lock().map_err(|_| unavailable())?;
        if !call.session.valid()
            || !live(&state)
            || state.active.as_deref() != Some(call.id())
            || state
                .step
                .as_ref()
                .is_none_or(|step| step.id != call.expected())
            || state.trae.issued
            || state.trae.opening.is_some()
            || !state.leases.is_empty()
        {
            return Err(unavailable());
        }
        // 已申请即永久消耗；bind、freeze或后续发布失败均不把资格放回。
        state.trae.issued = true;
        state.trae.opening = Some(reservation.clone());
        state.runtime.clone().ok_or_else(unavailable)?
    };
    let listener = tokio::select! {
        biased;
        _ = call.ended() => return Err(unavailable()),
        _ = runtime.stopped.cancelled() => return Err(unavailable()),
        result = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)) =>
            result.map_err(|_| GatewayError::with_status(409, "Trae 本次IPv4回环端口申请失败，未发起授权"))?,
    };
    let local = listener.local_addr().map_err(|_| unavailable())?;
    if local.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST) || local.port() == 0 {
        return Err(unavailable());
    }
    let protocol = ProtocolContext::generate(variant, local.port())?;
    let client = tokio::select! {
        biased;
        _ = call.ended() => return Err(unavailable()),
        _ = runtime.stopped.cancelled() => return Err(unavailable()),
        result = FrozenHttpClient::from_path(selected) => result.map_err(|_|
            GatewayError::with_status(503, "Trae 本次冻结出口不可用，未改道或发起授权"))?,
    };
    call.validate(call.provider())?;
    let id = super::nonce()?;
    let uri = protocol.callback.clone();
    let response = json!({"leaseId": id, "authorizationUrl": protocol.authorization,
        "expiresAt": call.session.expires_at});
    let receiver = Arc::new(ReceiverOwner {
        listener: Arc::new(listener),
        port: local.port(),
        stop_accept: call.session.cancel.child_token(),
        revoked: call.session.cancel.child_token(),
    });
    {
        let mut state = call.session.state.lock().map_err(|_| unavailable())?;
        if !call.session.valid()
            || !live(&state)
            || state.active.as_deref() != Some(call.id())
            || state
                .step
                .as_ref()
                .is_none_or(|step| step.id != call.expected())
            || state
                .runtime
                .as_ref()
                .is_none_or(|current| !Arc::ptr_eq(current, &runtime))
            || state.trae.opening.as_deref() != Some(reservation.as_str())
            || state.trae.context.is_some()
            || state.trae.current.is_some()
            || !state.leases.is_empty()
        {
            return Err(unavailable());
        }
        state.trae.opening = None;
        state.trae.current = Some(id.clone());
        state.trae.receiver = Some(receiver.clone());
        state.trae.context = Some(PrivateContext {
            protocol,
            client,
            runtime,
            session: Arc::downgrade(&call.session),
            receiver: Arc::downgrade(&receiver),
        });
        state.leases.insert(
            id,
            Lease {
                profile: managed_policy::TRAE_PROFILE,
                expected: call.expected().to_string(),
                uri,
                step: None,
                official_state: None,
                authorization_url: None,
                passport: None,
                kiro: None,
                consumed: false,
                queued: false,
                cancel: call.session.cancel.child_token(),
            },
        );
    }
    receiver::spawn(&receiver, &call.task, &call.session);
    Ok(response)
}

/// 在任何发布解析前消耗本轮唯一机会。失败不把listener/上下文变回可重新申请。
pub(super) fn reserve_publication(call: &Call) -> Result<(), GatewayError> {
    if !applies(call.provider(), call.method_index()) || !call.is_start() {
        return Err(unavailable());
    }
    let mut state = call.session.state.lock().map_err(|_| unavailable())?;
    if !call.session.valid()
        || !live(&state)
        || state.active.as_deref() != Some(call.id())
        || !state.trae.issued
        || state.trae.context.is_none()
        || state.trae.current.is_none()
        || state.trae.publication_attempted
    {
        return Err(unavailable());
    }
    state.trae.publication_attempted = true;
    Ok(())
}

/// 只认宿主自己生成的完整授权地址；不从复制的query恢复PKCE/设备绑定。
pub(super) fn publication(
    state: &mut SessionState,
    lease: &Lease,
    id: &str,
    raw: &str,
    step: &str,
) -> Result<(), GatewayError> {
    let context = state.context.as_ref().ok_or_else(unavailable)?;
    let receiver = state.receiver.as_ref().ok_or_else(unavailable)?;
    if !state.publication_attempted
        || state.published_step.is_some()
        || state.claimed
        || state.taken
        || state.current.as_deref() != Some(id)
        || lease.profile != managed_policy::TRAE_PROFILE
        || lease.closed()
        || lease.queued
        || lease.step.is_some()
        || lease.official_state.is_some()
        || raw != context.protocol.authorization
        || lease.uri != context.protocol.callback
        || receiver.port == 0
        || receiver.revoked.is_cancelled()
        || receiver.stop_accept.is_cancelled()
        || !Weak::ptr_eq(&context.receiver, &Arc::downgrade(receiver))
    {
        return Err(invalid());
    }
    state.published_step = Some(step.to_string());
    Ok(())
}

fn ready(state: &State, owner: &Arc<ReceiverOwner>, id: &str, step: &str) -> bool {
    live(state)
        && !owner.revoked.is_cancelled()
        && !state.trae.claimed
        && !state.trae.taken
        && state.trae.current.as_deref() == Some(id)
        && state.trae.published_step.as_deref() == Some(step)
        && state
            .trae
            .receiver
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, owner))
        && state
            .trae
            .context
            .as_ref()
            .is_some_and(|context| Weak::ptr_eq(&context.receiver, &Arc::downgrade(owner)))
        && state.step.as_ref().is_some_and(|current| {
            current.id == step
                && current.status == "ready"
                && current.browser.is_some()
                && current.lease.as_deref() == Some(id)
        })
        && state.leases.get(id).is_some_and(|lease| {
            lease.profile == managed_policy::TRAE_PROFILE
                && !lease.closed()
                && lease.step.as_deref() == Some(step)
                && lease.official_state.is_none()
        })
}

fn queue(
    session: &Arc<Session>,
    receiver: &Weak<ReceiverOwner>,
    input: callback::Input,
) -> Result<QueuedCallback, GatewayError> {
    let owner = receiver.upgrade().ok_or_else(unavailable)?;
    let mut state = session.state.lock().map_err(|_| unavailable())?;
    let id = state.trae.current.clone().ok_or_else(unavailable)?;
    let step = state.step.as_ref().ok_or_else(unavailable)?.id.clone();
    if !ready(&state, &owner, &id, &step)
        || state.leases.get(&id).is_none_or(|lease| lease.queued)
        || !session.valid()
        || owner.stop_accept.is_cancelled()
        || owner.revoked.is_cancelled()
    {
        drop(state);
        return Err(unavailable());
    }
    let Some(runtime) = state.runtime.clone() else {
        drop(state);
        return Err(unavailable());
    };
    state.leases.get_mut(&id).ok_or_else(unavailable)?.queued = true;
    // 队列只有一个真实候选，停止accept但不释放/重绑当轮端口；后到材料不能替换。
    owner.stop_accept.cancel();
    drop(state);
    Ok(QueuedCallback {
        receiver: receiver.clone(),
        session: Arc::downgrade(session),
        runtime,
        lease: id,
        step,
        input,
    })
}

pub(super) fn claim(call: &Call, queued: QueuedCallback) -> Result<AuthCodeReceipt, GatewayError> {
    call.validate(call.provider())?;
    let owner = queued.receiver.upgrade().ok_or_else(unavailable)?;
    let mut state = call.session.state.lock().map_err(|_| unavailable())?;
    if !call.session.valid()
        || !applies(call.provider(), call.method_index())
        || !call.is_callback()
        || state.active.as_deref() != Some(call.id())
        || queued.step != call.expected()
        || !Weak::ptr_eq(&queued.session, &Arc::downgrade(&call.session))
        || state
            .runtime
            .as_ref()
            .is_none_or(|runtime| !Arc::ptr_eq(runtime, &queued.runtime))
        || !ready(&state, &owner, &queued.lease, &queued.step)
        || state
            .leases
            .get(&queued.lease)
            .is_none_or(|lease| !lease.queued)
        || !call.session.valid()
        || owner.revoked.is_cancelled()
    {
        drop(state);
        return Err(unavailable());
    }
    let lease = state
        .leases
        .get_mut(&queued.lease)
        .ok_or_else(unavailable)?;
    lease.consumed = true;
    lease.queued = false;
    lease.cancel.cancel();
    state.trae.claimed = true;
    state.callback_step = Some(queued.step.clone());
    if let Some(step) = state.step.as_mut() {
        step.status = "processing";
        step.browser = None;
    }
    drop(state);
    let callback::Input::Code(code) = queued.input else {
        return Err(GatewayError::bad_request(
            "Trae 本次官方授权已拒绝，未产生可兑换材料",
        ));
    };
    Ok(AuthCodeReceipt {
        receiver: queued.receiver,
        session: queued.session,
        runtime: queued.runtime,
        lease: queued.lease,
        step: queued.step,
        call: call.id().to_string(),
        code,
    })
}

/// 只在真实当前Callback Call的Material内消费；外部不能传收据/code或选另外的context。
pub(super) fn take(
    call: &Call,
    state: &mut State,
    receipt: &AuthCodeReceipt,
) -> Result<TakenContext, GatewayError> {
    if !call.session.valid()
        || !live(state)
        || !applies(call.provider(), call.method_index())
        || !call.is_callback()
        || state.active.as_deref() != Some(call.id())
        || !state.trae.claimed
        || state.trae.taken
        || receipt.call != call.id()
        || receipt.step != call.expected()
        || state.trae.current.as_deref() != Some(receipt.lease.as_str())
        || !Weak::ptr_eq(&receipt.session, &Arc::downgrade(&call.session))
        || state
            .runtime
            .as_ref()
            .is_none_or(|runtime| !Arc::ptr_eq(runtime, &receipt.runtime))
        || state.trae.receiver.as_ref().is_none_or(|owner| {
            owner.revoked.is_cancelled() || !Weak::ptr_eq(&receipt.receiver, &Arc::downgrade(owner))
        })
        || state.callback_step.as_deref() != Some(call.expected())
        || state.leases.get(&receipt.lease).is_none_or(|lease| {
            !lease.consumed
                || lease.profile != managed_policy::TRAE_PROFILE
                || lease.step.as_deref() != Some(call.expected())
        })
    {
        return Err(unavailable());
    }
    let context = state.trae.context.as_ref().ok_or_else(unavailable)?;
    if !Weak::ptr_eq(&context.session, &receipt.session)
        || !Weak::ptr_eq(&context.receiver, &receipt.receiver)
        || !Arc::ptr_eq(&context.runtime, &receipt.runtime)
    {
        return Err(unavailable());
    }
    state.trae.taken = true;
    let context = state.trae.context.take().ok_or_else(unavailable)?;
    Ok(TakenContext(context))
}

// 同一个原Session门内，取消位与两个发送位争同一CAS；任何位都只能单向写入。
const EXCHANGE_SENT: u8 = 1;
const PROFILE_SENT: u8 = 2;
const SEND_CLOSED: u8 = 4;

fn exchange_current(
    state: &State,
    call: &Call,
    input: &ExchangeInput,
    operation: &Arc<worker::Operation>,
) -> bool {
    call.is_callback()
        && !call.published()
        && applies(call.provider(), call.method_index())
        && call.session.valid()
        && live(state)
        && input.call == call.id()
        && input.step == call.expected()
        && state.active.as_deref() == Some(call.id())
        && state.callback_step.as_deref() == Some(input.step.as_str())
        && state.trae.claimed
        && state.trae.taken
        && state.trae.context.is_none()
        && state.trae.current.as_deref() == Some(input.lease.as_str())
        && state.trae.published_step.as_deref() == Some(input.step.as_str())
        && state
            .trae
            .exchange
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, operation))
        && state.trae.witness.is_none()
        && Weak::ptr_eq(&input.context.session, &Arc::downgrade(&call.session))
        && state
            .runtime
            .as_ref()
            .is_some_and(|runtime| Arc::ptr_eq(runtime, &input.context.runtime))
        && state.trae.receiver.as_ref().is_some_and(|receiver| {
            !receiver.revoked.is_cancelled()
                && Weak::ptr_eq(&input.context.receiver, &Arc::downgrade(receiver))
        })
        && state.step.as_ref().is_some_and(|step| {
            step.id == input.step
                && step.status == "processing"
                && step.lease.as_deref() == Some(input.lease.as_str())
                && step.browser.is_none()
        })
        && state.leases.get(&input.lease).is_some_and(|lease| {
            lease.profile == managed_policy::TRAE_PROFILE
                && lease.consumed
                && !lease.queued
                && lease.step.as_deref() == Some(input.step.as_str())
                && lease.official_state.is_none()
                && lease.uri == input.context.protocol.callback
        })
}

/// 只在execution.with_current的短同步action内调用，Host→Session；不碰网络或执行槽。
fn admit_send(
    call: &Call,
    input: &ExchangeInput,
    operation: &Arc<worker::Operation>,
    execution: &crate::server::core::plugin_host::wasm::managed::TraeExecution,
    profile: bool,
) -> Result<(), GatewayError> {
    let state = call.session.state.try_lock().map_err(|_| unavailable())?;
    if !exchange_current(&state, call, input, operation)
        || input.context.runtime.identity != execution.identity()
        || input.context.runtime.generation != execution.generation()
    {
        return Err(unavailable());
    }
    let before = if profile { EXCHANGE_SENT } else { 0 };
    let after = before | if profile { PROFILE_SENT } else { EXCHANGE_SENT };
    operation
        .gate()
        .compare_exchange(before, after, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| stopped_error())?;
    Ok(())
}

/// worker异常/等待方丢弃只封原机会，不能回放或取消已发的HTTP未来。
fn cancel_exchange(call: &Call, operation: &worker::Operation) {
    let state = call
        .session
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    operation.cancel();
    drop(state);
}

pub(super) fn finish_call(call: &Call, state: &mut State) {
    if state
        .trae
        .exchange
        .as_ref()
        .is_some_and(|operation| operation.belongs_to_call(call))
    {
        if let Some(operation) = &state.trae.exchange {
            operation.cancel();
        }
    }
}

pub(super) fn stopped_error() -> GatewayError {
    GatewayError::with_status(
        502,
        "Trae 本轮授权码兑换失败、被中断或结果未确认，已停止；未保存账号，也不会重兑或改道",
    )
    .with_code("trae_oauth_code_exchange_stopped")
}

pub(super) fn identity_error() -> GatewayError {
    GatewayError::with_status(
        502,
        "Trae 本轮真实账号身份缺失、冲突或来源已变化，未保存账号；不补回调身份或unknown",
    )
    .with_code("trae_oauth_identity_unconfirmed")
}

/// 显式close不恢复issued；原listener/context在全部Session锁外析构。
pub(super) fn close(call: &Call, id: &str) -> Result<Value, GatewayError> {
    let (closed, lease, receiver, context, witness) = {
        let mut state = call.session.state.lock().map_err(|_| unavailable())?;
        if state.trae.current.as_deref() != Some(id) {
            return Ok(json!({"closed": false}));
        }
        if let Some(operation) = &state.trae.exchange {
            operation.cancel();
        }
        if let Some(receiver) = &state.trae.receiver {
            receiver.stop_accept.cancel();
            receiver.revoked.cancel();
        }
        state.trae.current = None;
        state.callback_step = None;
        if let Some(step) = &mut state.step {
            step.status = "processing";
            step.browser = None;
            step.lease = None;
        }
        (
            true,
            state.leases.remove(id),
            state.trae.receiver.take(),
            state.trae.context.take(),
            state.trae.witness.take(),
        )
    };
    drop(lease);
    drop(receiver);
    drop(context);
    drop(witness);
    Ok(json!({"closed": closed}))
}

pub(super) async fn receive(
    task: super::LoginTaskHandle,
    queued: QueuedCallback,
) -> Result<(), GatewayError> {
    let session = super::session(&task)?;
    let _operation = tokio::select! {
        biased;
        _ = session.cancel.cancelled() => return Err(unavailable()),
        _ = tokio::time::sleep_until(session.deadline) => return Err(unavailable()),
        operation = task.operation.lock() => operation,
    };
    let call = Call::trae_callback(&task, queued)?;
    // invoke仅在本轮真实witness验收与保存之后返回Done；Pending在原出口已拒绝。
    let result = super::invoke(&task, &call).await;
    if let Err(error) = &result {
        // 保存三态由持久化出口记录；不能用旧“exchange未接”覆盖真实saved=true。
        if !task.snapshot().done {
            super::fail(&task, &error.message);
        }
    }
    call.finish();
    if task.snapshot().done || task.snapshot().canceled {
        task.cancel_provider_flow_locked(call.flow()).await;
    }
    result.map(|_| ())
}
