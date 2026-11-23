//! 豆包固定每日活动的同源只读消费者。仅原活动Call获准时读取，不在执行终态后自动回查。
//! 原账号/出口只能来自同一DailyEgress整体，不能从账号名、proxy JSON或自由Client补造。
//! 只产生当日观测材料；没有签到/领奖/Pending/数据库保存/公开能力或生产调用登记。
//! 原Empty快照必须在第三次查询前绑定；官方true回执保持同一私有关联，不从view补造许可。

mod binding;
pub(in crate::server::core::plugin_host::wasm) mod credentials;
mod gate;
mod json_text;
mod network;
pub(in crate::server::core::plugin_host::wasm) mod parse;
mod claim;

pub(in crate::server::core::plugin_host::wasm::checkin_call) use claim::DailyRun;

#[derive(Clone)]
pub(in crate::server::core::plugin_host) struct SubmitRejection {
    pub(in crate::server::core::plugin_host) code: Option<i64>,
    pub(in crate::server::core::plugin_host) message: String,
}

use std::sync::{atomic::{AtomicBool, AtomicU8, Ordering}, Arc, Weak};

use super::{egress::DailyEgress, CheckinCallRef, Source};
use crate::server::core::beijing;
use tokio::time::Instant;

const PROVIDER: &str = "doubaowork";
const ORIGIN: &str = "https://www.doubao.com";
const V3_GROUP: &str = "pc_incentive_task_mvp_v3";
const MAX_RESPONSE: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::server::core::plugin_host::wasm) enum DailyError {
    CallInactive,
    SourceChanged,
    Unauthorized,
    SourceUnavailable,
    CredentialsInvalid,
    InvalidClock,
    CrossDay,
    Deadline,
    AlreadyConsumed,
    QueryUnknown,
    ResponseInvalid,
    ResponseRejected,
    SubmitRejected,
    PendingConfirmation,
    NoActivity,
    NoDaily,
    AmbiguousDaily,
    MissingOriginalTask,
    SnapshotNotEmpty,
}

/// 只表示本片读取是否进入；与签到发送、是否到账或能否重发完全无关。
#[must_use]
pub(in crate::server::core::plugin_host::wasm) enum SelectionOutcome {
    Selected(DailySelection),
    Unknown(DailyError),
    NotEntered(DailyError),
}

#[must_use]
pub(in crate::server::core::plugin_host::wasm) enum ObservationOutcome {
    OfficialConfirmation(OfficialConfirmation),
    ObservedFalse(ObservedFalse),
    Unknown(DailyError),
    NotEntered(DailyError),
}

struct ReadIdentity { marker: u8 }
struct Owner {
    call: CheckinCallRef,
    egress: DailyEgress,
    credentials: credentials::Credentials,
    binding: Arc<ReadIdentity>,
    day: String,
    selected_at: i64,
    deadline: Instant,
    phase: AtomicU8,
}

impl Owner {
    fn clock(&self) -> Result<i64, DailyError> {
        if Instant::now() >= self.deadline { return Err(DailyError::Deadline); }
        let now = crate::server::logging::now_ms();
        if now < self.selected_at || day(now)? != self.day { return Err(DailyError::CrossDay); }
        Ok(now)
    }

    fn advance(&self, expected: u8, next: u8) -> Result<(), DailyError> {
        self.phase.compare_exchange(expected, next, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ()).map_err(|_| DailyError::AlreadyConsumed)
    }
}

struct Target {
    discovery_group: String,
    scene: String,
    item: String,
    endpoint: network::Route,
}

/// 只能从同call私有原launch/detail读取回执选出；不允许自由tuple或旧unknown台账构造。
/// 私有字段，无Clone/Debug/serde；保留完整原账号与冻结出口整体直到观测结束。
#[must_use]
pub(in crate::server::core::plugin_host::wasm) struct DailySelection {
    owner: Owner,
    target: Target,
}

/// 同一原选择在第三次查询前签发的私有关联。快照身份沿selection->egress->DailySource持有。
/// 不导出原call/read/endpoint或自由tuple构造器，view不参与此对象身份的重建。
struct BindingIdentity {
    call: Weak<Source>,
    read: Arc<ReadIdentity>,
    target: Target,
    selected_day: String,
    selected_at: i64,
    deadline: Instant,
    consumed: AtomicBool,
}

/// 唯一绑定消费所有者；与BoundDailySelection共享同一关联Arc，不实现Clone/serde。
#[must_use]
pub(in crate::server::core::plugin_host::wasm) struct DailyBinding {
    identity: Arc<BindingIdentity>,
}

/// 只有真实Empty快照与原phase3选择同源绑定后才能取得；未绑定选择没有公开query入口。
#[must_use]
pub(in crate::server::core::plugin_host::wasm) struct BoundDailySelection {
    selection: DailySelection,
    identity: Arc<BindingIdentity>,
}

/// 只证明官方显示原任务在冻结当日已签到，不证明本次领取、金额、原POST或保存成功。
/// 没有bool/JSON构造器、Clone/serde或保存/发送入口；携同一个查询前绑定对象。
#[must_use]
pub(in crate::server::core::plugin_host::wasm) struct OfficialConfirmation {
    selection: BoundDailySelection,
    observed_at: i64,
}

