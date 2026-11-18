//! Cline Free仅新账号的明确首次登记；不读取Pass、不发送续期、不解封已有槽。

mod binding;
mod cleanup;
mod worker;

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{
    sync::{Notify, Semaphore},
    time::Instant,
};

use crate::server::access::local_session::Source;
use crate::server::core::account_store::{
    credential_version::{FreeEnrollmentInput, FreeEnrollmentOrigin},
    AccountStore,
};
use crate::server::core::plugin_host::Host;
use binding::Binding;

const TTL: Duration = Duration::from_secs(90);
const RETAIN: Duration = Duration::from_secs(300);
const MAX_INTENTS: usize = 64;

#[derive(Clone, Copy)]
pub(crate) enum EnrollmentError {
    Unavailable,
    InvalidInput,
    Expired,
    Canceled,
    Busy,
    SaveUnknown,
}

impl EnrollmentError {
    pub(crate) fn status(self) -> i32 {
        match self {
            Self::InvalidInput => 400,
            Self::Busy => 429,
            Self::SaveUnknown => 503,
            _ => 409,
        }
    }
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Unavailable => "Cline Free本次登记来源不可用，未重新选插件、账号或材料",
            Self::InvalidInput => "Cline Free首次登记请求或完整材料无效，未保存或补齐旧材料",
            Self::Expired => "Cline Free本次登记确认已过期，不会再次提交",
            Self::Canceled => "Cline Free本次登记等待已取消；已进入的保存仍按真实结果报告",
            Self::Busy => "Cline Free登记处理中或名额已满，未排队重试",
            Self::SaveUnknown => "Cline Free新账号与一次机会的保存未确认，不会再次保存或发送续期",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    AwaitingConfirmation,
    Saving,
    Saved,
    Failed,
    Canceled,
    Expired,
}

impl Phase {
    fn terminal(self) -> bool {
        matches!(
            self,
            Self::Saved | Self::Failed | Self::Canceled | Self::Expired
        )
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Confirmation {
    schema: u32,
    new_account_only: bool,
    one_opportunity: bool,
    auto_refresh: bool,
}

struct Materials {
    input: FreeEnrollmentInput,
    origin: FreeEnrollmentOrigin,
}

struct State {
    phase: Phase,
    confirmed: bool,
    entered: bool,
    saved: Option<bool>,
    ready: Option<bool>,
    storage_closed: Option<bool>,
    materials: Option<Materials>,
    message: &'static str,
    account: Option<String>,
    ended_at: Option<Instant>,
}

struct Operation {
    id: String,
    manager: Source,
    binding: Binding,
    deadline: Instant,
    expires_at: i64,
    state: Mutex<State>,
    job: Mutex<worker::JobState>,
    job_changed: Notify,
    completed: AtomicBool,
    cleanup_done: tokio_util::sync::CancellationToken,
}

impl Operation {
    fn admit<R>(
        &self,
        action: impl FnOnce(&Host, &mut State) -> Result<R, EnrollmentError>,
    ) -> Result<R, EnrollmentError> {
        self.manager
            .with_current(|| {
                self.binding.with_current(|host| {
                    let mut state = self.state.try_lock().map_err(|_| EnrollmentError::Busy)?;
                    if Instant::now() >= self.deadline {
                        return Err(EnrollmentError::Expired);
                    }
                    if state.phase.terminal() {
                        return Err(EnrollmentError::Canceled);
                    }
                    if self.manager.revocation().is_cancelled()
                        || self.binding.stopped.is_cancelled()
                    {
                        return Err(EnrollmentError::Unavailable);
                    }
                    action(host, &mut state)
                })
            })
            .ok_or(EnrollmentError::Unavailable)?
    }

    fn stop(&self, reason: EnrollmentError, phase: Phase) {
        let (materials, no_work) = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if state.saved == Some(true) {
                return;
            }
            if !state.phase.terminal() {
                state.phase = phase;
                state.message = if state.entered {
                    EnrollmentError::SaveUnknown.message()
                } else {
                    reason.message()
                };
                state.ended_at = Some(Instant::now());
            }
            (state.materials.take(), !state.confirmed)
        };
        drop(materials);
        if no_work {
            self.completed.store(true, Ordering::Release);
            self.job_changed.notify_waiters();
        }
        self.cleanup_done.cancel();
    }

    fn observe(&self) {
        if Instant::now() >= self.deadline {
            self.stop(EnrollmentError::Expired, Phase::Expired);
        } else if !self.manager.is_current() || self.binding.stopped.is_cancelled() {
            self.stop(EnrollmentError::Unavailable, Phase::Failed);
        }
    }

    fn public(&self, source: &Source) -> Result<Value, EnrollmentError> {
        let (phase, saved, ready, storage_closed, message, account) = source
            .with_current(|| {
                let state = self.state.try_lock().map_err(|_| EnrollmentError::Busy)?;
                Ok::<_, EnrollmentError>((
                    state.phase,
                    state.saved,
                    state.ready,
                    state.storage_closed,
                    state.message,
                    state.account.clone(),
                ))
            })
            .ok_or(EnrollmentError::Unavailable)??;
        Ok(json!({
            "provider":"cline-free", "intent":self.id, "phase":phase,
            "saved":saved, "readyRegistered":ready, "storageClosed":storage_closed,
            "expiresAt":self.expires_at,
            "remainingMs":self.deadline.saturating_duration_since(Instant::now()).as_millis() as u64,
            "package":self.binding.package(), "message":message,
            "account":account.map(|id| json!({"id":id,"provider":"cline-free"})),
        }))
    }
}

fn table() -> &'static Mutex<HashMap<String, Arc<Operation>>> {
    static TABLE: OnceLock<Mutex<HashMap<String, Arc<Operation>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn slots() -> Arc<Semaphore> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(Semaphore::new(4))).clone()
}

