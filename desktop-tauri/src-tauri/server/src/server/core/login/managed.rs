//! 登录任务持有步骤、监听器和执行体绑定；单次 Wasm 调用结束不会销毁整个流程。
//! 普通 JSON 不构成可信事件；一次性输入只经 Call 进入原执行体，绝不自动重放。
mod autoclaw;
mod call;
mod catpaw;
mod kiro;
mod loopback;
mod step;
mod trae;

pub use call::Call;
pub(crate) use call::{TraeCallSource, KiroCallSource};
pub(crate) use kiro::{KiroExchangeInput, KiroExchangePermit};
pub(crate) use trae::{close_trae_for_host, trae_release_ready, wait_trae_for_host};
pub(crate) use step::Step;

use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use serde_json::{json, Value};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use super::{LoginProxySelection, LoginTaskContext, LoginTaskHandle, LoginTaskState, LOGIN_TIMEOUT_MS};
use crate::server::{core::providers::adapter::ProviderAdapter, errors::GatewayError, logging, ServerState};

struct RuntimeBinding {
    identity: usize,
    generation: u64,
    stopped: CancellationToken,
    alive: Arc<dyn Fn() -> bool + Send + Sync>,
    cleanup: Arc<dyn Fn() + Send + Sync>,
}

struct State {
    step: Option<Step>,
    active: Option<String>,
    callback_step: Option<String>,
    runtime: Option<Arc<RuntimeBinding>>,
    leases: std::collections::HashMap<String, loopback::Lease>,
    catpaw: catpaw::SessionState,
    kiro: kiro::SessionState,
    trae: trae::SessionState,
}

pub(crate) struct Session {
    flow: String,
    provider: String,
    method_index: u64,
    expires_at: i64,
    deadline: tokio::time::Instant,
    cancel: CancellationToken,
    local_source: Option<crate::server::access::local_session::Source>,
    state: Mutex<State>,
    kiro_completed: Mutex<Option<kiro::Completion>>,
    _reservation: OwnedSemaphorePermit,
}

impl Session {
    fn valid(&self) -> bool {
        !self.cancel.is_cancelled() && tokio::time::Instant::now() < self.deadline
            && logging::now_ms() < self.expires_at
            && self.local_source.as_ref().is_none_or(|source| !source.revocation().is_cancelled())
    }

    fn close(&self) {
        let (runtime, leases, catpaw, kiro, trae, step, kiro_completed) = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            // Trae发送门与关闭共用原Session锁；关闭先赢则worker绝不提交新HTTP。
            state.trae.revoke();
            self.cancel.cancel();
            let step = state.step.take();
            state.active = None;
            state.callback_step = None;
            let kiro_completed = self.kiro_completed.lock().unwrap_or_else(|error| error.into_inner()).take();
            (state.runtime.take(), std::mem::take(&mut state.leases), std::mem::take(&mut state.catpaw),
                std::mem::take(&mut state.kiro), std::mem::take(&mut state.trae), step, kiro_completed)
        };
        // 先撤销会话，再在同步锁外销毁客户端、端点租约及有限身份回执。
        drop(trae);
        drop(kiro_completed);
        drop(kiro);
        drop(catpaw);
        drop(leases);
        drop(step);
        if let Some(runtime) = runtime { (runtime.cleanup)(); }
    }
}

impl Drop for Session {
    fn drop(&mut self) { self.close(); }
}

#[derive(Clone)]
struct WeakTask {
    inner: Weak<Mutex<LoginTaskState>>,
    context: Weak<LoginTaskContext>,
    operation: Weak<tokio::sync::Mutex<()>>,
    ticket: u64,
}

impl WeakTask {
    fn new(task: &LoginTaskHandle) -> Result<Self, GatewayError> {
        Ok(Self { inner: Arc::downgrade(&task.inner),
            context: Arc::downgrade(task.context.as_ref().ok_or_else(unavailable)?),
            operation: Arc::downgrade(&task.operation), ticket: task.ticket })
    }
    fn upgrade(&self) -> Option<LoginTaskHandle> {
        Some(LoginTaskHandle { inner: self.inner.upgrade()?, context: Some(self.context.upgrade()?),
            operation: self.operation.upgrade()?, ticket: self.ticket })
    }
}

pub(super) fn unavailable() -> GatewayError {
    GatewayError::with_status(409, "托管登录已结束、过期或执行体已变化，请重新发起")
}

