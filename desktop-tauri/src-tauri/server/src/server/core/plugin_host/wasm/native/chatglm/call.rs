//! 本次明确确认到固定读取、原 WASM 归一化和原账号保存的单向执行链。
//! Host/Runtime 的接线只接收 Prepared/DesktopCall，不能从请求 JSON 构造它们。

use std::sync::{atomic::Ordering, Arc, Weak};
use std::time::Duration;

use agent2api_provider_protocol as proto;
use futures::FutureExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::OwnedSemaphorePermit;
use tokio_util::sync::CancellationToken;

use crate::server::core::account_store::AccountStore;
use crate::server::core::plugin_host::install::PackRegistry;
use crate::server::core::plugin_host::packages::provenance::{self, InstallLease};
use crate::server::core::plugin_host::wasm::{context::Shared, manifest, runtime::Runtime};
use crate::server::core::plugin_host::{self as plugin, Host};

use super::consent::{ImportError, Operation, Phase, Saved, CAPABILITY, PROVIDER};

pub(super) struct Binding {
    host: Weak<Host>,
    runtime: Weak<Runtime>,
    shared: Weak<Shared>,
    pub stopped: CancellationToken,
    generation: u64,
    lease: Option<Arc<InstallLease>>,
    package_id: String,
    package_version: String,
    archive_digest: String,
    loaded_attempt: u64,
    source_digest: [u8; 32],
    module_digest: String,
}

impl Binding {
    pub(super) fn capture() -> Result<Self, ImportError> {
        if plugin::runtime::admission_closed() || plugin::registry::is_switching(PROVIDER) {
            return Err(ImportError::unavailable());
        }
        let registry = PackRegistry::load();
        let owners = plugin::ownership::owners(&registry);
        let id = owners.get(PROVIDER).ok_or_else(ImportError::unavailable)?;
        let mut found = registry.packs.iter().filter(|pack| &pack.id == id);
        let pack = found.next().ok_or_else(ImportError::unavailable)?;
        if found.next().is_some()
            || !pack.enabled
            || !pack.providers.get(PROVIDER).copied().unwrap_or(true)
            || pack.manifest_providers.len() != 1
            || pack.manifest_providers[0].id.as_str() != PROVIDER
        {
            return Err(ImportError::unavailable());
        }
        // 先区分缺少进程内安装见证与检查不可用；忙、写入中或坏见证不能回落。
        let lease = provenance::try_current_optional(id).map_err(|_| ImportError::unavailable())?;
        let host = plugin::host_for_pack(id).ok_or_else(ImportError::unavailable)?;
        if !plugin::host_for_provider(PROVIDER).is_some_and(|owner| Arc::ptr_eq(&owner, &host))
            || !plugin::serves_provider(PROVIDER)
            || host.options.script
        {
            return Err(ImportError::unavailable());
        }
        let runtime = host
            .embedded
            .runtime
            .try_read()
            .map_err(|_| ImportError::unavailable())?
            .clone()
            .ok_or_else(ImportError::unavailable)?;
        let generation = host.generation.load(Ordering::Acquire);
        let source_digest = *runtime.shared.options.source_digest();
        let module_digest = runtime.module_digest().to_string();
        let loaded = plugin::startup::provider(PROVIDER).ok_or_else(ImportError::unavailable)?;
        if loaded.pack_id != *id
            || !loaded.pack_enabled
            || !loaded.enabled
            || !loaded.available
            || loaded.startup.attempt == 0
            || host.options.exe
                != plugin::install::plugins_dir()
                    .join(id)
                    .join(&pack.version)
                    .join(&pack.entry)
        {
            return Err(ImportError::unavailable());
        }
        if let Some(lease) = &lease {
            let artifact = lease.artifact();
            let declaration_digest: [u8; 32] =
                Sha256::digest(artifact.wasm_bytes().ok_or_else(ImportError::unavailable)?).into();
            if !lease.matches_record(pack)
                || artifact.wasm_provider() != Some(PROVIDER)
                || artifact.target() != manifest::TARGET
                || declaration_digest != source_digest
                || hex(artifact
                    .module_digest()
                    .ok_or_else(ImportError::unavailable)?)
                    != module_digest
                || !lease
                    .started_runtime()
                    .is_some_and(|started| started.matches(&host, &runtime, generation))
            {
                return Err(ImportError::unavailable());
            }
        }
        // 冷启动没有原归档见证，只绑定此次真正已就绪的Host/Runtime；登记摘要仅供显示。
        let archive_digest = lease
            .as_ref()
            .map(|lease| hex(lease.artifact().archive_digest()))
            .unwrap_or_else(|| pack.sha256.clone());
        let binding = Self {
            host: Arc::downgrade(&host),
            runtime: Arc::downgrade(&runtime),
            shared: Arc::downgrade(&runtime.shared),
            stopped: runtime.shared.cancel.clone(),
            generation,
            lease,
            package_id: id.clone(),
            package_version: pack.version.clone(),
            archive_digest,
            loaded_attempt: loaded.startup.attempt,
            source_digest,
            module_digest,
        };
        binding.with_current(|_| Ok(()))?;
        Ok(binding)
    }

