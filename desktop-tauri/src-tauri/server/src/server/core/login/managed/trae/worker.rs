//! 原AuthCodeReceipt的一次宿主发送worker。登记Join/退出guard后才准入，不复活take。
//! waiter丢弃、WASM trap或Session取消不能丢已发HTTP的所有权；Host按原对象排空。

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU8, Ordering},
    Arc, Mutex, OnceLock, Weak,
};
use std::time::Duration;

use serde_json::Value;
use tokio::{
    sync::{oneshot, Notify, OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
    time::Instant,
};

use super::super::{Call, RuntimeBinding, Session, TraeCallSource};
use super::{identity, network, response, ExchangeInput, EXCHANGE_SENT, PROFILE_SENT, SEND_CLOSED};
use crate::server::core::plugin_host::{
    wasm::{managed::TraeExecution, manifest::Options},
    Host,
};
use crate::server::errors::GatewayError;

const MAX_WORKERS: usize = 64;
const TOTAL_BUDGET: Duration = Duration::from_secs(45);

pub(super) struct Operation {
    id: String,
    source: TraeCallSource,
    session: Weak<Session>,
    runtime: Arc<RuntimeBinding>,
    execution: Weak<TraeExecution>,
    host: Weak<Host>,
    call: String,
    step: String,
    lease: String,
    gate: AtomicU8,
    success: AtomicBool,
    finished: AtomicBool,
    stop: tokio_util::sync::CancellationToken,
    job: Mutex<Option<JoinHandle<()>>>,
    join_waiting: AtomicBool,
    changed: Notify,
}

impl Operation {
    pub(super) fn gate(&self) -> &AtomicU8 {
        &self.gate
    }
    pub(super) fn cancel(&self) {
        self.gate.fetch_or(SEND_CLOSED, Ordering::AcqRel);
        self.stop.cancel();
        self.changed.notify_waiters();
    }
    pub(super) fn belongs_to_call(&self, call: &Call) -> bool {
        self.source.matches(call) && self.call == call.id() && self.step == call.expected()
    }
    pub(super) fn matches_state(&self, state: &super::super::State) -> bool {
        self.finished.load(Ordering::Acquire)
            && self.success.load(Ordering::Acquire)
            && self.gate.load(Ordering::Acquire) == (EXCHANGE_SENT | PROFILE_SENT)
            && state.active.as_deref() == Some(self.call.as_str())
            && state.callback_step.as_deref() == Some(self.step.as_str())
            && state.trae.current.as_deref() == Some(self.lease.as_str())
            && state
                .trae
                .exchange
                .as_ref()
                .is_some_and(|original| std::ptr::eq(original.as_ref(), self))
            && state
                .runtime
                .as_ref()
                .is_some_and(|runtime| Arc::ptr_eq(runtime, &self.runtime))
            && self.source.matches_state(state)
            && self.session.upgrade().is_some_and(|session| {
                session.valid() && session.provider == "trae" && session.method_index == 0
            })
            && self.execution.upgrade().is_some()
    }
    pub(super) fn same_execution(&self, execution: &Arc<TraeExecution>) -> bool {
        Weak::ptr_eq(&self.execution, &Arc::downgrade(execution))
    }
    fn for_host(&self, host: &Host) -> bool {
        std::ptr::eq(self.host.as_ptr(), host)
    }
    fn ended(&self) {
        self.finished.store(true, Ordering::Release);
        self.changed.notify_waiters();
    }
}

struct Registry {
    workers: HashMap<String, Arc<Operation>>,
    closed_hosts: Vec<Weak<Host>>,
}
fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        Mutex::new(Registry {
            workers: HashMap::new(),
            closed_hosts: Vec::new(),
        })
    })
}
fn slots() -> Arc<Semaphore> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(Semaphore::new(4))).clone()
}

/// 退出guard在spawn和future首次poll前构造；先析构完整上下文/lease，再宣布本地结束。
struct Ownership {
    operation: Arc<Operation>,
    input: Option<ExchangeInput>,
    execution: Option<Arc<TraeExecution>>,
    permit: Option<OwnedSemaphorePermit>,
}
impl Drop for Ownership {
    fn drop(&mut self) {
        if !self.operation.success.load(Ordering::Acquire) {
            self.operation.cancel();
        }
        drop(self.input.take());
        drop(self.execution.take());
        drop(self.permit.take());
        self.operation.ended();
    }
}

struct Waiter {
    call: Call,
    operation: Arc<Operation>,
    returned: bool,
}
impl Drop for Waiter {
    fn drop(&mut self) {
        if !self.returned {
            super::cancel_exchange(&self.call, &self.operation);
            self.call.session.close();
        }
    }
}

