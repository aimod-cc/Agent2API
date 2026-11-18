//! 原Kiro回调的一次兑换和身份查询由独立工作持有；取消等待不会丢已发请求或恢复材料。

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, AtomicU8, Ordering},
    Arc, Mutex, OnceLock, Weak,
};

use agent2api_provider_protocol::NormalizedAccount;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::{
    sync::{oneshot, Notify, OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
};
use zeroize::Zeroizing;

use super::{
    kiro_execution::KiroExecution,
    kiro_network::{self as network, Account},
    kiro_response::TokenResponse,
};
use crate::server::{
    core::{
        egress::managed::FrozenHttpClient,
        login::managed::{Call, KiroCallSource, KiroExchangeInput},
        plugin_host::Host,
    },
    errors::GatewayError,
};

const TOKEN_SENT: u8 = 1;
const PROFILE_SENT: u8 = 2;
const MAX_WORKERS: usize = 64;

struct Job {
    handle: Option<JoinHandle<()>>,
    waiting: bool,
    settled: bool,
}
struct Operation {
    id: String,
    host: Weak<Host>,
    call: Weak<Call>,
    execution: Weak<KiroExecution>,
    issued: AtomicU8,
    success: AtomicBool,
    finished: AtomicBool,
    job: Mutex<Job>,
    changed: Notify,
}
struct Ownership {
    operation: Arc<Operation>,
    call: Option<Arc<Call>>,
    execution: Option<Arc<KiroExecution>>,
    input: Option<KiroExchangeInput>,
    consumed: Option<Arc<KiroExchangeInput>>,
    client: Option<FrozenHttpClient>,
    token: Option<TokenResponse>,
    permit: Option<OwnedSemaphorePermit>,
}
impl Drop for Ownership {
    fn drop(&mut self) {
        if !self.operation.success.load(Ordering::Acquire) {
            if let Some(call) = &self.call {
                call.invalidate_kiro();
            }
        }
        drop(self.token.take());
        drop(self.input.take());
        drop(self.consumed.take());
        drop(self.client.take());
        drop(self.call.take());
        drop(self.execution.take());
        drop(self.permit.take());
        self.operation.finished.store(true, Ordering::Release);
        self.operation.changed.notify_waiters();
    }
}
struct Waiter {
    call: Call,
    returned: bool,
}
impl Drop for Waiter {
    fn drop(&mut self) {
        if !self.returned {
            self.call.invalidate_kiro();
        }
    }
}

/// 只由本模块实际两次HTTP成功后创建；普通候选账号、输入消费或JSON不能构造。
pub(crate) struct Completed {
    operation: Arc<Operation>,
    input: Arc<KiroExchangeInput>,
    identity: String,
    secret: [u8; 32],
    public: [u8; 32],
    execution: Arc<KiroExecution>,
}
impl Completed {
    pub(crate) fn source(&self) -> &KiroCallSource {
        self.input.source()
    }
    pub(crate) fn input(&self) -> &KiroExchangeInput {
        &self.input
    }
    pub(crate) fn matches(&self, call: &Call) -> bool {
        self.source().matches(call)
            && self.operation.finished.load(Ordering::Acquire)
            && self.operation.success.load(Ordering::Acquire)
            && self.operation.issued.load(Ordering::Acquire) == (TOKEN_SENT | PROFILE_SENT)
            && self.operation.execution.upgrade().is_some()
    }
    pub(crate) fn with_execution<R>(
        &self,
        call: &Call,
        action: impl FnOnce() -> Result<R, GatewayError>,
    ) -> Result<R, GatewayError> {
        if !self.matches(call) {
            return Err(network::unconfirmed());
        }
        self.execution.with_current(call, action)
    }

    pub(crate) fn locally_complete(&self) -> bool {
        self.operation.finished.load(Ordering::Acquire)
            && self.operation.success.load(Ordering::Acquire)
            && self.operation.issued.load(Ordering::Acquire) == (TOKEN_SENT | PROFILE_SENT)
    }

    pub(crate) fn matches_account(&self, account: &NormalizedAccount) -> bool {
        !account.desktop
            && account.identity == self.identity
            && digest(&account.secret) == self.secret
            && digest(&account.public) == self.public
    }
}
fn digest(value: &Value) -> [u8; 32] {
    let text = Zeroizing::new(value.to_string());
    Sha256::digest(text.as_bytes()).into()
}

fn registry() -> &'static Mutex<HashMap<String, Arc<Operation>>> {
    static ITEMS: OnceLock<Mutex<HashMap<String, Arc<Operation>>>> = OnceLock::new();
    ITEMS.get_or_init(|| Mutex::new(HashMap::new()))
}
fn slots() -> Arc<Semaphore> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(Semaphore::new(4))).clone()
}
fn finished(operation: &Operation) -> bool {
    operation.finished.load(Ordering::Acquire)
        && operation.job.try_lock().is_ok_and(|job| {
            !job.waiting
                && (job.settled || job.handle.as_ref().is_some_and(JoinHandle::is_finished))
        })
}

