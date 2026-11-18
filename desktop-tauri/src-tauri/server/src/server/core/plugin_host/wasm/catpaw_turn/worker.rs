//! 原创建worker延续为唯一读者和清理worker；每次read只送一笔有限命令。
//! 等待端取消不abort已投创建/stop/event；最终退场先drop完整future再记本地结束。
use std::{future::Future, pin::Pin, sync::Arc, task::{Context, Poll}};

use agent2api_provider_protocol as proto;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::Value;
use tokio::{runtime::Handle, sync::{mpsc, oneshot}};

use super::{cleanup::{Outcome, Reason}, error, owner::Owner, read, registry::{Record, Registry}, settle, settlement};
use crate::server::core::plugin_host::wasm::conversation::SourceRef;

pub(super) enum Command { Read(read::Command), Settle(settle::Command) }
enum Consumption { Cleanup(Reason), Completed, AwaitingTools, CompletionUnknown }

pub(in crate::server::core::plugin_host::wasm) struct CreationWait {
    receiver: oneshot::Receiver<()>,
    record: Arc<Record>,
    completed: bool,
}
impl CreationWait {
    pub(in crate::server::core::plugin_host::wasm) async fn wait(mut self) -> Result<Value, proto::WireError> {
        let ended = self.record.ended_token();
        let call = self.record.source.ended_token();
        let delivered = tokio::select! {
            biased;
            result = &mut self.receiver => result,
            _ = ended.cancelled() => return Err(error::ended()),
            _ = call.cancelled() => return Err(error::ended()),
            _ = tokio::time::sleep_until(self.record.source.deadline()) => return Err(error::ended()),
        };
        delivered.map_err(|_| error::unknown())?;
        // 原worker仍持Response等read，不再等整个worker退场。
        let value = self.record.source.with_submission_gate(|_| self.record.response())?;
        self.completed = true;
        Ok(value)
    }
}
impl Drop for CreationWait { fn drop(&mut self) { if !self.completed { self.record.abandon(); } } }

pub(super) fn start(registry: &Arc<Registry>, source: &SourceRef, params: Value) -> Result<CreationWait, proto::WireError> {
    let handle = params["handle"].as_str().ok_or_else(error::invalid)?;
    let record = registry.get(source, handle)?;
    let input = record.input(&params)?;
    let runtime = Handle::try_current().map_err(|_| error::unavailable())?;
    let mut guard = WorkerGuard { record: record.clone(), registry: registry.clone(), active: false };
    let account = record.reserve()?;
    guard.active = true;
    let owner = if let Some(previous) = record.predecessor() {
        drop(account);
        let mut owner = previous.take_handoff_owner(&record.source, &input)?;
        if let Err(error) = owner.rebind_continuation(record.source.clone(), input) {
            previous.retain_lost_owner(owner);
            return Err(error);
        }
        owner
    } else { Owner::new(record.source.clone(), account, input)? };
    let (start, ready) = oneshot::channel();
    let (delivery, receiver) = oneshot::channel();
    let (commands, reads) = mpsc::channel(1);
    let future = OwnedWorker { work: Some(Box::pin(run(OwnerHold { owner: Some(owner), record: record.clone() }, record.clone(), registry.clone(), ready, delivery, reads))), guard };
    let join = runtime.spawn(future);
    let abort = join.abort_handle();
    if record.track(abort.clone(), commands).is_err() || !registry.owns(&record) {
        // ready未发，所有HTTP尚未poll；唯一允许abort的未投递窗口。
        abort.abort(); return Err(error::unavailable());
    }
    if start.send(()).is_err() { record.abandon(); return Err(error::unknown()); }
    drop(join);
    Ok(CreationWait { receiver, record, completed: false })
}

async fn run(mut held: OwnerHold, record: Arc<Record>, registry: Arc<Registry>, ready: oneshot::Receiver<()>, delivery: oneshot::Sender<()>, mut reads: mpsc::Receiver<Command>) {
    let Some(owner) = held.owner.as_mut() else { record.note_worker_failure(); return; };
    let failure = if ready.await.is_err() || !registry.owns(&record) { Some(error::unavailable()) } else { owner.execute(&record).await.err() };
    let mut failed = failure.is_some();
    if record.creation_ready(owner, failure).is_err() { record.note_worker_failure(); failed = true; }
    if delivery.send(()).is_err() { record.abandon(); }
    let consumption = if failed {
        Consumption::Cleanup(if record.abandoned() { Reason::Canceled } else { Reason::Failed })
    } else {
        consume(owner, &record, &mut reads).await
    };
    if matches!(consumption, Consumption::AwaitingTools) {
        if let Some(owner) = held.owner.take() {
            if let Err(owner) = record.keep_handoff_owner(owner) {
                record.retain_lost_owner(owner);
                record.note_worker_failure();
            } else {
                super::handoff_cleanup::watch(record.clone(), registry.clone());
            }
        }
        return;
    }
    let outcome = match consumption {
        // 正常完成不再走无条件close/stop/canceled，原worker独立释放后退场。
        Consumption::Completed => Outcome::NotNeeded,
        Consumption::CompletionUnknown | Consumption::AwaitingTools => Outcome::Unconfirmed,
        Consumption::Cleanup(reason) => owner.close(&record, reason).await,
    };
    let owner = held.owner.take();
    if outcome == Outcome::Unconfirmed {
        if let Some(owner) = owner { record.retain_lost_owner(owner); }
    } else { drop(owner); }
    if record.finish_cleanup(outcome).is_err() { record.note_worker_failure(); }
}