pub(in crate::server::core::login::managed) async fn exchange(
    call: &Call,
    params: &Value,
    options: &Options,
) -> Result<Value, GatewayError> {
    call.validate("trae")?;
    if !super::applies(call.provider(), call.method_index())
        || !call.is_callback()
        || call.published()
        || params
            .as_object()
            .is_none_or(|fields| fields.len() != 1 || !fields.contains_key("expected"))
        || params["expected"].as_str() != Some(call.expected())
        || options.provider != "trae"
        || !options.allows("login.managed.v1")
        || !options.allows("login.loopback.v1")
        || options
            .manifest
            .managed_login
            .as_ref()
            .and_then(|declaration| {
                declaration.profile(
                    "trae",
                    0,
                    crate::server::core::plugin_host::wasm::managed_policy::TRAE_PROFILE,
                )
            })
            .is_none()
    {
        return Err(super::stopped_error());
    }
    let execution = call.trae_execution()?;
    let original = call.trae_call_source()?;
    let id = super::super::nonce()?;
    let input = match call.take_trae_exchange() {
        Ok(input) => input,
        Err(error) => {
            call.session.close();
            return Err(error);
        }
    };
    let operation = Arc::new(Operation {
        id,
        source: original,
        session: Arc::downgrade(&call.session),
        runtime: input.context.runtime.clone(),
        execution: Arc::downgrade(&execution),
        host: Arc::downgrade(execution.host()),
        call: input.call.clone(),
        step: input.step.clone(),
        lease: input.lease.clone(),
        gate: AtomicU8::new(0),
        success: AtomicBool::new(false),
        finished: AtomicBool::new(false),
        stop: tokio_util::sync::CancellationToken::new(),
        job: Mutex::new(None),
        join_waiting: AtomicBool::new(false),
        changed: Notify::new(),
    });
    let mut waiter = Waiter {
        call: call.clone(),
        operation: operation.clone(),
        returned: false,
    };
    let mut ownership = Ownership {
        operation: operation.clone(),
        input: Some(input),
        execution: Some(execution.clone()),
        permit: None,
    };
    ownership.permit = Some(
        slots()
            .try_acquire_owned()
            .map_err(|_| super::stopped_error())?,
    );
    register(&operation)?;
    execution.with_current(call, || {
        let mut state = call
            .session
            .state
            .try_lock()
            .map_err(|_| super::super::unavailable())?;
        if state.trae.exchange.is_some() || state.trae.witness.is_some() {
            return Err(super::stopped_error());
        }
        state.trae.exchange = Some(operation.clone());
        if !super::exchange_current(
            &state,
            call,
            ownership.input.as_ref().ok_or_else(super::stopped_error)?,
            &operation,
        ) {
            return Err(super::stopped_error());
        }
        Ok(())
    })?;
    let deadline = call.session.deadline.min(Instant::now() + TOTAL_BUDGET);
    let (begin, ready) = oneshot::channel();
    let (result_tx, result_rx) = oneshot::channel();
    let original_call = call.clone();
    let worker = crate::spawn_task(async move {
        let mut ownership = ownership;
        let result = if ready.await.is_ok() {
            run(&original_call, &mut ownership, deadline).await
        } else {
            Err(super::stopped_error())
        };
        drop(ownership);
        drop(original_call);
        let _ = result_tx.send(result);
    });
    {
        let mut slot = operation.job.lock().map_err(|_| super::stopped_error())?;
        if slot.is_some() {
            return Err(super::stopped_error());
        }
        *slot = Some(worker);
    }
    operation.changed.notify_waiters();
    begin.send(()).map_err(|_| super::stopped_error())?;
    let result = tokio::select! {
        biased;
        _ = call.ended() => return Err(super::stopped_error()),
        _ = operation.stop.cancelled() => return Err(super::stopped_error()),
        _ = operation.runtime.stopped.cancelled() => return Err(super::stopped_error()),
        result = result_rx => result.map_err(|_| super::stopped_error())??,
    };
    identity::validate_value(
        call,
        result
            .value()
            .get("account")
            .ok_or_else(super::identity_error)?,
    )?;
    waiter.returned = true;
    Ok(result.into_value())
}

fn register(operation: &Arc<Operation>) -> Result<(), GatewayError> {
    let mut table = registry().lock().map_err(|_| super::stopped_error())?;
    table.closed_hosts.retain(|host| host.strong_count() > 0);
    if table
        .closed_hosts
        .iter()
        .any(|host| Weak::ptr_eq(host, &operation.host))
    {
        return Err(super::stopped_error());
    }
    let ended: Vec<_> = table
        .workers
        .iter()
        .filter(|(_, operation)| release_ready(operation))
        .map(|(id, _)| id.clone())
        .collect();
    let removed: Vec<_> = ended
        .into_iter()
        .filter_map(|id| table.workers.remove(&id))
        .collect();
    if table.workers.len() >= MAX_WORKERS || table.workers.contains_key(&operation.id) {
        drop(table);
        drop(removed);
        return Err(super::stopped_error());
    }
    table
        .workers
        .insert(operation.id.clone(), operation.clone());
    drop(table);
    drop(removed);
    Ok(())
}

