//! CodeArts 原调用的一次换证工作；取消等待不丢已消费 Pending、已发 HTTP 或同步保存。
//! 原行、原出口、原请求和响应均由一个工作所有者持到实际收尾，未知结果不重新发送。

use std::collections::HashMap;
use std::sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use agent2api_provider_protocol as proto;
use serde_json::Value;
use tokio::{sync::{oneshot, Notify, OwnedSemaphorePermit, Semaphore}, task::JoinHandle, time::Instant};
use zeroize::Zeroizing;

use super::{call::Call, protocol::{RefreshMaterial, RequestParts, ResponseCandidate, TOKEN_URI, MAX_RESPONSE_BYTES}};
use crate::server::core::{
    account_store::{credential_version::{PendingSlot, PendingCompletionCallback, PendingCompletionState, PendingCommitFact, ReadyRead, SlotOutcome, SlotSettlement}, plugin_accounts},
    egress::managed::FrozenHttpClient,
};

const MAX_WORKERS: usize = 64;

struct Job {
    handle: Option<JoinHandle<()>>,
    waiting: bool,
    settled: bool,
}
struct Operation {
    id: String,
    host: Weak<crate::server::core::plugin_host::Host>,
    call: Weak<Call>,
    job: Mutex<Job>,
    completed: AtomicBool,
    save_commit: std::sync::atomic::AtomicU8,
    save_storage_closed: AtomicBool,
    changed: Notify,
}
struct Ownership {
    operation: Arc<Operation>,
    call: Option<Arc<Call>>,
    ready: Option<ReadyRead>,
    pending: Option<PendingSlot>,
    client: Option<FrozenHttpClient>,
    parts: Option<RequestParts>,
    request: Option<reqwest::Request>,
    candidate: Option<ResponseCandidate>,
    permit: Option<OwnedSemaphorePermit>,
}
impl Drop for Ownership {
    fn drop(&mut self) {
        // 未完成工作销毁保持原持久 Pending，不在析构联网、重读账号或恢复 Ready。
        drop(self.request.take());
        drop(self.candidate.take());
        drop(self.parts.take());
        drop(self.ready.take());
        drop(self.pending.take());
        drop(self.client.take());
        drop(self.call.take());
        drop(self.permit.take());
        self.operation.completed.store(true, Ordering::Release);
        self.operation.changed.notify_waiters();
    }
}
struct Waiter { call: Arc<Call>, returned: bool }
impl Drop for Waiter {
    fn drop(&mut self) { if !self.returned { self.call.close(); } }
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
    operation.completed.load(Ordering::Acquire) && operation.job.try_lock().is_ok_and(|job| {
        !job.waiting && (job.settled || job.handle.as_ref().is_some_and(JoinHandle::is_finished))
    })
}

pub(super) async fn exchange(call: Arc<Call>) -> Result<Value, proto::WireError> {
    let mut waiter = Waiter { call: call.clone(), returned: false };
    call.prepare_once()?;
    let permit = slots().try_acquire_owned().map_err(|_| unknown())?;
    let mut bytes = [0u8; 24];
    getrandom::getrandom(&mut bytes).map_err(|_| unknown())?;
    if bytes.iter().all(|byte| *byte == 0) { return Err(unknown()); }
    let id = bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    let operation = Arc::new(Operation { id: id.clone(), host: Arc::downgrade(call.host()), call: Arc::downgrade(&call),
        job: Mutex::new(Job { handle: None, waiting: false, settled: false }),
        completed: AtomicBool::new(false), save_commit: std::sync::atomic::AtomicU8::new(0),
        save_storage_closed: AtomicBool::new(false), changed: Notify::new() });
    let mut ownership = Ownership { operation: operation.clone(), call: Some(call.clone()),
        ready: None, pending: None, client: None, parts: None, request: None, candidate: None, permit: Some(permit) };
    let removed = {
        let mut table = registry().lock().map_err(|_| unknown())?;
        let expired = table.iter().filter(|(_, operation)| finished(operation)).map(|(id, _)| id.clone()).collect::<Vec<_>>();
        let removed = expired.into_iter().filter_map(|id| table.remove(&id)).collect::<Vec<_>>();
        if table.len() >= MAX_WORKERS || table.contains_key(&id)
            || table.values().any(|old| Weak::ptr_eq(&old.call, &operation.call)) {
            drop(table); drop(removed); return Err(unknown());
        }
        table.insert(id, operation.clone());
        removed
    };
    drop(removed);
    let (begin, ready) = oneshot::channel();
    let (sent, received) = oneshot::channel();
    {
        let mut job = operation.job.lock().map_err(|_| unknown())?;
        if job.handle.is_some() || job.waiting || job.settled { return Err(unknown()); }
        job.handle = Some(crate::spawn_task(async move {
            let outcome = if ready.await.is_ok() { run(&mut ownership).await } else { Err(unknown()) };
            drop(ownership);
            let _ = sent.send(outcome);
        }));
    }
    operation.changed.notify_waiters();
    begin.send(()).map_err(|_| unknown())?;
    let outcome = tokio::select! {
        biased;
        _ = call.closed() => return Err(unknown()),
        outcome = received => outcome.map_err(|_| unknown())?,
    };
    waiter.returned = true;
    outcome
}

