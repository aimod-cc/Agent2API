use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak, atomic::Ordering};
use std::time::Duration;

use serde_json::{json, Value};

use super::{declaration, storage::Target};
use crate::server::access::local_session::Source;
use crate::server::core::account_store::{desktop_launch::LaunchOrigin, AccountStore};
use crate::server::core::plugin_host::{self as plugin, Host};
use crate::server::core::plugin_host::wasm::{context::Context, runtime::Runtime};

pub struct Binding {
    pub host: Arc<Host>,
    pub runtime: Arc<Runtime>,
    pub provider: String,
    pub account: String,
    pub client: declaration::Client,
    pub generation: u64,
    pub package: Value,
    pub origin: Option<Arc<LaunchOrigin>>,
}

impl Binding {
    pub fn capture(store: &AccountStore, provider: &str, account: &str, client: &str) -> Result<Self, String> {
        let mut binding = Self::recovery(provider, account, client)?;
        binding.origin = Some(Arc::new(store.desktop_launch_origin(provider, account)?));
        binding.current()?;
        Ok(binding)
    }

    pub fn recovery(provider: &str, account: &str, client: &str) -> Result<Self, String> {
        if !cfg!(windows) || !crate::server::core::update::desktop_shell() { return Err("本机启动仅在当前 Windows 桌面端可用".into()); }
        if plugin::runtime::admission_closed() || plugin::registry::is_switching(provider) { return Err("插件正在切换，请稍后重试".into()); }
        let host = plugin::host_for_provider(provider).filter(|host| host.ready()).ok_or("提供商插件尚未就绪")?;
        let runtime = host.embedded.runtime.try_read().map_err(|_| "插件运行实例正在切换")?.clone().ok_or("插件运行实例不可用")?;
        let declaration = runtime.shared.options.manifest.desktop_launch.as_ref().ok_or("插件没有声明本机启动能力")?;
        let client = declaration.clients.iter().find(|item| item.id == client).cloned().ok_or("插件没有声明该客户端")?;
        let pack = plugin::provider_pack(provider).ok_or("插件归属不可用")?;
        let entry = plugin::PackRegistry::load().packs.into_iter().find(|entry| entry.id == pack).ok_or("插件安装记录不可用")?;
        let generation = host.generation.load(Ordering::Acquire);
        let package = json!({ "id": pack, "version": entry.version, "archiveDigest": entry.sha256,
            "moduleDigest": runtime.module_digest(), "declarationDigest": digest(runtime.shared.options.source_digest()) });
        let binding = Self { host, runtime, provider: provider.into(), account: account.into(), client, generation, package, origin: None };
        binding.current()?;
        Ok(binding)
    }

    pub fn origin(&self) -> Result<&LaunchOrigin, String> {
        self.origin.as_deref().ok_or_else(|| "恢复操作没有账号凭证准备权限".into())
    }

    pub fn current(&self) -> Result<(), String> {
        let _lifecycle = self.host.lifecycle.try_lock().map_err(|_| "插件运行状态正忙，请稍后重试")?;
        let runtime = self.host.embedded.runtime.try_read().map_err(|_| "插件运行实例正在切换")?;
        if !self.host.ready() || self.host.activity.retired() || self.runtime.shared.cancel.is_cancelled()
            || self.host.generation.load(Ordering::Acquire) != self.generation
            || runtime.as_ref().is_none_or(|current| !Arc::ptr_eq(current, &self.runtime))
            || plugin::runtime::admission_closed() || plugin::registry::is_switching(&self.provider)
            || !plugin::host_for_provider(&self.provider).is_some_and(|host| Arc::ptr_eq(&host, &self.host))
            || !plugin::serves_provider(&self.provider) { return Err("插件内容或运行实例已变化，本次启动确认已失效".into()); }
        Ok(())
    }

    pub fn failure(&self, mut message: String) -> String {
        if let Some(origin) = &self.origin { message = origin.redact(&message); }
        if !message.is_empty() && message.len() <= 2048 && !message.chars().any(char::is_control) { message }
        else { "插件本机凭证准备失败；未关闭客户端或改写本机凭证".into() }
    }

