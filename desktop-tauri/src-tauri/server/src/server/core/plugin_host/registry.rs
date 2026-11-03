//! 注册表保留切换期间的只读元数据；业务调用持有对应宿主的使用凭证。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::Notify;

use super::activity::{AvailabilityError, PauseGuard};
use super::rpc::RpcAdapter;
use super::Host;
use crate::server::core::providers::adapter::ProviderAdapter;
use crate::server::errors::GatewayError;

#[derive(Default)]
struct Registry {
    adapters: HashMap<String, Arc<RpcAdapter>>,
    staged: HashMap<String, Arc<RpcAdapter>>,
    switching: HashSet<String>,
}

fn table() -> &'static Mutex<Registry> {
    static TABLE: OnceLock<Mutex<Registry>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(Registry::default()))
}

fn changed() -> &'static Notify {
    static CHANGED: OnceLock<Notify> = OnceLock::new();
    CHANGED.get_or_init(Notify::new)
}

pub fn install(id: String, adapter: Arc<RpcAdapter>) {
    if crate::server::core::providers::availability::is_suspended(&id) { return; }
    crate::server::core::providers::catalog::invalidate();
    table().lock().unwrap_or_else(|error| error.into_inner()).adapters.insert(id, adapter);
    crate::server::core::providers::catalog::invalidate();
    changed().notify_waiters();
}

pub(super) fn stage_all(adapters: Vec<(String, Arc<RpcAdapter>)>) {
    let mut table = table().lock().unwrap_or_else(|error| error.into_inner());
    crate::server::core::providers::catalog::invalidate();
    for (id, adapter) in adapters {
        if table.switching.contains(&id) { table.staged.insert(id, adapter); }
        else { table.adapters.insert(id, adapter); }
    }
    crate::server::core::providers::catalog::invalidate();
}

pub(super) fn callback_adapter(id: &str) -> Option<Arc<dyn ProviderAdapter>> {
    let table = table().lock().unwrap_or_else(|error| error.into_inner());
    let adapter = table.staged.get(id).or_else(|| table.adapters.get(id))?;
    adapter.leased().map(|adapter| Arc::new(adapter) as Arc<dyn ProviderAdapter>)
}

pub fn lookup(id: &str) -> Option<Arc<dyn ProviderAdapter>> {
    if crate::server::core::providers::availability::is_suspended(id) { return None; }
    let table = table().lock().unwrap_or_else(|error| error.into_inner());
    let adapter = table.adapters.get(id)?;
    if !table.switching.contains(id) {
        if let Some(adapter) = adapter.leased() { return Some(Arc::new(adapter)); }
    }
    Some(Arc::new(adapter.deferred()))
}

pub(super) async fn acquire(id: &str) -> Result<Option<RpcAdapter>, GatewayError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let notified = changed().notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        {
            let table = table().lock().unwrap_or_else(|error| error.into_inner());
            if !table.switching.contains(id) {
                return match table.adapters.get(id) {
                    Some(adapter) => adapter.leased().map(Some)
                        .ok_or_else(|| AvailabilityError::Unavailable.gateway()),
                    None => Ok(None),
                };
            }
        }
        if tokio::time::timeout_at(deadline, notified).await.is_err() {
            return Err(AvailabilityError::Updating.gateway());
        }
    }
}

pub async fn lookup_ready(id: &str) -> Result<Option<Arc<dyn ProviderAdapter>>, GatewayError> {
    if crate::server::core::providers::availability::is_suspended(id) { return Ok(None); }
    Ok(acquire(id).await?.map(|adapter| Arc::new(adapter) as Arc<dyn ProviderAdapter>))
}

pub(super) fn is_switching(id: &str) -> bool {
    table().lock().unwrap_or_else(|error| error.into_inner()).switching.contains(id)
}

pub fn serves(id: &str) -> bool {
    if crate::server::core::providers::availability::is_suspended(id) { return false; }
    table().lock().unwrap_or_else(|error| error.into_inner()).adapters.contains_key(id)
}

pub fn installed_ids() -> Vec<String> {
    let table = table().lock().unwrap_or_else(|error| error.into_inner());
    let mut ids: Vec<_> = table.adapters.keys()
        .filter(|id| !crate::server::core::providers::availability::is_suspended(id))
        .cloned().collect();
    ids.sort();
    ids
}

pub fn remove(id: &str) {
    crate::server::core::providers::catalog::invalidate();
    table().lock().unwrap_or_else(|error| error.into_inner()).adapters.remove(id);
    crate::server::core::providers::catalog::invalidate();
    super::rpc_cache::forget(id);
    super::schedules::forget(id);
    changed().notify_waiters();
}

pub fn clear() {
    crate::server::core::providers::catalog::invalidate();
    table().lock().unwrap_or_else(|error| error.into_inner()).adapters.clear();
    crate::server::core::providers::catalog::invalidate();
    super::schedules::clear_all();
    changed().notify_waiters();
}

pub(super) struct SwitchGuard {
    providers: HashSet<String>,
    pauses: Vec<PauseGuard>,
    previous: HashMap<String, Arc<RpcAdapter>>,
}

impl SwitchGuard {
    pub fn begin(hosts: &[Arc<Host>], providers: HashSet<String>) -> Option<Self> {
        let mut table = table().lock().unwrap_or_else(|error| error.into_inner());
        if providers.iter().any(|id| table.switching.contains(id)) { return None; }
        let mut pauses = Vec::new();
        for host in hosts { pauses.push(host.activity.pause()?); }
        table.switching.extend(providers.iter().cloned());
        let previous = providers.iter().filter_map(|id| {
            table.adapters.get(id).map(|adapter| (id.clone(), adapter.clone()))
        }).collect();
        Some(Self { providers, pauses, previous })
    }

    pub fn clear_adapters(&self) {
        let mut table = table().lock().unwrap_or_else(|error| error.into_inner());
        crate::server::core::providers::catalog::invalidate();
        for id in &self.providers {
            table.staged.remove(id);
            match self.previous.get(id) {
                Some(adapter) => { table.adapters.insert(id.clone(), adapter.clone()); }
                None => { table.adapters.remove(id); }
            }
        }
        crate::server::core::providers::catalog::invalidate();
        drop(table);
        for id in &self.providers { super::schedules::forget(id); }
    }
}

impl Drop for SwitchGuard {
    fn drop(&mut self) {
        let mut table = table().lock().unwrap_or_else(|error| error.into_inner());
        for id in &self.providers {
            if let Some(adapter) = table.staged.remove(id).filter(|adapter| adapter.host().ready()) {
                table.adapters.insert(id.clone(), adapter);
            }
            table.switching.remove(id);
            if table.adapters.get(id).is_some_and(|adapter| adapter.host().activity.retired()) {
                table.adapters.remove(id);
            }
        }
        self.pauses.clear();
        crate::server::core::providers::catalog::invalidate();
        drop(table);
        changed().notify_waiters();
    }
}
