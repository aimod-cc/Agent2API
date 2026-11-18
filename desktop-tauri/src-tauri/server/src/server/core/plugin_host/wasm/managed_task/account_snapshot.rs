//! 当前真实任务调用的封闭账号来源；出生身份来自RowSource，不由摘要或再次读取补签。
//! 点时复核不是批准；原来源随材料移交，唯一消费仍须在最终锁链内完成。

mod final_check;
mod material;
mod reading;

pub(super) use final_check::{FinalCallback, FinalCheck, FinalCheckError, FinalCheckState};

use std::sync::Arc;

use tokio::{sync::Semaphore, time::Instant};

use super::super::conversation::{CallGate, TaskCallRef};
use super::{input::FrozenUserAgent, policy::ProjectedCookie, status::NotSentReason};
use crate::server::core::account_store::{
    credential_version::RowSource,
    AccountStore,
};

pub(super) const MAX_SECRET_BYTES: usize = 256 * 1024;
const MAX_ACCOUNT_BYTES: usize = 512;
const MAX_IDENTITY_BYTES: usize = 1024;

#[derive(Clone, Copy)]
pub(super) enum SnapshotError {
    CallEnded,
    InvalidAccount,
    AccountChanged,
    MissingUserAgent,
    InvalidUserAgent,
    InvalidCredential,
    TooLarge,
    Busy,
    TimedOut,
    StorageUnavailable,
}

impl SnapshotError {
    pub(super) fn message(self) -> &'static str {
        match self {
            Self::CallEnded => "当前任务调用已结束，未读取或使用账号快照",
            Self::InvalidAccount => "任务账号不存在、已停用或身份不符合当前调用",
            Self::AccountChanged => "任务账号身份、凭据或客户端标识已变化，未继续使用旧快照",
            Self::MissingUserAgent => "任务账号没有明确的 User-Agent，未猜测默认客户端标识",
            Self::InvalidUserAgent => "任务账号的 User-Agent 不符合固定请求头规则",
            Self::InvalidCredential => "任务账号凭据不是完整对象或 Session 投影无效",
            Self::TooLarge => "任务账号快照超过本层大小或结构限制",
            Self::Busy => "任务账号快照读取通道已满，未排队或重试",
            Self::TimedOut => "任务账号快照读取超时，未使用迟到结果",
            Self::StorageUnavailable => "任务账号快照读取不可用，未使用替代账号",
        }
    }
}

/// 字段私有，无Clone/serde/Debug；完整授权投影只留在原RowSource中，不导出凭据。
/// 原来源与TaskCallRef一起移交，不反向强持有Host/Shared，也不接受替代存储句柄。
pub(super) struct AccountSnapshot {
    call: TaskCallRef,
    material: Option<Material>,
    read_slot: Arc<Semaphore>,
    preparation_started: Instant,
}

struct Material {
    source: RowSource,
    credential_digest: [u8; 32],
    cookie: ProjectedCookie,
    user_agent: FrozenUserAgent,
}

/// 只供可信内部装配消耗；仍不是创建许可，没有Value或字段公开导出。
pub(super) struct SnapshotMaterial {
    call: TaskCallRef,
    material: Material,
    preparation_started: Instant,
}

/// 只有已完成原RowSource重检的SnapshotMaterial能签发；无Clone/serde/自由字段构造。
/// 持有整份账号材料，不把身份和摘要拆成可与另一份来源重新配对的公开投影。
pub(super) struct BindingAccount {
    account: SnapshotMaterial,
}

/// 固定凭据移入Session后仍持原行来源；摘要只用于绑定，不替代出生版本和完整投影。
pub(super) struct FinalAccount {
    source: RowSource,
    call: TaskCallRef,
    credential_digest: [u8; 32],
}

impl AccountSnapshot {
    pub(super) async fn read(
        call: &TaskCallRef,
        store: &AccountStore,
    ) -> Result<Self, SnapshotError> {
        // 第一笔账号准备前固定时刻，之后重检/装配只能沿用，不从较晚的now重新计时。
        let preparation_started = Instant::now();
        check_call(call)?;
        let read_slot = Arc::new(Semaphore::new(1));
        let material = reading::read(call, store, read_slot.clone()).await?;
        check_preparation(preparation_started, call)?;
        Ok(Self {
            call: call.clone(),
            material: Some(material),
            read_slot,
            preparation_started,
        })
    }

