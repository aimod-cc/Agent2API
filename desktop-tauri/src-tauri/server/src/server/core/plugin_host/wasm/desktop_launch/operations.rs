mod recovery;
pub(super) mod settings;
mod worker;
pub use recovery::{restore, retry_start};

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Notify;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{binding::{Binding, Call}, declaration, paths, proxy, storage, windows};
use crate::server::{access::local_session::Source, core::account_store::AccountStore, db::Db};
use crate::server::core::plugin_host::Host;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Selection {
    pub provider: String,
    pub account: String,
    pub client: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PrepareSelection {
    pub provider: String,
    pub account: String,
    pub client: String,
    #[serde(default)]
    pub use_proxy: Option<bool>,
}

pub(super) struct State {
    pub phase: &'static str,
    pub message: String,
    pub confirmed: bool,
    pub worker: bool,
    pub committed: bool,
    pub started: bool,
    pub restored: bool,
    pub client_running: bool,
    pub backup: Option<String>,
    pub error: bool,
}

pub(super) struct Operation {
    pub id: String,
    pub manager: Source,
    pub binding: Binding,
    pub db: Db,
    pub executable: PathBuf,
    pub targets: Arc<BTreeMap<String, storage::Target>>,
    pub setting: settings::Setting,
    pub restart: bool,
    pub hot_switch: bool,
    pub initial_running: bool,
    pub proxy: proxy::LaunchProxyState,
    pub pending_backup: Option<String>,
    pub cancel: CancellationToken,
    pub deadline: Instant,
    pub state: Mutex<State>,
    pub changed: Notify,
    pub expected: Mutex<Option<super::Secret>>,
    pub(crate) recovery_only: bool,
}

impl Operation {
    pub fn check(&self) -> Result<(), String> {
        if !self.manager.is_current() { return Err("本机管理会话已变化，本次确认已失效".into()); }
        if self.cancel.is_cancelled() { return Err("本次启动已取消".into()); }
        if Instant::now() >= self.deadline { return Err("本次启动确认已过期".into()); }
        self.binding.current()?;
        if settings::read(&self.db, &self.binding.provider, &self.binding.client.id)?.path != self.setting.path {
            return Err("本机客户端路径设置已变化，请重新确认".into());
        }
        paths::identity(&self.binding.client, &self.executable)
    }

    pub fn check_proxy(&self) -> Result<(), String> {
        let origin = self.binding.origin()?;
        origin.check_proxy()?;
        if self.proxy.enabled && !self.proxy.available {
            return Err(self.proxy.error.clone().unwrap_or_else(|| "当前账号代理不可用，无法带代理启动".into()));
        }
        if let Some(expected) = &self.proxy.resolved {
            proxy::reverify(origin.initial_proxy(), expected)?;
        }
        Ok(())
    }

    pub fn public(&self) -> Value {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        json!({ "intent": self.id, "provider": self.binding.provider, "account": self.binding.account,
            "client": { "id": self.binding.client.id, "label": self.binding.client.label },
            "phase": state.phase, "message": state.message, "error": state.error,
            "committed": state.committed, "started": state.started, "restored": state.restored,
            "running": state.client_running, "requiresRestart": self.restart, "hotSwitch": self.hot_switch,
            "proxy": self.proxy.to_json(),
            "executable": self.executable.display().to_string(), "package": self.binding.package,
            "resources": self.binding.client.resources.iter().map(|resource| json!({ "id": resource.id, "label": resource.label,
                "location": self.targets.get(&resource.id).map(storage::Target::display) })).collect::<Vec<_>>(),
            "backup": state.backup, "pendingBackup": self.pending_backup,
            "remainingMs": self.deadline.saturating_duration_since(Instant::now()).as_millis() as u64,
            "canRestore": (state.backup.is_some() || self.pending_backup.is_some()) && !state.restored && !state.worker,
            "canRetryStart": state.committed && !state.started && !state.restored && !state.worker })
    }

    pub fn finish(&self, phase: &'static str, message: impl Into<String>, error: bool) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.phase = phase; state.message = message.into(); state.error = error; state.worker = false;
        drop(state); self.changed.notify_waiters();
    }
}

fn table() -> &'static Mutex<HashMap<String, Arc<Operation>>> {
    static TABLE: OnceLock<Mutex<HashMap<String, Arc<Operation>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn find(manager: &Source, intent: &str) -> Result<Arc<Operation>, String> {
    let operation = table().lock().map_err(|_| "本机启动状态不可用")?.get(intent).cloned().ok_or("本次启动状态不存在或已过期")?;
    if !manager.same(&operation.manager) || !manager.is_current() { return Err("本机启动只能由原桌面管理会话继续".into()); }
    Ok(operation)
}

fn valid_parts(provider: &str, client: &str, account: &str) -> Result<(), String> {
    if !declaration::identifier(provider) || !declaration::identifier(client)
        || !declaration::text(account, 512) { return Err("本机启动选择无效".into()); }
    Ok(())
}

fn valid(selection: &Selection) -> Result<(), String> {
    valid_parts(&selection.provider, &selection.client, &selection.account)
}

pub async fn prepare(manager: Source, store: AccountStore, db: Db, selection: PrepareSelection) -> Result<Value, String> {
    valid_parts(&selection.provider, &selection.client, &selection.account)?;
    let binding = Binding::capture(&store, &selection.provider, &selection.account, &selection.client)?;
    let check = binding.invoke(Call::new(&binding, manager.clone(), None, None), declaration::CHECK).await?;
    binding.current()?;
    if check.as_object().is_none_or(|fields| fields.len() != 2) || !check["available"].is_boolean()
        || check["reason"].as_str().is_none_or(|reason| reason.len() > 2048) { return Err("插件返回的本机启动资格格式无效".into()); }
    if check["available"] != true {
        return Err(binding.failure(check["reason"].as_str().filter(|reason| !reason.is_empty()).unwrap_or("该账号没有可切换的完整凭证").to_string()));
    }
    let origin = binding.origin()?;
    origin.check(origin.initial())?;
    origin.check_proxy()?;
    let proxy_supported = binding.client.proxy_launch.is_some();
    let raw_proxy = origin.initial_proxy().clone();
    let requested_use_proxy = selection.use_proxy;
    let setting = settings::read(&db, &binding.provider, &binding.client.id)?;
    let client = binding.client.clone();
    let override_path = setting.path.clone();
    let (executable, targets, running, hot_switch, proxy_state, pending) = crate::server::core::blocking::run(move || {
        let proxy_state = proxy::prepare_state(proxy_supported, &raw_proxy, requested_use_proxy)?;
        let executable = paths::detect(&client, &override_path)?.ok_or("没有找到本机客户端，请先选择程序位置")?;
        let targets = storage::targets(&client, &executable)?;
        for target in targets.values() { target.probe()?; }
        let running = windows::running(&executable)?;
        let hot_switch = if running && proxy_state.resolved.is_none() {
            windows::hot_switch_targets(&executable, client.hot_switch.as_ref())?.is_some()
        } else {
            false
        };
        let pending = storage::Journal::pending(&executable, &targets)?;
        Ok::<_, String>((executable, targets, running, hot_switch, proxy_state, pending))
    }).await.map_err(|_| "本机客户端检查中断")??;
    binding.current()?;
    if !manager.is_current() { return Err("本机管理会话已变化".into()); }
    let id = random_id()?;
    let operation = Arc::new(Operation { id: id.clone(), manager, binding, db, executable, targets: Arc::new(targets), setting,
        restart: running && !hot_switch, hot_switch, initial_running: running, proxy: proxy_state, pending_backup: pending, cancel: CancellationToken::new(), deadline: Instant::now() + Duration::from_secs(180),
        state: Mutex::new(State { phase: "awaiting_confirmation", message: String::new(), confirmed: false, worker: false,
            committed: false, started: false, restored: false, client_running: running, backup: None, error: false }), changed: Notify::new(),
        expected: Mutex::new(None), recovery_only: false });
    insert(operation.clone())?;
    Ok(operation.public())
}

pub(super) fn insert(operation: Arc<Operation>) -> Result<(), String> {
    let mut entries = table().lock().map_err(|_| "本机启动状态不可用")?;
    entries.retain(|_, operation| operation.state.lock().map(|state| state.worker || Instant::now() < operation.deadline + Duration::from_secs(900)).unwrap_or(true));
    if entries.len() >= 64 { return Err("本机启动操作较多，请稍后重试".into()); }
    entries.insert(operation.id.clone(), operation);
    Ok(())
}

pub(super) fn backup_running(id: &str) -> bool {
    table().lock().map(|entries| entries.values().any(|operation| {
        operation.state.lock().map(|state| state.worker
            && (state.backup.as_deref() == Some(id) || operation.pending_backup.as_deref() == Some(id))).unwrap_or(true)
    })).unwrap_or(true)
}

pub fn status(manager: Source, intent: &str) -> Result<Value, String> { Ok(find(&manager, intent)?.public()) }

pub fn cancel(manager: Source, intent: &str) -> Result<Value, String> {
    let operation = find(&manager, intent)?;
    operation.cancel.cancel();
    let mut state = operation.state.lock().map_err(|_| "本机启动状态不可用")?;
    if !state.worker && !state.committed { state.phase = "canceled"; state.message = "本次启动已取消".into(); }
    drop(state);
    Ok(operation.public())
}

pub fn confirm(manager: Source, intent: &str, restart_confirmed: bool) -> Result<Value, String> {
    let operation = find(&manager, intent)?;
    {
        let state = operation.state.lock().map_err(|_| "本机启动状态不可用")?;
        if state.confirmed || state.worker || state.phase != "awaiting_confirmation" { drop(state); return Ok(operation.public()); }
    }
    if operation.recovery_only { return Err("恢复确认不能用于启动另一个账号".into()); }
    let client = operation.binding.client.clone();
    let executable = operation.executable.clone();
    let targets = operation.targets.clone();
    let use_proxy = operation.proxy.resolved.is_some();
    let (running, hot_switch, pending) = crate::server::core::blocking::in_place(move || {
        paths::identity(&client, &executable)?;
        let running = windows::running(&executable)?;
        let hot_switch = if running && !use_proxy {
            windows::hot_switch_targets(&executable, client.hot_switch.as_ref())?.is_some()
        } else {
            false
        };
        Ok::<_, String>((running, hot_switch, storage::Journal::pending(&executable, &targets)?))
    })?;
    operation.check()?;
    operation.binding.origin()?.check(operation.binding.origin()?.initial())?;
    operation.check_proxy()?;
    if operation.pending_backup.is_some() || pending.is_some() { return Err("上次本机切换未完整结束，请先恢复原凭证".into()); }
    if running && !operation.initial_running { return Err("客户端在准备后已启动，请重新确认".into()); }
    if operation.hot_switch && running && !hot_switch { return Err("客户端后台服务状态已变化，请重新确认".into()); }
    if operation.restart && !restart_confirmed { return Err("客户端正在运行，请确认已保存内容并允许重启".into()); }
    {
        let state = operation.state.lock().map_err(|_| "本机启动状态不可用")?;
        if state.confirmed || state.worker || state.phase != "awaiting_confirmation" { drop(state); return Ok(operation.public()); }
    }
    let host = operation.binding.host.clone();
    let _lifecycle = host.lifecycle.try_lock().map_err(|_| "插件运行状态正忙，请稍后重试")?;
    if host.activity.retired() || !host.ready() || crate::server::core::plugin_host::runtime::admission_closed() {
        return Err("插件内容或运行实例已变化，本次启动确认已失效".into());
    }
    {
        let mut state = operation.state.lock().map_err(|_| "本机启动状态不可用")?;
        if state.confirmed || state.worker || state.phase != "awaiting_confirmation" { drop(state); return Ok(operation.public()); }
        state.confirmed = true; state.worker = true; state.phase = "preparing";
    }
    drop(_lifecycle);
    let runner = operation.clone();
    crate::spawn_task(async move { worker::run(runner).await; });
    Ok(operation.public())
}

pub async fn settings(manager: Source, store: AccountStore, db: Db, selection: Selection) -> Result<Value, String> {
    valid(&selection)?;
    if !manager.is_current() { return Err("本机管理会话不可用".into()); }
    let binding = Binding::capture(&store, &selection.provider, &selection.account, &selection.client)?;
    let setting = settings::read(&db, &selection.provider, &selection.client)?;
    let client = binding.client.clone();
    let configured = setting.path.clone();
    let detected = crate::server::core::blocking::run(move || paths::detect(&client, &configured)).await.map_err(|_| "客户端位置检查中断")?;
    binding.current()?;
    if !manager.is_current() { return Err("本机管理会话已变化".into()); }
    Ok(json!({ "provider": selection.provider, "client": binding.client.id, "path": setting.path,
        "detected": detected.as_ref().ok().and_then(|path| path.as_ref()).map(|path| path.display().to_string()),
        "error": detected.err(), "revision": settings::revision(&binding.package, &setting.path),
        "permission": "per_operation_confirmation" }))
}

pub async fn configure(manager: Source, store: AccountStore, db: Db, selection: Selection, path: String, revision: &str) -> Result<Value, String> {
    valid(&selection)?;
    let binding = Binding::capture(&store, &selection.provider, &selection.account, &selection.client)?;
    let previous = settings::read(&db, &selection.provider, &selection.client)?;
    if settings::revision(&binding.package, &previous.path) != revision || path.len() > 2048 { return Err("插件或程序路径设置已变化，请重新读取".into()); }
    if !path.is_empty() {
        let client = binding.client.clone(); let candidate = path.clone();
        crate::server::core::blocking::run(move || paths::identity(&client, &PathBuf::from(candidate))).await.map_err(|_| "程序路径检查中断")??;
    }
    if !manager.is_current() { return Err("本机管理会话已变化，未保存程序路径".into()); }
    binding.current()?;
    settings::save(&db, &selection.provider, &selection.client, settings::Setting { path }, &previous.path)?;
    settings(manager, store, db, selection).await
}

pub(super) fn random_id() -> Result<String, String> {
    let mut bytes = [0u8; 24]; getrandom::getrandom(&mut bytes).map_err(|_| "本机启动标识生成失败")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn close_for_host(host: &Host) {
    if let Ok(entries) = table().lock() {
        for operation in entries.values().filter(|operation| std::ptr::eq(Arc::as_ptr(&operation.binding.host), host)) { operation.cancel.cancel(); }
    }
}

pub async fn wait_for_host(host: &Host) {
    let operations = table().lock().map(|entries| entries.values().filter(|operation| std::ptr::eq(Arc::as_ptr(&operation.binding.host), host)).cloned().collect::<Vec<_>>()).unwrap_or_default();
    for operation in operations {
        loop {
            let changed = operation.changed.notified(); tokio::pin!(changed); changed.as_mut().enable();
            if operation.state.lock().is_ok_and(|state| !state.worker) { break; }
            changed.await;
        }
    }
}

pub fn release_ready(host: &Host) -> bool {
    table().lock().is_ok_and(|entries| entries.values().filter(|operation| std::ptr::eq(Arc::as_ptr(&operation.binding.host), host))
        .all(|operation| operation.state.lock().is_ok_and(|state| !state.worker)))
}
