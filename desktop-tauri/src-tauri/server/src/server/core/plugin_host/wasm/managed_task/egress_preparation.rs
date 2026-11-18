//! 原Preparing记录的单次出口材料：取槽、异步冻结、同源复核；不是批准或网络发送许可。
//! 本入口尚无生产调用者。失败只销毁材料，不回补选路，不解除replay，不关闭CallLease。

use std::sync::Arc;

use tokio::time::{sleep_until, Instant};

use super::super::conversation::{CallGate, ConsumedEgress, TaskCallRef};
use super::{
    lifecycle::Record,
    registry::{self, PreparingGuard},
    status::NotSentReason,
};
use crate::server::core::{egress::managed::FrozenHttpClient, proxies::chain::SelectedPath};

/// 身份不含代理材料；原Record/TaskCallRef均不能延长真实调用租约或恢复旧代次。
struct Source {
    record: Arc<Record>,
    call: TaskCallRef,
    consumed: ConsumedEgress,
    prepare_deadline: Instant,
}

#[must_use]
pub(super) struct EgressPreparation {
    path: SelectedPath,
    source: Source,
}

/// 无Clone/serde/整客户端导出；完整来源组合仅由原worker的Session持有，不进入轻量终态。
#[must_use]
pub(super) struct PreparedEgress {
    frozen: FrozenHttpClient,
    source: Source,
}

impl EgressPreparation {
    /// 入口在未持调用/记录锁时使用，内部进入原call.with_gate；不能在已有门内再次调用begin。
    /// 所有检查与take都在同一门内，不新建CallWindow，不补造Record或预留。
    pub(super) fn begin(record: &Arc<Record>) -> Result<Self, NotSentReason> {
        let call = record.task_call().clone();
        let (path, consumed) = call.with_gate(|gate| {
            let preparing = registry::try_preparing(record, gate)?;
            let (path, consumed) = gate.take_egress(&preparing)?;
            // 原期限或结束意图可能在同步复制路径期间到达；已take也不返还机会。
            if !preparing.matches(gate) {
                return Err(record
                    .preparing_error()
                    .unwrap_or(NotSentReason::InvalidBinding));
            }
            Ok((path, consumed))
        })?;
        Ok(Self {
            path,
            source: Source {
                record: record.clone(),
                call,
                consumed,
                prepare_deadline: record.prepare_deadline(),
            },
        })
    }

    /// 无Signal/Host/Table/Scope/Record/选路槽锁跨await；只初始化本机桥，不请求代理探测。
    /// 使用原绝对准备截止；同步客户端构造无法抢占，所以完成后必须重新核对。
    pub(super) async fn freeze(self) -> Result<PreparedEgress, NotSentReason> {
        let Self { path, source } = self;
        source.check_current()?;
        let result = tokio::select! {
            biased;
            _ = source.record.ended() => return Err(NotSentReason::CallEnded),
            _ = sleep_until(source.prepare_deadline) => return Err(NotSentReason::PreparationExpired),
            result = FrozenHttpClient::from_path(path) => result,
        };
        // 守卫已在check_current返回时全部释放；迟到/失效只drop整份客户端和端点租约。
        // 同步构造即使已经返回错误，也先复核原期限/调用，不能据此启动新的准备窗口。
        source.check_current()?;
        let frozen = result.map_err(|_| NotSentReason::RequestUnavailable)?;
        Ok(PreparedEgress { frozen, source })
    }
}

impl Source {
    fn check_current(&self) -> Result<(), NotSentReason> {
        if let Some(reason) = self.record.preparing_error() {
            return Err(reason);
        }
        self.call.with_gate(|gate| {
            let preparing = registry::try_preparing(&self.record, gate)?;
            self.check(gate, &preparing)
        })
    }

    /// 调用方已经持有Preparing记录门；只读现成身份，不重复进入Table/Scope/Record。
    fn check(
        &self,
        gate: &CallGate<'_>,
        preparing: &PreparingGuard<'_>,
    ) -> Result<(), NotSentReason> {
        if !Arc::ptr_eq(&self.record, preparing.record())
            || !self.call.same_source(self.record.task_call())
            || !gate.matches(&self.call)
            || !gate.matches_consumed_egress(&self.record, &self.consumed)
            || self.prepare_deadline != self.record.prepare_deadline()
        {
            return Err(NotSentReason::InvalidBinding);
        }
        if let Some(reason) = self.record.preparing_error() {
            return Err(reason);
        }
        if !preparing.matches(gate) {
            return Err(NotSentReason::InvalidBinding);
        }
        Ok(())
    }
}

impl PreparedEgress {
    /// 最末同步同源检查；调用者持原CallGate和PreparingGuard，不再重锁Record。
    pub(super) fn check(
        &self,
        gate: &CallGate<'_>,
        preparing: &PreparingGuard<'_>,
    ) -> Result<(), NotSentReason> {
        self.source.check(gate, preparing)
    }

    /// 客户端与Source不分离；仅网络模块私有借用钥匙可读取，不能导出整包装重新配对。
    pub(super) fn client(
        &self,
        _access: super::network::EgressUse,
    ) -> Result<&reqwest::Client, String> {
        self.frozen.client()
    }
}