    /// 仅复核第一次取得的真实来源；不再次prepare、不换号、不因失效重新签发。
    /// 成功仅说明本次核对点一致，不能跨await或后续网络提交缓存为原子许可。
    pub(super) async fn recheck(&mut self) -> Result<(), SnapshotError> {
        // 按值移入原worker；取消/失败/迟到均不放回材料，快照不能继续被消费。
        let previous = self.material.take().ok_or(SnapshotError::AccountChanged)?;
        check_preparation(self.preparation_started, &self.call)?;
        let previous = reading::recheck(&self.call, previous, self.read_slot.clone()).await?;
        check_preparation(self.preparation_started, &self.call)?;
        self.material = Some(previous);
        Ok(())
    }

    /// 先点时复核再消耗；未来发送仍须另做批准和原子最终检查。
    pub(super) async fn into_material(mut self) -> Result<SnapshotMaterial, SnapshotError> {
        self.recheck().await?;
        Ok(SnapshotMaterial {
            call: self.call,
            material: self.material.take().ok_or(SnapshotError::AccountChanged)?,
            preparation_started: self.preparation_started,
        })
    }
}

impl SnapshotMaterial {
    /// input独占的借用钥匙；只在真实原调用门内按值取得整份绑定材料，不读库或取消token。
    pub(super) fn into_binding_account(
        self,
        call: &TaskCallRef,
        gate: &CallGate<'_>,
        _access: super::input::AccountBindingUse,
    ) -> Result<BindingAccount, NotSentReason> {
        if !gate.matches(call) || !self.belongs_to(call) {
            return Err(NotSentReason::InvalidBinding);
        }
        if Instant::now() >= call.call_deadline() {
            return Err(NotSentReason::CallEnded);
        }
        Ok(BindingAccount { account: self })
    }

    pub(super) fn belongs_to(&self, call: &TaskCallRef) -> bool {
        self.call.same_source(call) && source_matches_call(&self.material.source, call)
    }

    /// 仅Session能按值拆分；凭据与同次原行来源不能自由配对，后续终检仍消费此来源。
    pub(super) fn into_session(
        self,
        _access: super::network::AccountUse,
    ) -> Result<(FinalAccount, ProjectedCookie, FrozenUserAgent), SnapshotError> {
        check_call(&self.call)?;
        if !source_matches_call(&self.material.source, &self.call) {
            return Err(SnapshotError::InvalidAccount);
        }
        let Material {
            source,
            credential_digest,
            cookie,
            user_agent,
        } = self.material;
        Ok((
            FinalAccount {
                source,
                call: self.call,
                credential_digest,
            },
            cookie,
            user_agent,
        ))
    }

    pub(super) fn matches_binding(&self, binding: &super::input::Binding) -> bool {
        source_matches_call(&self.material.source, &self.call)
            && binding.matches_account(
                &self.call,
                self.material.source.identity(),
                &self.material.credential_digest,
            )
    }
}

impl BindingAccount {
    pub(super) fn identity(&self) -> &str {
        self.account.material.source.identity()
    }

    pub(super) fn credential_digest(&self) -> &[u8; 32] {
        &self.account.material.credential_digest
    }

    pub(super) fn preparation_started(&self) -> Instant {
        self.account.preparation_started
    }

    /// 只交还原整体；此回执不能与另一份SnapshotMaterial组合。
    pub(super) fn into_account(self) -> SnapshotMaterial {
        self.account
    }
}

impl FinalAccount {
    pub(super) fn matches_binding(
        &self,
        binding: &super::input::Binding,
        call: &TaskCallRef,
    ) -> bool {
        self.call.same_source(call)
            && source_matches_call(&self.source, call)
            && binding.matches_account(&self.call, self.source.identity(), &self.credential_digest)
    }
}

/// 仅比较私有来源已带出的只读投影与可信调用；字符串相同不签发或替换RowSource。
fn source_matches_call(source: &RowSource, call: &TaskCallRef) -> bool {
    source.provider().id() == call.identity().provider()
        && super::policy::profile(call.identity().provider()).is_some()
        && source.account() == call.identity().account()
}

fn check_call(call: &TaskCallRef) -> Result<(), SnapshotError> {
    let identity = call.identity();
    if super::policy::profile(identity.provider()).is_none()
        || !bounded_text(identity.account(), MAX_ACCOUNT_BYTES)
    {
        return Err(SnapshotError::InvalidAccount);
    }
    if call.ended_token().is_cancelled() || tokio::time::Instant::now() >= call.call_deadline() {
        return Err(SnapshotError::CallEnded);
    }
    Ok(())
}

fn check_preparation(started: Instant, call: &TaskCallRef) -> Result<(), SnapshotError> {
    check_call(call)?;
    if Instant::now() >= started + super::types::PREPARE_TIMEOUT {
        return Err(SnapshotError::TimedOut);
    }
    Ok(())
}

fn bounded_text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}
