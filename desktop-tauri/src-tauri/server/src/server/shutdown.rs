//! 服务器库的单次关闭协调，不负责退出进程或销毁宿主Tokio运行时。
//! 原调用方仍可用oneshot兼容入口；需要真实结果的调用方必须保存ServerHandle并等待。
//! 到限不abort监听/插件任务；固定数量的句柄保留在同一Shared，没有全局历史表或轮询器。

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::FutureExt;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::{sleep_until, Instant};

use crate::port_conflict::PortConflict;

use super::request_stats::RequestStats;
use super::{core, listener, logging, ServerState};

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(55);
const MAX_ERROR_CHARS: usize = 2048;
const TASKS: usize = 4;

#[derive(Clone, Copy)]
pub(super) enum Role {
    Main,
    Panel,
    Plugins,
    Stats,
}

impl Role {
    fn index(self) -> usize {
        match self {
            Self::Main => 0,
            Self::Panel => 1,
            Self::Plugins => 2,
            Self::Stats => 3,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Main => "主监听",
            Self::Panel => "面板监听",
            Self::Plugins => "插件关闭",
            Self::Stats => "统计收尾",
        }
    }
}

#[derive(Clone)]
struct Outcome {
    result: Result<(), String>,
    local_confirmed: bool,
    finished_at: Instant,
}

impl Outcome {
    fn complete() -> Self {
        Self {
            result: Ok(()),
            local_confirmed: true,
            finished_at: Instant::now(),
        }
    }
}

struct State {
    deadline: Option<Instant>,
    outcomes: [Option<Outcome>; TASKS],
    listener_errors: [Option<String>; 2],
    plugins_claimed: bool,
    stats_claimed: bool,
    completion: Option<Result<(), String>>,
    coordinator_lost: bool,
    startup_failed: bool,
    jobs_retained: bool,
    stats: Option<Arc<RequestStats>>,
    // 数量固定：最多双监听、一个插件关闭、一个统计收尾、一个协调任务及一个兼容转接。
    jobs: [Option<JoinHandle<()>>; TASKS],
    // 统计阻塞工作句柄也保留，超时不能仅保留包装任务却丢掉实际检查点工作。
    blocking_stats: Option<JoinHandle<()>>,
    coordinator: Option<JoinHandle<()>>,
    compatibility: Option<JoinHandle<()>>,
}

pub(super) struct Shared {
    state: Mutex<State>,
    changed: Notify,
    panel_events_stop: tokio_util::sync::CancellationToken,
}

/// Clone只共享本轮句柄；Drop不取消后台关闭。最后一个公开句柄丢弃会请求停止。
/// wait_shutdown只等待、不隐式请求停止；返回Ok仅确认本轮HTTP/插件/统计调用已完成。
#[derive(Clone)]
pub struct ServerHandle {
    owner: Arc<Owner>,
}

struct Owner {
    shared: Arc<Shared>,
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.shared.request_shutdown();
    }
}

impl ServerHandle {
    pub fn request_shutdown(&self) {
        self.owner.shared.request_shutdown();
    }

    /// 首次停止可继承外层启动/退出期限；已开始的关闭不重新计时。
    pub fn request_shutdown_until(&self, deadline: Instant) {
        self.owner.shared.request_shutdown_until(deadline);
    }

    /// 仅观察首个关闭通知，供服务控制器在清理期间报告正在停止。
    pub async fn wait_shutdown_requested(&self) {
        self.owner.shared.requested().await;
    }

    pub async fn wait_shutdown(&self) -> Result<(), String> {
        let shared = &self.owner.shared;
        loop {
            let changed = shared.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(result) = shared.lock().completion.clone() {
                return result;
            }
            changed.await;
        }
    }

    pub(super) fn compatibility(self) -> tokio::sync::oneshot::Sender<()> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let shared = self.owner.shared.clone();
        let finished = shared.clone();
        let job = crate::spawn_task(async move {
            // 内部异常已经启动关闭时不必继续保留一个永远不结束的兼容转接任务。
            tokio::select! {
                _ = receiver => {},
                _ = finished.requested() => {},
            }
            self.request_shutdown();
        });
        shared.lock().compatibility = Some(job);
        sender
    }
}