pub(super) async fn exchange(
    call: &Call,
    execution: Arc<KiroExecution>,
    params: &Value,
) -> Result<Value, GatewayError> {
    let mut waiter = Waiter {
        call: call.clone(),
        returned: false,
    };
    if params
        .as_object()
        .is_none_or(|fields| fields.len() != 1 || !fields.contains_key("body"))
    {
        return Err(network::unconfirmed());
    }
    let input = call.take_kiro_exchange(&params["body"])?;
    let original = Arc::new(call.clone());
    let permit = slots()
        .try_acquire_owned()
        .map_err(|_| network::unconfirmed())?;
    let mut bytes = [0u8; 24];
    getrandom::getrandom(&mut bytes).map_err(|_| network::unconfirmed())?;
    if bytes.iter().all(|byte| *byte == 0) {
        return Err(network::unconfirmed());
    }
    let id = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let operation = Arc::new(Operation {
        id: id.clone(),
        host: Arc::downgrade(execution.host()),
        call: Arc::downgrade(&original),
        execution: Arc::downgrade(&execution),
        issued: AtomicU8::new(0),
        success: AtomicBool::new(false),
        finished: AtomicBool::new(false),
        job: Mutex::new(Job {
            handle: None,
            waiting: false,
            settled: false,
        }),
        changed: Notify::new(),
    });
    let mut owner = Ownership {
        operation: operation.clone(),
        call: Some(original),
        execution: Some(execution),
        input: Some(input),
        consumed: None,
        client: None,
        token: None,
        permit: Some(permit),
    };
    let removed = {
        let mut table = registry().lock().map_err(|_| network::unconfirmed())?;
        let ended = table
            .iter()
            .filter(|(_, old)| finished(old))
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let removed = ended
            .into_iter()
            .filter_map(|id| table.remove(&id))
            .collect::<Vec<_>>();
        if table.len() >= MAX_WORKERS || table.contains_key(&id) {
            drop(table);
            drop(removed);
            return Err(network::unconfirmed());
        }
        table.insert(id, operation.clone());
        removed
    };
    drop(removed);
    let (begin, ready) = oneshot::channel();
    let (sent, received) = oneshot::channel();
    {
        let mut job = operation.job.lock().map_err(|_| network::unconfirmed())?;
        if job.handle.is_some() || job.waiting || job.settled {
            return Err(network::unconfirmed());
        }
        job.handle = Some(crate::spawn_task(async move {
            let result = if ready.await.is_ok() {
                run(&mut owner).await
            } else {
                Err(network::unconfirmed())
            };
            drop(owner);
            let _ = sent.send(result);
        }));
    }
    operation.changed.notify_waiters();
    begin.send(()).map_err(|_| network::unconfirmed())?;
    let (account, completed) = tokio::select! {
        biased;
        _ = call.ended() => return Err(network::unconfirmed()),
        result = received => result.map_err(|_| network::unconfirmed())??,
    };
    call.install_kiro_completed(completed)?;
    waiter.returned = true;
    Ok(account.into_value())
}