    pub(super) fn package(&self) -> Value {
        json!({ "id":self.package_id, "version":self.package_version, "archiveDigest":self.archive_digest,
            "moduleDigest":self.module_digest, "declarationDigest":hex(&self.source_digest) })
    }

    /// 锁序 Host lifecycle → Runtime try_read → 原运行/安装来源。action 不读文件/库、不 await。
    pub(super) fn with_current<R>(
        &self,
        action: impl FnOnce(&Host) -> Result<R, ImportError>,
    ) -> Result<R, ImportError> {
        let host = self.host.upgrade().ok_or_else(ImportError::unavailable)?;
        let _lifecycle = host
            .lifecycle
            .try_lock()
            .map_err(|_| ImportError::unavailable())?;
        let current = host
            .embedded
            .runtime
            .try_read()
            .map_err(|_| ImportError::unavailable())?;
        let runtime = current.as_ref().ok_or_else(ImportError::unavailable)?;
        let options = &runtime.shared.options;
        if self.generation == 0
            || host.generation.load(Ordering::Acquire) != self.generation
            || !std::ptr::eq(self.runtime.as_ptr(), Arc::as_ptr(runtime))
            || !std::ptr::eq(self.shared.as_ptr(), Arc::as_ptr(&runtime.shared))
            || host.closing.load(Ordering::Acquire)
            || !host.ready()
            || plugin::runtime::admission_closed()
            || host.activity.retired()
            || self.stopped.is_cancelled()
            || runtime.shared.cancel.is_cancelled()
            || options.provider != PROVIDER
            || options.source_digest() != &self.source_digest
            || runtime.module_digest() != self.module_digest
            || !options.allows(CAPABILITY)
            || !options.allows("accounts.cas.v1")
            || !options.allows("login.forms.v1")
            || host.options.wasm.as_ref().is_none_or(|options| {
                options.provider != PROVIDER
                    || options.source_digest() != &self.source_digest
                    || !options.allows(CAPABILITY)
            })
            || plugin::registry::is_switching(PROVIDER)
            || !plugin::host_for_pack(&self.package_id)
                .is_some_and(|current| Arc::ptr_eq(&current, &host))
            || !plugin::host_for_provider(PROVIDER)
                .is_some_and(|current| Arc::ptr_eq(&current, &host))
            || plugin::startup::provider(PROVIDER).is_none_or(|current| {
                current.pack_id != self.package_id
                    || !current.pack_enabled
                    || !current.enabled
                    || !current.available
                    || current.startup.attempt != self.loaded_attempt
            })
        {
            return Err(ImportError::unavailable());
        }
        let current_lease = provenance::try_current_optional(&self.package_id)
            .map_err(|_| ImportError::unavailable())?;
        let _install = match (&self.lease, current_lease) {
            (Some(lease), Some(current))
                if Arc::ptr_eq(lease, &current)
                    && lease.started_runtime().is_some_and(|started| {
                        started.matches(&host, runtime, self.generation)
                    }) =>
            {
                Some(lease.try_hold().map_err(|_| ImportError::unavailable())?)
            }
            (None, None) => None,
            _ => return Err(ImportError::unavailable()),
        };
        // 原用户确认和原Host生命周期门内执行；安装切换要先暂停Activity，实际读取/保存已持UsageLease。
        action(&host)
    }

