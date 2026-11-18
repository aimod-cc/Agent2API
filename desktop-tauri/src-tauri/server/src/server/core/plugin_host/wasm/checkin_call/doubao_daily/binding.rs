//! 原phase3选择与真实Empty快照的内存绑定；不调用SQL终检、网络、保存或生产分派。
//! 私有关联与真回执按值消费，view只能展示；取消/失败不重签，false没有事实出口。

use std::sync::{atomic::Ordering, Arc, Weak};

use super::{
    day, BindingIdentity, BoundDailySelection, DailyBinding, DailyError, DailySelection,
    ObservationOutcome, ObservationView, ObservedFalse, OfficialConfirmation, OfficialDailyFact,
    Target, ORIGIN, PROVIDER,
};
use super::super::{CheckinCallRef, EndReason};
use crate::server::core::account_store::credential_version::doubao_state::SnapshotReceipt;

impl DailySelection {
    /// 只在原第三读前签发一对同关联所有者；账号相同或view相同不能代替私有双ptr核对。
    /// 按值输入失败也不返回选择，不把失败当作新一轮查询机会。
    pub(in crate::server::core::plugin_host::wasm) fn bind_snapshot(
        self, snapshot: &SnapshotReceipt,
    ) -> Result<(BoundDailySelection, DailyBinding), DailyError> {
        empty(snapshot)?;
        if !self.owner.egress.matches_snapshot(snapshot) {
            return Err(DailyError::SourceChanged);
        }
        self.owner.clock()?;
        let identity = Arc::new(BindingIdentity {
            call: Arc::downgrade(&self.owner.call.source), read: self.owner.binding.clone(),
            target: Target {
                discovery_group: self.target.discovery_group.clone(), scene: self.target.scene.clone(),
                item: self.target.item.clone(), endpoint: self.target.endpoint,
            },
            selected_day: self.owner.day.clone(), selected_at: self.owner.selected_at,
            deadline: self.owner.deadline, consumed: std::sync::atomic::AtomicBool::new(false),
        });
        current(&self.owner.call, true, || {
            empty(snapshot)?;
            identity.matches(&self)?;
            if self.owner.phase.load(Ordering::Acquire) != 3
                || !self.owner.egress.matches(&self.owner.call)
                || !self.owner.egress.matches_snapshot(snapshot) {
                return Err(DailyError::SourceChanged);
            }
            self.owner.clock()?;
            if terminal(&self.owner.call) { return Err(DailyError::CallInactive); }
            Ok(())
        })?;
        let binding = DailyBinding { identity: identity.clone() };
        Ok((BoundDailySelection { selection: self, identity }, binding))
    }
}

impl BoundDailySelection {
    /// 按值消费唯一已绑定选择，只调用原第三次固定查询，不加端点、重试或跨日补查。
    pub(in crate::server::core::plugin_host::wasm) async fn query_once(self) -> ObservationOutcome {
        if self.identity.consumed.load(Ordering::Acquire) {
            return ObservationOutcome::NotEntered(DailyError::AlreadyConsumed);
        }
        if let Err(error) = self.identity.matches(&self.selection) {
            return ObservationOutcome::NotEntered(error);
        }
        let before = self.selection.owner.phase.load(Ordering::Acquire);
        if before != 3 {
            return ObservationOutcome::NotEntered(DailyError::AlreadyConsumed);
        }
        let result = self.selection.query_once().await;
        match result {
            Ok((true, observed_at)) => ObservationOutcome::OfficialConfirmation(OfficialConfirmation {
                selection: self, observed_at,
            }),
            Ok((false, observed_at)) => ObservationOutcome::ObservedFalse(ObservedFalse {
                selection: self, observed_at,
            }),
            Err(error) if self.selection.owner.phase.load(Ordering::Acquire) == before => {
                ObservationOutcome::NotEntered(error)
            }
            Err(error) => ObservationOutcome::Unknown(error),
        }
    }
}

impl DailyBinding {
    /// 只消费本次关联的肯定回执；没有bool/ObservedFalse/JSON入口，也不证明本次执行成功。
    /// 这里核私有来源与内存事实，不执行SQL或发查询。原桥仍须核ResultReceipt及自己的消费门。
    pub(in crate::server::core::plugin_host::wasm::checkin_call) fn consume_official(
        self, official: OfficialConfirmation, call: &CheckinCallRef, snapshot: &SnapshotReceipt,
    ) -> Result<OfficialDailyFact, DailyError> {
        if !Arc::ptr_eq(&self.identity, &official.selection.identity) {
            return Err(DailyError::SourceChanged);
        }
        let selection = &official.selection.selection;
        self.identity.matches(selection)?;
        if !Weak::ptr_eq(&self.identity.call, &Arc::downgrade(&call.source))
            || !Arc::ptr_eq(&selection.owner.call.source, &call.source)
            || !selection.owner.egress.matches(call)
            || !selection.owner.egress.matches_snapshot(snapshot) {
            return Err(DailyError::SourceChanged);
        }
        empty(snapshot)?;
        if !matches!(selection.owner.phase.load(Ordering::Acquire), 5 | 9) {
            return Err(DailyError::SourceChanged);
        }
        if official.observed_at < self.identity.selected_at
            || day(official.observed_at)? != self.identity.selected_day {
            return Err(DailyError::CrossDay);
        }
        // 已产生回执可供原执行收尾消费；终态不恢复query资格，来源替换/关闭仍一律拒绝。
        current(call, false, || {
            self.identity.matches(selection)?;
            if !selection.owner.egress.matches_snapshot(snapshot)
                || !matches!(selection.owner.phase.load(Ordering::Acquire), 5 | 9) {
                return Err(DailyError::SourceChanged);
            }
            self.identity.consumed.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .map_err(|_| DailyError::AlreadyConsumed)?;
            Ok(())
        })?;
        Ok(OfficialDailyFact { binding: self, official })
    }
}