pub(crate) fn prepare(
    manager: Source,
    store: AccountStore,
    raw: &[u8],
) -> Result<Value, EnrollmentError> {
    if !cfg!(windows) || !manager.is_current() {
        return Err(EnrollmentError::Unavailable);
    }
    let input = FreeEnrollmentInput::parse(raw).map_err(|_| EnrollmentError::InvalidInput)?;
    let binding = Binding::capture()?;
    let origin = store
        .prepare_cline_free_enrollment_origin()
        .map_err(|_| EnrollmentError::Unavailable)?;
    let mut random = [0u8; 24];
    getrandom::getrandom(&mut random).map_err(|_| EnrollmentError::Unavailable)?;
    if random.iter().all(|byte| *byte == 0) {
        return Err(EnrollmentError::Unavailable);
    }
    let id: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let now = Instant::now();
    let operation = Arc::new(Operation {
        id: id.clone(),
        manager,
        binding,
        deadline: now + TTL,
        expires_at: crate::server::logging::now_ms().saturating_add(TTL.as_millis() as i64),
        state: Mutex::new(State {
            phase: Phase::AwaitingConfirmation,
            confirmed: false,
            entered: false,
            saved: Some(false),
            ready: Some(false),
            storage_closed: None,
            materials: Some(Materials { input, origin }),
            message:
                "本次完整材料已暂存，尚未查账号或保存；确认后新建Free账号并登记受控续期，本次不发送续期",
            account: None,
            ended_at: None,
        }),
        job: Mutex::new(worker::JobState::default()),
        job_changed: Notify::new(),
        completed: AtomicBool::new(false),
        cleanup_done: tokio_util::sync::CancellationToken::new(),
    });
    operation.admit(|_, _| Ok(()))?;
    let removed = {
        let mut entries = table().lock().map_err(|_| EnrollmentError::Unavailable)?;
        let expired = entries
            .iter()
            .filter_map(|(id, old)| {
                let state = old.state.try_lock().ok()?;
                let no_work = !state.confirmed || worker::release_ready(old);
                let expired = state
                    .ended_at
                    .is_some_and(|at| now.saturating_duration_since(at) >= RETAIN)
                    || !state.confirmed && now.saturating_duration_since(old.deadline) >= RETAIN;
                (no_work && expired).then(|| id.clone())
            })
            .collect::<Vec<_>>();
        let removed = expired
            .into_iter()
            .filter_map(|id| entries.remove(&id))
            .collect::<Vec<_>>();
        if entries.len() >= MAX_INTENTS || entries.contains_key(&id) {
            drop(entries);
            drop(removed);
            return Err(EnrollmentError::Busy);
        }
        entries.insert(id, operation.clone());
        removed
    };
    drop(removed);
    if let Err(error) = cleanup::register(&operation) {
        operation.stop(error, Phase::Failed);
        return Err(error);
    }
    operation.public(&operation.manager)
}

fn find(manager: &Source, id: &str) -> Result<Arc<Operation>, EnrollmentError> {
    if id.len() != 48
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(EnrollmentError::InvalidInput);
    }
    let operation = table()
        .lock()
        .map_err(|_| EnrollmentError::Unavailable)?
        .get(id)
        .cloned()
        .ok_or(EnrollmentError::Expired)?;
    if !operation.manager.same(manager) || !manager.is_current() {
        return Err(EnrollmentError::Unavailable);
    }
    operation.observe();
    Ok(operation)
}

pub(crate) fn status(manager: Source, id: &str) -> Result<Value, EnrollmentError> {
    find(&manager, id)?.public(&manager)
}

pub(crate) fn cancel(manager: Source, id: &str) -> Result<Value, EnrollmentError> {
    let operation = find(&manager, id)?;
    operation.stop(EnrollmentError::Canceled, Phase::Canceled);
    operation.public(&manager)
}

pub(crate) fn confirm(
    manager: Source,
    id: &str,
    confirmation: Confirmation,
) -> Result<Value, EnrollmentError> {
    if confirmation.schema != 2 || !confirmation.new_account_only || !confirmation.one_opportunity || !confirmation.auto_refresh {
        return Err(EnrollmentError::InvalidInput);
    }
    let operation = find(&manager, id)?;
    let duplicate = operation
        .state
        .lock()
        .map_err(|_| EnrollmentError::Unavailable)?;
    if duplicate.confirmed || duplicate.phase.terminal() {
        drop(duplicate);
        return operation.public(&manager);
    }
    drop(duplicate);
    worker::start(operation.clone())?;
    operation.public(&manager)
}

fn for_host(host: &Host) -> Vec<Arc<Operation>> {
    table()
        .lock()
        .map(|entries| {
            entries
                .values()
                .filter(|op| op.binding.belongs_to_host(host))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

pub(in crate::server::core::plugin_host::wasm) fn close_for_host(host: &Host) {
    for operation in for_host(host) {
        operation.stop(EnrollmentError::Unavailable, Phase::Failed);
    }
}

pub(in crate::server::core::plugin_host::wasm) async fn wait_workers(host: &Host) {
    for operation in for_host(host) {
        worker::wait(operation).await;
    }
}

pub(in crate::server::core::plugin_host::wasm) fn release_ready(host: &Host) -> bool {
    table().lock().is_ok_and(|entries| {
        entries
            .values()
            .filter(|op| op.binding.belongs_to_host(host))
            .all(|op| {
                op.state
                    .try_lock()
                    .is_ok_and(|state| !state.confirmed || worker::release_ready(op))
            })
    })
}