    fn runtime(&self) -> Result<Arc<Runtime>, ImportError> {
        self.runtime.upgrade().ok_or_else(ImportError::unavailable)
    }
    fn host(&self) -> Result<Arc<Host>, ImportError> {
        self.host.upgrade().ok_or_else(ImportError::unavailable)
    }
    fn matches_shared(&self, shared: &Shared) -> bool {
        std::ptr::eq(self.shared.as_ptr(), shared)
            && shared.options.provider == PROVIDER
            && shared.options.source_digest() == &self.source_digest
            && shared.options.allows(CAPABILITY)
    }
}
fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 不含可重发索引或公共构造器；只由一次真实读取成功后装入。
#[must_use]
pub(in crate::server::core::plugin_host::wasm) struct Prepared {
    call: DesktopCall,
    runtime: Arc<Runtime>,
    params: Value,
}
pub(in crate::server::core::plugin_host::wasm) struct Entered {
    pub runtime: Arc<Runtime>,
    pub call: DesktopCall,
    pub params: Value,
    pub timeout: Duration,
}

impl Prepared {
    /// 成功后 Host 必须立即建立 Active；不重新 ensure、不等待或重选 Runtime。
    pub(in crate::server::core::plugin_host::wasm) fn enter(
        self,
        host: &Host,
    ) -> Result<Entered, proto::WireError> {
        if !std::ptr::eq(self.call.operation.binding.host.as_ptr(), host) {
            return Err(ImportError::unavailable().wire());
        }
        self.call
            .operation
            .admit(|state| {
                if state.phase != Phase::Normalizing
                    || !state.read_entered
                    || state.host_entered
                    || state.material.is_none()
                {
                    return Err(ImportError::unavailable());
                }
                state.host_entered = true;
                // admit 正持原 Host 生命周期及安装门；与关闭/运行代次变化串行。
                host.embedded.active.fetch_add(1, Ordering::AcqRel);
                Ok(())
            })
            .map_err(ImportError::wire)?;
        let timeout = self
            .call
            .operation
            .deadline
            .saturating_duration_since(tokio::time::Instant::now());
        Ok(Entered {
            runtime: self.runtime,
            call: self.call,
            params: self.params,
            timeout,
        })
    }
}