pub(super) fn start(state: &ServerState) -> Result<ServerHandle, PortConflict> {
    let listeners = listener::bind(state)?;
    let panel = listeners
        .iter()
        .any(|bound| matches!(bound.role, Role::Panel));
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            deadline: None,
            outcomes: [None, (!panel).then(Outcome::complete), None, None],
            listener_errors: [None, None],
            plugins_claimed: false,
            stats_claimed: false,
            completion: None,
            coordinator_lost: false,
            startup_failed: false,
            jobs_retained: false,
            stats: Some(state.request_stats()),
            jobs: std::array::from_fn(|_| None),
            blocking_stats: None,
            coordinator: None,
            compatibility: None,
        }),
        changed: Notify::new(),
        panel_events_stop: state.panel_events_stop.clone(),
    });
    let handle = ServerHandle {
        owner: Arc::new(Owner {
            shared: shared.clone(),
        }),
    };
    let mut startup = StartupGuard {
        shared: shared.clone(),
        completed: false,
    };
    // guard在future外构造：未首次poll就被运行时丢弃，也只能产生明确错误。
    let guard = CoordinatorGuard {
        shared: shared.clone(),
        completed: false,
    };
    let runner = shared.clone();
    let job = crate::spawn_task(async move {
        coordinate(runner, guard).await;
    });
    shared.lock().coordinator = Some(job);
    for bound in listeners {
        let role = bound.role;
        let guard = TaskGuard::new(shared.clone(), role);
        let runner = shared.clone();
        let job = crate::spawn_task(async move {
            listener::serve(bound, runner, guard).await;
        });
        shared.lock().jobs[role.index()] = Some(job);
    }
    // 保留原来的后台整理触发点，不把它冒称为本轮HTTP关闭所跟踪的任务。
    if !shared.stopping() {
        state.start_background_maintenance();
        #[cfg(not(test))]
        drop(tokio::task::spawn_blocking(crate::server::core::agents::warm_up));
    }
    startup.complete();
    // 启动装配全部完成后才允许给出最终结果；不能在迟到启动异常之前发布Ok。
    shared.lock().jobs_retained = true;
    shared.changed.notify_waiters();
    Ok(handle)
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub(super) fn stopping(&self) -> bool {
        self.lock().deadline.is_some()
    }

    pub(super) fn request_shutdown(&self) {
        self.request_shutdown_until(Instant::now() + SHUTDOWN_TIMEOUT);
    }

    fn request_shutdown_until(&self, requested: Instant) {
        let mut state = self.lock();
        state
            .deadline
            .get_or_insert_with(|| requested.min(Instant::now() + SHUTDOWN_TIMEOUT));
        drop(state);
        self.panel_events_stop.cancel();
        // 先使两监听的停止信号就绪，再由同一协调任务启动插件排空。
        self.changed.notify_waiters();
    }

    pub(super) async fn requested(&self) -> Instant {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(deadline) = self.lock().deadline {
                return deadline;
            }
            changed.await;
        }
    }

    pub(super) fn listener_failed(&self, role: Role, error: String) {
        if !matches!(role, Role::Main | Role::Panel) {
            return;
        }
        self.lock().listener_errors[role.index()]
            .get_or_insert_with(|| error.chars().take(MAX_ERROR_CHARS).collect());
        self.request_shutdown();
    }

    fn record(self: &Arc<Self>, role: Role, result: Result<(), String>, local_confirmed: bool) {
        let result = result.map_err(|error| error.chars().take(MAX_ERROR_CHARS).collect());
        {
            let mut state = self.lock();
            if state.outcomes[role.index()].is_none() || !local_confirmed {
                state.outcomes[role.index()] = Some(Outcome {
                    result,
                    local_confirmed,
                    finished_at: Instant::now(),
                });
            }
        }
        if matches!(role, Role::Main | Role::Panel) {
            self.request_shutdown();
            // drop保护只能登记/唤醒，绝不spawn；正常serve确认退出才有资格触发一次flush。
            if local_confirmed {
                self.maybe_flush();
            }
        }
        self.changed.notify_waiters();
        self.finish_lost_coordinator();
    }

    // 协调器异常时插件任务仍独立运行：只有拿到它的真实结果/明确丢失结果后才交付Err。
    // 若协调器从未poll，根本没有启动插件关闭，只能立即报告未确认，不能从Drop补网络。
    fn finish_lost_coordinator(&self) {
        let ready = {
            let state = self.lock();
            state.coordinator_lost
                && (!state.plugins_claimed || state.outcomes[Role::Plugins.index()].is_some())
        };
        if ready {
            self.publish(self.result(false));
        }
    }

    fn maybe_flush(self: &Arc<Self>) {
        let stats = {
            let mut state = self.lock();
            if state.coordinator_lost
                || state.stats_claimed
                || ![Role::Main, Role::Panel].iter().all(|role| {
                    state.outcomes[role.index()]
                        .as_ref()
                        .is_some_and(|outcome| outcome.local_confirmed)
                })
            {
                return;
            }
            state.stats_claimed = true;
            state.stats.take()
        };
        let Some(stats) = stats else {
            return;
        };
        // 两监听虽都报告serve已返，统计工作仍是独立阻塞任务；不靠丢JoinHandle假装它完成。
        if self.lock().coordinator_lost {
            return;
        }
        let guard = TaskGuard::new(self.clone(), Role::Stats);
        // 不在Tokio worker直接做同步检查点；不abort阻塞任务，也不说其返回等于WAL已截断。
        // 先登记异步包装句柄，再允许它派发阻塞检查点；未开闩就被丢弃时guard仍记失败。
        let (begin, ready) = tokio::sync::oneshot::channel();
        let runner = self.clone();
        let job = crate::spawn_task(async move {
            if ready.await.is_err() {
                return;
            }
            let block = tokio::task::spawn_blocking(move || {
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| stats.flush()))
                        .map_err(|_| "统计收尾发生异常，完成状态未确认".to_string());
                let confirmed = result.is_ok();
                guard.complete(result, confirmed);
            });
            runner.lock().blocking_stats = Some(block);
            runner.changed.notify_waiters();
        });
        self.lock().jobs[Role::Stats.index()] = Some(job);
        let _ = begin.send(());
    }

    // 插件与HTTP共用首个关闭截止；到期仍须等插件返回真实结果，不能直接丢弃清理任务。
    fn spawn_plugins(self: &Arc<Self>, deadline: Instant) {
        let guard = TaskGuard::new(self.clone(), Role::Plugins);
        {
            let mut state = self.lock();
            if state.plugins_claimed {
                // 当前只有单个协调器调用；重复防御分支不让未使用guard伪造第二次结果。
                drop(state);
                guard.disarm();
                return;
            }
            state.plugins_claimed = true;
        }
        let job = crate::spawn_task(async move {
            let result = std::panic::AssertUnwindSafe(core::plugin_host::shutdown_all_until(deadline))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| Err("插件关闭协调异常中断；本地排空与远端状态未确认".into()));
            let confirmed = result.is_ok();
            guard.complete(result, confirmed);
        });
        self.lock().jobs[Role::Plugins.index()] = Some(job);
    }

    fn retained_listeners(self: &Arc<Self>) -> Option<Vec<WaitingTask>> {
        let mut state = self.lock();
        if !state.jobs_retained
            || ![Role::Main, Role::Panel]
                .iter()
                .all(|role| state.outcomes[role.index()].is_some())
        {
            return None;
        }
        Some(
            [Role::Main, Role::Panel]
                .into_iter()
                .filter_map(|role| {
                    state.jobs[role.index()].take().map(|job| WaitingTask {
                        shared: self.clone(),
                        role,
                        job: Some(job),
                    })
                })
                .collect(),
        )
    }

    fn publish(&self, result: Result<(), String>) {
        let mut state = self.lock();
        if state.completion.is_none() {
            state.completion = Some(result);
        }
        drop(state);
        self.changed.notify_waiters();
    }

    fn result(&self, deadline_reached: bool) -> Result<(), String> {
        let state = self.lock();
        let mut errors: Vec<_> = state.listener_errors.iter().flatten().cloned().collect();
        for role in [Role::Main, Role::Panel, Role::Plugins, Role::Stats] {
            match &state.outcomes[role.index()] {
                Some(Outcome {
                    result: Err(error), ..
                }) => errors.push(error.clone()),
                Some(outcome) if !outcome.local_confirmed => {
                    errors.push(format!("{}本地完成状态未确认", role.label()))
                }
                Some(_) if matches!(role, Role::Stats) && state.blocking_stats.is_none() => {
                    errors.push("统计收尾工作尚未完成句柄登记，不能确认完成".into());
                }
                Some(outcome)
                    if !matches!(role, Role::Plugins)
                        && state
                            .deadline
                            .is_some_and(|deadline| outcome.finished_at > deadline) =>
                {
                    errors.push(format!(
                        "{}未在首个关闭截止内确认完成，迟到完成不改写本次超时结果",
                        role.label()
                    ));
                }
                None => errors.push(format!("{}尚未确认完成，仍保留其运行任务", role.label())),
                _ => {}
            }
        }
        if state.coordinator_lost {
            errors.push("服务器关闭协调任务已中断，不能确认完成".into());
        }
        if state.startup_failed || !state.jobs_retained {
            errors.push("服务器启动装配未完成，不能确认正常启动或完整关闭".into());
        }
        if errors.is_empty() {
            Ok(())
        } else {
            let prefix = if deadline_reached {
                "服务器关闭等待已到首个55秒截止"
            } else {
                "服务器关闭未完整确认"
            };
            Err(format!(
                "{prefix}：{}；不代表所有进程后台任务已停止或远端环境已回收",
                errors.join("；")
            ))
        }
    }
}

