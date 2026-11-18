//! Kiro 固定门户/AWS 两阶段回调。Receiver 属于原 Session，advance 不释放或重绑端口。
//! 门户路径/端口与字段依据本仓原生 Kiro 和 magpie-community/plugins/packages/kiro。
//! 此模块不发上游请求，不提供设备流或本机文件/进程/环境入口。

mod advance;
mod completion;
mod exchange;
mod receiver;
mod url;

pub(in crate::server::core::login::managed) use self::completion::{
    completes, persistable, Completion,
};
pub(in crate::server::core::login::managed) use self::exchange::Prepared;
pub(crate) use self::exchange::{KiroExchangeInput, KiroExchangePermit};

use std::sync::{Arc, Weak};

use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use super::{loopback::Lease, unavailable, Call, State};
use crate::server::core::plugin_host::wasm::managed_policy::{
    self as policy, CallbackKind, Profile,
};
use crate::server::errors::GatewayError;

pub(super) use self::advance::advance;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Portal,
    Aws,
}

impl Phase {
    fn profile(self) -> &'static str {
        match self {
            Self::Portal => policy::KIRO_PORTAL_PROFILE,
            Self::Aws => policy::KIRO_AWS_PROFILE,
        }
    }
    fn callback(self, port: u16) -> String {
        match self {
            Self::Portal => format!("http://localhost:{port}/signin/callback"),
            Self::Aws => format!("http://127.0.0.1:{port}/oauth/callback"),
        }
    }
    fn callbacks(self, port: u16) -> Vec<String> {
        match self {
            Self::Portal => vec![
                self.callback(port),
                format!("http://localhost:{port}/oauth/callback"),
            ],
            Self::Aws => vec![self.callback(port)],
        }
    }
    fn redirect(self, port: u16) -> String {
        match self {
            Self::Portal => format!("http://localhost:{port}"),
            Self::Aws => self.callback(port),
        }
    }
}

