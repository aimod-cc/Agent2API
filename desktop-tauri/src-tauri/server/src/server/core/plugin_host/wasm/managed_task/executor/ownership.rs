//! 预留与实际准备分开，但始终只有一个退出所有者；不把准备句柄交到executor之外。
//! 外层Future先销毁内层整个worker（含未poll输入/待处理材料），随后才settle/release。

use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use tokio::sync::oneshot;

use super::{
    super::{
        input::PreparedInput,
        lifecycle::Record,
        registry,
        status::{EndReason, NotSentReason, StageResult},
    },
    Delivery, Worker,
};

pub(super) enum WorkerExit {
    NotSent(NotSentReason),
    FinalCheckFailed(NotSentReason),
    SubmittedUnknown,
    Finished,
}

/// 仅start栈内持有，不含客户端/凭据/回调；取得一次预留后只能按值移入原worker。
pub(super) struct Reserved {
    guard: WorkerGuard,
}

impl Reserved {
    pub(super) fn acquire(
        record: Arc<Record>,
        start: oneshot::Sender<()>,
    ) -> Result<Self, NotSentReason> {
        // 退出保护先于预留建立；预留成功后没有可失败的所有权空窗。
        let mut guard = WorkerGuard {
            record,
            active: false,
        };
        registry::reserve(&guard.record, guard.record.binding(), start)?;
        guard.active = true;
        Ok(Self { guard })
    }

    pub(super) fn into_worker(
        self,
        input: PreparedInput,
        ready: oneshot::Receiver<()>,
        delivery: oneshot::Sender<Delivery>,
    ) -> impl Future<Output = ()> + Send + 'static {
        let worker = Worker {
            record: self.guard.record.clone(),
            delivery: Some(delivery),
        };
        OwnedWorker {
            work: Some(Box::pin(worker.run(input, ready))),
            guard: self.guard,
        }
    }
}

struct OwnedWorker {
    // 构造器不接受任意future/闭包。唯一来源是同一个Worker::run，仍只spawn一次。
    work: Option<Pin<Box<dyn Future<Output = WorkerExit> + Send + 'static>>>,
    guard: WorkerGuard,
}

impl Future for OwnedWorker {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let outcome = match this.work.as_mut() {
            Some(work) => match work.as_mut().poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(outcome) => outcome,
            },
            None => return Poll::Ready(()),
        };
        // 输出没有重材料；即使已Ready也显式销毁整份future，不依赖其局部变量析构顺序。
        drop(this.work.take());
        this.guard.finish(outcome);
        Poll::Ready(())
    }
}

impl Drop for OwnedWorker {
    fn drop(&mut self) {
        // panic/abort/runtime退出/从未poll都先毁掉准备或网络future，再由guard结清。
        drop(self.work.take());
        self.guard.lost();
    }
}

struct WorkerGuard {
    record: Arc<Record>,
    active: bool,
}

impl WorkerGuard {
    fn finish(&mut self, outcome: WorkerExit) {
        if !self.active {
            return;
        }
        match outcome {
            WorkerExit::NotSent(reason) => self.record_not_sent(reason),
            WorkerExit::FinalCheckFailed(reason) => {
                self.record.request_stop(EndReason::ExecutionInterrupted);
                self.record_not_sent(reason);
            }
            WorkerExit::SubmittedUnknown => {
                self.record.complete_creation(StageResult::unknown());
                self.record.request_stop(EndReason::ExecutionInterrupted);
            }
            WorkerExit::Finished => {}
        }
        self.record.settle(false);
        registry::release(&self.record);
        self.active = false;
    }

    fn record_not_sent(&self, reason: NotSentReason) {
        // 回调进入不等于CAS成功；只在原子机会确实未消耗时记录NotSent。
        if self.record.submitted() {
            self.record.complete_creation(StageResult::unknown());
            self.record.request_stop(EndReason::ExecutionInterrupted);
        } else {
            self.record.revert_preparation();
            self.record.lock().snapshot.mark_not_sent(reason);
        }
    }

    fn lost(&mut self) {
        if !self.active {
            return;
        }
        // 能确认未提交时保留原取消/期限原因，不把所有drop都覆盖成运行时故障。
        if !self.record.submitted() {
            if let Some(reason) = self.record.preparing_error() {
                self.finish(WorkerExit::NotSent(reason));
                return;
            }
        }
        self.record.request_stop(EndReason::ExecutorLost);
        self.record.settle(true);
        registry::release(&self.record);
        self.active = false;
    }
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.lost();
    }
}
