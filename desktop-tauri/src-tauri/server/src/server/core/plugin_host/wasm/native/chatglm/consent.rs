//! 一次性本机壳确认；索引只是查表键，不能由 JSON/摘要重建安装或管理来源。
//! 固定 ChatGLM 桌面导入不经过通用 native.call，不保存或向管理端返回凭据。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::Semaphore;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::server::access::local_session::Source;
use crate::server::core::account_store::AccountStore;

use super::{call, CredentialContent, ReadError};

pub(super) const PROVIDER: &str = "chatglm";
pub(super) const CAPABILITY: &str = "login.desktop.chatglm.v1";
const TTL: Duration = Duration::from_secs(90);
const RETAIN: Duration = Duration::from_secs(5 * 60);
const MAX_INTENTS: usize = 64;

/// 所有公开错误为固定文案，不承载库/路径/插件回包等原始诊断。
#[derive(Clone, Copy)]
pub(crate) struct ImportError {
    status: i32,
    code: &'static str,
    message: &'static str,
}

impl ImportError {
    pub(crate) fn status(self) -> i32 { self.status }
    pub(crate) fn code(self) -> &'static str { self.code }
    pub(crate) fn message(self) -> &'static str { self.message }
    pub(super) fn wire(self) -> agent2api_provider_protocol::WireError {
        agent2api_provider_protocol::WireError::with_code(self.message, self.code)
    }
    pub(crate) const fn invalid_request() -> Self {
        Self { status: 400, code: "desktop_import_bad_request", message: "本机导入请求格式无效，未读取凭据" }
    }
    pub(crate) const fn forbidden() -> Self {
        Self { status: 403, code: "desktop_import_local_session_required", message: "本机导入仅允许原本机桌面管理会话，不能由远程网页或新会话继续" }
    }
    pub(super) const fn unavailable() -> Self {
        Self { status: 409, code: "desktop_import_source_unavailable", message: "本次导入的安装来源或运行实例已失效，未重新选择执行体" }
    }
    pub(super) const fn expired() -> Self {
        Self { status: 409, code: "desktop_import_expired", message: "本次导入确认已过期，未自动重发确认、读取或保存" }
    }
    pub(super) const fn canceled() -> Self {
        Self { status: 409, code: "desktop_import_canceled", message: "本次导入已取消，未自动重试" }
    }
    pub(super) const fn interrupted() -> Self {
        Self { status: 503, code: "desktop_import_interrupted", message: "本次导入执行中断，未自动重读凭据或重试保存" }
    }
    pub(super) const fn protocol() -> Self {
        Self { status: 502, code: "desktop_import_protocol", message: "插件未按本次桌面导入合同消费或返回账号，未保存" }
    }
    pub(super) const fn normalize_failed() -> Self {
        Self { status: 502, code: "desktop_import_normalize_failed", message: "本次桌面凭据校验未确认完成，原始诊断已隐藏，未保存" }
    }
    pub(super) const fn save_unknown() -> Self {
        Self { status: 503, code: "desktop_import_save_unconfirmed", message: "本次账号保存已进入但结果未确认，未自动再次保存" }
    }
    fn busy() -> Self {
        Self { status: 429, code: "desktop_import_busy", message: "本机导入处理中或确认较多，未派发新的凭据读取" }
    }
    fn platform() -> Self {
        Self { status: 501, code: "desktop_import_platform_unsupported", message: "当前平台不支持智谱清言本机凭据读取" }
    }
    pub(super) fn read(error: ReadError) -> Self {
        Self { status: 409, code: match error {
            ReadError::Unsupported => "desktop_import_platform_unsupported",
            ReadError::UnsupportedLayout => "desktop_import_layout_unsupported",
            ReadError::NotFound => "desktop_import_file_missing",
            ReadError::Busy => "desktop_import_file_busy",
            ReadError::NoCredentials => "desktop_import_credentials_missing",
            ReadError::Decryption => "desktop_import_decryption_failed",
            ReadError::TooLarge => "desktop_import_limit",
            ReadError::Conflict => "desktop_import_credentials_conflict",
            ReadError::InvalidInput | ReadError::Malformed => "desktop_import_credentials_malformed",
            ReadError::ReadFailed => "desktop_import_read_failed",
        }, message: error.message() }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase { Awaiting, Reading, Normalizing, Saving, Saved, Failed, Canceled, Expired }
impl Phase {
    fn text(self) -> &'static str {
        match self {
            Self::Awaiting => "awaiting_confirmation", Self::Reading => "reading",
            Self::Normalizing => "normalizing", Self::Saving => "saving", Self::Saved => "saved",
            Self::Failed => "failed", Self::Canceled => "canceled", Self::Expired => "expired",
        }
    }
    pub(super) fn terminal(self) -> bool { matches!(self, Self::Saved | Self::Failed | Self::Canceled | Self::Expired) }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Saved { NotEntered, Unconfirmed, Confirmed }
impl Saved {
    fn value(self) -> Value {
        match self { Self::NotEntered => json!(false), Self::Unconfirmed => Value::Null, Self::Confirmed => json!(true) }
    }
}

pub(super) struct State {
    pub phase: Phase,
    pub saved: Saved,
    pub confirmed: bool,
    pub read_entered: bool,
    pub host_entered: bool,
    pub context_entered: bool,
    pub consumed: bool,
    pub material: Option<CredentialContent>,
    pub message: &'static str,
    account: Option<String>,
    ended_at: Option<Instant>,
}

pub(super) struct Operation {
    pub id: String,
    pub manager: Source,
    pub binding: call::Binding,
    pub deadline: Instant,
    pub cancel: CancellationToken,
    pub state: Mutex<State>,
    expires_at: i64,
}

impl Operation {
    pub(super) fn admit<R>(&self, action: impl FnOnce(&mut State) -> Result<R, ImportError>) -> Result<R, ImportError> {
        // 唯一锁序：原本机会话 -> 原 Host/Runtime -> 安装见证 -> 本次状态。
        // 闭包只做有界同步准入，不读文件/账号、不发网络、不 spawn、不反向进入会话表。
        self.manager.with_current(|| self.binding.with_current(|_| {
            let mut state = self.state.try_lock().map_err(|_| ImportError::unavailable())?;
            if Instant::now() >= self.deadline { return Err(ImportError::expired()); }
            if self.cancel.is_cancelled() { return Err(ImportError::canceled()); }
            action(&mut state)
        })).ok_or_else(ImportError::forbidden)?
    }

    pub(super) fn check(&self) -> Result<(), ImportError> {
        self.admit(|state| if state.phase.terminal() { Err(ImportError::canceled()) } else { Ok(()) })
    }

    fn public_for(&self, manager: &Source) -> Result<Value, ImportError> {
        manager.with_current(|| self.public()).ok_or_else(ImportError::forbidden)
    }

    pub(super) fn stop(&self, error: ImportError, phase: Phase) {
        let material = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if state.saved == Saved::Confirmed { return; }
            if !state.phase.terminal() {
                state.phase = phase;
                state.message = if state.saved == Saved::Unconfirmed { ImportError::save_unknown().message } else { error.message };
                state.ended_at = Some(Instant::now());
            }
            state.material.take()
        };
        self.cancel.cancel();
        drop(material);
    }

    pub(super) fn finish_saved(&self, account: String) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        // 即使取消/超时发生在真实保存进入之后，也必须保留本次实际 COMMIT 回执。
        if state.saved == Saved::Unconfirmed {
            state.saved = Saved::Confirmed;
            state.phase = Phase::Saved;
            state.message = "本次本机导入账号已确认保存";
            state.account = Some(account);
            state.ended_at = Some(Instant::now());
        }
    }