async fn coordinate(shared: Arc<Shared>, mut guard: CoordinatorGuard) {
    let deadline = shared.requested().await;
    shared.spawn_plugins(deadline);
    // 保留已有签到停止动作，不增加普通调度或原生会话清理的范围。
    core::auto_checkin::stop_global();
    shared.maybe_flush();
    let mut listener_joins_checked = false;
    loop {
        let changed = shared.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        let (plugins_done, all_done, retained) = {
            let state = shared.lock();
            (
                state.outcomes[Role::Plugins.index()].is_some(),
                state.outcomes.iter().all(Option::is_some) && state.blocking_stats.is_some(),
                state.jobs_retained,
            )
        };
        if !listener_joins_checked {
            if let Some(jobs) = shared.retained_listeners() {
                listener_joins_checked = true;
                for mut waiting in jobs {
                    let role = waiting.role;
                    let Some(job) = waiting.job.as_mut() else {
                        continue;
                    };
                    match tokio::time::timeout_at(deadline, job).await {
                        Ok(Ok(())) => {
                            waiting.job.take();
                        }
                        Ok(Err(_)) => {
                            waiting.job.take();
                            shared.lock().outcomes[role.index()] = Some(Outcome {
                                result: Err(format!("{}执行任务未正常退出", role.label())),
                                local_confirmed: false,
                                finished_at: Instant::now(),
                            });
                        }
                        Err(_) => {
                            shared.lock().outcomes[role.index()] = Some(Outcome {
                                result: Err(format!(
                                    "{}尚未确认任务退出，已保留运行句柄",
                                    role.label()
                                )),
                                local_confirmed: false,
                                finished_at: Instant::now(),
                            });
                        }
                    }
                }
                continue;
            }
        }
        let expired = Instant::now() >= deadline;
        // 插件自身必须await到真实Result；HTTP/flush不能让等待无限增加，也不能挤掉插件清理。
        if plugins_done && (retained && all_done || expired) {
            let result = shared.result(expired);
            shared.publish(result.clone());
            guard.complete();
            if let Err(error) = &result {
                logging::log("[Server]", error);
            } else {
                logging::log("[Server]", "本轮HTTP监听、插件排空与统计收尾已完成");
            }
            return;
        }
        if expired {
            changed.await;
        } else {
            tokio::select! {
                _ = &mut changed => {},
                _ = sleep_until(deadline) => {},
            }
        }
    }
}

