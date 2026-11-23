//! 原豆包签到执行的结果保存；官方观测、已存历史和本次未知执行分别保留。
//! 原快照在插件执行前绑定，结束后不重读换版；没有签到发送或后台回查许可。
//! 归还借用只覆盖正常Runtime收尾，不承诺宿主任务直接abort或进程退出后的独立保存。

mod daily;
mod summary;

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock, Weak,
};
use std::time::Duration;

use agent2api_provider_protocol as proto;
use serde_json::{json, Value};
use tokio::{sync::Semaphore, time::Instant};

use super::super::runtime::Runtime;
use super::{
    bind_result_snapshot, BoundResultSnapshot, CheckinCallRef, RecordedReply, ResultConsumer,
    Source,
};
use crate::server::core::account_store::{
    credential_version::{
        doubao_state::{SaveOutcome, SnapshotOutcome},
        RowProvider,
    },
    plugin_accounts,
};
use summary::OfficialSummary;

const EXECUTION_READY: bool = true;

pub(in crate::server::core::plugin_host::wasm) fn guard(
    provider: &str,
) -> Result<(), proto::WireError> {
    if provider == "doubaowork" && !EXECUTION_READY {
        return Err(proto::WireError::with_code(
            "豆包工作签到的发送与官方确认链尚未接通，未执行签到",
            "doubao_checkin_not_connected",
        ));
    }
    Ok(())
}

#[derive(Clone)]
pub(in crate::server::core::plugin_host::wasm) struct PersistenceSlot {
    inner: Arc<Slot>,
}

struct Slot {
    source: Weak<Source>,
    prepared: AtomicBool,
    consumed: AtomicBool,
    collector: Mutex<Option<Collector>>,
}

struct Collector {
    bound: BoundResultSnapshot,
    consumer: ResultConsumer,
    official: Option<super::OfficialDailyFact>,
    summary: Option<OfficialSummary>,
    submitted: bool,
    claimed: bool,
    rollback_pending: bool,
    rejection: Option<super::doubao_daily::SubmitRejection>,
    failure: Option<super::doubao_daily::DailyError>,
}

pub(in crate::server::core::plugin_host) struct Settlement {
    saved: Option<bool>,
    reported: bool,
    summary: Option<OfficialSummary>,
    save_requested: bool,
    submitted: bool,
    claimed: bool,
    rollback_pending: bool,
    rejection: Option<super::doubao_daily::SubmitRejection>,
    failure: Option<super::doubao_daily::DailyError>,
    pending: bool,
}

impl PersistenceSlot {
    pub(in crate::server::core::plugin_host::wasm) fn for_call(call: &CheckinCallRef) -> Self {
        Self {
            inner: Arc::new(Slot {
                source: Arc::downgrade(&call.source),
                prepared: AtomicBool::new(false),
                consumed: AtomicBool::new(false),
                collector: Mutex::new(None),
            }),
        }
    }