/// Clone 只保留本次私有状态，不复制凭据。Context装配和凭据取出仍各只能消费一次。
#[derive(Clone)]
pub(in crate::server::core::plugin_host::wasm) struct DesktopCall {
    operation: Arc<Operation>,
}
impl DesktopCall {
    pub(in crate::server::core::plugin_host::wasm) fn enter_context(
        &self,
        runtime: &Runtime,
        method: &str,
        params: &Value,
    ) -> Result<(), proto::WireError> {
        if method != proto::methods::gateway::ACCOUNT_NORMALIZE
            || !self.operation.binding.matches_shared(&runtime.shared)
            || !std::ptr::eq(self.operation.binding.runtime.as_ptr(), runtime)
            || params["provider"].as_str() != Some(PROVIDER)
            || params["account"].as_str() != Some("")
            || params["source"].as_object().is_none_or(|source| {
                source.len() != 1 || source.get("kind").and_then(Value::as_str) != Some("desktop")
            })
        {
            return Err(ImportError::protocol().wire());
        }
        self.operation
            .admit(|state| {
                if state.phase != Phase::Normalizing
                    || !state.host_entered
                    || state.context_entered
                    || state.material.is_none()
                {
                    return Err(ImportError::protocol());
                }
                state.context_entered = true;
                Ok(())
            })
            .map_err(ImportError::wire)
    }
    pub(in crate::server::core::plugin_host::wasm) fn validate(
        &self,
        shared: &Shared,
    ) -> Result<(), proto::WireError> {
        if !self.operation.binding.matches_shared(shared) {
            return Err(ImportError::protocol().wire());
        }
        self.operation
            .admit(|state| {
                if state.phase != Phase::Normalizing
                    || !state.host_entered
                    || !state.context_entered
                {
                    Err(ImportError::protocol())
                } else {
                    Ok(())
                }
            })
            .map_err(ImportError::wire)
    }
    pub(in crate::server::core::plugin_host::wasm) fn take(
        &self,
        shared: &Shared,
        params: &Value,
    ) -> Result<Value, proto::WireError> {
        if params.as_object().is_none_or(|fields| !fields.is_empty())
            || !self.operation.binding.matches_shared(shared)
        {
            return Err(ImportError::protocol().wire());
        }
        let material = self
            .operation
            .admit(|state| {
                if state.phase != Phase::Normalizing
                    || !state.host_entered
                    || !state.context_entered
                    || state.consumed
                {
                    return Err(ImportError::protocol());
                }
                let material = state.material.take().ok_or_else(ImportError::protocol)?;
                state.consumed = true;
                Ok(material)
            })
            .map_err(ImportError::wire)?;
        material
            .into_input()
            .map_err(|error| ImportError::read(error).wire())
    }
    pub(in crate::server::core::plugin_host::wasm) fn consumed(&self) -> bool {
        self.operation
            .state
            .lock()
            .is_ok_and(|state| state.consumed)
    }
    pub(in crate::server::core::plugin_host::wasm) async fn ended(&self) {
        self.operation.ended().await;
    }
}

pub(super) struct WorkerGuard {
    operation: Arc<Operation>,
    finished: bool,
}
impl WorkerGuard {
    pub(super) fn new(operation: Arc<Operation>) -> Self {
        Self {
            operation,
            finished: false,
        }
    }
    fn finish(mut self) {
        self.finished = true;
    }
}
impl Drop for WorkerGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.operation
                .stop(ImportError::interrupted(), Phase::Failed);
        }
    }
}

pub(super) async fn run(
    operation: Arc<Operation>,
    store: AccountStore,
    permit: OwnedSemaphorePermit,
    guard: WorkerGuard,
) {
    let outcome = std::panic::AssertUnwindSafe(execute(operation.clone(), store, permit))
        .catch_unwind()
        .await;
    match outcome {
        Ok(Ok(())) => guard.finish(),
        Ok(Err(error)) => operation.stop(error, Phase::Failed),
        Err(_) => operation.stop(ImportError::interrupted(), Phase::Failed),
    }
}