/// 真实false只作观测，不清Pending/unknown，不签发再次签到资格或OfficialDailyFact。
#[must_use]
pub(in crate::server::core::plugin_host::wasm) struct ObservedFalse {
    selection: BoundDailySelection,
    observed_at: i64,
}

/// 仅consume_official按值核验后产生，保持原关联及完整账号出口/快照身份链至后续消费。
/// 无Clone/serde、拆配或保存入口；只证明原任务当日官方已完成，不改变原执行未知。
#[must_use]
pub(in crate::server::core) struct OfficialDailyFact {
    binding: DailyBinding,
    official: OfficialConfirmation,
}

pub(in crate::server::core) struct ObservationView<'a> {
    pub discovery_group: &'a str,
    pub scene: &'a str,
    pub item_code: &'a str,
    pub selected_day: &'a str,
    pub selected_at: i64,
    pub observed_at: i64,
}

/// 原活动入场后调用；无外层锁，无终态自动query，不创建或重新冻结出口。
/// BasicLaunch一次、固定发现group的detail一次；不遇404换launch，也不遇缺任务换组重查。
pub(in crate::server::core::plugin_host::wasm) async fn select_original_daily(
    call: CheckinCallRef, egress: DailyEgress,
) -> SelectionOutcome {
    let selected_at = crate::server::logging::now_ms();
    let day = match day(selected_at) {
        Ok(day) => day,
        Err(error) => return SelectionOutcome::NotEntered(error),
    };
    if !egress.matches(&call) { return SelectionOutcome::NotEntered(DailyError::SourceChanged); }
    let credentials = match credentials::from_original(egress.account(), selected_at) {
        Ok(credentials) => credentials,
        Err(error) => return SelectionOutcome::NotEntered(error),
    };
    let owner = Owner { deadline: call.deadline(), call, egress, credentials,
        binding: Arc::new(ReadIdentity { marker: 1 }), day, selected_at, phase: AtomicU8::new(0) };
    let result = select(&owner).await;
    match result {
        Ok(target) => SelectionOutcome::Selected(DailySelection { owner, target }),
        Err(error) if owner.phase.load(Ordering::Acquire) == 0 => SelectionOutcome::NotEntered(error),
        Err(error) => SelectionOutcome::Unknown(error),
    }
}

async fn select(owner: &Owner) -> Result<Target, DailyError> {
    let launch = network::read(owner, network::Route::Launch, None, 0, 1).await?;
    let group = parse::launch_group(launch.value(owner, network::Route::Launch, None)?, &owner.credentials)?;
    let endpoint = network::Route::for_group(&group);
    let detail = network::read(owner, endpoint, Some(&group), 1, 2).await?;
    let daily = parse::unique_daily(detail.value(owner, endpoint, Some(&group))?, &group, endpoint, &owner.credentials)?;
    let target = Target { discovery_group: group, endpoint,
        scene: daily.scene, item: daily.item };
    drop(launch);
    drop(detail);
    gate::checkpoint(owner, Some((2, 3))).await?;
    Ok(target)
}

impl DailySelection {
    /// 原第三次固定查询只作为BoundDailySelection的私有执行片，不能产未绑定官方回执。
    async fn query_once(&self) -> Result<(bool, i64), DailyError> {
        self.observe().await
    }

    async fn observe(&self) -> Result<(bool, i64), DailyError> {
        let value = network::read(&self.owner, self.target.endpoint, Some(&self.target.discovery_group), 3, 4).await?;
        let task = parse::unique_daily(value.value(&self.owner, self.target.endpoint, Some(&self.target.discovery_group))?,
            &self.target.discovery_group, self.target.endpoint, &self.owner.credentials)?;
        if task.scene != self.target.scene || task.item != self.target.item { return Err(DailyError::MissingOriginalTask); }
        let checked = task.checked;
        drop(value);
        gate::checkpoint(&self.owner, Some((4, 5))).await?;
        Ok((checked, self.owner.clock()?))
    }

    fn view(&self, observed_at: i64) -> ObservationView<'_> {
        ObservationView { discovery_group: &self.target.discovery_group, scene: &self.target.scene,
            item_code: &self.target.item, selected_day: &self.owner.day, selected_at: self.owner.selected_at, observed_at }
    }
}

impl OfficialConfirmation {
    pub(in crate::server::core::plugin_host::wasm) fn view(&self) -> ObservationView<'_> {
        self.selection.selection.view(self.observed_at)
    }
}

impl ObservedFalse {
    pub(in crate::server::core::plugin_host::wasm) fn view(&self) -> ObservationView<'_> {
        self.selection.selection.view(self.observed_at)
    }
}

fn day(now: i64) -> Result<String, DailyError> {
    if now <= 0 || now > 253_402_300_799_999 { return Err(DailyError::InvalidClock); }
    beijing::date_key_of_ms(now).filter(|day| day.len() == 10).ok_or(DailyError::InvalidClock)
}