    pub(super) fn save_failed(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.saved == Saved::Unconfirmed {
            // 原同步保存返回 Err 不代表能排除 COMMIT 结果未知；不伪称确定未写。
            state.phase = Phase::Failed;
            state.message = ImportError::save_unknown().message;
            state.ended_at = Some(Instant::now());
        }
    }

    pub(super) async fn ended(&self) {
        // 取消通知仅检查期限/来源，不抢正在执行的Host或状态try锁；
        // 真正阶段准入另行检查原Runtime，不能把自身同步锁竞争误判成主动取消。
        loop {
            if self.cancel.is_cancelled() || self.binding.stopped.is_cancelled()
                || Instant::now() >= self.deadline || !self.manager.is_current() { return; }
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => return,
                _ = self.binding.stopped.cancelled() => return,
                _ = tokio::time::sleep_until(self.deadline) => return,
                _ = tokio::time::sleep(Duration::from_millis(250)) => {},
            }
        }
    }

    fn observe(&self) {
        let terminal = self.state.lock().map(|state| state.phase.terminal()).unwrap_or(false);
        if terminal { return; }
        if Instant::now() >= self.deadline { self.stop(ImportError::expired(), Phase::Expired); }
        else if self.binding.stopped.is_cancelled() || !self.manager.is_current() {
            self.stop(ImportError::unavailable(), Phase::Failed);
        }
        // 普通status不能抢占准入门后撤销正在正常运行的导入；安装/代次由各阶段和Context复核。
    }

    fn public(&self) -> Value {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let remaining = self.deadline.saturating_duration_since(Instant::now()).as_millis() as u64;
        json!({ "provider": PROVIDER, "intent": self.id, "phase": state.phase.text(), "saved": state.saved.value(),
            "expiresAt": self.expires_at, "remainingMs": remaining, "package": self.binding.package(), "scope": scope(),
            "message": state.message, "account": state.account.as_ref().map(|id| json!({"id":id,"provider":PROVIDER})) })
    }
}

fn scope() -> Value {
    json!({ "platform":"windows", "application":"chatglm", "root":"current_user_roaming",
        "files":["chatglm/Local State","chatglm/Network/Cookies"], "cookieHost":".chatglm.cn",
        "cookieNames":["chatglm_token","chatglm_refresh_token"], "access":"read_only",
        "excludes":["other_apps","browser_profiles","other_windows_users"], "credentialsReturnedToUi":false })
}

