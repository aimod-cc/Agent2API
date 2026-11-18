//! 宿主跟踪的有界 worker：Preparing -> Creating -> Owned -> Stopping -> Finished。
//! 预留先冻结replay，唯一worker才消费出口；重材料不交给外部准备句柄。
//! 只有Preparing可立即取消；Creating保留同一次future，停止只消费真实创建收据。
//! 最终资格只经批准/安装/账号同步检查转换；真实批准签发和生产入口仍关闭。

mod ownership;
mod preparation;

use std::sync::Arc;

use tokio::{
    runtime::Handle,
    sync::{mpsc, oneshot},
};

use self::ownership::{Reserved, WorkerExit};

use super::{
    input::PreparedInput,
    lifecycle::{CreatedReceipt, Invocation, Record},
    network::{CreateResult, ObservationEnd, ObservationFailure, Session},
    receipt::{Failure, StopOutcome},
    registry,
    status::{EndReason, NotSentReason, ResultKind, Snapshot, StageResult},
};

pub(super) enum StartResult {
    // 仅本次start未发送；同一Invocation此前可能已提交，必须同时读取禁止重放标记。
    NotSent {
        reason: NotSentReason,
        replay_forbidden: bool,
    },
    Waiting(CreationWait),
}

pub(super) enum Delivery {
    Created(CreatedReceipt),
    NotSent(NotSentReason),
    Rejected {
        result: StageResult,
        replay_forbidden: bool,
    },
    Unknown {
        result: StageResult,
        replay_forbidden: bool,
    },
    CallEnded(Snapshot),
}

/// wait(self)将整个接收端带进future；取消/drop它立刻置结束意图，不取消创建worker。
/// 通道只容纳一个结果，终态由Invocation读取，不积累事件或消息历史。
pub(super) struct CreationWait {
    receiver: oneshot::Receiver<Delivery>,
    record: Arc<Record>,
    completed: bool,
}

impl CreationWait {
    pub(super) async fn wait(mut self) -> Delivery {
        // 已到达的结果优先保留；Created仍须在返回前重新核对粘性结束意图。
        let delivered = tokio::select! {
            biased;
            result = &mut self.receiver => result,
            _ = self.record.ended() => {
                self.completed = true;
                if !self.record.submitted() {
                    // 原所有者释放准备材料并结清后，读取它实际记录的原因。
                    self.record.settled().await;
                    return Delivery::NotSent(settled_not_sent(&self.record));
                }
                return Delivery::CallEnded(self.record.snapshot());
            }
        };
        self.completed = true;
        match delivered {
            Ok(Delivery::Created(receipt)) => {
                if self.record.accept_receipt_delivery() {
                    Delivery::Created(receipt)
                } else {
                    drop(receipt);
                    Delivery::CallEnded(self.record.snapshot())
                }
            }
            Ok(Delivery::NotSent(_)) => {
                self.record.settled().await;
                Delivery::NotSent(settled_not_sent(&self.record))
            }
            Ok(result) => result,
            Err(_) => {
                // 通道先关闭可能只是原future正在退场，不能抢先覆盖其真实结束原因。
                self.record.settled().await;
                let snapshot = self.record.snapshot();
                if snapshot.submitted() {
                    match snapshot.creation().kind() {
                        ResultKind::Rejected => Delivery::Rejected {
                            result: snapshot.creation(),
                            replay_forbidden: true,
                        },
                        ResultKind::Unknown => Delivery::Unknown {
                            result: snapshot.creation(),
                            replay_forbidden: true,
                        },
                        _ => Delivery::CallEnded(snapshot),
                    }
                } else {
                    Delivery::NotSent(settled_not_sent(&self.record))
                }
            }
        }
    }

    pub(super) fn snapshot(&self) -> Snapshot {
        self.record.snapshot()
    }
}

impl Drop for CreationWait {
    fn drop(&mut self) {
        if !self.completed {
            self.record.request_stop(EndReason::ReceiverDropped);
        }
    }
}

