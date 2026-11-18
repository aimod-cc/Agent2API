//! 豆包原签到调用的单次结果托管。只接原Context与ExecutionGuard，不启动任务或发送请求。
//! ResultOwner只持弱Source；消费者/回执可保留原事实，但不持Waiter或强Host/Runtime/Shared。
//! 这里只记录内存结果归属，不开放发送；持久桥只消费原回执并保存同源unknown记忆。

mod projection;

use std::sync::{atomic::{AtomicUsize, Ordering}, Arc, Weak};

use agent2api_provider_protocol as proto;
use serde_json::Value;

use super::{CheckinCallRef, EndReason, Source};

const MAX_RESULT_OWNERS: usize = 64;
static RESULT_OWNERS: AtomicUsize = AtomicUsize::new(0);

struct Slot;

impl Slot {
    fn acquire() -> Result<Self, proto::WireError> {
        RESULT_OWNERS.fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count < MAX_RESULT_OWNERS).then_some(count + 1)
        }).map_err(|_| failure("checkin_result_capacity", "签到结果托管容量已满，未开始本次插件执行"))?;
        Ok(Self)
    }
}

impl Drop for Slot {
    fn drop(&mut self) { RESULT_OWNERS.fetch_sub(1, Ordering::AcqRel); }
}

/// 仅指本回执的持久阶段，绝不表示上游写请求是否发送。原回执未进入结果桥时为NotEntered。
/// 原快照CAS的提交事实由结果桥返回SaveOutcome；这里不接bool/插件JSON改变状态。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::server::core::plugin_host::wasm) enum PersistenceStatus {
    Confirmed,
    Unknown,
    NotEntered,
}

/// 普通插件回包不是受控HTTP写回执。当前只能如实记录“远端状态未知”。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::server::core::plugin_host::wasm) enum RemoteStatus { Unknown }

/// 只保留既有签到协议的固定标量；这些是插件报告，不是宿主验证过的远端事实。
/// 不保存message/credit/任意details/原始响应/saved/凭据/代理/验证码或一次性材料。
pub(in crate::server::core::plugin_host::wasm) enum RecordedReply {
    Report {
        outcome: proto::CheckinOutcome,
        completed: Option<bool>,
        sources_complete: Option<bool>,
        query_only: Option<bool>,
        confirming: Option<bool>,
        streak: Option<u32>,
    },
    PluginError { status: Option<u16> },
    InvalidReply,
    NoReply,
}

struct Record {
    execution: EndReason,
    reply: RecordedReply,
    remote: RemoteStatus,
    persistence: PersistenceStatus,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Delivery { Unassigned, Assigned, Consumed, Abandoned }

/// 只在真实原Context入场时创建；Signal->ResultOwner->Weak<Source>不形成引用环。
/// 槽随原Source/Signal释放；即使结果已消费，保留回执也继续占用名额，不能绕过总量上限。
pub(super) struct ResultOwner {
    source: Weak<Source>,
    _slot: Slot,
    delivery: Delivery,
    recorded: bool,
    record: Option<Record>,
}

impl ResultOwner {
    pub(super) fn for_call(call: &CheckinCallRef) -> Result<Option<Self>, proto::WireError> {
        if call.source.provider != "doubaowork" { return Ok(None); }
        Ok(Some(Self {
            source: Arc::downgrade(&call.source), _slot: Slot::acquire()?,
            delivery: Delivery::Unassigned, recorded: false, record: None,
        }))
    }

    fn belongs_to(&self, source: &Arc<Source>) -> bool {
        Weak::ptr_eq(&self.source, &Arc::downgrade(source))
    }

    /// 仅给同级封闭结果桥；不把记录字段或可改写保存状态交给外层调用者。
    pub(super) fn pending_for_snapshot(&self, source: &Arc<Source>) -> bool {
        self.belongs_to(source) && !self.recorded && self.record.is_none()
            && self.delivery == Delivery::Unassigned
    }

    pub(super) fn delivered_for_snapshot(&self, source: &Arc<Source>, execution: EndReason) -> bool {
        self.belongs_to(source) && self.recorded && self.record.is_none()
            && self.delivery == Delivery::Consumed && matches!(execution,
                EndReason::ExecutionReturned | EndReason::ExecutionFailed | EndReason::ExecutionDropped
                    | EndReason::Trap | EndReason::ResourceLimit | EndReason::Timeout | EndReason::RuntimeCancelled)
    }