// future第一次poll之前即持有原Owner；运行时丢失仅同步保留，不spawn/重发清理。
struct OwnerHold { owner: Option<Owner>, record: Arc<Record> }
impl Drop for OwnerHold {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.take() { self.record.retain_lost_owner(owner); }
    }
}

async fn consume(owner: &mut Owner, record: &Arc<Record>, reads: &mut mpsc::Receiver<Command>) -> Consumption {
    let ended = record.ended_token();
    let call = record.source.ended_token();
    loop {
        let command = tokio::select! {
            biased;
            _ = ended.cancelled() => return Consumption::Cleanup(Reason::Canceled),
            _ = call.cancelled() => { record.abandon(); return Consumption::Cleanup(Reason::Canceled); },
            _ = tokio::time::sleep_until(record.source.deadline()) => { record.abandon(); return Consumption::Cleanup(Reason::Canceled); },
            command = reads.recv() => command,
        };
        let Some(command) = command else { record.abandon(); return Consumption::Cleanup(Reason::Canceled); };
        let command = match command {
            Command::Read(command) => command,
            Command::Settle(command) => {
                return match settlement::execute(owner, record, command).await {
                    settlement::ResultKind::Completed => Consumption::Completed,
                    settlement::ResultKind::AwaitingTools => Consumption::AwaitingTools,
                    settlement::ResultKind::Unconfirmed => Consumption::CompletionUnknown,
                    settlement::ResultKind::Rejected => Consumption::Cleanup(Reason::Failed),
                };
            }
        };
        if !record.read_reserved(command.sequence) || owner.check_read_source().is_err() {
            record.note_failure(error::delivery()); let _ = command.delivery.send(Err(error::delivery())); return Consumption::Cleanup(Reason::Failed);
        }
        let read = tokio::select! {
            biased;
            _ = ended.cancelled() => { let _ = command.delivery.send(Err(error::ended())); return Consumption::Cleanup(Reason::Canceled); },
            _ = call.cancelled() => { record.abandon(); let _ = command.delivery.send(Err(error::ended())); return Consumption::Cleanup(Reason::Canceled); },
            _ = tokio::time::sleep_until(record.source.deadline()) => { record.abandon(); let _ = command.delivery.send(Err(error::ended())); return Consumption::Cleanup(Reason::Canceled); },
            result = owner.read() => result,
        };
        // 即使EOF协议检查失败，正常transport EOF事实独立记录，但不发eof成功回包。
        if owner.transport_eof() && record.note_transport_eof(command.sequence).is_err() {
            record.note_failure(error::protocol()); let _ = command.delivery.send(Err(error::protocol())); return Consumption::Cleanup(Reason::Failed);
        }
        let chunk = match read {
            Ok(chunk) => chunk,
            Err(failure) => { record.note_failure(failure.clone()); let _ = command.delivery.send(Err(failure)); return Consumption::Cleanup(Reason::Failed); }
        };
        let mut value = match record.offer_read(command.sequence, chunk.eof) {
            Ok(value) => value,
            Err(failure) => { let _ = command.delivery.send(Err(failure)); return Consumption::Cleanup(Reason::Canceled); }
        };
        value["data"] = Value::String(STANDARD.encode(&chunk.bytes));
        if command.delivery.send(Ok(value)).is_err() { record.lose_read(command.sequence); return Consumption::Cleanup(Reason::Canceled); }
        // EOF立即交付，原worker留原证据收本次settle/close，不重读Response。
    }
}

struct OwnedWorker { work: Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>, guard: WorkerGuard }
impl Future for OwnedWorker {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        if this.work.as_mut().is_some_and(|work| work.as_mut().poll(cx).is_pending()) { return Poll::Pending; }
        drop(this.work.take()); this.guard.record.worker_finished(); this.guard.active = false; Poll::Ready(())
    }
}
impl Drop for OwnedWorker { fn drop(&mut self) { drop(self.work.take()); self.guard.lost(); } }
struct WorkerGuard { record: Arc<Record>, registry: Arc<Registry>, active: bool }
impl WorkerGuard {
    fn lost(&mut self) {
        if self.active { if self.registry.owns(&self.record) { self.record.worker_lost(); } self.active = false; }
    }
}
impl Drop for WorkerGuard { fn drop(&mut self) { self.lost(); } }