fn settled_not_sent(record: &Record) -> NotSentReason {
    let snapshot = record.snapshot();
    snapshot.not_sent_reason().unwrap_or_else(|| {
        if snapshot.reason().is_some() {
            NotSentReason::CallEnded
        } else {
            NotSentReason::RuntimeUnavailable
        }
    })
}

/// 无await的预留与跟踪；出口消费/冻结只能在同一个worker收到start之后执行。
/// Tokio::spawn不会同步poll，但关闭中的runtime可能立即drop future；因此绝不持锁spawn。
/// 此封闭入口没有生产调用者；只接真实私有输入，不接预先冻结的客户端。
pub(super) fn start(invocation: &Invocation, input: PreparedInput) -> StartResult {
    if !Arc::ptr_eq(input.binding(), invocation.record().binding()) {
        return StartResult::NotSent {
            reason: NotSentReason::InvalidBinding,
            replay_forbidden: invocation.replay_forbidden(),
        };
    }
    let runtime = match Handle::try_current() {
        Ok(runtime) => runtime,
        Err(_) => {
            return StartResult::NotSent {
                reason: NotSentReason::RuntimeUnavailable,
                replay_forbidden: invocation.replay_forbidden(),
            }
        }
    };
    let record = invocation.record().clone();
    let (start, ready) = oneshot::channel();
    let (delivery, receiver) = oneshot::channel();
    let reserved = match Reserved::acquire(record.clone(), start) {
        Ok(reserved) => reserved,
        Err(reason) => {
            return StartResult::NotSent {
                reason,
                replay_forbidden: invocation.replay_forbidden(),
            }
        }
    };
    // Reserved及其退出保护在任何future之外建立，从未poll也只有这一个结清所有者。
    let join = runtime.spawn(reserved.into_worker(input, ready, delivery));
    let abort = join.abort_handle();
    if !registry::track(&record, abort.clone()) {
        // 不由调用者release：只有原任务的退出保护能在材料销毁后结清。
        abort.abort();
    }
    drop(join);
    StartResult::Waiting(CreationWait {
        receiver,
        record,
        completed: false,
    })
}

struct Worker {
    record: Arc<Record>,
    delivery: Option<oneshot::Sender<Delivery>>,
}

impl Worker {
    async fn run(mut self, input: PreparedInput, ready: oneshot::Receiver<()>) -> WorkerExit {
        let (session, request, permit, nonce) =
            match preparation::prepare(&self.record, input, ready).await {
                Ok(prepared) => prepared,
                Err(registry::FinalizeFailure::NotSent(reason)) => {
                    return WorkerExit::NotSent(reason)
                }
                Err(registry::FinalizeFailure::EnteredWithoutSubmission(reason)) => {
                    return WorkerExit::FinalCheckFailed(reason);
                }
                Err(registry::FinalizeFailure::SubmittedUnknown) => {
                    return WorkerExit::SubmittedUnknown
                }
            };
        self.execute(session, request, permit, nonce).await
    }

