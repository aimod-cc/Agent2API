use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use agent2api_provider_protocol as proto;

use super::ownership::ProviderCategory;
use super::{Host, HostStatus};

struct LoadedPack {
    host: Arc<Host>,
    category: ProviderCategory,
    providers: Vec<String>,
}

fn table() -> &'static Mutex<BTreeMap<String, LoadedPack>> {
    static HOSTS: OnceLock<Mutex<BTreeMap<String, LoadedPack>>> = OnceLock::new();
    HOSTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(super) fn install_pack(id: &str, category: ProviderCategory, host: Arc<Host>) {
    if let Ok(mut hosts) = table().lock() {
        hosts.insert(id.to_string(), LoadedPack { host, category, providers: Vec::new() });
    }
}

pub(super) fn set_providers(id: &str, providers: Vec<String>) {
    if let Ok(mut hosts) = table().lock() {
        if let Some(pack) = hosts.get_mut(id) {
            pack.providers = providers;
        }
    }
}

pub fn host_for_pack(id: &str) -> Option<Arc<Host>> {
    table().lock().ok()?.get(id).map(|pack| pack.host.clone())
}

pub fn host_for_provider(provider: &str) -> Option<Arc<Host>> {
    table().lock().ok()?.values()
        .find(|pack| pack.providers.iter().any(|id| id == provider))
        .map(|pack| pack.host.clone())
}

pub fn provider_pack(provider: &str) -> Option<String> {
    table().lock().ok()?.iter()
        .find(|(_, pack)| pack.providers.iter().any(|id| id == provider))
        .map(|(id, _)| id.clone())
}

pub fn provider_category(provider: &str) -> ProviderCategory {
    table().lock().ok().and_then(|hosts| {
        hosts.values().find(|pack| pack.providers.iter().any(|id| id == provider))
            .map(|pack| pack.category)
    }).unwrap_or_default()
}

pub(super) fn provider_infos() -> Vec<proto::ProviderInfo> {
    let Ok(hosts) = table().lock() else { return Vec::new() };
    hosts.values().flat_map(|pack| {
        pack.host.providers.lock().map(|infos| {
            infos.iter().filter(|info| {
                pack.providers.iter().any(|id| id == info.id.as_str())
                    && super::serves_provider(info.id.as_str())
            }).cloned().collect::<Vec<_>>()
        }).unwrap_or_default()
    }).collect()
}

pub(super) fn provider_info(provider: &str) -> Option<proto::ProviderInfo> {
    if !super::serves_provider(provider) { return None; }
    let host = host_for_provider(provider)?;
    let infos = host.providers.lock().ok()?;
    infos.iter().find(|info| info.id.as_str() == provider).cloned()
}

pub fn current() -> Option<Arc<Host>> {
    table().lock().ok()?.values().next().map(|pack| pack.host.clone())
}

pub fn install(host: Arc<Host>) {
    let providers = host.providers.lock()
        .map(|infos| infos.iter().map(|info| info.id.0.clone()).collect())
        .unwrap_or_default();
    install_pack("legacy", ProviderCategory::Agent, host);
    set_providers("legacy", providers);
}

pub fn detach() {
    if let Ok(mut hosts) = table().lock() { hosts.clear(); }
}

pub(super) fn loaded_hosts(ids: &[String]) -> Vec<Arc<Host>> {
    let hosts = table().lock().unwrap_or_else(|error| error.into_inner());
    ids.iter().filter_map(|id| hosts.get(id).map(|pack| pack.host.clone())).collect()
}

pub(super) fn providers_for_host(host: &Arc<Host>) -> Vec<String> {
    table().lock().unwrap_or_else(|error| error.into_inner()).values()
        .filter(|pack| Arc::ptr_eq(&pack.host, host))
        .flat_map(|pack| pack.providers.iter().cloned()).collect()
}

pub(super) fn loaded_ids() -> Vec<String> {
    table().lock().unwrap_or_else(|error| error.into_inner()).keys().cloned().collect()
}

pub(super) async fn shutdown_packs(ids: &[String]) -> Result<(), String> {
    for id in ids {
        let Some(host) = host_for_pack(id) else { continue };
        host.shutdown().await;
        if host.current_run().is_some() {
            return Err(format!("插件包「{id}」尚未确认退出，已停止替换文件"));
        }
        let mut hosts = table().lock().unwrap_or_else(|error| error.into_inner());
        if hosts.get(id).is_some_and(|pack| Arc::ptr_eq(&pack.host, &host)) { hosts.remove(id); }
    }
    Ok(())
}

pub async fn shutdown_all() {
    let hosts: Vec<Arc<Host>> = table().lock()
        .map(|mut hosts| std::mem::take(&mut *hosts).into_values().map(|pack| pack.host).collect())
        .unwrap_or_default();
    for host in hosts { host.shutdown().await; }
}

pub(super) fn status() -> Option<HostStatus> {
    let hosts = table().lock().ok()?;
    if hosts.is_empty() { return None; }
    let mut statuses = Vec::new();
    let mut providers = Vec::new();
    let mut errors = Vec::new();
    for (id, pack) in hosts.iter() {
        let status = pack.host.status();
        if let Some(error) = &status.last_error { errors.push(format!("{id}: {error}")); }
        providers.extend(pack.providers.iter().cloned());
        statuses.push(status);
    }
    providers.sort();
    providers.dedup();
    Some(HostStatus {
        running: statuses.iter().any(|status| status.running),
        providers,
        crashes: statuses.iter().map(|status| status.crashes).max().unwrap_or(0),
        disabled: statuses.iter().all(|status| status.disabled),
        last_error: (!errors.is_empty()).then(|| errors.join("；")),
    })
}