async fn execute(
    operation: Arc<Operation>,
    store: AccountStore,
    permit: OwnedSemaphorePermit,
) -> Result<(), ImportError> {
    // 同步读取任务一旦派出就由它持 permit 至真实结束；外层取消/超时不提前释放读取名额。
    operation.admit(|state| {
        if state.phase.terminal() {
            return Err(ImportError::canceled());
        }
        Ok(())
    })?;
    let host = operation.binding.host()?;
    let usage_lease = host
        .activity
        .acquire()
        .ok_or_else(ImportError::unavailable)?;
    let reading = operation.clone();
    let job = tokio::task::spawn_blocking(move || {
        let permit = permit;
        let read_host = host;
        let read_usage = usage_lease;
        reading.admit(|state| {
            if state.phase != Phase::Reading || !state.confirmed || state.read_entered {
                return Err(ImportError::unavailable());
            }
            state.read_entered = true;
            Ok(())
        })?;
        let result = super::read(&json!({})).map_err(ImportError::read)?;
        reading.check()?;
        Ok((result, permit, read_host, read_usage))
    });
    let (content, permit, host, usage_lease) = tokio::select! {
        biased;
        _=operation.ended()=>return Err(ImportError::unavailable()),
        result=job=>result.map_err(|_|ImportError::interrupted())??,
    };
    let mut material = Some(content);
    operation.admit(|state| {
        if state.phase != Phase::Reading || state.material.is_some() {
            return Err(ImportError::unavailable());
        }
        state.material = material.take();
        state.phase = Phase::Normalizing;
        state.message = "本机凭据已读取，正在同一插件实例中校验；凭据不会返回界面";
        Ok(())
    })?;
    let runtime = operation.binding.runtime()?;
    let call = DesktopCall {
        operation: operation.clone(),
    };
    let prepared = Prepared {
        runtime,
        call: call.clone(),
        params: json!({"provider":PROVIDER,"account":"","source":{"kind":"desktop"}}),
    };
    let response = tokio::select! {
        biased;
        _=operation.ended()=>return Err(ImportError::unavailable()),
        result=host.call_wasm_desktop_import(prepared)=>result.map_err(|_|ImportError::normalize_failed())?,
    };
    let mut response = SecretJson(response);
    operation.check()?;
    if !call.consumed() {
        return Err(ImportError::protocol());
    }
    validate_account(&response.0)?;
    let account = std::mem::replace(&mut response.0, Value::Null);
    // 保存闭包先装入spawn_blocking，实际同步内核入场前才把 saved 改成 null；不重试、不猜未写。
    let saving = operation.clone();
    let account = SecretJson(account); // 在后台future/闭包被丢弃前就建立敏感值清理所有者。
    let job = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let _usage = usage_lease;
        let _host = host;
        save(saving, store, account)
    });
    tokio::select! {
        biased;
        _=operation.ended()=>Err(ImportError::unavailable()),
        result=job=>result.map_err(|_|ImportError::save_unknown())?,
    }
}

fn validate_account(account: &Value) -> Result<(), ImportError> {
    if account.as_object().is_none_or(|fields| {
        fields
            .keys()
            .any(|key| !matches!(key.as_str(), "identity" | "public" | "secret" | "desktop"))
    }) || account["desktop"] != true
        || account["secret"]["source"] != "desktop"
        || account["identity"].as_str().is_none_or(|id| {
            id.trim().is_empty() || id.len() > 512 || id.chars().any(char::is_control)
        })
    {
        return Err(ImportError::protocol());
    }
    crate::server::core::plugin_host::wasm::public_view::validate(
        &account["public"],
        &account["secret"],
    )
    .map_err(|_| ImportError::protocol())
}

fn save(
    operation: Arc<Operation>,
    store: AccountStore,
    account: SecretJson,
) -> Result<(), ImportError> {
    operation.admit(|state| {
        if state.phase != Phase::Normalizing || !state.consumed || state.saved != Saved::NotEntered
        {
            return Err(ImportError::protocol());
        }
        state.phase = Phase::Saving;
        state.saved = Saved::Unconfirmed;
        state.message = "本次账号保存已进入，结果尚未确认；请勿重复确认";
        Ok(())
    })?;
    // 从准入到这次原保存调用没有 await，不枚举账号、不重新 normalize、不触发其他业务动作。
    let value = &account.0;
    let id = value["identity"]
        .as_str()
        .ok_or_else(ImportError::protocol)?;
    let name = value["public"]["name"]
        .as_str()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("智谱清言账号");
    let result = store.plugin_upsert_account_with_source_and_proxy(
        id,
        PROVIDER,
        name,
        &value["secret"],
        &value["public"],
        true,
        "plugin",
        None,
    );
    match result {
        Ok((record, _)) => {
            let id = record["id"]
                .as_str()
                .filter(|id| !id.is_empty() && id.len() <= 512 && !id.chars().any(char::is_control))
                .ok_or_else(ImportError::save_unknown)?;
            operation.finish_saved(id.to_string());
            Ok(())
        }
        Err(_) => {
            operation.save_failed();
            Err(ImportError::save_unknown())
        }
    }
}

/// 不序列化、不Debug；释放归一化回包时尽量清零宿主侧明文值。
struct SecretJson(Value);
impl Drop for SecretJson {
    fn drop(&mut self) {
        super::wipe_value(&mut self.0);
    }
}