    async fn execute(
        &mut self,
        mut session: Session,
        request: super::network::PreparedCreate,
        permit: registry::CreatePermit,
        nonce: [u8; 32],
    ) -> WorkerExit {
        // 固定deadline；观察普通取消仅置意图，不丢弃或重建创建future。
        let creation = {
            let creating = session.create(request, permit);
            tokio::pin!(creating);
            tokio::select! {
                biased;
                _ = self.record.ended() => {
                    // 只登记停止意图，继续poll同一个创建future；没有取消/重建或重计时。
                    creating.as_mut().await
                }
                result = &mut creating => result,
            }
        };
        match creation {
            CreateResult::Created(creation) => {
                let (commands, incoming) = mpsc::channel(1);
                let receipt = registry::record_created(&self.record, creation, nonce, commands);
                if let Some(receipt) = receipt {
                    self.deliver(Delivery::Created(receipt));
                } else {
                    self.deliver(Delivery::CallEnded(self.record.snapshot()));
                }
                // 原worker等唯一观察命令；没有命令或取消都继续承担同一次stop责任。
                self.observe_owned(&session, incoming).await;
                self.stop_once(&mut session).await;
            }
            CreateResult::Rejected(failure) => {
                let result = failure_result(ResultKind::Rejected, failure);
                self.record.complete_creation(result);
                self.deliver(Delivery::Rejected {
                    result,
                    replay_forbidden: true,
                });
            }
            CreateResult::Unknown(failure) => {
                let result = failure_result(ResultKind::Unknown, failure);
                self.record.complete_creation(result);
                self.deliver(Delivery::Unknown {
                    result,
                    replay_forbidden: true,
                });
            }
        }
        drop(session);
        // 只交回无材料结果；外层唯一所有者先销毁整个future，再结清/释放名额。
        WorkerExit::Finished
    }

    fn deliver(&mut self, value: Delivery) {
        let Some(sender) = self.delivery.take() else {
            self.record.request_stop(EndReason::ReceiverDropped);
            return;
        };
        if matches!(&value, Delivery::Created(_)) && !self.record.accept_receipt_delivery() {
            drop(value);
            let _ = sender.send(Delivery::CallEnded(self.record.snapshot()));
            return;
        }
        if let Err(value) = sender.send(value) {
            self.record.request_stop(EndReason::ReceiverDropped);
            drop(value);
        }
        // send(Ok)不等于接收成功。CreationWait以及CreatedReceipt的Drop继续负责置位。
    }

    async fn observe_owned(
        &self,
        session: &Session,
        mut incoming: mpsc::Receiver<registry::ObserveCommand>,
    ) {
        let record = &self.record;
        let command = tokio::select! {
            biased;
            _ = record.ended() => None,
            command = incoming.recv() => command,
        };
        incoming.close();
        drop(incoming);
        let Some(command) = command else {
            if record.end_reason().is_none() {
                record.request_stop(EndReason::ExecutorLost);
            }
            record.finish_observation(ObservationEnd::Failed(ObservationFailure::Closed));
            return;
        };
        let (permit, output) = command.into_parts();
        // observe返回前已drop所有读取future和实际socket；终态不通过有界文本队列等待。
        let ended = session.observe(record, permit, &output).await;
        record.finish_observation(ended);
        record.request_stop(match ended {
            ObservationEnd::RoundEnded => EndReason::Normal,
            _ => EndReason::ExecutionInterrupted,
        });
        drop(output);
    }

    async fn stop_once(&self, session: &mut Session) {
        let Some(creation) = registry::begin_stopping(&self.record) else {
            self.record.complete_stop(StageResult::unknown());
            return;
        };
        // 同一stop机会包含登记+构造+响应读取，最多20秒；不依赖已到期的普通WASM截止。
        let outcome = match session.prepare_stop(creation) {
            Ok(request) => session.stop(request).await,
            Err(_) => {
                self.record.complete_stop(StageResult::new(
                    ResultKind::Unknown,
                    None,
                    None,
                    "固定停止请求未能提交，远端任务状态未确认；不更换出口或重试",
                ));
                return;
            }
        };
        let result = match outcome {
            StopOutcome::Acknowledged { http_status } => StageResult::new(
                ResultKind::Acknowledged,
                Some(http_status),
                Some(0),
                "停止接口已确认受理；不能据此证明执行环境已回收",
            ),
            StopOutcome::Rejected(failure) => failure_result(ResultKind::Rejected, failure),
            StopOutcome::Unknown(failure) => failure_result(ResultKind::Unknown, failure),
        };
        self.record.complete_stop(result);
    }
}

fn failure_result(kind: ResultKind, failure: Failure) -> StageResult {
    StageResult::new(
        kind,
        failure.http_status(),
        failure.business_code(),
        failure.message(),
    )
}