    pub(in crate::server::core::plugin_host::wasm) async fn bind(
        &self,
        call: &CheckinCallRef,
        runtime: &Runtime,
    ) -> Result<(), proto::WireError> {
        if call.source.provider != "doubaowork"
            || !runtime.shared.options.allows("checkin.rolling.v1")
        {
            return Ok(());
        }
        if !Weak::ptr_eq(&self.inner.source, &Arc::downgrade(&call.source))
            || !call.matches_runtime(runtime)
            || self.inner.prepared.swap(true, Ordering::AcqRel)
        {
            return Err(unavailable());
        }
        current(call)?;
        let store = plugin_accounts::shared().ok_or_else(unavailable)?;
        let permit = slots().try_acquire_owned().map_err(|_| unavailable())?;
        let deadline = call.deadline().min(Instant::now() + Duration::from_secs(2));
        let original = call.clone();
        let work = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            current(&original)?;
            if Instant::now() >= deadline {
                return Err(unavailable());
            }
            let row = store
                .prepare_maintenance_row_source(RowProvider::DoubaoWork, &original.source.account)
                .map_err(|_| unavailable())?;
            let snapshot = match row.read_doubao_state() {
                SnapshotOutcome::Read(snapshot) => snapshot,
                _ => return Err(unavailable()),
            };
            current(&original)?;
            if Instant::now() >= deadline {
                return Err(unavailable());
            }
            Ok::<_, proto::WireError>(snapshot)
        });
        let snapshot = tokio::select! {
            biased;
            _ = call.source.signal.ended.cancelled() => return Err(unavailable()),
            _ = call.source.runtime_cancel.cancelled() => return Err(unavailable()),
            result = tokio::time::timeout_at(deadline, work) => result
                .map_err(|_| unavailable())?.map_err(|_| unavailable())??,
        };
        current(call)?;
        let summary = OfficialSummary::from_snapshot(&snapshot)?;
        let bound = bind_result_snapshot(call, snapshot).map_err(|_| unavailable())?;
        let consumer = call.take_result_consumer()?;
        let collector = Collector {
            bound,
            consumer,
            official: None,
            summary,
            submitted: false,
            claimed: false,
            rollback_pending: false,
            rejection: None,
            failure: None,
        };
        let mut slot = self.inner.collector.try_lock().map_err(|_| unavailable())?;
        if !self.inner.prepared.load(Ordering::Acquire)
            || self.inner.consumed.load(Ordering::Acquire)
            || slot.is_some()
        {
            return Err(unavailable());
        }
        *slot = Some(collector);
        Ok(())
    }

    pub(in crate::server::core::plugin_host::wasm) async fn settle(
        self,
        call: &CheckinCallRef,
    ) -> Result<Option<Settlement>, proto::WireError> {
        if !Weak::ptr_eq(&self.inner.source, &Arc::downgrade(&call.source)) {
            return Err(unavailable());
        }
        // 取锁成功后才核对/消费；锁忙或借出不能先封死归还，也不能视为合法空槽。
        let collector = {
            let mut slot = self.inner.collector.try_lock().map_err(|_| unavailable())?;
            if self.inner.consumed.load(Ordering::Acquire) {
                return Err(unavailable());
            }
            if call.source.provider != "doubaowork" {
                if slot.is_some() || self.inner.prepared.load(Ordering::Acquire) {
                    return Err(unavailable());
                }
                self.inner.consumed.store(true, Ordering::Release);
                return Ok(None);
            }
            if !self.inner.prepared.load(Ordering::Acquire) {
                return Err(unavailable());
            }
            let original = slot.as_ref().ok_or_else(unavailable)?;
            match (&original.official, &original.summary) {
                (Some(fact), Some(summary)) if summary.matches_fact(fact) => {}
                (None, Some(summary)) if summary.historical() => {}
                (None, None) => {}
                _ => return Err(unavailable()),
            }
            let collector = slot.take().ok_or_else(unavailable)?;
            self.inner.consumed.store(true, Ordering::Release);
            collector
        };
        let Collector {
            bound,
            mut consumer,
            official,
            summary,
            submitted,
            claimed,
            rollback_pending,
            rejection,
            failure,
        } = collector;
        let pending = bound.pending_today();
        let result = consumer.try_take(call)?.ok_or_else(unavailable)?;
        let reported = matches!(result.reply(), RecordedReply::Report { .. });
        if summary.as_ref().is_some_and(OfficialSummary::historical) {
            // 原快照已有真实官方记忆；没有本次Fact，不发新查询，也不尝试save_unknown降级。
            drop(result);
            drop(bound);
            return Ok(Some(Settlement {
                saved: Some(false),
                reported,
                summary,
                save_requested: false,
                submitted,
                claimed,
                rollback_pending: false,
                rejection: None,
                failure,
                pending,
            }));
        }
        if official.is_none() && !bound.pending_reserved() {
            drop(result);
            drop(bound);
            return Ok(Some(Settlement {
                saved: Some(false), reported, summary, save_requested: false,
                submitted, claimed, rollback_pending: false, rejection, failure, pending,
            }));
        }
        // 同步保存任务仍按值持原桥/回执/typed Fact；摘要只展示，不能替代它的保存许可。
        let outcome = tokio::task::spawn_blocking(move || match official {
            Some(fact) => bound.save_official_fact(result, fact),
            None if rollback_pending => bound.clear_pending(result),
            None => bound.save_unknown(result),
        })
        .await;
        let saved = match outcome {
            Ok(SaveOutcome::Committed(_)) => Some(true),
            Ok(SaveOutcome::NotEntered { .. } | SaveOutcome::Rejected { .. }) => Some(false),
            Ok(SaveOutcome::CommitUnknown { .. }) | Err(_) => None,
        };
        Ok(Some(Settlement {
            saved,
            reported,
            summary,
            save_requested: true,
            submitted,
            claimed,
            rollback_pending,
            rejection,
            failure,
            pending,
        }))
    }
}

