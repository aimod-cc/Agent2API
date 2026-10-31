use std::collections::{BTreeSet, HashSet};
use std::time::Duration;

use super::install::{InstalledPack, PackRegistry};
use super::registry::SwitchGuard;
use super::{bootstrap, ownership, runtime};
use crate::server::core::account_store::AccountStore;

pub(super) struct ChangeScope {
    pub packs: Vec<String>,
    providers: HashSet<String>,
}

impl ChangeScope {
    pub fn between(before: &PackRegistry, after: &PackRegistry, target: &str, prepared: Option<&InstalledPack>) -> Self {
        let old = ownership::owners(before);
        let new = ownership::owners_with(after, prepared);
        let mut packs = BTreeSet::from([target.to_string()]);
        for provider in old.keys().chain(new.keys()) {
            if old.get(provider) != new.get(provider) {
                packs.extend(old.get(provider).cloned());
                packs.extend(new.get(provider).cloned());
            }
        }
        let providers: HashSet<String> = old.iter().chain(new.iter())
            .filter(|(_, pack)| packs.contains(*pack)).map(|(provider, _)| provider.clone()).collect();
        Self { packs: packs.into_iter().collect(), providers }
    }

    pub fn begin(&self) -> Option<SwitchGuard> {
        let hosts = runtime::loaded_hosts(&self.packs);
        let mut providers = self.providers.clone();
        for host in &hosts {
            providers.extend(runtime::providers_for_host(host));
        }
        SwitchGuard::begin(&hosts, providers)
    }

    pub async fn stop(&self) -> Result<(), String> {
        runtime::shutdown_packs(&self.packs).await
    }

    pub async fn start(&self, store: &AccountStore) -> Result<(), String> {
        let registry = PackRegistry::load();
        let owners = ownership::owners(&registry);
        let mut failures = Vec::new();
        for pack in registry.packs.iter().filter(|pack| pack.enabled && self.packs.contains(&pack.id)) {
            if runtime::host_for_pack(&pack.id).is_some_and(|host| host.ready()) { continue; }
            if runtime::host_for_pack(&pack.id).is_some_and(|host| host.current_run().is_some()) {
                failures.push(format!("{} 的旧进程尚未确认退出", pack.id));
                continue;
            }
            let started = tokio::time::timeout(Duration::from_secs(60),
                bootstrap::start_pack(store.clone(), pack, &owners)).await;
            if !matches!(started, Ok(Some(ref host)) if host.ready()) {
                let reason = runtime::host_for_pack(&pack.id).and_then(|host| host.status().last_error)
                    .unwrap_or_else(|| "启动失败或加载超时，请检查插件日志".to_string());
                failures.push(format!("{}：{reason}", pack.id));
                if let Err(error) = runtime::shutdown_packs(std::slice::from_ref(&pack.id)).await {
                    failures.push(error);
                }
            }
        }
        if failures.is_empty() { Ok(()) } else { Err(failures.join("；")) }
    }

    pub fn refresh_catalogs(&self) {
        crate::server::core::providers::adapter::restore_cached_catalogs();
        if crate::server::core::checkin_schedules::extend_plugin_providers_once() {
            crate::server::logging::log("[Checkin]", "新安装插件的每日签到提供商已补入已有签到范围");
        }
        super::schedules::wake();
    }
}