async fn run(
    call: &Call,
    ownership: &mut Ownership,
    deadline: Instant,
) -> Result<response::SecretJson, GatewayError> {
    let input = ownership.input.as_ref().ok_or_else(super::stopped_error)?;
    let execution = ownership
        .execution
        .as_ref()
        .ok_or_else(super::stopped_error)?;
    let operation = &ownership.operation;
    let worker_deadline = deadline.min(Instant::now() + network::Kind::Exchange.budget());
    if Instant::now() >= worker_deadline {
        return Err(super::stopped_error());
    }
    let request = network::build(input, network::Kind::Exchange, None)?;
    execution.with_current(call, || {
        if Instant::now() >= worker_deadline {
            return Err(super::stopped_error());
        }
        super::admit_send(call, input, operation, execution, false)
    })?;
    let response =
        network::observe(input, request, network::Kind::Exchange, worker_deadline).await?;
    let tokens = response::tokens(response.value(), input.context.protocol.variant)?;
    drop(response);
    if tokens
        .expires_at
        .is_some_and(|at| at <= crate::server::logging::now_ms())
    {
        return Err(super::stopped_error());
    }
    let worker_deadline = deadline.min(Instant::now() + network::Kind::Profile.budget());
    if Instant::now() >= worker_deadline {
        return Err(super::identity_error());
    }
    let request = network::build(input, network::Kind::Profile, Some(tokens.access.as_str()))?;
    execution.with_current(call, || {
        if Instant::now() >= worker_deadline {
            return Err(super::identity_error());
        }
        super::admit_send(call, input, operation, execution, true)
    })?;
    let response =
        network::observe(input, request, network::Kind::Profile, worker_deadline).await?;
    let profile = response::profile(response.value(), input.context.protocol.variant, &tokens)?;
    drop(response);
    if Instant::now() >= deadline {
        return Err(super::identity_error());
    }
    let result = identity::account(input, &tokens, &profile)?;
    identity::install(
        call,
        input,
        operation,
        execution,
        &profile,
        result
            .value()
            .get("account")
            .ok_or_else(super::identity_error)?,
    )?;
    operation.success.store(true, Ordering::Release);
    Ok(result)
}

fn for_host(host: &Host) -> Vec<Arc<Operation>> {
    registry()
        .lock()
        .map(|table| {
            table
                .workers
                .values()
                .filter(|operation| operation.for_host(host))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Host关闭先封原对象登记门，锁外只取消对应Session的未来准入；已发HTTP仍归原worker。
pub(crate) fn close_trae_for_host(host: &Host) {
    let operations = {
        let mut table = registry().lock().unwrap_or_else(|error| error.into_inner());
        let operations: Vec<_> = table
            .workers
            .values()
            .filter(|operation| operation.for_host(host))
            .cloned()
            .collect();
        if let Some(original) = operations
            .iter()
            .find_map(|operation| operation.host.upgrade())
        {
            let weak = Arc::downgrade(&original);
            if !table
                .closed_hosts
                .iter()
                .any(|old| Weak::ptr_eq(old, &weak))
            {
                table.closed_hosts.push(weak);
            }
        }
        operations
    };
    for operation in operations {
        if let Some(session) = operation.session.upgrade() {
            let state = session
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            operation.cancel();
            drop(state);
            session.close();
        } else {
            operation.cancel();
        }
    }
}

struct Waiting {
    operation: Arc<Operation>,
    job: Option<JoinHandle<()>>,
}
impl Drop for Waiting {
    fn drop(&mut self) {
        if let Some(job) = self.job.take() {
            let mut slot = self
                .operation
                .job
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if slot.is_none() {
                *slot = Some(job);
            }
            self.operation.join_waiting.store(false, Ordering::Release);
            drop(slot);
            self.operation.changed.notify_waiters();
        }
    }
}
async fn wait(operation: Arc<Operation>) {
    loop {
        let changed = operation.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        let job = {
            let mut slot = operation
                .job
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let job = slot.take();
            if job.is_some() {
                operation.join_waiting.store(true, Ordering::Release);
            }
            job
        };
        let mut waiting = Waiting {
            operation: operation.clone(),
            job,
        };
        if let Some(job) = waiting.job.as_mut() {
            let _ = job.await;
            waiting.job.take();
            operation.join_waiting.store(false, Ordering::Release);
            operation.changed.notify_waiters();
            return;
        }
        if operation.finished.load(Ordering::Acquire)
            && !operation.join_waiting.load(Ordering::Acquire)
        {
            return;
        }
        changed.await;
    }
}

pub(crate) async fn wait_trae_for_host(host: &Host) {
    for operation in for_host(host) {
        wait(operation).await;
    }
}
fn release_ready(operation: &Operation) -> bool {
    operation.finished.load(Ordering::Acquire)
        && operation.job.try_lock().is_ok_and(|job| {
            !operation.join_waiting.load(Ordering::Acquire)
                && job.as_ref().is_none_or(JoinHandle::is_finished)
        })
}

/// 真实guard已释放所有worker输入且Join已结束才可放Runtime；锁异常宁可保留旧实例。
pub(crate) fn trae_release_ready(host: &Host) -> bool {
    registry().try_lock().is_ok_and(|table| {
        table
            .workers
            .values()
            .filter(|operation| operation.for_host(host))
            .all(|operation| release_ready(operation))
    })
}