async fn run(owner: &mut Ownership) -> Result<(Account, Arc<Completed>), GatewayError> {
    let call = owner
        .call
        .as_ref()
        .cloned()
        .ok_or_else(network::unconfirmed)?;
    let execution = owner
        .execution
        .as_ref()
        .cloned()
        .ok_or_else(network::unconfirmed)?;
    execution.with_current(&call, || Ok(()))?;
    let path = call.kiro_selected_path()?;
    owner.client = Some(
        tokio::time::timeout_at(call.deadline(), FrozenHttpClient::from_path(path))
            .await
            .map_err(|_| network::unconfirmed())?
            .map_err(|_| network::unconfirmed())?,
    );
    let client = owner.client.as_ref().ok_or_else(network::unconfirmed)?;
    let input = owner.input.as_ref().ok_or_else(network::unconfirmed)?;
    let request = network::exchange_request(client, input)?;
    let input = owner.input.take().ok_or_else(network::unconfirmed)?;
    let permit = execution.with_current(&call, || {
        let permit = input.consume(&call)?;
        owner
            .operation
            .issued
            .compare_exchange(0, TOKEN_SENT, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| network::unconfirmed())?;
        Ok(permit)
    })?;
    owner.consumed = Some(Arc::new(permit.into_input()));
    let input = owner.consumed.as_ref().ok_or_else(network::unconfirmed)?;
    owner.token = Some(network::exchange_response(client, input, request).await?);
    let token = owner.token.as_ref().ok_or_else(network::unconfirmed)?;
    let (request, url, region, profile_arn) = network::profile_request(client, input, token)?;
    execution.with_current(&call, || {
        input.source().with_current(&call, || {
            owner
                .operation
                .issued
                .compare_exchange(
                    TOKEN_SENT,
                    TOKEN_SENT | PROFILE_SENT,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .map_err(|_| network::unconfirmed())?;
            Ok(())
        })
    })?;
    let profile = network::profile_response(client, input, token, request, &url).await?;
    execution.with_current(&call, || input.source().with_current(&call, || Ok(())))?;
    let account = network::account(input, token, &profile, &region, &profile_arn)?;
    let completed = Arc::new(Completed {
        operation: owner.operation.clone(),
        execution,
        input: input.clone(),
        identity: account.value()["identity"]
            .as_str()
            .ok_or_else(network::unconfirmed)?
            .to_string(),
        secret: digest(&account.value()["secret"]),
        public: digest(&account.value()["public"]),
    });
    owner.operation.success.store(true, Ordering::Release);
    Ok((account, completed))
}

fn for_host(host: &Host) -> Vec<Arc<Operation>> {
    registry()
        .lock()
        .map(|table| {
            table
                .values()
                .filter(|operation| std::ptr::eq(operation.host.as_ptr(), host))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}
pub(in crate::server::core::plugin_host::wasm) fn close_for_host(host: &Host) {
    for operation in for_host(host) {
        if let Some(call) = operation.call.upgrade() {
            call.invalidate_kiro();
        }
    }
}
pub(in crate::server::core::plugin_host::wasm) fn release_ready(host: &Host) -> bool {
    registry().try_lock().is_ok_and(|table| {
        table
            .values()
            .filter(|operation| std::ptr::eq(operation.host.as_ptr(), host))
            .all(|operation| finished(operation))
    })
}
pub(in crate::server::core::plugin_host::wasm) async fn wait_for_host(host: &Host) {
    for operation in for_host(host) {
        wait(operation).await;
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
            if slot.handle.is_none() {
                slot.handle = Some(job);
            }
            slot.waiting = false;
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
            if slot.settled {
                return;
            }
            if !slot.waiting && slot.handle.is_some() {
                slot.waiting = true;
                slot.handle.take()
            } else if !slot.waiting && operation.finished.load(Ordering::Acquire) {
                return;
            } else {
                None
            }
        };
        let mut waiting = Waiting {
            operation: operation.clone(),
            job,
        };
        if let Some(job) = waiting.job.as_mut() {
            let _ = job.await;
            waiting.job.take();
            let mut slot = operation
                .job
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            slot.waiting = false;
            slot.settled = true;
            drop(slot);
            operation.changed.notify_waiters();
            return;
        }
        changed.await;
    }
}