    pub async fn invoke(&self, call: Call, method: &str) -> Result<Value, String> {
        self.origin()?;
        self.current()?;
        let _lease = self.host.activity.acquire().ok_or("插件正在切换")?;
        let params = json!({ "provider": self.provider, "account": self.account });
        self.host.embedded.active.fetch_add(1, Ordering::AcqRel);
        struct Active(Arc<Host>);
        impl Drop for Active { fn drop(&mut self) { self.0.embedded.active.fetch_sub(1, Ordering::AcqRel); self.0.embedded.idle.notify_waiters(); } }
        let _active = Active(self.host.clone());
        let reply = self.runtime.invoke_desktop_launch(call, method, params, Duration::from_secs(120)).await
            .map_err(|error| self.failure(error.message))?;
        let mut result = crate::server::core::plugin_host::wasm::errors::decode(reply).map_err(|error| self.failure(error.message))?;
        if method == declaration::PREPARE && result.to_string().len() > 1024 * 1024 {
            super::wipe(&mut result);
            return Err("插件凭证准备结果过大".into());
        }
        Ok(result)
    }
}

#[derive(Clone)]
pub struct Call {
    host: Weak<Host>,
    runtime: Weak<Runtime>,
    generation: u64,
    provider: String,
    account: String,
    client: String,
    manager: Source,
    executable: Option<PathBuf>,
    resources: Option<Arc<BTreeMap<String, Target>>>,
    origin: Option<Arc<LaunchOrigin>>,
    reads: Arc<Mutex<BTreeMap<String, super::Secret>>>,
    cancel: tokio_util::sync::CancellationToken,
}

impl Call {
    pub fn new(binding: &Binding, manager: Source, executable: Option<PathBuf>, resources: Option<Arc<BTreeMap<String, Target>>>) -> Self {
        Self { host: Arc::downgrade(&binding.host), runtime: Arc::downgrade(&binding.runtime), generation: binding.generation,
            provider: binding.provider.clone(), account: binding.account.clone(), client: binding.client.id.clone(), manager, executable, resources,
            origin: binding.origin.clone(), reads: Arc::new(Mutex::new(BTreeMap::new())), cancel: tokio_util::sync::CancellationToken::new() }
    }

    pub fn with_cancel(mut self, cancel: tokio_util::sync::CancellationToken) -> Self { self.cancel = cancel; self }

    pub fn validate(&self, context: &Context) -> Result<(), agent2api_provider_protocol::WireError> {
        let error = || agent2api_provider_protocol::WireError::with_code("本机启动调用来源已失效", "forbidden");
        let host = self.host.upgrade().ok_or_else(error)?;
        let runtime = self.runtime.upgrade().ok_or_else(error)?;
        if self.cancel.is_cancelled() || !self.manager.is_current() || !host.ready() || host.generation.load(Ordering::Acquire) != self.generation
            || runtime.shared.cancel.is_cancelled() || !Arc::ptr_eq(&runtime.shared, &context.shared)
            || context.account != self.account || context.shared.options.provider != self.provider
            || !plugin::host_for_provider(&self.provider).is_some_and(|current| Arc::ptr_eq(&host, &current)) {
            return Err(error());
        }
        Ok(())
    }

    pub fn observe_account(&self, record: &Value) -> Result<(), String> {
        self.origin.as_ref().ok_or("当前调用没有原账号凭证来源")?.validate_record(record)
    }

    pub fn update(&self, expect: &Value, secret: &Value, public: &Value) -> Result<Value, String> {
        if !self.preparing() { return Err("本机启动资格检查不能修改账号".into()); }
        if self.cancel.is_cancelled() { return Err("本次启动已取消".into()); }
        self.origin.as_ref().ok_or("当前调用没有原账号凭证来源")?.update(&self.manager, expect, secret, public)
    }

    pub fn preparing(&self) -> bool { self.resources.is_some() }

    pub fn context(&self) -> Value { json!({ "client": self.client, "platform": "windows", "preparing": self.preparing() }) }

    pub fn read(&self, resource: &str) -> Result<Value, String> {
        let target = self.resources.as_ref().and_then(|resources| resources.get(resource)).ok_or("当前调用不能读取此客户端资源")?;
        if self.executable.is_none() { return Err("本次客户端路径未确认".into()); }
        let mut reads = self.reads.lock().map_err(|_| "本机凭证读取快照不可用")?;
        if let Some(before) = reads.get(resource) { return Ok(target.exposed(&before.0)); }
        let before = super::Secret(target.snapshot()?);
        let value = target.exposed(&before.0);
        reads.insert(resource.to_string(), before);
        Ok(value)
    }

    pub fn observed(&self) -> Result<BTreeMap<String, Value>, String> {
        self.reads.lock().map(|reads| reads.iter().map(|(id, value)| (id.clone(), value.0.clone())).collect())
            .map_err(|_| "本机凭证读取快照不可用".into())
    }
}

fn digest(bytes: &[u8; 32]) -> String { bytes.iter().map(|byte| format!("{byte:02x}")).collect() }