fn nonce() -> Result<String, GatewayError> {
    let mut bytes = [0; 24];
    getrandom::getrandom(&mut bytes).map_err(|_| GatewayError::with_status(500, "无法生成安全的登录标识"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn session(task: &LoginTaskHandle) -> Result<Arc<Session>, GatewayError> {
    let context = task.context.as_ref().ok_or_else(unavailable)?;
    let session = context.managed.lock().map_err(|_| unavailable())?.clone().ok_or_else(unavailable)?;
    let snapshot = task.snapshot();
    if !snapshot.managed || snapshot.done || snapshot.canceled || !session.valid()
        || snapshot.provider != session.provider || snapshot.state.as_deref() != Some(session.flow.as_str())
        || snapshot.method_index != Some(session.method_index) { return Err(unavailable()); }
    Ok(session)
}

pub(crate) fn interrupt(task: &LoginTaskHandle) {
    let session = task.context.as_ref().and_then(|context|
        context.managed.lock().ok().and_then(|guard| guard.clone()));
    if let Some(session) = session { session.close(); }
}

/// 调用方已持任务锁；原本机会话→Session短门内只消费保存入场，不把门带入数据库。
pub(super) fn admit_persist(task: &LoginTaskHandle, snapshot: &mut LoginTaskState) -> Result<(), GatewayError> {
    if !snapshot.managed { return Ok(()); }
    let session = task.context.as_ref().ok_or_else(unavailable)?.managed.lock()
        .map_err(|_| unavailable())?.clone().ok_or_else(unavailable)?;
    let mut admit = || {
        let state = session.state.try_lock().map_err(|_| unavailable())?;
        if !session.valid() || snapshot.done || snapshot.canceled || snapshot.provider != session.provider
            || snapshot.state.as_deref() != Some(session.flow.as_str()) || snapshot.method_index != Some(session.method_index)
            || state.active.is_none() || state.runtime.as_ref().is_none_or(|runtime| runtime.stopped.is_cancelled() || !(runtime.alive)())
            || state.step.as_ref().is_none_or(|step| step.status != "processing" || state.callback_step.as_deref() != Some(step.id.as_str()))
            || kiro::applies(&session.provider, session.method_index) && !kiro::persistable(&session, &state)
            || trae::applies(&session.provider, session.method_index) && !trae::persistable(&state)
            || snapshot.persistence != super::persistence::SaveState::NotEntered {
            return Err(unavailable());
        }
        snapshot.persistence = super::persistence::SaveState::Unconfirmed;
        Ok(())
    };
    if let Some(source) = &session.local_source {
        source.with_current(admit).ok_or_else(unavailable)?
    } else { admit() }
}

pub(crate) fn snapshot(task: &LoginTaskHandle) -> Result<Value, GatewayError> {
    let session = session(task)?;
    let state = session.state.lock().map_err(|_| unavailable())?;
    if !session.valid() { return Err(unavailable()); }
    let runtime = state.runtime.as_ref().ok_or_else(unavailable)?;
    if runtime.stopped.is_cancelled() || !(runtime.alive)() { return Err(unavailable()); }
    Ok(state.step.as_ref().map(|step| step.public(&session)).unwrap_or(Value::Null))
}

/// 只在本地登记，返回后才在后台读取上游配置；不能等验证码或等授权地址。
pub(crate) fn start(state: &ServerState, adapter: Arc<dyn ProviderAdapter>, method_index: u64,
    options: Value, proxy: Option<LoginProxySelection>, local_source: Option<crate::server::access::local_session::Source>,
) -> Result<Value, GatewayError> {
    if trae::applies(adapter.provider_id(), method_index) && local_source.as_ref().is_none_or(|source| !source.is_current()) {
        return Err(GatewayError::with_status(403, "Trae托管网页登录只能从网关本机桌面发起，请改用凭据导入"));
    }
    let adapter = adapter.managed_login_owner().ok_or_else(unavailable)?;
    static LIMIT: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let reservation = LIMIT.get_or_init(|| Arc::new(Semaphore::new(64))).clone().try_acquire_owned()
        .map_err(|_| GatewayError::with_status(429, "待完成的登录较多，请先完成或取消其他登录"))?;
    let flow = format!("managed_{}", nonce()?);
    let now = logging::now_ms();
    let task = state.login().new_handle_for_adapter("", adapter.clone(), proxy);
    let session = Arc::new(Session { flow: flow.clone(), provider: adapter.provider_id().into(), method_index,
        expires_at: now + LOGIN_TIMEOUT_MS as i64, deadline: tokio::time::Instant::now() + Duration::from_millis(LOGIN_TIMEOUT_MS),
        cancel: CancellationToken::new(), local_source, state: Mutex::new(State { step: Some(Step::preparing(nonce()?)),
            active: None, callback_step: None, runtime: None, leases: Default::default(),
            catpaw: Default::default(), kiro: Default::default(), trae: Default::default() }),
            kiro_completed: Mutex::new(None), _reservation: reservation });
    *task.context.as_ref().ok_or_else(unavailable)?.managed.lock().map_err(|_| unavailable())? = Some(session.clone());
    task.update(|snapshot| {
        snapshot.state = Some(flow.clone());
        snapshot.method_index = Some(method_index);
        snapshot.managed = true;
        snapshot.validates_callback = true;
        snapshot.polling = true;
    });
    let call = Call::start(&task, options)?;
    adapter.prepare_managed_login(&call)?;
    let runtime = session.state.lock().map_err(|_| unavailable())?.runtime.clone().ok_or_else(unavailable)?;
    monitor(&task, &session, runtime)?;
    state.login().tasks().register(&flow, task.clone());
    crate::spawn_task(async move {
        {
            let _operation = task.operation.lock().await;
            if let Err(error) = invoke(&task, &call).await {
                fail(&task, &error.message);
                task.cancel_provider_flow_locked(call.flow()).await;
                return;
            }
        }
        drop(call);
        poll(task).await;
    });
    Ok(json!({ "state": flow, "flow": flow, "provider": session.provider, "methodIndex": method_index,
        "managedLogin": true, "authUrl": "", "url": "", "browserMode": "none", "manualCallback": false,
        "callbackInput": "url", "pollIntervalMs": 1500, "timeoutMs": LOGIN_TIMEOUT_MS, "remainingMs": LOGIN_TIMEOUT_MS }))
}

fn monitor(task: &LoginTaskHandle, session: &Arc<Session>, runtime: Arc<RuntimeBinding>) -> Result<(), GatewayError> {
    let weak = WeakTask::new(task)?;
    let session = Arc::downgrade(session);
    crate::spawn_task(async move {
        loop {
            let Some(session) = session.upgrade() else { return; };
            let revoked = session.local_source.as_ref().map(|source| source.revocation());
            let revocation = async { match revoked { Some(token) => token.cancelled().await, None => std::future::pending().await } };
            tokio::select! {
                biased;
                _ = session.cancel.cancelled() => return,
                _ = revocation => {},
                _ = runtime.stopped.cancelled() => {},
                _ = tokio::time::sleep_until(session.deadline) => {},
                _ = tokio::time::sleep(Duration::from_millis(250)) => {
                    if session.valid() && (runtime.alive)() { continue; }
                }
            }
            let Some(task) = weak.upgrade() else { session.close(); return; };
            let message = if session.valid() { "登录插件已停用或切换，请重新发起" } else { "托管登录已过期，请重新发起" };
            // 先撤销输入与监听器，再等待长网络操作的串行锁。
            fail(&task, message);
            let _operation = task.operation.lock().await;
            task.cancel_provider_flow_locked(&session.flow).await;
            return;
        }
    });
    Ok(())
}

fn fail(task: &LoginTaskHandle, message: &str) {
    super::fail_plugin_login(task, message);
    interrupt(task);
}

struct InvocationGuard { task: LoginTaskHandle, completed: bool }
impl Drop for InvocationGuard {
    fn drop(&mut self) {
        if !self.completed { fail(&self.task, "登录步骤被中断，未自动重放验证或授权码；请重新发起"); }
    }
}

async fn invoke(task: &LoginTaskHandle, call: &Call) -> Result<Value, GatewayError> {
    let mut guard = InvocationGuard { task: task.clone(), completed: false };
    let adapter = task.adapter().ok_or_else(unavailable)?;
    call.validate_adapter(adapter.as_ref())?;
    catpaw::prepare_input(call).await?;
    let result = adapter.managed_login_action(call).await;
    call.validate(adapter.provider_id())?;
    let result = match result {
        Ok(value) => value,
        Err(error) => { fail(task, &error.message); guard.completed = true; return Err(error); }
    };
    if !call.consumed_input() {
        return Err(GatewayError::with_status(502, "插件未消费当前可信登录材料，不能确认接受"));
    }
    if call.is_start() {
        if result["flow"].as_str() != Some(call.flow()) || !call.published() {
            return Err(GatewayError::with_status(502, "插件未发布有效的托管登录步骤"));
        }
    } else {
        let poll: agent2api_provider_protocol::LoginPoll = serde_json::from_value(result.clone())
            .map_err(|_| GatewayError::with_status(502, "插件返回的托管登录结果无效"))?;
        match poll {
            agent2api_provider_protocol::LoginPoll::Done { account } => {
                if call.is_captcha() || !call.can_complete()? {
                    return Err(GatewayError::with_status(502, "未完成当前可信验证或回调，不能直接作为账号登录成功"));
                }
                call.validate(adapter.provider_id())?;
                catpaw::validate_account(call, &account)?;
                call.validate_trae_account(&account)?;
                call.validate_kiro_account(&account)?;
                super::persist_plugin_login(task, adapter.as_ref(), &account)?.ok_or_else(unavailable)?;
                interrupt(task);
            }
            agent2api_provider_protocol::LoginPoll::Failed { message } => {
                fail(task, &message);
                guard.completed = true;
                return Err(GatewayError::with_status(400, message));
            }
            agent2api_provider_protocol::LoginPoll::Pending { hint } => {
                if trae::applies(call.provider(), call.method_index()) && call.is_callback() {
                    return Err(trae::stopped_error());
                }
                if catpaw::applies(call.provider(), call.method_index()) && call.is_callback() {
                    return Err(GatewayError::with_status(502, "CatPaw 本次可信登录输入未直接完成归一化，未缓存或重放凭据"));
                }
                if call.kiro_needs_next()? && !call.published_browser()? {
                    return Err(GatewayError::with_status(502, "Kiro 门户选择尚未推进并发布固定 AWS 授权步骤，未重放旧回调"));
                }
                if call.is_captcha() && !call.published_browser()? {
                    return Err(GatewayError::with_status(502, "验证尚未换得获准的授权页面，请重新发起"));
                }
                super::set_plugin_login_hint(task, hint.as_deref());
            }
        }
    }
    call.finish();
    guard.completed = true;
    Ok(result)
}

async fn poll(task: LoginTaskHandle) {
    loop {
        let Ok(session) = session(&task) else { return; };
        let interval = if catpaw::applies(&session.provider, session.method_index) { 1000 } else { 1500 };
        tokio::select! {
            biased;
            _ = session.cancel.cancelled() => return,
            _ = tokio::time::sleep_until(session.deadline) => {
                fail(&task, "托管登录已过期，请重新发起");
                return;
            },
            _ = tokio::time::sleep(Duration::from_millis(interval)) => {}
        }
        let _operation = task.operation.lock().await;
        if task.snapshot().done || task.snapshot().canceled { return; }
        let result = if catpaw::applies(&session.provider, session.method_index) {
            poll_catpaw(&task).await
        } else {
            match Call::poll(&task) {
                Ok(call) => invoke(&task, &call).await.map(|_| ()),
                Err(error) => Err(error),
            }
        };
        if let Err(error) = result { fail(&task, &error.message); }
        if task.snapshot().done || task.snapshot().canceled {
            task.cancel_provider_flow_locked(&session.flow).await;
            return;
        }
    }
}

/// 仅原poll持有operation时进入；拿到token就在同一个Call中晋升，不递归取得操作锁。
async fn poll_catpaw(task: &LoginTaskHandle) -> Result<(), GatewayError> {
    let mut guard = InvocationGuard { task: task.clone(), completed: false };
    let mut call = Call::poll(task)?;
    let result = match catpaw::poll(&mut call).await {
        Ok(true) => invoke(task, &call).await.map(|_| ()),
        Ok(false) => { call.finish(); Ok(()) }
        Err(error) => { call.finish(); Err(error) }
    };
    guard.completed = true;
    result
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    state: String,
    interaction: String,
    inputs: Value,
    captcha: Value,
}

/// 调用方必须持有 task.operation；只校验一次、消费一次，不把请求失败当成可重放。
pub(crate) async fn submit(task: &LoginTaskHandle, value: Value) -> Result<Value, GatewayError> {
    let input: Input = serde_json::from_value(value).map_err(|_| GatewayError::bad_request("托管登录输入字段无效"))?;
    let call = Call::submit(task, &input)?;
    let captcha = call.is_captcha();
    let result = invoke(task, &call).await;
    if let Err(error) = result {
        fail(task, &error.message);
        task.cancel_provider_flow_locked(&input.state).await;
        return Err(error);
    }
    if task.snapshot().done {
        task.cancel_provider_flow_locked(&input.state).await;
        return Ok(json!({ "accepted": true, "interaction": null }));
    }
    let interaction = snapshot(task)?;
    if captcha { Ok(json!({ "accepted": true, "captchaResult": true, "bizResult": true, "interaction": interaction })) }
    else { Ok(json!({ "accepted": true, "interaction": interaction })) }
}

async fn receive_callback(task: LoginTaskHandle, lease: &str, raw: &str) -> Result<(), GatewayError> {
    let session = session(&task)?;
    let _operation = tokio::select! {
        biased;
        _ = session.cancel.cancelled() => return Err(unavailable()),
        operation = task.operation.lock() => operation,
    };
    let call = Call::callback(&task, lease, raw)?;
    let result = invoke(&task, &call).await;
    if let Err(error) = &result { fail(&task, &error.message); }
    if task.snapshot().done || task.snapshot().canceled {
        task.cancel_provider_flow_locked(call.flow()).await;
    }
    result.map(|_| ())
}
