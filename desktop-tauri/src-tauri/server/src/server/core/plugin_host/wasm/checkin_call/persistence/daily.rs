//! 本次豆包选择、原空快照与结果桥共持；只有原调用的真实肯定查询可移交官方事实。
//! 读取结果不是本次领取成功；已有官方快照只留历史摘要，不补查询、降级保存或按日期换任务。
//! 归还守卫仅覆盖原Runtime正常返回/超时取消，不构成宿主任务abort后的独立保存所有者。

use std::sync::{atomic::Ordering, Arc};

use super::super::doubao_daily::{self, ObservationOutcome, SelectionOutcome};
use super::{
    current, summary::OfficialSummary, unavailable, CheckinCallRef, Collector, PersistenceSlot,
};
use agent2api_provider_protocol as proto;

struct BorrowedCollector {
    owner: Arc<super::Slot>,
    collector: Option<Collector>,
}

impl Drop for BorrowedCollector {
    fn drop(&mut self) {
        let Some(collector) = self.collector.take() else {
            return;
        };
        let mut retained = Some(collector);
        {
            let mut slot = self
                .owner
                .collector
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if slot.is_none()
                && !self
                    .owner
                    .consumed
                    .load(std::sync::atomic::Ordering::Acquire)
            {
                *slot = retained.take();
            }
        }
        drop(retained);
    }
}

impl PersistenceSlot {
    /// 原Runtime入场之后、插件执行之前调用。失败只保留结果桥，不补新快照或新每日查询。
    pub(in crate::server::core::plugin_host::wasm) async fn observe_original_daily(
        &self,
        call: &CheckinCallRef,
    ) -> Result<(), proto::WireError> {
        if call.source.provider != "doubaowork" {
            return Ok(());
        }
        if !std::sync::Weak::ptr_eq(&self.inner.source, &Arc::downgrade(&call.source)) {
            return Err(unavailable());
        }
        current(call)?;
        let collector = {
            let mut slot = self.inner.collector.try_lock().map_err(|_| unavailable())?;
            if !self.inner.prepared.load(Ordering::Acquire)
                || self.inner.consumed.load(Ordering::Acquire)
            {
                return Err(unavailable());
            }
            slot.take().ok_or_else(unavailable)?
        };
        let mut retained = BorrowedCollector {
            owner: self.inner.clone(),
            collector: Some(collector),
        };
        let collector = retained.collector.as_mut().ok_or_else(unavailable)?;
        if collector.official.is_some() { return Err(unavailable()); }
        if collector.summary.as_ref().is_some_and(|summary|
            summary.current_projection(crate::server::logging::now_ms()).is_some()) {
            return Ok(());
        }
        collector.summary = None;
        let snapshot = collector.bound.snapshot_for_daily_mut(call).map_err(|_| unavailable())?;
        let egress = call.take_daily_egress(snapshot).await?;
        let selected = match doubao_daily::select_original_daily(call.clone(), egress).await {
            SelectionOutcome::Selected(selected) => selected,
            SelectionOutcome::NotEntered(error) | SelectionOutcome::Unknown(error) => {
                collector.failure = Some(error);
                return Ok(());
            }
        };
        let (selected, binding) = selected.bind_snapshot(snapshot).map_err(|_| unavailable())?;
        let observation = selected.query_once().await;
        match observation {
            ObservationOutcome::OfficialConfirmation(official) => {
                let fact = binding.consume_official(official, call, snapshot).map_err(|_| unavailable())?;
                let summary = OfficialSummary::from_fact(&fact)?;
                collector.official = Some(fact);
                collector.summary = Some(summary);
            }
            ObservationOutcome::ObservedFalse(_) => {
                // 豆包工作每日签到已锁定官方桌面端安全校验，接口提交会触发 710010702 并消耗当日领取资格，此处只读观测不提交。
                collector.failure = Some(doubao_daily::DailyError::DesktopOnly);
            }
            ObservationOutcome::NotEntered(error) | ObservationOutcome::Unknown(error) => {
                collector.failure = Some(error);
            }
        }
        Ok(())
    }
}
