//! 常规Tauri退出只提交意图；后端确认成功才允许唯一最终动作。
//! 不拦截平台强杀，不覆盖macOS AppKit原生终止。错误不重开后端、不自动重试安装器。

mod installer;
mod ui;
mod worker;

use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::Value;
use tauri::{AppHandle, Manager};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::{backend, state::AppState};

pub(crate) use installer::run as run_installer;
pub(crate) use ui::startup_result;

const CLEANUP_FAILED: &str = "未能确认本地网关和插件已安全停止，已取消自动退出、重启或安装。程序和托盘将保持打开；请查看运行日志。不会自动重试或强制终止。";
const ACTION_FAILED: &str = "最终操作未能完成，程序保持打开。后台服务不会自动重新启动；请查看运行日志确认状态。";
const INTERRUPTED: &str = "退出操作在确认完成前中断，当前结果未知。程序不会自动再次退出、重启或运行安装器。";
const CONFLICT: &str = "已有另一项退出、重启或安装请求，未覆盖原请求，也未执行新的操作。";

#[derive(Clone, PartialEq, Eq)]
enum Action {
    Quit(i32),
    Restart,
    Install(installer::Verified),
}

impl Action {
    fn closes_backend(&self) -> bool {
        match self {
            Self::Install(installer) => installer.closes_backend(),
            _ => true,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Allowed {
    Quit(i32),
    Restart,
}

#[derive(Clone)]
enum Phase {
    Running,
    Done(Value),
    Failed(&'static str),
}

struct OperationState {
    phase: Phase,
    poisoned: bool,
    task: Option<JoinHandle<()>>,
    final_task: Option<JoinHandle<()>>,
    cleanup_confirmed: bool,
}

struct Operation {
    action: Action,
    state: Mutex<OperationState>,
    changed: Notify,
}

impl Operation {
    fn new(action: Action) -> Arc<Self> {
        Arc::new(Self {
            action,
            state: Mutex::new(OperationState {
                phase: Phase::Running, poisoned: false, task: None, final_task: None,
                cleanup_confirmed: false,
            }),
            changed: Notify::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, OperationState> {
        match self.state.lock() {
            Ok(state) => state,
            Err(error) => {
                let mut state = error.into_inner();
                if !state.poisoned {
                    state.poisoned = true;
                    state.phase = Phase::Failed(INTERRUPTED);
                    self.changed.notify_waiters();
                }
                state
            }
        }
    }

    fn fail(&self, message: &'static str) {
        {
            let mut state = self.lock();
            if !matches!(state.phase, Phase::Failed(_)) {
                state.phase = Phase::Failed(message);
            }
        }
        self.changed.notify_waiters();
    }
}

#[derive(Default)]
struct State {
    // 一份安装回执和一份最终退出请求；保留已完成回执，重复点击不重复执行安装器。
    installer: Option<Arc<Operation>>,
    terminal: Option<Arc<Operation>>,
    backend_closing: bool,
    cleaned: bool,
    allowed: Option<Allowed>,
    dialog_pending: bool,
    early_failure: bool,
    poisoned: bool,
}

#[derive(Default)]
pub(crate) struct Coordinator {
    state: Mutex<State>,
}

impl Coordinator {
    fn lock(&self) -> MutexGuard<'_, State> {
        match self.state.lock() {
            Ok(state) => state,
            Err(error) => {
                let mut state = error.into_inner();
                if !state.poisoned {
                    state.poisoned = true;
                    state.allowed = None;
                    for operation in state.installer.iter().chain(state.terminal.iter()) {
                        operation.fail(INTERRUPTED);
                    }
                }
                state
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct Ticket {
    operation: Arc<Operation>,
}

impl Ticket {
    pub(crate) async fn wait(&self) -> Result<Value, String> {
        loop {
            let notified = self.operation.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            match self.operation.lock().phase.clone() {
                Phase::Done(value) => return Ok(value),
                Phase::Failed(message) => return Err(message.into()),
                Phase::Running => {}
            }
            notified.await;
        }
    }
}

fn coordinator(app: &AppHandle) -> Arc<Coordinator> {
    app.state::<AppState>().exit.clone()
}

fn request(app: &AppHandle, action: Action) -> Result<Ticket, String> {
    let shared = coordinator(app);
    let selected = {
        let mut state = shared.lock();
        let duplicate = [&state.installer, &state.terminal].into_iter()
            .flatten().find(|operation| operation.action == action).cloned();
        if state.early_failure || state.poisoned {
            Err(INTERRUPTED)
        } else if let Some(operation) = duplicate {
            Ok((operation, None))
        } else {
            let occupied = state.terminal.is_some() || state.installer.as_ref().is_some_and(|operation| {
                // 安装结束后可显式申请退出，但进行中或尚未真正退场时拒绝冲突动作。
                let current = operation.lock();
                matches!(current.phase, Phase::Running)
                    || current.task.as_ref().is_none_or(|task| !task.is_finished())
                    || current.final_task.as_ref().is_some_and(|task| !task.is_finished())
                    || matches!(&current.phase, Phase::Done(_))
                        && matches!(&operation.action, Action::Install(installer) if installer.exit_after())
            });
            if occupied || matches!(action, Action::Install(_)) && state.installer.is_some() {
                Err(CONFLICT)
            } else {
                let operation = Operation::new(action.clone());
                if matches!(action, Action::Install(_)) {
                    state.installer = Some(operation.clone());
                } else {
                    state.terminal = Some(operation.clone());
                }
                state.backend_closing |= action.closes_backend();
                let guard = worker::TaskGuard::new(shared.clone(), operation.clone());
                Ok((operation, Some(guard)))
            }
        }
    };
    let (operation, guard) = match selected {
        Ok(selected) => selected,
        Err(error) => {
            ui::error(app, &shared, error);
            return Err(error.into());
        }
    };
    let phase = operation.lock().phase.clone();
    if let Phase::Failed(message) = phase {
        ui::error(app, &shared, message);
        return Err(message.into());
    }
    // 申请顺序先在内存中封闭后端，再把异步等待交给唯一任务；重复请求不派第二份动作。
    if guard.is_some() && action.closes_backend() {
        backend::shutdown(&app.state::<AppState>());
    }
    if let Some(guard) = guard {
        worker::spawn(app.clone(), shared, operation.clone(), guard);
    }
    Ok(Ticket { operation })
}

pub(crate) fn request_quit(app: &AppHandle, code: i32) -> Result<Ticket, String> {
    if code == tauri::RESTART_EXIT_CODE {
        request_restart(app)
    } else {
        request(app, Action::Quit(code))
    }
}

pub(crate) fn request_restart(app: &AppHandle) -> Result<Ticket, String> {
    request(app, Action::Restart)
}

/// 只有关闭后台的意图阻止普通隐藏；dmg/deb打开不改变“关闭到托盘”。
pub(crate) fn requested(state: &AppState) -> bool {
    let shared = state.exit.lock();
    shared.early_failure || shared.backend_closing || shared.terminal.is_some() || shared.poisoned
}

pub(crate) fn exit_in_flight(state: &AppState) -> bool {
    let shared = state.exit.lock();
    shared.early_failure || shared.poisoned || shared.terminal.is_some()
        || shared.installer.as_ref().is_some_and(|operation| matches!(operation.lock().phase, Phase::Running))
}

pub(crate) fn allowed(state: &AppState) -> bool {
    let shared = state.exit.lock();
    shared.allowed.is_some() && !shared.poisoned
}

fn allows_code(state: &AppState, code: Option<i32>) -> bool {
    let shared = state.exit.lock();
    if shared.poisoned {
        return false;
    }
    match (shared.allowed, code) {
        (Some(Allowed::Restart), Some(code)) => code == tauri::RESTART_EXIT_CODE,
        (Some(Allowed::Restart), None) => true,
        (Some(Allowed::Quit(expected)), Some(code)) => code == expected,
        (Some(Allowed::Quit(_)), None) => true,
        _ => false,
    }
}

pub(crate) fn on_exit_requested(app: &AppHandle, api: &tauri::ExitRequestApi, code: Option<i32>) {
    let state = app.state::<AppState>();
    if allows_code(&state, code) {
        return;
    }
    // Wry在回调返回后立即try_recv，不能把prevent_exit挪进异步任务。
    api.prevent_exit();
    let early_failure = state.exit.lock().early_failure;
    if early_failure {
        // 迁移前失败没有启动后端，不能为了退出再访问库或插件。
        early_exit(app);
        return;
    }
    if code.is_none() && state.window.close_to_tray() && !requested(&state) {
        return;
    }
    if code == Some(tauri::RESTART_EXIT_CODE) {
        // Tauri忽略此退出码的prevent_exit；所有本项目重启必须在请求此码之前完成清理。
        eprintln!("[Exit] 收到未经协调的Tauri重启，平台回调无法拦截；未冒称清理完成");
        return;
    }
    if exit_in_flight(&state) {
        show_failure(app);
        return;
    }
    let _ = request_quit(app, code.unwrap_or(0));
}

pub(crate) fn show_failure(app: &AppHandle) {
    let shared = coordinator(app);
    let operations = {
        let state = shared.lock();
        if state.poisoned {
            drop(state);
            ui::error(app, &shared, INTERRUPTED);
            return;
        }
        [state.terminal.clone(), state.installer.clone()]
    };
    for operation in operations.into_iter().flatten() {
        let phase = operation.lock().phase.clone();
        if let Phase::Failed(message) = phase {
            ui::error(app, &shared, message);
            break;
        }
    }
}

pub(crate) fn on_exit(app: &AppHandle) {
    if !allowed(&app.state::<AppState>()) {
        // Exit已不能阻止进程结束，不在这里派出清理后假装它有时间完成。
        eprintln!("[Exit] 程序在未取得安全退出确认时结束；未确认所有本地任务或远端资源已回收");
    }
}

pub(crate) fn mark_startup_failure(app: &AppHandle) {
    let shared = coordinator(app);
    let mut state = shared.lock();
    // 只用于setup迁移失败、任何后端任务尚未派发的固定分支。
    if state.installer.is_none() && state.terminal.is_none() && !state.backend_closing {
        state.early_failure = true;
    }
}

pub(crate) fn early_exit(app: &AppHandle) {
    let shared = coordinator(app);
    let issue = {
        let mut state = shared.lock();
        if state.poisoned || !state.early_failure || state.allowed.is_some() {
            false
        } else {
            state.allowed = Some(Allowed::Quit(1));
            true
        }
    };
    if issue {
        app.exit(1);
    }
}