impl Settlement {
    pub(in crate::server::core::plugin_host) fn claim(self) -> Value {
        let now = crate::server::logging::now_ms();
        let historical = self
            .summary
            .as_ref()
            .is_some_and(OfficialSummary::historical);
        let official = self
            .summary
            .as_ref()
            .and_then(|summary| summary.current_projection(now));
        let message = match &self.summary {
            Some(summary) => summary.message(self.saved, now),
            None => match self.saved {
                Some(true) => {
                    "豆包工作本次远端签到状态未知，未知记忆已保存；不记签到成功，也不会自动重发"
                }
                Some(false) => {
                    "豆包工作本次远端签到状态未知，未知记忆未保存；不记签到成功，也不会自动重发"
                }
                None => {
                    "豆包工作本次远端签到状态未知，未知记忆保存未确认；不记签到成功，也不会自动重发"
                }
            },
        };
        let confirmed_today = official.is_some();
        let completed = confirmed_today && (historical || self.saved == Some(true));
        let inactive = matches!(self.failure, Some(super::doubao_daily::DailyError::NoActivity | super::doubao_daily::DailyError::NoDaily));
        let rolled_back = self.rollback_pending && self.saved == Some(true);
        let pending = !completed && !rolled_back && (self.pending || self.save_requested);
        let rejected_text = self.rejection.as_ref().map(|rej| match rej.code {
            Some(code) => format!("豆包工作签到被拒绝（{code}）：{}", rej.message),
            None => format!("豆包工作签到被拒绝：{}", rej.message),
        });
        let message = if completed {
            "豆包工作今日签到已由官方确认；本次收益未查询，不补零或猜测奖励".to_string()
        } else if pending && self.submitted {
            "豆包工作本次领取结果仍待确认；领取记录已保留，再次点击仅查询结果，不会重复领取".to_string()
        } else if pending {
            "豆包工作原签到结果仍待确认；本次仅回查，没有再次领取，原待确认记录保持不变".to_string()
        } else if let Some(text) = rejected_text {
            text
        } else if !self.save_requested {
            daily_failure_message(self.failure).to_string()
        } else {
            message.to_string()
        };
        let history_saved = historical.then_some(true);
        let history_applies_today = historical.then_some(confirmed_today);
        json!({
            "success": completed && self.claimed, "alreadyCompleted": completed && !self.claimed,
            "outcome": if completed { if self.claimed { "claimed" } else { "done" } }
                else if inactive { "inactive" } else { "failed" },
            "credit": null, "streak": null, "msg": message, "retryAt": null,
            "dailyCheckin": {
                "status": if confirmed_today { "official_confirmed" } else if inactive { "inactive" }
                    else if pending { "confirming" } else { "failed" },
                "completed": if completed { Some(true) } else { None }, "execution": "unknown", "saved": self.saved,
                "saveRequested": self.save_requested, "historySaved": history_saved,
                "historyAppliesToday": history_applies_today, "official": official,
                "pluginReported": self.reported, "queryOnly": !self.submitted, "sourcesComplete": completed,
                "confirming": pending, "retryAt": null, "message": message,
            },
        })
    }
}

fn daily_failure_message(error: Option<super::doubao_daily::DailyError>) -> &'static str {
    use super::doubao_daily::DailyError;
    match error {
        Some(DailyError::NoActivity) => "豆包工作官方未向当前账号开放签到活动；本次未领取，也未新增待确认记录",
        Some(DailyError::NoDaily) => "豆包工作当前活动没有每日签到项；本次未领取，也未新增待确认记录",
        Some(DailyError::CredentialsInvalid) => "豆包工作登录快照无法用于活动请求，请重新导入当前账号登录态；本次未领取",
        Some(DailyError::ResponseRejected | DailyError::SubmitRejected) => "豆包工作活动请求被官方拒绝，请在官方客户端核对登录和活动资格；本次未领取，也未新增待确认记录",
        Some(DailyError::AmbiguousDaily | DailyError::MissingOriginalTask | DailyError::ResponseInvalid) => "豆包工作活动查询未确认唯一的原签到任务；本次未领取，也未新增待确认记录",
        Some(DailyError::CrossDay | DailyError::InvalidClock | DailyError::Deadline) => "豆包工作本次签到查询已超时或日期变化；本次未领取，也未新增待确认记录",
        _ => "豆包工作本次账号核对或活动查询未完成；未领取，也未新增当天待确认记录，可重新查询",
    }
}

fn current(call: &CheckinCallRef) -> Result<(), proto::WireError> {
    let state = call
        .source
        .signal
        .state
        .try_lock()
        .map_err(|_| unavailable())?;
    if !state.bound
        || !state.entered
        || !state.context_entered
        || state.execution.is_some()
        || state.reason.is_some()
        || call.source.signal.ended.is_cancelled()
        || Instant::now() >= call.source.deadline
    {
        return Err(unavailable());
    }
    let host = call.source.host.upgrade().ok_or_else(unavailable)?;
    let _life = host.lifecycle.try_lock().map_err(|_| unavailable())?;
    let runtime = host
        .embedded
        .runtime
        .try_read()
        .map_err(|_| unavailable())?;
    if !host.ready()
        || host.activity.retired()
        || super::super::super::runtime::admission_closed()
        || host.generation.load(Ordering::Acquire) != call.source.generation
        || call.source.runtime_cancel.is_cancelled()
        || !runtime
            .as_ref()
            .is_some_and(|runtime| call.matches_runtime(runtime))
    {
        return Err(unavailable());
    }
    Ok(())
}

fn slots() -> Arc<Semaphore> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(Semaphore::new(4))).clone()
}

fn unavailable() -> proto::WireError {
    proto::WireError::with_code(
        "豆包工作原签到结果与账号快照未完成同源绑定，未继续执行或补签保存资格",
        "doubao_result_snapshot_unavailable",
    )
}