async fn run(owner: &mut Ownership) -> Result<Value, proto::WireError> {
    let call = owner.call.as_ref().cloned().ok_or_else(unknown)?;
    let store = plugin_accounts::shared().ok_or_else(unknown)?;
    call.with_current(|| Ok(()))?;
    let account = call.account().to_string();
    let result = tokio::task::spawn_blocking(move || store.read_codearts_ready(&account)).await
        .map_err(|_| unknown())?.map_err(|_| unknown())?;
    owner.ready = match result { SlotOutcome::Committed(ready) => Some(ready), _ => return Err(unknown()) };
    call.with_current(|| Ok(()))?;
    let material = RefreshMaterial::from_stamp(owner.ready.as_ref().ok_or_else(unknown)?.borrow_stamp())
        .map_err(|_| unknown())?;
    let mut jti = [0u8; 32];
    getrandom::getrandom(&mut jti).map_err(|_| unknown())?;
    owner.parts = Some(material.build(crate::server::logging::now_ms(), jti).map_err(|_| unknown())?);
    let path = call.take_path(owner.ready.as_ref().ok_or_else(unknown)?)?;
    owner.client = Some(tokio::time::timeout_at(call.deadline().min(Instant::now() + Duration::from_secs(5)),
        FrozenHttpClient::from_path(path)).await.map_err(|_| unknown())?.map_err(|_| unknown())?);
    let parts = owner.parts.as_ref().ok_or_else(unknown)?;
    let client = owner.client.as_ref().ok_or_else(unknown)?;
    let mut request = client.client().map_err(|_| unknown())?.post(parts.uri());
    for (name, value) in parts.headers() { request = request.header(name, value); }
    let request = request.body(parts.form().as_bytes().to_vec()).build().map_err(|_| unknown())?;
    if request.method() != reqwest::Method::POST || request.url().as_str() != TOKEN_URI
        || request.body().and_then(reqwest::Body::as_bytes) != Some(parts.form().as_bytes()) {
        return Err(unknown());
    }
    owner.request = Some(request);
    call.with_current(|| Ok(()))?;
    let ready = owner.ready.take().ok_or_else(unknown)?;
    let result = tokio::task::spawn_blocking(move || ready.consume_pending()).await
        .map_err(|_| unknown())?.map_err(|_| unknown())?;
    owner.pending = match result { SlotOutcome::Committed(pending) => Some(pending), _ => return Err(unknown()) };
    if call.submit_pending_once(owner.pending.as_ref().ok_or_else(unknown)?).is_err() {
        return block(owner).await;
    }
    let request = owner.request.take().ok_or_else(unknown)?;
    let client = owner.client.as_ref().ok_or_else(unknown)?;
    let deadline = call.deadline().min(Instant::now() + Duration::from_secs(25));
    let read = tokio::time::timeout_at(deadline, async {
        let mut response = client.client().map_err(|_| unknown())?.execute(request).await.map_err(|_| unknown())?;
        let status = response.status().as_u16();
        if response.url().as_str() != TOKEN_URI || response.headers().contains_key("location")
            || response.content_length().is_some_and(|length| length > MAX_RESPONSE_BYTES as u64) { return Err(unknown()); }
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| unknown())? {
            if bytes.len().checked_add(chunk.len()).is_none_or(|length| length > MAX_RESPONSE_BYTES) { return Err(unknown()); }
            bytes.extend_from_slice(&chunk);
        }
        ResponseCandidate::parse(status, &bytes, crate::server::logging::now_ms()).map_err(|_| unknown())
    }).await;
    owner.candidate = match read { Ok(Ok(candidate)) => Some(candidate), _ => return block(owner).await };
    if call.with_current(|| Ok(())).is_err() { return block(owner).await; }
    let candidate = owner.candidate.take().ok_or_else(unknown)?;
    let pending = owner.pending.take().ok_or_else(unknown)?;
    let original = call.clone();
    let operation = owner.operation.clone();
    let result = tokio::task::spawn_blocking(move || {
        let access = Zeroizing::new(candidate.credentials()["access_key_id"].as_str().ok_or_else(unknown)?.to_string());
        let completion = pending.prepare_completion(candidate.credentials()).map_err(|_| unknown())?;
        let checked = original.complete_pending_once(&completion)?;
        let (commit, storage_closed) = checked.facts();
        operation.save_commit.store(match commit {
            PendingCommitFact::NotAttempted => 0,
            PendingCommitFact::Confirmed => 1,
            PendingCommitFact::Unconfirmed => 2,
        }, Ordering::Release);
        operation.save_storage_closed.store(storage_closed, Ordering::Release);
        let state = checked.into_state();
        drop(completion);
        match state {
            PendingCompletionState::Entered {
                result: PendingCompletionCallback::Returned(Ok(SlotSettlement::Rotated(_))),
                commit: PendingCommitFact::Confirmed, storage_closed: true,
            } => Ok(Value::String(access.to_string())),
            _ => { original.close(); Err(unknown()) },
        }
    }).await.map_err(|_| unknown())?;
    result
}