fn table() -> &'static Mutex<HashMap<String, Arc<Operation>>> {
    static TABLE: OnceLock<Mutex<HashMap<String, Arc<Operation>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}
fn slots() -> Arc<Semaphore> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(Semaphore::new(4))).clone()
}
fn valid_id(id: &str) -> bool { id.len() == 48 && id.bytes().all(|byte| byte.is_ascii_hexdigit()) }

pub(crate) fn prepare(manager: Source) -> Result<Value, ImportError> {
    if !cfg!(windows) { return Err(ImportError::platform()); }
    if !manager.is_current() { return Err(ImportError::forbidden()); }
    let binding = call::Binding::capture()?;
    let mut random = [0u8; 24];
    getrandom::getrandom(&mut random).map_err(|_| ImportError::interrupted())?;
    let id: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let now = Instant::now();
    let operation = Arc::new(Operation { id: id.clone(), manager, binding, deadline: now + TTL,
        expires_at: crate::server::logging::now_ms().saturating_add(TTL.as_millis() as i64), cancel: CancellationToken::new(),
        state: Mutex::new(State { phase: Phase::Awaiting, saved: Saved::NotEntered, confirmed: false, read_entered: false,
            host_entered: false, context_entered: false, consumed: false, material: None,
            message: "请确认本次固定范围的本机登录态读取；准备阶段尚未读取凭据", account: None, ended_at: None }) });
    operation.check()?;
    {
        let mut entries = table().lock().map_err(|_| ImportError::unavailable())?;
        entries.retain(|_, operation| match operation.state.try_lock() {
            Ok(state) => match state.ended_at {
                Some(ended) => now.saturating_duration_since(ended) < RETAIN,
                None => state.confirmed || now.saturating_duration_since(operation.deadline) < RETAIN,
            },
            Err(_) => true, // 正在读写的对象不能因try锁竞争被无声移出状态表。
        });
        if entries.len() >= MAX_INTENTS || entries.contains_key(&id) { return Err(ImportError::busy()); }
        entries.insert(id, operation.clone());
    }
    operation.public_for(&operation.manager)
}

fn find(manager: &Source, id: &str) -> Result<Arc<Operation>, ImportError> {
    if !valid_id(id) { return Err(ImportError::invalid_request()); }
    let operation = table().lock().map_err(|_| ImportError::unavailable())?
        .get(id).cloned().ok_or_else(ImportError::expired)?;
    if !operation.manager.same(manager) { return Err(ImportError::forbidden()); }
    if !manager.is_current() {
        operation.stop(ImportError::forbidden(), Phase::Failed);
        return Err(ImportError::forbidden());
    }
    operation.observe();
    if !manager.is_current() { return Err(ImportError::forbidden()); }
    Ok(operation)
}

pub(crate) fn status(manager: Source, id: &str) -> Result<Value, ImportError> {
    let operation = find(&manager, id)?;
    operation.public_for(&manager)
}

pub(crate) fn cancel(manager: Source, id: &str) -> Result<Value, ImportError> {
    let operation = find(&manager, id)?;
    operation.stop(ImportError::canceled(), Phase::Canceled);
    operation.public_for(&manager)
}

pub(crate) fn confirm(manager: Source, id: &str, store: AccountStore) -> Result<Value, ImportError> {
    let operation = find(&manager, id)?;
    // 重复确认严格只读：运行中、已取消、超时或已保存都不再触及准入/读取/保存。
    let duplicate = operation.state.lock().map_err(|_| ImportError::unavailable())?;
    if duplicate.confirmed || duplicate.phase.terminal() {
        drop(duplicate);
        return operation.public_for(&manager);
    }
    drop(duplicate);
    let first = operation.admit(|state| {
        if state.confirmed || state.phase.terminal() { return Ok(false); }
        if state.phase != Phase::Awaiting { return Err(ImportError::unavailable()); }
        state.confirmed = true;
        state.phase = Phase::Reading;
        state.message = "本次读取已确认，正在处理；重复确认不会再次读取";
        Ok(true)
    });
    match first {
        Ok(false) => return operation.public_for(&manager),
        Err(error) => { operation.stop(error, Phase::Failed); return operation.public_for(&manager); }
        Ok(true) => {},
    }
    let permit = match slots().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => { operation.stop(ImportError::busy(), Phase::Failed); return operation.public_for(&manager); }
    };
    // Guard 在 spawn 前创建，后台未 poll 就丢弃也会撤销本次意图，不保留隐形读取许可。
    let guard = call::WorkerGuard::new(operation.clone());
    let worker = call::run(operation.clone(), store, permit, guard);
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::spawn_task(worker))).is_err() {
        operation.stop(ImportError::interrupted(), Phase::Failed);
    }
    operation.public_for(&manager)
}