impl OfficialDailyFact {
    /// 原存储层只借此封闭核对；不导出RowSource、Db或可以拼配的快照身份。
    pub(in crate::server::core) fn matches_snapshot(&self, snapshot: &SnapshotReceipt) -> bool {
        let selection = &self.official.selection.selection;
        Arc::ptr_eq(&self.binding.identity, &self.official.selection.identity)
            && self.binding.identity.consumed.load(Ordering::Acquire)
            && self.binding.identity.matches(selection).is_ok()
            && matches!(selection.owner.phase.load(Ordering::Acquire), 5 | 9)
            && selection.owner.egress.matches_snapshot(snapshot)
    }

    /// 保留真实固定任务/日期/观测时刻，只作展示或存储固定字段，不允许由view反造事实。
    pub(in crate::server::core) fn view(&self) -> ObservationView<'_> {
        let identity = &self.binding.identity;
        ObservationView {
            discovery_group: &identity.target.discovery_group, scene: &identity.target.scene,
            item_code: &identity.target.item, selected_day: &identity.selected_day,
            selected_at: identity.selected_at, observed_at: self.official.observed_at,
        }
    }
}

impl BindingIdentity {
    fn matches(&self, selection: &DailySelection) -> Result<(), DailyError> {
        let owner = &selection.owner;
        if !Weak::ptr_eq(&self.call, &Arc::downgrade(&owner.call.source))
            || !Arc::ptr_eq(&self.read, &owner.binding) || self.read.marker != 1
            || self.deadline != owner.deadline || self.deadline != owner.call.deadline()
            || self.selected_at != owner.selected_at || self.selected_day != owner.day
            || self.target.discovery_group != selection.target.discovery_group
            || self.target.scene != selection.target.scene || self.target.item != selection.target.item
            || self.target.endpoint != selection.target.endpoint {
            return Err(DailyError::SourceChanged);
        }
        Ok(())
    }
}

fn empty(snapshot: &SnapshotReceipt) -> Result<(), DailyError> {
    let state = snapshot.state();
    if state.schema != 1 {
        return Err(DailyError::SnapshotNotEmpty);
    }
    Ok(())
}

/// 仅原Call->生命周期->Runtime短内存门；不借账号/Db锁，不调用gate::checkpoint或await。
fn current(
    call: &CheckinCallRef, binding: bool, consume: impl FnOnce() -> Result<(), DailyError>,
) -> Result<(), DailyError> {
    let source = &call.source;
    let state = source.signal.state.try_lock().map_err(|_| DailyError::SourceUnavailable)?;
    if source.provider != PROVIDER || !state.bound || !state.entered || !state.context_entered
        || !state.daily_egress_issued {
        return Err(DailyError::CallInactive);
    }
    if binding && (state.execution.is_some() || state.reason.is_some() || source.signal.ended.is_cancelled()) {
        return Err(DailyError::CallInactive);
    }
    if !binding && matches!(state.reason, Some(
        EndReason::HostClosed | EndReason::RuntimeReplaced | EndReason::RuntimeCancelled
        | EndReason::Unavailable | EndReason::InvalidContext | EndReason::EnsureFailed
        | EndReason::PreparationFailed,
    )) {
        return Err(DailyError::SourceChanged);
    }
    if source.runtime_cancel.is_cancelled() { return Err(DailyError::SourceChanged); }
    if binding && tokio::time::Instant::now() >= source.deadline { return Err(DailyError::Deadline); }
    let host = source.host.upgrade().ok_or(DailyError::SourceUnavailable)?;
    let _life = host.lifecycle.try_lock().map_err(|_| DailyError::SourceUnavailable)?;
    let runtime = host.embedded.runtime.try_read().map_err(|_| DailyError::SourceUnavailable)?;
    if !host.ready() || host.closing.load(Ordering::Acquire) || host.activity.retired()
        || super::super::super::super::runtime::admission_closed()
        || host.generation.load(Ordering::Acquire) != source.generation
        || !runtime.as_ref().is_some_and(|runtime| call.matches_runtime(runtime)) {
        return Err(DailyError::SourceChanged);
    }
    let runtime = runtime.as_ref().ok_or(DailyError::SourceChanged)?;
    if !runtime.shared.options.allows("http.v1") || !runtime.shared.options.allows("http.timeout.v1")
        || !runtime.shared.options.allows("checkin.rolling.v1")
        || runtime.shared.options.check_url(ORIGIN, false).is_err() {
        return Err(DailyError::Unauthorized);
    }
    if source.runtime_cancel.is_cancelled() { return Err(DailyError::SourceChanged); }
    if binding && source.signal.ended.is_cancelled() { return Err(DailyError::CallInactive); }
    if host.closing.load(Ordering::Acquire) || host.activity.retired()
        || super::super::super::super::runtime::admission_closed()
        || host.generation.load(Ordering::Acquire) != source.generation {
        return Err(DailyError::SourceChanged);
    }
    consume()
}

fn terminal(call: &CheckinCallRef) -> bool {
    call.source.signal.ended.is_cancelled() || call.source.runtime_cancel.is_cancelled()
}