/// 不包含业务输入的退出保护；只记录固定错误，不启动网络或重复关闭。
pub(super) struct TaskGuard {
    shared: Arc<Shared>,
    role: Role,
    completed: bool,
}

impl TaskGuard {
    fn new(shared: Arc<Shared>, role: Role) -> Self {
        Self {
            shared,
            role,
            completed: false,
        }
    }

    fn disarm(mut self) {
        self.completed = true;
    }

    pub(super) fn complete(mut self, result: Result<(), String>, local_confirmed: bool) {
        self.shared.record(self.role, result, local_confirmed);
        self.completed = true;
    }
}

impl Drop for TaskGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.shared.record(
                self.role,
                Err(format!(
                    "{}执行任务在确认完成前消失或异常退出",
                    self.role.label()
                )),
                false,
            );
        }
    }
}

/// 协调器等待Join期间若被drop，尚未确认退出的句柄放回同一共享槽，而不是丢弃。
struct WaitingTask {
    shared: Arc<Shared>,
    role: Role,
    job: Option<JoinHandle<()>>,
}

impl Drop for WaitingTask {
    fn drop(&mut self) {
        if let Some(job) = self.job.take() {
            self.shared.lock().jobs[self.role.index()] = Some(job);
        }
    }
}

struct StartupGuard {
    shared: Arc<Shared>,
    completed: bool,
}

impl StartupGuard {
    fn complete(&mut self) {
        self.completed = true;
    }
}

impl Drop for StartupGuard {
    fn drop(&mut self) {
        if !self.completed {
            let mut state = self.shared.lock();
            state.startup_failed = true;
            // 结束登记等待，但未启动的监听仍保持Unknown，不能伪造成功。
            state.jobs_retained = true;
            drop(state);
            self.shared.request_shutdown();
        }
    }
}

struct CoordinatorGuard {
    shared: Arc<Shared>,
    completed: bool,
}

impl CoordinatorGuard {
    fn complete(&mut self) {
        self.completed = true;
    }
}

impl Drop for CoordinatorGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.shared.request_shutdown();
            self.shared.lock().coordinator_lost = true;
            self.shared.finish_lost_coordinator();
        }
    }
}