/// 一组实际 listener 的唯一强所有者仅存于 SessionState；worker 只留 Weak。
struct ReceiverOwner {
    port: u16,
    v4: Arc<TcpListener>,
    v6: Option<Arc<TcpListener>>,
    cancel: CancellationToken,
}
impl Drop for ReceiverOwner {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[derive(Default)]
pub(super) struct SessionState {
    receiver: Option<Arc<ReceiverOwner>>,
    opening: Option<String>,
    issued: bool,
    current: Option<String>,
    // 固定两个阶段的撤销标识只为重复 close 作幂等；绝不作为重新申请凭据。
    retired: Vec<String>,
}

/// 无字段公开/Clone/serde；仅专用 open 从实际 listener 和新随机 state 创建。
pub(super) struct Binding {
    receiver: Weak<ReceiverOwner>,
    phase: Phase,
    oauth_state: Option<String>,
    accepted: Option<Outcome>,
    advanced: bool,
    publication: Option<Publication>,
    consumed_state: Option<String>,
    consumed_challenge: Option<String>,
    next_origin: Option<&'static str>,
    login_provider: Option<&'static str>,
    terminal: Option<Weak<exchange::TerminalProof>>,
}

struct Publication {
    origin: String,
    challenge: zeroize::Zeroizing<String>,
    client_id: Option<zeroize::Zeroizing<String>>,
}

/// 只从原租约URL校验取得，claim保留完整terminal材料而不是只留下outcome。
struct CallbackFacts {
    outcome: Outcome,
    next_origin: Option<&'static str>,
    login_provider: Option<&'static str>,
    terminal: Option<exchange::TerminalProof>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    SocialCode,
    PortalAwsChoice,
    AwsCode,
    Refused,
}

/// 只从真实 claim 返回，随后封存在该原 Call 的私有 Material 中。
pub(super) struct Receipt {
    receiver: Weak<ReceiverOwner>,
    lease: String,
    step: String,
    outcome: Outcome,
    next_origin: Option<&'static str>,
    login_provider: Option<&'static str>,
    terminal: Option<Arc<exchange::TerminalProof>>,
    call: Option<zeroize::Zeroizing<String>>,
}

impl Receipt {
    pub(in crate::server::core::login::managed) fn bind_call(
        &mut self,
        call: &Call,
    ) -> Result<(), GatewayError> {
        if self.call.is_some()
            || self.step != call.expected()
            || !call.is_callback()
            || !applies(call.provider(), call.method_index())
        {
            return Err(unavailable());
        }
        self.call = Some(zeroize::Zeroizing::new(call.id().to_string()));
        Ok(())
    }
    pub(super) fn choice(&self) -> bool {
        self.outcome == Outcome::PortalAwsChoice
    }
}

pub(super) fn applies(provider: &str, method: u64) -> bool {
    provider == "kiro" && method == 0
}
fn invalid() -> GatewayError {
    GatewayError::bad_request("Kiro 授权地址或阶段材料不属于本次登录，未继续")
}

fn live(state: &State) -> bool {
    state
        .runtime
        .as_ref()
        .is_some_and(|runtime| !runtime.stopped.is_cancelled() && (runtime.alive)())
}

pub(super) async fn open(call: &Call, profile: Profile) -> Result<Value, GatewayError> {
    if !applies(call.provider(), call.method_index())
        || !call.is_start()
        || profile.ports != policy::KIRO_PORTS
    {
        return Err(unavailable());
    }
    let choice = call.kiro_start_choice()?;
    if (choice == "portal" && profile.callback != CallbackKind::KiroPortal)
        || (choice != "portal" && profile.callback != CallbackKind::KiroAws)
    {
        return Err(invalid());
    }
    let phase = match profile.callback {
        CallbackKind::KiroPortal => Phase::Portal,
        CallbackKind::KiroAws => Phase::Aws,
        _ => return Err(invalid()),
    };
    let login_provider = match choice {
        "portal" => None,
        "builderid" => Some("builderid"),
        "enterprise" => Some("enterprise"),
        _ => return Err(invalid()),
    };
    let direct_origin = if phase == Phase::Aws {
        Some(crate::server::core::plugin_host::wasm::aws_oidc::origin(call.kiro_start_region()?)
            .ok_or_else(invalid)?)
    } else {
        None
    };
    let reservation = super::nonce()?;
    {
        let mut state = call.session.state.lock().map_err(|_| unavailable())?;
        if state.kiro.issued
            || state.kiro.opening.is_some()
            || state.kiro.receiver.is_some()
            || !state.leases.is_empty()
            || !call.session.valid()
            || !live(&state)
            || state.active.as_deref() != Some(call.id())
        {
            return Err(unavailable());
        }
        state.kiro.opening = Some(reservation.clone());
    }
    // 只有此首次 Start 真正 bind 候选端口；失败不会退回任意端口。
    let (port, v4, v6) = receiver::bind(profile.ports).await?;
    call.validate(call.provider())?;
    let id = super::nonce()?;
    let oauth_state = super::nonce()?;
    if oauth_state == call.flow() || oauth_state == id {
        return Err(unavailable());
    }
    let receiver = Arc::new(ReceiverOwner {
        port,
        v4: Arc::new(v4),
        v6: v6.map(Arc::new),
        cancel: call.session.cancel.child_token(),
    });
    let response = response(&id, phase, port, &oauth_state, call.session.expires_at);
    {
        let mut state = call.session.state.lock().map_err(|_| unavailable())?;
        if state.kiro.opening.as_deref() != Some(&reservation)
            || state.kiro.issued
            || !state.leases.is_empty()
            || !call.session.valid()
            || !live(&state)
            || state.active.as_deref() != Some(call.id())
            || state
                .step
                .as_ref()
                .is_none_or(|step| step.id != call.expected())
        {
            return Err(unavailable());
        }
        let mut stage = lease(&receiver, call.expected(), phase, oauth_state);
        let next_origin = match phase {
            Phase::Portal => None,
            Phase::Aws => Some(direct_origin.ok_or_else(unavailable)?),
        };
        let binding = stage.kiro.as_mut().ok_or_else(unavailable)?;
        binding.next_origin = next_origin;
        binding.login_provider = login_provider;
        state.kiro.opening = None;
        state.kiro.issued = true;
        state.kiro.current = Some(id.clone());
        state.kiro.receiver = Some(receiver.clone());
        state.leases.insert(id.clone(), stage);
    }
    receiver::spawn(&receiver, &call.task, &call.session);
    Ok(response)
}

fn lease(
    receiver: &Arc<ReceiverOwner>,
    expected: &str,
    phase: Phase,
    oauth_state: String,
) -> Lease {
    Lease {
        profile: phase.profile(),
        expected: expected.into(),
        uri: phase.callback(receiver.port),
        step: None,
        official_state: None,
        authorization_url: None,
        passport: None,
        kiro: Some(Binding {
            receiver: Arc::downgrade(receiver),
            phase,
            oauth_state: Some(oauth_state),
            accepted: None,
            advanced: false,
            publication: None,
            consumed_state: None,
            consumed_challenge: None,
            next_origin: None,
            login_provider: None,
            terminal: None,
        }),
        consumed: false,
        queued: false,
        cancel: receiver.cancel.child_token(),
    }
}

fn response(id: &str, phase: Phase, port: u16, state: &str, expires: i64) -> Value {
    json!({ "leaseId": id, "callbackUrl": phase.callback(port), "callbackUrls": phase.callbacks(port),
        "redirectUri": phase.redirect(port), "port": port, "oauthState": state, "expiresAt": expires })
}

pub(super) fn authorization(lease: &mut Lease, raw: &str) -> Result<String, GatewayError> {
    let binding = lease.kiro.as_mut().ok_or_else(invalid)?;
    let receiver = binding
        .receiver
        .upgrade()
        .filter(|receiver| !receiver.cancel.is_cancelled())
        .ok_or_else(unavailable)?;
    if lease.consumed || lease.queued || binding.publication.is_some() {
        return Err(unavailable());
    }
    let state = binding.oauth_state.as_deref().ok_or_else(unavailable)?;
    let publication = url::authorization(raw, binding.phase, receiver.port, state)?;
    if binding.consumed_challenge.as_deref() == Some(publication.challenge.as_str())
        || binding
            .next_origin
            .is_some_and(|origin| publication.origin != origin)
    {
        return Err(invalid());
    }
    binding.consumed_challenge.take();
    binding.publication = Some(publication);
    Ok(state.to_string())
}

pub(super) fn claim(
    state: &mut State,
    id: &str,
    step: &str,
    raw: &str,
    manual: bool,
) -> Result<Receipt, GatewayError> {
    let current = state.step.as_ref().ok_or_else(unavailable)?;
    if current.id != step
        || current.lease.as_deref() != Some(id)
        || current.status != "ready"
        || current.browser.is_none()
        || state.kiro.current.as_deref() != Some(id)
    {
        return Err(unavailable());
    }
    let lease = state.leases.get_mut(id).ok_or_else(unavailable)?;
    if manual && lease.queued || !manual && !lease.queued {
        return Err(invalid());
    }
    let facts = url::callback_facts(lease, step, raw)?;
    let CallbackFacts {
        outcome,
        next_origin,
        login_provider,
        terminal,
    } = facts;
    let terminal = terminal.map(Arc::new);
    let binding = lease.kiro.as_mut().ok_or_else(invalid)?;
    let receiver = state.kiro.receiver.as_ref().ok_or_else(unavailable)?;
    if !Weak::ptr_eq(&binding.receiver, &Arc::downgrade(receiver)) || receiver.cancel.is_cancelled()
    {
        return Err(unavailable());
    }
    if outcome == Outcome::AwsCode && binding.login_provider != login_provider {
        return Err(invalid());
    }
    binding.accepted = Some(outcome);
    binding.login_provider = login_provider;
    binding.terminal = terminal.as_ref().map(Arc::downgrade);
    let old_state = binding.oauth_state.take();
    let publication = binding.publication.take();
    // 仅choice在同一个真实Call内保留反复用比较材料，advance原子清除；不能恢复旧资格。
    if binding.accepted == Some(Outcome::PortalAwsChoice) {
        binding.consumed_state = old_state;
        binding.consumed_challenge =
            publication.map(|publication| publication.challenge.to_string());
    }
    lease.official_state.take();
    lease.consumed = true;
    lease.queued = false;
    // 只撤销本阶段，绝不取消原 Receiver；advance 仍须原 Call 的私有 choice 收据。
    lease.cancel.cancel();
    state.callback_step = Some(step.to_string());
    Ok(Receipt {
        receiver: Arc::downgrade(receiver),
        lease: id.into(),
        step: step.into(),
        outcome,
        next_origin,
        login_provider,
        terminal,
        call: None,
    })
}

pub(super) fn choice_needs_next(state: &State) -> bool {
    state
        .kiro
        .current
        .as_ref()
        .and_then(|id| state.leases.get(id))
        .and_then(|lease| lease.kiro.as_ref())
        .is_some_and(|binding| binding.accepted == Some(Outcome::PortalAwsChoice))
}

pub(super) fn close(call: &Call, id: &str) -> Result<Value, GatewayError> {
    let (closed, leases, receiver) = {
        let mut state = call.session.state.lock().map_err(|_| unavailable())?;
        if state.kiro.retired.iter().any(|old| old == id) {
            return Ok(json!({ "closed": false }));
        }
        if state.kiro.current.as_deref() != Some(id) {
            return Ok(json!({ "closed": false }));
        }
        if state.kiro.retired.len() < 2 {
            state.kiro.retired.push(id.into());
        }
        state.kiro.current = None;
        if let Some(step) = &mut state.step {
            step.status = "processing";
            step.browser = None;
            step.lease = None;
        }
        state.callback_step = None;
        (
            true,
            std::mem::take(&mut state.leases),
            state.kiro.receiver.take(),
        )
    };
    drop(leases);
    drop(receiver);
    Ok(json!({ "closed": closed }))
}