    /// 调用者持原Signal锁；只由原ExecutionGuard的首个真实返回/Drop事件调用。
    /// 等待方离开不调用此入口，因此不会用提前的“未知”覆盖后到达的真实运行结果。
    pub(super) fn seal(&mut self, execution: EndReason, reply: Option<&Result<Value, proto::WireError>>) {
        if self.recorded { return; }
        self.recorded = true;
        let reply = match reply {
            Some(Ok(value)) => projection::read(value),
            Some(Err(_)) | None => RecordedReply::NoReply,
        };
        self.record = Some(Record {
            execution, reply, remote: RemoteStatus::Unknown,
            persistence: PersistenceStatus::NotEntered,
        });
    }
}

/// 唯一消费凭证，无Clone/serde/公开构造。不是发送、查库或保存许可。
#[must_use]
pub(in crate::server::core::plugin_host::wasm) struct ResultConsumer {
    source: Arc<Source>,
    consumed: bool,
}

impl CheckinCallRef {
    /// 只在原Context已入场后发放一次。终态后仍可读取历史结果，不复活原调用或期限。
    /// 原结果桥须在真实入场时先绑定最初快照；本入口仍不查账号、不注册能力或后台等待。
    pub(in crate::server::core::plugin_host::wasm) fn take_result_consumer(&self) -> Result<ResultConsumer, proto::WireError> {
        let mut state = self.source.signal.state.lock().map_err(|_| unavailable())?;
        if !state.bound || !state.entered || !state.context_entered { return Err(unavailable()); }
        let owner = state.result.as_mut().filter(|owner| owner.belongs_to(&self.source)).ok_or_else(unavailable)?;
        if owner.delivery != Delivery::Unassigned { return Err(consumed()); }
        owner.delivery = Delivery::Assigned;
        Ok(ResultConsumer { source: self.source.clone(), consumed: false })
    }
}

impl ResultConsumer {
    /// 仅向同一个私有Source交付一次；相同provider/account的新调用也不匹配。
    /// None只是原执行尚未结束，不延长期限、不产生重试/轮询或未发送结论。
    pub(in crate::server::core::plugin_host::wasm) fn try_take(&mut self, call: &CheckinCallRef) -> Result<Option<ResultReceipt>, proto::WireError> {
        if !Arc::ptr_eq(&self.source, &call.source) { return Err(unavailable()); }
        if self.consumed { return Err(consumed()); }
        let mut state = self.source.signal.state.lock().map_err(|_| unavailable())?;
        let owner = state.result.as_mut().filter(|owner| owner.belongs_to(&self.source)).ok_or_else(unavailable)?;
        if owner.delivery != Delivery::Assigned { return Err(consumed()); }
        let Some(record) = owner.record.take() else { return Ok(None); };
        owner.delivery = Delivery::Consumed;
        self.consumed = true;
        drop(state);
        Ok(Some(ResultReceipt { source: self.source.clone(), record }))
    }
}

impl Drop for ResultConsumer {
    fn drop(&mut self) {
        if self.consumed { return; }
        let mut state = self.source.signal.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(owner) = state.result.as_mut().filter(|owner| owner.belongs_to(&self.source)) {
            if owner.delivery == Delivery::Assigned { owner.delivery = Delivery::Abandoned; }
        }
        // 丢弃消费者只封死交付资格：不重置结果、不取消旧执行、更不安排重发。
    }
}

/// 回执不可Clone/serde；保存阶段只读，没有accept_saved(bool)或任意闭包“确认”入口。
/// 所有权只证明结果来自哪次调用；不替代原RowSource出生版本、授权投影及状态CAS。
#[must_use]
pub(in crate::server::core::plugin_host::wasm) struct ResultReceipt {
    source: Arc<Source>,
    record: Record,
}

impl ResultReceipt {
    pub(in crate::server::core::plugin_host::wasm) fn belongs_to(&self, call: &CheckinCallRef) -> bool {
        Arc::ptr_eq(&self.source, &call.source)
    }

    pub(in crate::server::core::plugin_host::wasm) fn execution(&self) -> EndReason { self.record.execution }

    pub(in crate::server::core::plugin_host::wasm) fn reply(&self) -> &RecordedReply { &self.record.reply }

    pub(in crate::server::core::plugin_host::wasm) fn remote(&self) -> RemoteStatus { self.record.remote }

    pub(in crate::server::core::plugin_host::wasm) fn persistence(&self) -> PersistenceStatus { self.record.persistence }
}

fn failure(code: &str, message: &str) -> proto::WireError { proto::WireError::with_code(message, code) }
fn unavailable() -> proto::WireError { failure("checkin_result_unavailable", "签到结果不属于原调用或托管状态不可用") }
fn consumed() -> proto::WireError { failure("checkin_result_consumed", "本次签到结果的唯一消费资格已交付或放弃") }