async fn block(owner: &mut Ownership) -> Result<Value, proto::WireError> {
    if let Some(pending) = owner.pending.take() {
        // 同步存储工作仍由原独立worker等待，取消方不能把它丢弃后重新占槽。
        let _ = tokio::task::spawn_blocking(move || pending.block()).await;
    }
    Err(unknown())
}

fn for_host(host: &crate::server::core::plugin_host::Host) -> Vec<Arc<Operation>> {
    registry().lock().map(|table| table.values().filter(|operation| {
        std::ptr::eq(operation.host.as_ptr(), host)
    }).cloned().collect()).unwrap_or_default()
}

pub(in crate::server::core::plugin_host::wasm) fn close_for_host(host: &crate::server::core::plugin_host::Host) {
    for operation in for_host(host) {
        if let Some(call) = operation.call.upgrade() { call.close(); }
    }
}

pub(in crate::server::core::plugin_host::wasm) fn release_ready(host: &crate::server::core::plugin_host::Host) -> bool {
    registry().try_lock().is_ok_and(|table| table.values().filter(|operation| {
        std::ptr::eq(operation.host.as_ptr(), host)
    }).all(|operation| finished(operation)))
}

pub(in crate::server::core::plugin_host::wasm) async fn wait_for_host(host: &crate::server::core::plugin_host::Host) {
    for operation in for_host(host) { wait(operation).await; }
}
struct Waiting { operation: Arc<Operation>, job: Option<JoinHandle<()>> }
impl Drop for Waiting {
    fn drop(&mut self) {
        if let Some(job) = self.job.take() {
            let mut slot = self.operation.job.lock().unwrap_or_else(|error| error.into_inner());
            if slot.handle.is_none() { slot.handle = Some(job); }
            slot.waiting = false;
            drop(slot);
            self.operation.changed.notify_waiters();
        }
    }
}
async fn wait(operation: Arc<Operation>) {
    loop {
        let changed = operation.changed.notified();
        tokio::pin!(changed); changed.as_mut().enable();
        let job = {
            let mut slot = operation.job.lock().unwrap_or_else(|error| error.into_inner());
            if slot.settled { return; }
            if !slot.waiting && slot.handle.is_some() { slot.waiting = true; slot.handle.take() }
            else if !slot.waiting && operation.completed.load(Ordering::Acquire) { return; }
            else { None }
        };
        let mut waiting = Waiting { operation: operation.clone(), job };
        if let Some(job) = waiting.job.as_mut() {
            let _ = job.await; waiting.job.take();
            let mut slot = operation.job.lock().unwrap_or_else(|error| error.into_inner());
            slot.waiting = false; slot.settled = true;
            drop(slot); operation.changed.notify_waiters(); return;
        }
        changed.await;
    }
}
fn unknown() -> proto::WireError {
    proto::WireError::with_code("CodeArts本次换证、原槽保存或收尾未确认，未重发、重新占槽或沿用旧值冒充成功", "codearts_refresh_unconfirmed")
}
